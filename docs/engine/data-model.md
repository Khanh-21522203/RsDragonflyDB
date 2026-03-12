# In-Memory Data Model

## Document Purpose
This document defines the in-memory data structures, memory layout, and ownership semantics for RsDragonflyDB's storage engine.

**Audience**: Implementation engineers, performance engineers

---

## Core Data Structures

### Per-Shard Storage

```rust
pub struct Shard {
    id: ShardId,
    data: HashMap<Key, Entry>,
    ttl_queue: BinaryHeap<Expiry>,
    metrics: ShardMetrics,
}

pub struct Entry {
    value: Value,
    expiry: Option<Instant>,
}
```

**Ownership**: Each `Shard` is owned exclusively by its thread. No shared references.

> **Note**: The `metadata` fields (`created_at`, `last_accessed`, `access_count`) are intentionally excluded from the MVP `Entry` struct. They are reserved for future eviction policies (LRU/LFU). Including them in MVP would add ~40 bytes per entry unnecessarily, pushing overhead above the 50-byte target.

---

## Key Representation

### Key Type

```rust
pub type Key = Vec<u8>;
```

**Rationale**:
- Keys are arbitrary byte sequences (not UTF-8 strings)
- `Vec<u8>` allows zero-copy from RESP2 parser
- Owned type (no lifetime management)

**Constraints**:
- Max key size: 512 bytes (configurable)
- Min key size: 1 byte
- Empty keys are rejected

**Memory Layout**:
```
Vec<u8>:
  ptr: *mut u8     (8 bytes)
  len: usize       (8 bytes)
  cap: usize       (8 bytes)
  data: [u8; len]  (len bytes, heap-allocated)

Total overhead: 24 bytes + len
```

---

### Key Hashing

**HashMap Key**: Uses `Key` directly (not hash).

**Hash Function**: `std::collections::hash_map::DefaultHasher` (SipHash 1-3).

**Rationale**:
- SipHash is cryptographically strong (prevents hash collision attacks)
- Fast enough for in-memory workloads (~10ns per key)

**Alternative** (future optimization):
- Use `ahash` or `fxhash` for 2-3x faster hashing
- Trade-off: Weaker collision resistance

---

## Value Representation

### Value Type

```rust
pub enum Value {
    String(Vec<u8>),
    // Future: Integer(i64), List(Vec<Value>), etc.
}
```

**MVP**: Only string values (byte arrays).

**Memory Layout**:
```
Value::String(Vec<u8>):
  discriminant: u8     (1 byte, enum tag)
  padding: [u8; 7]     (7 bytes, alignment)
  ptr: *mut u8         (8 bytes)
  len: usize           (8 bytes)
  cap: usize           (8 bytes)
  data: [u8; len]      (len bytes, heap-allocated)

Total overhead: 32 bytes + len
```

**Constraints**:
- Max value size: 512 MB (configurable)
- Min value size: 0 bytes (empty string allowed)

---

## Entry Representation

### Entry Layout

```rust
pub struct Entry {
    value: Value,            // 32 bytes + value_len
    expiry: Option<Instant>, // 16 bytes (Some) or 1 byte (None, padded to 16)
}
```

**Total Overhead per Entry**:
- Without TTL: 32 + 1 = ~48 bytes + value_len (with padding/alignment)
- With TTL: 32 + 16 = 48 bytes + value_len

---

## HashMap Configuration

### HashMap Parameters

```rust
let mut data = HashMap::with_capacity_and_hasher(
    initial_capacity: 1_000_000,
    hasher: RandomState::new(),
);
```

**Initial Capacity**: 1M entries per shard (for 64M total keys).

**Load Factor**: 0.875 (default for Rust HashMap).

**Resize Strategy**: Double capacity when load factor exceeded.

**Memory Overhead**:
```
HashMap overhead = capacity × (key_size + value_size + 16 bytes)
                 = 1M × (24 + 57 + 16)
                 = 97 MB per shard (empty)
```

---

### HashMap Performance

| Operation | Time Complexity | Actual Latency |
|-----------|-----------------|----------------|
| Insert | O(1) amortized | ~50ns |
| Lookup | O(1) average | ~30ns |
| Delete | O(1) average | ~40ns |
| Resize | O(n) | ~10ms per 1M keys |

**Resize Impact**:
- Blocks shard thread during resize
- Occurs when capacity exceeded (rare after initial growth)
- Mitigation: Pre-allocate capacity based on expected key count

---

## TTL Expiration Queue

### Data Structure

```rust
pub struct Expiry {
    expiry_time: Instant,
    key: Key,
}

impl Ord for Expiry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap: earliest expiry first
        other.expiry_time.cmp(&self.expiry_time)
    }
}

pub type TtlQueue = BinaryHeap<Expiry>;
```

**Properties**:
- Min-heap: Root is earliest expiry
- Insert: O(log n)
- Pop min: O(log n)
- Peek min: O(1)

---

### TTL Queue Operations

