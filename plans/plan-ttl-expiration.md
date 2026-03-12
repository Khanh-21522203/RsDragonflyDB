# Feature: TTL Expiration

## 1. Purpose

TTL (Time-To-Live) expiration ensures that keys with a configured lifetime are removed from the store after their deadline passes. RsDragonflyDB uses a **hybrid strategy**: lazy expiration on access (zero overhead when keys are not accessed) plus active background expiration (prevents unbounded memory growth from stale keys that are never accessed again).

This feature lives entirely inside `crates/engine` — it is part of the `Shard` implementation and shares its data structures (`HashMap`, `BinaryHeap<Expiry>`).

## 2. Responsibilities

- Attach an `Option<Instant>` expiry to every `Entry` at SET / EXPIRE time
- Implement lazy expiration: check TTL on GET and TTL command; remove and return nil if expired
- Implement active expiration: background sweep in the shard event loop, every 100ms, checking up to 20 entries per pass
- Maintain the TTL min-heap (`BinaryHeap<Expiry>`) as an efficient priority queue for active expiration
- Persist expiry timestamps across restarts (via snapshot — see `plan-persistence.md`)
- Implement `EXPIRE` command: set TTL on an existing key
- Implement `TTL` command: query remaining lifetime in seconds

## 3. Non-Responsibilities

- Does not coordinate TTL expiration across shards
- Does not implement memory-pressure eviction (future LRU/LFU policy)
- Does not support millisecond-precision TTL (MVP: 1-second precision only)
- Does not implement `PERSIST` command (post-MVP)

## 4. Architecture Design

```
SET mykey value EX 30
        │
        ▼
shard.set(key, value, ttl_secs=30)
  ├─ Entry { value, expiry: Some(Instant::now() + 30s) } → HashMap
  └─ Expiry { expiry_time, key } → BinaryHeap (min-heap)

        │ 100ms later (event loop timeout)
        ▼
shard.expire_keys()
  peek BinaryHeap root (earliest expiry)
  if expiry_time <= now:
    pop → verify entry still matches → remove from HashMap
  repeat up to 20 times

GET mykey (lazy check)
        │
        ▼
shard.get(key)
  entry = HashMap.get(key)?
  if entry.expiry <= now:
    HashMap.remove(key)
    return nil
  return entry.value
```

### Why Hybrid?

- **Lazy only**: Memory grows unboundedly if expired keys are never accessed again
- **Active only**: Scanning all keys every tick is O(n) and blocks command processing
- **Hybrid**: Active sweep drains the min-heap (O(log n) per pop); lazy catches the rest with zero extra cost

### TTL Queue Invariant

The BinaryHeap may contain **stale entries** — for keys that were deleted or had their TTL overwritten. The active sweep validates each popped entry against the HashMap before removing it. This is intentional: cleaning the heap on every DEL would be O(n) (heap has no efficient delete-by-key).

## 5. Core Data Structures

```rust
// These are defined in crates/common; repeated here for reference.

pub struct Entry {
    pub value: Value,
    pub expiry: Option<Instant>,  // None = no TTL
}

pub struct Expiry {
    pub expiry_time: Instant,
    pub key: Key,  // duplication is intentional; heap does not reference HashMap
}

// Min-heap: pop returns the entry with the earliest (smallest) expiry_time
pub type TtlQueue = BinaryHeap<Expiry>;
```

Memory overhead per key with TTL:
```
HashMap entry:   Key (24 + key_len) + Entry (32 + value_len) = 56 + key_len + value_len bytes
TtlQueue entry:  Expiry { Instant (16) + Key (24 + key_len) } = 40 + key_len bytes
Total overhead:  ~96 + 2×key_len + value_len bytes per TTL key
```

## 6. Public Interfaces

All methods are on `Shard`, defined in `crates/engine`:

```rust
impl Shard {
    /// Set a key with optional TTL. Called by SET and by EXPIRE indirectly.
    pub fn set(&mut self, key: Key, value: Value, ttl_secs: Option<u64>) -> Response

    /// Set TTL on an existing key. Returns 1 if key exists, 0 if not.
    pub fn expire(&mut self, key: &Key, ttl_secs: u64) -> Response

    /// Query remaining TTL in seconds.
    /// Returns: -2 if key not found, -1 if key exists but has no TTL, >=0 seconds remaining.
    pub fn ttl(&mut self, key: &Key) -> Response

    /// GET with lazy expiration built in.
    pub fn get(&mut self, key: &Key) -> Response

    /// Active TTL sweep. Called every 100ms by the event loop.
    /// Processes at most 20 expired entries per call.
    pub fn expire_keys(&mut self)
}
```

## 7. Internal Algorithms

### Setting TTL on INSERT (SET EX)

```
set_with_ttl(key, value, ttl_secs):
  expiry_time = Instant::now() + Duration::from_secs(ttl_secs)
  data.insert(key.clone(), Entry { value, expiry: Some(expiry_time) })
  ttl_queue.push(Expiry { expiry_time, key })
```

### Updating TTL on Existing Key (EXPIRE)

```
expire(key, ttl_secs):
  if !data.contains_key(key): return Integer(0)

  expiry_time = Instant::now() + Duration::from_secs(ttl_secs)
  // Update HashMap entry's expiry in place
  data.get_mut(key).unwrap().expiry = Some(expiry_time)
  // Push new expiry to heap (old one becomes stale — cleaned lazily)
  ttl_queue.push(Expiry { expiry_time, key: key.clone() })
  return Integer(1)
```

### Lazy Expiration on GET

