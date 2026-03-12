# TTL Expiration Strategy

## Document Purpose
This document defines the time-to-live (TTL) expiration mechanism, including lazy and active expiration strategies, performance characteristics, and edge cases.

**Audience**: Implementation engineers, performance engineers

---

## TTL Fundamentals

### TTL Semantics

**Definition**: TTL (Time-To-Live) specifies how long a key should exist before automatic deletion.

**Precision**: 1 second (not milliseconds in MVP).

**Storage**: Absolute expiration time (Unix timestamp), not relative TTL.

**Commands**:
- `SET key value EX seconds` - Set key with TTL
- `EXPIRE key seconds` - Set TTL on existing key
- `TTL key` - Query remaining TTL

---

### TTL Storage Format

```rust
pub struct Entry {
    value: Value,
    expiry: Option<Instant>,  // None = no TTL, Some = expiration time
}
```

**Instant vs Unix Timestamp**:
- `Instant`: Monotonic clock (not affected by system time changes)
- Stored as `Instant` in memory
- Serialized as Unix timestamp in snapshots

**Conversion**:
```rust
// Set TTL
let expiry = Instant::now() + Duration::from_secs(ttl_seconds);

// Check expiration
if Instant::now() >= expiry {
    // Expired
}

// Serialize
let unix_timestamp = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .as_secs() + ttl_seconds;
```

---

## Expiration Strategies

### Hybrid Approach

RsDragonflyDB uses **both** lazy and active expiration:

1. **Lazy Expiration**: Check TTL on access (GET, SET, etc.)
2. **Active Expiration**: Background task periodically scans for expired keys

**Rationale**:
- Lazy alone: Expired keys linger if never accessed (memory leak)
- Active alone: High CPU overhead scanning all keys
- Hybrid: Best of both worlds

---

### Lazy Expiration

**Trigger**: Every key access (GET, SET, DEL, etc.).

**Implementation**:
```rust
fn get(&mut self, key: &Key) -> Option<&Value> {
    let entry = self.data.get(key)?;
    
    // Check if expired
    if let Some(expiry) = entry.expiry {
        if Instant::now() >= expiry {
            // Expired: delete and return None
            self.data.remove(key);
            self.metrics.keys_expired.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    }
    
    Some(&entry.value)
}
```

**Performance**:
- Overhead: ~5ns per access (one comparison)
- No impact on latency (inline check)

**Limitation**: Keys never accessed remain in memory until active expiration.

---

### Active Expiration

**Trigger**: Background task runs every 100ms per shard.

**Algorithm**:
```rust
fn expire_keys(&mut self) {
    const KEYS_TO_CHECK: usize = 20;
    let now = Instant::now();
    let mut expired_count = 0;
    
    // Pop expired keys from TTL queue
    while let Some(expiry) = self.ttl_queue.peek() {
        if expiry.expiry_time > now {
            break;  // No more expired keys
        }
        
        let expiry = self.ttl_queue.pop().unwrap();
        
        // Verify key still exists and has same expiry
        if let Some(entry) = self.data.get(&expiry.key) {
            if entry.expiry == Some(expiry.expiry_time) {
                self.data.remove(&expiry.key);
                expired_count += 1;
            }
        }
        
        // Limit work per iteration
        if expired_count >= KEYS_TO_CHECK {
            break;
        }
    }
    
    self.metrics.keys_expired.fetch_add(expired_count, Ordering::Relaxed);
}
```

**Parameters**:
- **Frequency**: 100ms (10 times per second)
- **Keys per iteration**: 20 (configurable)
- **Max CPU time**: ~2μs per iteration (20 keys × 100ns)

**CPU Overhead**:
- Per shard: 10 iterations/sec × 2μs = 20μs/sec = 0.002% CPU
- Total (64 shards): 0.128% CPU
- **Negligible**

---

### TTL Queue Design

**Data Structure**: Min-heap (BinaryHeap).

**Ordering**: Earliest expiry at root.