**Insert** (when key has TTL):
```rust
fn set_with_ttl(&mut self, key: Key, value: Value, ttl_secs: u64) {
    let expiry_time = Instant::now() + Duration::from_secs(ttl_secs);

    // Insert into HashMap
    self.data.insert(key.clone(), Entry {
        value,
        expiry: Some(expiry_time),
    });

    // Insert into TTL queue
    self.ttl_queue.push(Expiry { expiry_time, key });
}
```

**Expire** (background task):
```rust
fn expire_keys(&mut self) {
    let now = Instant::now();
    
    while let Some(expiry) = self.ttl_queue.peek() {
        if expiry.expiry_time > now {
            break;  // No more expired keys
        }
        
        let expiry = self.ttl_queue.pop().unwrap();
        
        // Check if key still exists and has same expiry
        if let Some(entry) = self.data.get(&expiry.key) {
            if entry.expiry == Some(expiry.expiry_time) {
                self.data.remove(&expiry.key);
                self.metrics.keys_expired.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
```

**Lazy Expiration** (on access):
```rust
fn get(&mut self, key: &Key) -> Option<&Value> {
    let entry = self.data.get(key)?;
    
    // Check if expired
    if let Some(expiry) = entry.expiry {
        if Instant::now() >= expiry {
            self.data.remove(key);
            return None;
        }
    }
    
    Some(&entry.value)
}
```

---

### TTL Queue Memory Overhead

**Per Entry with TTL**:
```
Expiry struct:
  expiry_time: Instant  (16 bytes)
  key: Vec<u8>          (24 bytes + key_len)

Total: 40 bytes + key_len
```

**Duplicate Key Storage**:
- Key stored in HashMap: 24 bytes + key_len
- Key stored in TTL queue: 24 bytes + key_len
- **Total**: 48 bytes + 2 × key_len

**Optimization** (future):
- Store key hash instead of full key in TTL queue
- Reduces overhead to 40 + 8 = 48 bytes (no key duplication)

---

## Memory Ownership

### Rust Ownership Rules

**Rule 1**: Each shard owns its HashMap and TTL queue.

```rust
impl Shard {
    fn new(id: ShardId) -> Self {
        Shard {
            id,
            data: HashMap::new(),  // Owned
            ttl_queue: BinaryHeap::new(),  // Owned
            metrics: ShardMetrics::default(),
        }
    }
}
```

**Rule 2**: Keys and values are owned by the HashMap.

```rust
// Insert takes ownership of key and value
self.data.insert(key, entry);  // key and entry moved into HashMap

// Get returns a reference (no ownership transfer)
let value = self.data.get(&key);  // Borrows value
```

**Rule 3**: No shared references across threads.

```rust
// This does NOT compile:
let shard_ref = &shards[0];
std::thread::spawn(move || {
    shard_ref.get(b"key");  // ERROR: cannot send &Shard across threads
});
```

---

### Lifetime Management

**No Lifetimes**: All data is owned (no borrowed references in storage).

**Rationale**:
- Simplifies API (no lifetime annotations)
- Avoids borrow checker complexity
- Enables zero-copy serialization (future)

**Trade-off**: More allocations (keys/values are cloned on insert).

---

## Memory Allocation Strategy

### Allocator Choice

**Default**: System allocator (glibc malloc on Linux).

**Recommended**: jemalloc (better for multi-threaded workloads).

```toml
# Cargo.toml
[dependencies]
jemallocator = "0.5"

# main.rs
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;
```

**Benefits of jemalloc**:
- Lower fragmentation (arena-based allocation)
- Better multi-threaded performance (per-thread caches)
- Memory profiling support

---

### Allocation Patterns

**Hot Path Allocations** (per SET command):
- Key: 1 allocation (`Vec<u8>`)
- Value: 1 allocation (`Vec<u8>`)
- Entry: **0 extra allocations** — `Entry` is stored inline in the HashMap (not heap-allocated separately)
- **Total**: 2 allocations per SET command

**Note**: Rust's `HashMap` stores values inline by value, so `Entry` itself is not boxed. The only heap allocations are the key and value byte vectors.

**Optimization** (future):
- Object pool or bump allocator for short-lived keys
- Reduces allocations to ~1 per SET for fixed-size keys

---

### Memory Fragmentation

**Problem**: Frequent alloc/free causes fragmentation.

**Measurement**:
```rust
// Using jemalloc stats
let allocated = jemalloc_ctl::stats::allocated::read().unwrap();
let resident = jemalloc_ctl::stats::resident::read().unwrap();
let fragmentation_ratio = resident as f64 / allocated as f64;

// Target: < 1.2x
```

**Mitigation**:
- Use jemalloc (better than glibc malloc)
- Periodic compaction (future: copy live keys to new HashMap)

---

## Memory Limits

### Per-Shard Limits

**Max Keys**: 100M per shard (configurable).

**Max Memory**: 20 GB per shard (configurable).

