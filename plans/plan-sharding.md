# Feature: Sharding

## 1. Purpose

The sharding module implements the key routing function that maps any key to its owning shard. It is a pure, stateless computation: `route_key(key: &[u8]) -> ShardId`. There are no locks, no state, no allocation.

This module also defines the scatter-gather pattern for multi-key commands (DEL) which must touch multiple shards without cross-shard coordination.

The routing function lives in `crates/common` (used by both frontend and engine); the scatter-gather pattern is implemented in `crates/frontend`.

## 2. Responsibilities

- Implement `route_key(key: &[u8]) -> ShardId` using CRC16 with power-of-2 bitmasking
- Implement `crc16(data: &[u8]) -> u16` using the CCITT-16 polynomial (same as Redis Cluster)
- Guarantee deterministic, uniform key distribution
- Define the scatter-gather algorithm for DEL across multiple shards
- Expose shard distribution testing utilities

## 3. Non-Responsibilities

- Does not manage shard threads or channels
- Does not implement dynamic resharding (post-MVP)
- Does not implement hash tags (post-MVP; would affect routing for keys with `{tag}`)
- Does not guarantee locality for related keys
- Does not implement client-side routing hints

## 4. Architecture Design

```
Frontend thread receives: DEL key1 key2 key3

┌─────────────────────────────────────────────────────────┐
│  route_key(b"key1") → ShardId(10)                       │
│  route_key(b"key2") → ShardId(10)                       │  pure fn, ~50ns per key
│  route_key(b"key3") → ShardId(25)                       │
└─────────────────────────────────────────────────────────┘
        │                        │
        ▼                        ▼
  Shard 10 channel         Shard 25 channel
  Del { keys: [key1,key2] }  Del { keys: [key3] }
        │                        │
        ▼                        ▼
  Response::Integer(2)     Response::Integer(1)
        │                        │
        └──────────┬─────────────┘
                   ▼
           aggregate total = 3
           Response::Integer(3) → client
```

### CRC16 and Power-of-2 Modulo

```
// Standard modulo (slow):
shard_id = crc16(key) % SHARD_COUNT

// Power-of-2 bitwise AND (10x faster, same result):
shard_id = crc16(key) & (SHARD_COUNT - 1)  // SHARD_COUNT must be power of 2
```

## 5. Core Data Structures

```rust
// crates/common/src/routing.rs

use crate::{ShardId, SHARD_COUNT};

/// Map a key to its owning shard.
/// Deterministic, pure function. No allocation.
/// Uses CRC16-CCITT with power-of-2 bitmasking.
pub fn route_key(key: &[u8]) -> ShardId {
    ShardId((crc16(key) as usize & (SHARD_COUNT - 1)) as u16)
}

/// CRC16-CCITT (polynomial 0x1021, init 0x0000).
/// Matches Redis Cluster's key hashing algorithm.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}
```

There are no structs specific to the sharding module — it operates entirely on existing types (`&[u8]`, `ShardId`).

## 6. Public Interfaces

```rust
// In crates/common — available to all crates
pub fn route_key(key: &[u8]) -> ShardId
pub fn crc16(data: &[u8]) -> u16

// Scatter-gather is implemented in crates/frontend (see plan-frontend.md)
// The grouping helper is also exposed for testing:
pub fn group_keys_by_shard<'a>(keys: &'a [Vec<u8>]) -> HashMap<ShardId, Vec<&'a Vec<u8>>>
```

## 7. Internal Algorithms

### CRC16-CCITT

The algorithm processes one byte at a time, XOR-ing into the high byte of the 16-bit CRC register, then running 8 bit-by-bit polynomial divisions:

```
crc16(data):
  crc = 0x0000
  for byte in data:
    crc = crc XOR (byte << 8)
    for 8 iterations:
      if (crc & 0x8000) != 0:
        crc = (crc << 1) XOR 0x1021
      else:
        crc = crc << 1
  return crc & 0xFFFF
```

**Properties**:
- Polynomial: 0x1021 (CCITT-16, same as Redis Cluster)
- Initial value: 0x0000
- No final XOR
- Speed: ~10ns for a 10-20 byte key