**Operations**:
```rust
// Insert (when setting TTL)
self.ttl_queue.push(Expiry {
    expiry_time: Instant::now() + Duration::from_secs(ttl),
    key: key.clone(),
});

// Peek (check earliest expiry)
if let Some(expiry) = self.ttl_queue.peek() {
    if expiry.expiry_time <= Instant::now() {
        // Expired
    }
}

// Pop (remove earliest expiry)
let expiry = self.ttl_queue.pop().unwrap();
```

**Complexity**:
- Insert: O(log n)
- Peek: O(1)
- Pop: O(log n)

---

### Stale Entries in TTL Queue

**Problem**: When a key is deleted (DEL command), its TTL queue entry remains.

**Example**:
```
SET key1 "value" EX 60
→ HashMap: {key1: Entry}
→ TTL queue: [Expiry{key1, t+60}]

DEL key1
→ HashMap: {}
→ TTL queue: [Expiry{key1, t+60}]  ← Stale entry
```

**Impact**: TTL queue grows with deleted keys.

**Mitigation**: Verify key existence during active expiration.

```rust
// During active expiration
let expiry = self.ttl_queue.pop().unwrap();

if let Some(entry) = self.data.get(&expiry.key) {
    // Key exists: check if expiry matches
    if entry.expiry == Some(expiry.expiry_time) {
        self.data.remove(&expiry.key);  // Delete expired key
    }
    // Else: expiry was updated, ignore stale entry
} else {
    // Key already deleted, ignore stale entry
}
```

**Memory Overhead**: Stale entries consume memory until popped.

**Worst Case**: All keys deleted → TTL queue contains all stale entries.

**Cleanup**: Stale entries are removed lazily (when popped).

---

## TTL Command Semantics

### SET with EX

**Syntax**: `SET key value EX seconds`

**Behavior**:
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

**Edge Cases**:
- `EX 0`: Expire immediately (key is deleted)
- `EX -1`: Invalid (return error)
- `EX 2^63`: Overflow (clamp to max)

---

### EXPIRE

**Syntax**: `EXPIRE key seconds`

**Behavior**:
```rust
fn expire(&mut self, key: &Key, ttl_secs: u64) -> Result<i64, Error> {
    let entry = self.data.get_mut(key).ok_or(Error::KeyNotFound)?;
    
    let expiry_time = Instant::now() + Duration::from_secs(ttl_secs);
    entry.expiry = Some(expiry_time);
    
    // Insert into TTL queue (old entry becomes stale)
    self.ttl_queue.push(Expiry {
        expiry_time,
        key: key.clone(),
    });
    
    Ok(1)  // Key exists
}
```

**Return Value**:
- `1`: Key exists, TTL set
- `0`: Key does not exist

**Edge Cases**:
- Key already has TTL: Overwrite with new TTL (old TTL queue entry becomes stale)
- Key has no TTL: Add TTL

---

### TTL

**Syntax**: `TTL key`

**Behavior**:
```rust
fn ttl(&self, key: &Key) -> i64 {
    match self.data.get(key) {
        Some(entry) => {
            match entry.expiry {
                Some(expiry) => {
                    let remaining = expiry.saturating_duration_since(Instant::now());
                    remaining.as_secs() as i64
                }
                None => -1,  // Key exists but no TTL
            }
        }
        None => -2,  // Key does not exist
    }
}
```

**Return Values**:
- `>= 0`: Remaining seconds
- `-1`: Key exists but no TTL
- `-2`: Key does not exist

**Edge Cases**:
- Key expired but not yet deleted: Returns `-2` (lazy expiration triggers)

---

## Performance Characteristics

### Latency Impact

**Lazy Expiration**:
- Overhead: ~5ns per access
- Impact: Negligible (< 1% of total latency)

**Active Expiration**:
- Frequency: Every 100ms
- Duration: ~2μs per iteration
- Impact: Does not block command execution (runs between commands)

**TTL Queue Operations**:
- Insert: ~50ns (log n for 1M keys)
- Pop: ~50ns
- Impact: Negligible

---

### Memory Impact

**Per Key with TTL**:
- HashMap entry: 72 bytes + key_len + value_len
- TTL queue entry: 40 bytes + key_len
- **Total**: 112 bytes + 2 × key_len + value_len