```
get(key):
  entry = match data.get(key):
    None → return BulkString(None)
    Some(e) → e

  if let Some(expiry_time) = entry.expiry:
    if Instant::now() >= expiry_time:
      // Expired — remove and return nil
      let removed = data.remove(key).unwrap()
      metrics.key_count.fetch_sub(1, Relaxed)
      metrics.memory_bytes.fetch_sub(key.len() + removed.value.len() + ENTRY_OVERHEAD, Relaxed)
      metrics.keys_expired.fetch_add(1, Relaxed)
      return BulkString(None)

  return BulkString(Some(entry.value.as_bytes().to_vec()))
```

### Active Expiration Sweep

```
expire_keys():
  now = Instant::now()
  processed = 0

  while processed < 20:
    match ttl_queue.peek():
      None → break       // queue empty
      Some(expiry) if expiry.expiry_time > now → break  // nothing expired yet
      _ → {}

    expiry = ttl_queue.pop().unwrap()
    processed += 1

    // Validate: the entry may have been deleted or had its TTL changed
    match data.get(&expiry.key):
      None → continue   // key already deleted; stale queue entry
      Some(entry) if entry.expiry != Some(expiry.expiry_time) → continue  // TTL updated; stale
      Some(_) →
        data.remove(&expiry.key)
        metrics.key_count.fetch_sub(1, Relaxed)
        metrics.memory_bytes.fetch_sub(...)
        metrics.keys_expired.fetch_add(1, Relaxed)
```

### TTL Query

```
ttl(key):
  match data.get(key):
    None → return Integer(-2)           // key not found

    Some(entry) →
      match entry.expiry:
        None → return Integer(-1)       // key exists, no TTL

        Some(expiry_time) →
          now = Instant::now()
          if expiry_time <= now:
            return Integer(0)           // expired but not yet lazily cleaned
          remaining_secs = (expiry_time - now).as_secs() as i64
          return Integer(remaining_secs)
```

### Expiry Conversion for Snapshots

```
// Used during serialization (persistence crate):
fn expiry_to_unix_secs(expiry: Instant) -> u64:
  let now_instant = Instant::now()
  let now_unix = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs()
  let remaining_secs = expiry.saturating_duration_since(now_instant).as_secs()
  return now_unix + remaining_secs

// Used during deserialization:
fn unix_secs_to_instant(unix_secs: u64) -> Instant:
  let now_unix = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_secs()
  if unix_secs <= now_unix: return Instant::now()  // already expired
  let remaining = Duration::from_secs(unix_secs - now_unix)
  return Instant::now() + remaining
```

## 8. Persistence Model

TTL expiry instants are not stored as `Instant` (which is relative to process boot) but as absolute **Unix timestamps** (seconds since epoch) in the snapshot. On load:
- If `expiry_timestamp <= now_unix`: key is skipped (already expired)
- Otherwise: `entry.expiry = Some(unix_secs_to_instant(expiry_timestamp))`

See `plan-persistence.md` for the on-disk format.

## 9. Concurrency Model

All TTL operations happen exclusively on the shard thread. No synchronization needed:
- `HashMap` and `BinaryHeap` are accessed only by the single shard thread
- `metrics.keys_expired` is `AtomicU64`; readable from other threads without a lock

## 10. Configuration

| Parameter | Source | Default | Notes |
|-----------|--------|---------|-------|
| Active expiry interval | Hardcoded | 100ms | TTL check every `recv_timeout` |
| Max entries per sweep | Hardcoded | 20 | Bounds per-sweep latency impact |
| TTL precision | Hardcoded | 1 second | `ttl_secs: u64`, not milliseconds |

## 11. Observability

- `metrics.keys_expired` — cumulative count of actively expired keys
- Lazy-expired keys also increment `keys_expired` in GET
- Log `DEBUG` for each active expiry batch if any keys were removed
- Metrics exposed by the metrics exporter as:
  ```
  rsdragonfly_shard_keys_expired_total{shard="0"} 5000
  ```

## 12. Testing Strategy

- **Unit tests**:
  - `test_lazy_expiry_on_get`: set key with 1ms TTL, sleep 2ms, GET → BulkString(None)
  - `test_lazy_expiry_updates_metrics`: verify key_count and keys_expired after lazy expiry
  - `test_active_expiry_removes_key`: push expired Expiry into heap, call expire_keys, assert HashMap empty
  - `test_active_expiry_stale_queue_entry`: delete key, push old Expiry into heap, call expire_keys, assert no crash
  - `test_active_expiry_max_20_entries`: insert 30 expired keys, call expire_keys once, assert 20 removed (not 30)
  - `test_expire_command_returns_1`: set key, EXPIRE 10s → 1
  - `test_expire_command_returns_0`: EXPIRE missing key → 0
  - `test_expire_updates_ttl_queue`: expire key twice; second expiry must be honored (first is stale)
  - `test_ttl_no_ttl`: set without TTL, TTL → -1
  - `test_ttl_missing_key`: TTL on missing key → -2
  - `test_ttl_with_remaining`: set with 100s TTL, TTL → value between 95 and 100
  - `test_ttl_zero_on_expired`: set with 1ms TTL, sleep 2ms, TTL → 0 (not yet lazily cleaned)
  - `test_set_overwrite_clears_ttl`: set key with TTL, overwrite without TTL, TTL → -1
  - `test_expiry_to_unix_secs_and_back`: round-trip through unix conversion, assert < 1s error

- **Integration tests**:
  - `test_key_expires_within_one_second`: redis-cli SET key EX 1, sleep 2s, GET → nil
  - `test_ttl_persists_across_restart`: SET with EX 3600, write snapshot, restart, TTL > 0

## 13. Open Questions

None.