**Test vector** (from Redis source):
- `crc16(b"") == 0x0000`
- `crc16(b"123456789") == 0x31C3`

### Key-to-Shard Mapping

```
route_key(key):
  hash = crc16(key)      // 0..65535
  shard = hash & (SHARD_COUNT - 1)  // 0..SHARD_COUNT-1
  return ShardId(shard as u16)
```

For SHARD_COUNT=64: `hash & 63` (6-bit mask).
For SHARD_COUNT=128: `hash & 127` (7-bit mask).

### Group Keys By Shard (for DEL scatter-gather)

```
group_keys_by_shard(keys):
  groups = HashMap::new()
  for key in keys:
    shard_id = route_key(key)
    groups.entry(shard_id).or_default().push(key)
  return groups
```

### Distribution Uniformity Test

```
test_distribution(n_keys, shard_count):
  counts = vec![0; shard_count]
  for i in 0..n_keys:
    key = format!("key:{}", i)
    shard = route_key(key.as_bytes())
    counts[shard.0 as usize] += 1

  mean = n_keys / shard_count
  variance_pct = max_deviation(counts, mean) / mean * 100
  assert variance_pct < 5.0
```

## 8. Persistence Model

None. The routing function is pure computation with no state. Shard assignment for a given key is stable across restarts as long as `SHARD_COUNT` does not change.

**Warning**: Changing `SHARD_COUNT` (recompiling with a different Cargo feature) invalidates all existing snapshots — keys would route to different shards after the rebuild. Existing snapshot files must be deleted before restarting with a new `SHARD_COUNT`.

## 9. Concurrency Model

`route_key` and `crc16` are pure functions with no shared state — trivially thread-safe. They may be called from any number of threads simultaneously with no coordination.

## 10. Configuration

`SHARD_COUNT` is the only relevant parameter, and it is compile-time only (Cargo feature). No runtime configuration.

| SHARD_COUNT | Feature flag | Routing mask |
|-------------|-------------|-------------|
| 16 | `shard_count_16` | `& 0x0F` |
| 32 | `shard_count_32` | `& 0x1F` |
| 64 | *(default)* | `& 0x3F` |
| 128 | `shard_count_128` | `& 0x7F` |

## 11. Observability

Per-shard metrics expose the current key count:
```
rsdragonfly_shard_key_count{shard="0"} 15625
rsdragonfly_shard_key_count{shard="1"} 15601
...
```

High variance between shard key counts indicates a skewed workload. Alert threshold: if any shard has > 2× the mean key count.

No logs are emitted by the routing function itself — it is a pure hot-path computation.

## 12. Testing Strategy

- **Unit tests**:
  - `test_crc16_empty_string`: `crc16(b"") == 0x0000`
  - `test_crc16_test_vector`: `crc16(b"123456789") == 0x31C3` (Redis Cluster test vector)
  - `test_route_key_deterministic`: call `route_key(b"mykey")` twice, assert same result
  - `test_route_key_in_range`: for 1000 random keys, assert all shards in `0..SHARD_COUNT`
  - `test_route_key_power_of_two_equivalence`: assert `crc16(k) % SHARD_COUNT == crc16(k) & (SHARD_COUNT - 1)` for 1000 random keys
  - `test_distribution_random_keys`: 1M random keys, assert per-shard count variance < 5%
  - `test_distribution_sequential_keys`: 1M sequential `key:{i}` keys, assert variance < 5%
  - `test_group_keys_by_shard_single_shard`: 3 keys that all hash to same shard → 1 group
  - `test_group_keys_by_shard_multi_shard`: 3 keys across 2 shards → 2 groups with correct keys
  - `test_shard_count_is_power_of_two`: compile-time assertion: `SHARD_COUNT & (SHARD_COUNT-1) == 0`

- **Property-based tests** (using `proptest`):
  - For any key bytes: `route_key(key).0 < SHARD_COUNT`
  - For any key bytes: `route_key(key) == route_key(key)` (determinism)

- **Benchmark**:
  - `bench_route_key`: measure throughput of `route_key` on 10-byte keys; target > 50M/s

## 13. Open Questions

None.