**Stale Entries**:
- Worst case: All keys deleted → TTL queue size = original key count
- Memory: 40 bytes + key_len per stale entry
- Cleanup: Lazy (when popped)

**Mitigation**:
- Limit TTL queue size (future: cap at 10M entries)
- Periodic compaction (future: rebuild TTL queue)

---

### CPU Impact

**Active Expiration**:
- Per shard: 0.002% CPU
- Total (64 shards): 0.128% CPU
- **Negligible**

**Lazy Expiration**:
- Per access: 5ns
- At 1M QPS: 5ms/sec = 0.5% CPU
- **Negligible**

---

## Edge Cases and Corner Cases

### Case 1: System Time Change

**Problem**: If system time jumps backward, TTLs may expire prematurely.

**Mitigation**: Use `Instant` (monotonic clock), not `SystemTime`.

**Impact**: None (Instant is unaffected by system time changes).

---

### Case 2: Very Long TTL

**Problem**: TTL of 10 years (315M seconds) may overflow.

**Mitigation**: Clamp TTL to max value (2^31 seconds = 68 years).

```rust
const MAX_TTL_SECS: u64 = i32::MAX as u64;

fn set_with_ttl(&mut self, key: Key, value: Value, ttl_secs: u64) {
    let ttl_secs = ttl_secs.min(MAX_TTL_SECS);
    // ...
}
```

---

### Case 3: TTL During Snapshot

**Problem**: Key expires between snapshot start and snapshot write.

**Behavior**: Key is included in snapshot with expiry time.

**Recovery**: Expired keys are filtered during snapshot load.

```rust
fn deserialize(data: &[u8]) -> Result<Shard, Error> {
    // ...
    for entry in entries {
        if let Some(expiry) = entry.expiry {
            if Instant::now() >= expiry {
                continue;  // Skip expired key
            }
        }
        shard.set(entry.key, entry.value, entry.expiry);
    }
    // ...
}
```

---

### Case 4: Concurrent EXPIRE and GET

**Problem**: Client A sets TTL, Client B reads key before expiration.

**Scenario**:
```
t=0: SET key "value"
t=1: Client A: EXPIRE key 1
t=1.5: Client B: GET key → "value"
t=2.5: Client B: GET key → nil (expired)
```

**Behavior**: Correct (GET at t=1.5 returns value, GET at t=2.5 returns nil).

**Guarantee**: Lazy expiration ensures consistent behavior.

---

### Case 5: TTL Queue Overflow

**Problem**: TTL queue grows unbounded with stale entries.

**Mitigation** (future):
- Limit TTL queue size (e.g., 10M entries)
- If limit exceeded, rebuild queue (remove stale entries)

**MVP**: No limit (accept memory growth).

---

## Monitoring and Observability

### Metrics

**Per-Shard Metrics**:
```
shard_keys_expired_total{shard="0"}
shard_ttl_queue_size{shard="0"}
shard_active_expiration_duration_us{shard="0"}
```

**Alerting**:
- Alert if `ttl_queue_size > 10M` (potential memory leak)
- Alert if `active_expiration_duration_us > 10ms` (blocking shard)

---

### Logging

**Expired Key**:
```json
{
  "level": "debug",
  "shard_id": 0,
  "event": "key_expired",
  "key": "session:user123",
  "expiry_time": "2026-01-24T12:00:00Z",
  "method": "active"
}
```

**TTL Queue Overflow** (future):
```json
{
  "level": "warn",
  "shard_id": 0,
  "event": "ttl_queue_overflow",
  "queue_size": 10000000,
  "stale_entries": 5000000
}
```

---

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_lazy_expiration() {
    let mut shard = Shard::new(ShardId(0));
    shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), Some(Instant::now() + Duration::from_secs(1)));
    
    // Key exists before expiration
    assert!(shard.get(b"key").is_some());
    
    // Wait for expiration
    std::thread::sleep(Duration::from_secs(2));
    
    // Key is deleted on access
    assert!(shard.get(b"key").is_none());
}