**Enforcement**:
```rust
fn set(&mut self, key: Key, value: Value) -> Result<(), Error> {
    // Check key count limit
    if self.data.len() >= MAX_KEYS_PER_SHARD {
        return Err(Error::ShardFull);
    }
    
    // Check memory limit
    let entry_size = key.len() + value.len() + ENTRY_OVERHEAD;
    if self.metrics.memory_bytes.load(Ordering::Relaxed) + entry_size > MAX_MEMORY_PER_SHARD {
        return Err(Error::OutOfMemory);
    }
    
    self.data.insert(key, Entry { value, /* ... */ });
    self.metrics.memory_bytes.fetch_add(entry_size, Ordering::Relaxed);
    Ok(())
}
```

---

### Global Limits

**Max Total Memory**: 1.28 TB (64 shards × 20 GB).

**OOM Handling**:
- If shard exceeds limit: Reject new writes, return `-ERR OOM`
- If process exceeds OS limit: Kernel OOM killer terminates process

**Monitoring**:
```bash
# Per-shard memory usage
curl http://localhost:9090/metrics | grep shard_memory_bytes

# Total memory usage
ps aux | grep rsdragonfly
```

---

## Data Structure Invariants

### Invariant 1: HashMap and TTL Queue Consistency

**Property**: If a key has a TTL, it exists in both HashMap and TTL queue.

**Enforcement**:
```rust
// Insert with TTL
self.data.insert(key.clone(), entry);
self.ttl_queue.push(Expiry { expiry_time, key });

// Delete
self.data.remove(&key);
// TTL queue entry remains (cleaned up lazily)
```

**Lazy Cleanup**: TTL queue may contain stale entries (keys already deleted). Checked during expiration.

---

### Invariant 2: Key Uniqueness

**Property**: Each key exists at most once in the HashMap.

**Enforcement**: HashMap guarantees uniqueness (insert overwrites existing key).

---

### Invariant 3: Shard Isolation

**Property**: No key exists in multiple shards.

**Enforcement**: Key routing is deterministic (CRC16 hash).

---

## Serialization Format (Snapshots)

> **Authoritative source**: The snapshot binary format is fully specified in [persistence.md](../persistence/persistence.md#snapshot-format). That document defines the 64-byte header layout, field offsets, checksum algorithm, and entry encoding. This section provides only a high-level summary.

### Binary Format (Summary)

The snapshot file has a fixed **64-byte header** followed by a variable-length sequence of entries. Key fields:

| Section | Size | Notes |
|---------|------|-------|
| Header | 64 bytes | Magic "RSDF", version, shard ID, key count, timestamp, two CRC64 checksums, reserved |
| Per entry | variable | key length (u32), key bytes, value length (u32), value bytes, has-expiry flag (u8), optional expiry timestamp (u64) |

See [persistence.md](../persistence/persistence.md) for the complete field-by-field specification, write algorithm, and read/validation procedure.

---

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_entry_size() {
    let entry = Entry {
        value: Value::String(vec![0u8; 100]),
        expiry: Some(Instant::now()),
    };

    let size = std::mem::size_of_val(&entry);
    assert!(size < 100, "Entry overhead too high");
}

#[test]
fn test_ttl_queue_consistency() {
    let mut shard = Shard::new(ShardId(0));
    shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), Some(Instant::now() + Duration::from_secs(10)));
    
    assert_eq!(shard.data.len(), 1);
    assert_eq!(shard.ttl_queue.len(), 1);
    
    shard.expire_keys();
    assert_eq!(shard.data.len(), 1);  // Not expired yet
    
    std::thread::sleep(Duration::from_secs(11));
    shard.expire_keys();
    assert_eq!(shard.data.len(), 0);  // Expired
}
```

---

### Memory Tests

```rust
#[test]
fn test_memory_overhead() {
    let mut shard = Shard::new(ShardId(0));
    
    let initial_memory = get_process_memory();
    
    // Insert 1M keys
    for i in 0..1_000_000 {
        let key = format!("key:{}", i).into_bytes();
        let value = b"value".to_vec();
        shard.set(key, Value::String(value), None);
    }
    
    let final_memory = get_process_memory();
    let overhead_per_key = (final_memory - initial_memory) / 1_000_000;
    
    assert!(overhead_per_key < 100, "Memory overhead too high: {} bytes", overhead_per_key);
}
```

---

## Definition of Done

Data model is complete when:

1. HashMap and TTL queue are implemented and tested
2. Memory overhead is measured and within target (< 50 bytes/key)
3. Serialization/deserialization is correct and tested
4. Memory limits are enforced
5. Ownership rules are enforced by Rust type system
6. Fragmentation is measured and acceptable (< 1.2x)
7. Performance tests meet latency targets (< 100ns per operation)

---

## References

- [overview.md](../architecture/overview.md) - System architecture
- [ttl-expiration.md](ttl-expiration.md) - TTL implementation details
- [persistence.md](../persistence/persistence.md) - Snapshot format