#[test]
fn test_active_expiration() {
    let mut shard = Shard::new(ShardId(0));
    shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), Some(Instant::now() + Duration::from_secs(1)));
    
    // Wait for expiration
    std::thread::sleep(Duration::from_secs(2));
    
    // Run active expiration
    shard.expire_keys();
    
    // Key is deleted
    assert_eq!(shard.data.len(), 0);
}

#[test]
fn test_stale_ttl_queue_entries() {
    let mut shard = Shard::new(ShardId(0));
    shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), Some(Instant::now() + Duration::from_secs(60)));
    
    assert_eq!(shard.ttl_queue.len(), 1);
    
    // Delete key (TTL queue entry becomes stale)
    shard.del(&[b"key".to_vec()]);
    
    assert_eq!(shard.data.len(), 0);
    assert_eq!(shard.ttl_queue.len(), 1);  // Stale entry remains
    
    // Active expiration cleans up stale entry
    std::thread::sleep(Duration::from_secs(61));
    shard.expire_keys();
    
    assert_eq!(shard.ttl_queue.len(), 0);  // Stale entry removed
}
```

---

### Integration Tests

```rust
#[test]
fn test_ttl_across_restart() {
    let server = start_test_server();
    
    // Set key with TTL
    set_with_ttl(&server, "key", "value", 60);
    
    // Restart server
    restart_server(&server);
    
    // Key should still exist
    assert_eq!(get_key(&server, "key"), Some("value"));
    
    // Wait for expiration
    std::thread::sleep(Duration::from_secs(61));
    
    // Key should be expired
    assert_eq!(get_key(&server, "key"), None);
}
```

---

### Performance Tests

```rust
#[bench]
fn bench_lazy_expiration_overhead(b: &mut Bencher) {
    let mut shard = Shard::new(ShardId(0));
    shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), Some(Instant::now() + Duration::from_secs(3600)));
    
    b.iter(|| {
        shard.get(b"key");
    });
}

#[bench]
fn bench_active_expiration(b: &mut Bencher) {
    let mut shard = Shard::new(ShardId(0));
    
    // Insert 1000 keys with TTL
    for i in 0..1000 {
        let key = format!("key:{}", i).into_bytes();
        shard.set(key, Value::String(b"value".to_vec()), Some(Instant::now() + Duration::from_secs(1)));
    }
    
    std::thread::sleep(Duration::from_secs(2));
    
    b.iter(|| {
        shard.expire_keys();
    });
}
```

---

## Operational Runbook

### High TTL Queue Size

**Symptom**: `shard_ttl_queue_size` metric is very high (> 10M).

**Diagnosis**:
```bash
# Check TTL queue size per shard
curl http://localhost:9090/metrics | grep shard_ttl_queue_size

# Check key count per shard
curl http://localhost:9090/metrics | grep shard_key_count
```

**Resolution**:
1. If `ttl_queue_size >> key_count`: Many stale entries, restart server to rebuild queue
2. If `ttl_queue_size ≈ key_count`: Normal (most keys have TTL)

---

### Slow Active Expiration

**Symptom**: `shard_active_expiration_duration_us` is high (> 10ms).

**Diagnosis**:
```bash
# Check active expiration duration
curl http://localhost:9090/metrics | grep shard_active_expiration_duration_us
```

**Resolution**:
1. Reduce `KEYS_TO_CHECK` parameter (trade-off: slower expiration)
2. Increase active expiration frequency (trade-off: higher CPU usage)

---

## Definition of Done

TTL expiration is complete when:

1. Lazy and active expiration are implemented and tested
2. TTL queue handles stale entries correctly
3. Performance overhead is negligible (< 1% CPU)
4. Edge cases are handled (system time change, overflow, etc.)
5. Metrics and logging are in place
6. Integration tests verify TTL across restarts
7. Operational runbook covers common issues

---

## References

- [data-model.md](data-model.md) - In-memory data structures
- [persistence.md](../persistence/persistence.md) - TTL in snapshots
- [overview.md](../architecture/overview.md) - System architecture
