# Feature: Shard Engine

## 1. Purpose

The `crates/engine` crate implements the per-shard storage engine: the `Shard` struct and its synchronous event loop. Each shard owns a `HashMap<Key, Entry>` (the primary store) and a `BinaryHeap<Expiry>` (the TTL min-heap). The event loop receives `Command` messages, executes the appropriate operation, and sends back a `Response`.

This is the performance-critical core of the system. All write operations happen here; no shared mutable state exists across shard boundaries.

## 2. Responsibilities

- Own and manage `HashMap<Key, Entry>` exclusively per shard thread
- Implement all command operations: PING, GET, SET, DEL, EXPIRE, TTL, INFO
- Enforce per-shard memory and key-count limits
- Run the synchronous event loop: `recv_timeout(100ms)` → execute → reply
- Trigger periodic TTL expiration checks (every 100ms)
- Trigger periodic snapshot requests (every `snapshot_interval_secs`)
- Track per-shard metrics (key count, memory bytes, QPS, latency)
- Enforce lazy TTL expiration on every GET/TTL access

## 3. Non-Responsibilities

- Does not parse RESP2 protocol
- Does not manage TCP connections
- Does not write snapshots to disk (delegates to `crates/persistence` via channel)
- Does not handle INFO aggregation across shards (metrics exporter does that)

## 4. Architecture Design

```
Frontend thread
     │  Command { op, reply_tx }
     │  crossbeam::channel::unbounded
     ▼
┌────────────────────────────────────────────────┐
│  shard_worker_thread(shard_id, command_rx,     │
│                      snapshot_tx, config)      │
│                                                │
│  loop {                                        │
│    recv_timeout(100ms)                         │
│      ├─ Ok(cmd) → shard.execute(cmd)           │
│      │            cmd.reply_tx.send(response)  │
│      ├─ Timeout → (maintenance pass)           │
│      └─ Disconnected → break                  │
│                                               │
│    if last_expire >= 100ms:                    │
│      shard.expire_keys()                       │
│    if last_snapshot >= interval:               │
│      trigger_snapshot()                        │
│  }                                             │
│                                                │
│  Shard {                                       │
│    id: ShardId                                 │
│    data: HashMap<Key, Entry>                   │
│    ttl_queue: BinaryHeap<Expiry>               │
│    metrics: ShardMetrics                       │
│    config: Arc<Config>                         │
│  }                                             │
└────────────────────────────────────────────────┘
     │  SnapshotRequest { shard_id, data }
     │  crossbeam::channel::bounded(1)
     ▼
Snapshot writer thread
```

## 5. Core Data Structures

```rust
// crates/engine/src/shard.rs

use common::{Key, Value, Entry, ShardId, Expiry, ShardMetrics, Config,
             Command, Operation, Response, SnapshotRequest,
             MAX_KEYS_PER_SHARD, MAX_MEMORY_PER_SHARD, ENTRY_OVERHEAD};
use std::collections::{HashMap, BinaryHeap};
use std::sync::Arc;

/// One shard: owns all data for a disjoint subset of the keyspace.
/// Never shared across threads — not Send.
pub struct Shard {
    pub id: ShardId,
    pub data: HashMap<Key, Entry>,
    pub ttl_queue: BinaryHeap<Expiry>,
    pub metrics: ShardMetrics,
    config: Arc<Config>,
}

impl Shard {
    pub fn new(id: ShardId, config: Arc<Config>) -> Self {
        Self {
            id,
            data: HashMap::with_capacity(1_000_000),
            ttl_queue: BinaryHeap::new(),
            metrics: ShardMetrics::new(),
            config,
        }
    }

    /// Execute a command and return the response. Core hot path.
    pub fn execute(&mut self, op: &Operation) -> Response {
        match op {
            Operation::Ping { message } => self.ping(message),
            Operation::Get { key }      => self.get(key),
            Operation::Set { key, value, ttl_secs } => self.set(key, value.clone(), *ttl_secs),
            Operation::Del { keys }     => self.del(keys),
            Operation::Expire { key, ttl_secs } => self.expire(key, *ttl_secs),
            Operation::Ttl { key }      => self.ttl(key),
            Operation::Info { .. }      => Response::Error("INFO routed to metrics exporter".into()),
            Operation::Shutdown         => Response::Ok,
        }
    }
}
```

## 6. Public Interfaces

```rust
// Shard construction and execution
pub fn Shard::new(id: ShardId, config: Arc<Config>) -> Self
pub fn Shard::execute(&mut self, op: &Operation) -> Response

// Individual operations (called by execute; also exposed for testing)
pub fn Shard::ping(&self, message: &Option<Vec<u8>>) -> Response
pub fn Shard::get(&mut self, key: &Key) -> Response
pub fn Shard::set(&mut self, key: Key, value: Value, ttl_secs: Option<u64>) -> Response
pub fn Shard::del(&mut self, keys: &[Key]) -> Response
pub fn Shard::expire(&mut self, key: &Key, ttl_secs: u64) -> Response
pub fn Shard::ttl(&mut self, key: &Key) -> Response
pub fn Shard::expire_keys(&mut self)   // active TTL sweep

// Entry point for the shard OS thread
pub fn shard_worker_thread(
    shard_id: ShardId,
    command_rx: Receiver<Command>,
    snapshot_tx: Sender<SnapshotRequest>,
    config: Arc<Config>,
)
```

## 7. Internal Algorithms

### PING

```
ping(message):
  return Pong(message.clone())
```

### GET (with lazy expiration)

```
get(key):
  entry = data.get(key)?
  if let Some(expiry) = entry.expiry:
    if Instant::now() >= expiry:
      data.remove(key)
      metrics.key_count.fetch_sub(1, Relaxed)
      metrics.memory_bytes.fetch_sub(key.len() + entry.value.len() + ENTRY_OVERHEAD, Relaxed)
      return BulkString(None)
  return BulkString(Some(entry.value.as_bytes().to_vec()))
```

### SET

```
set(key, value, ttl_secs):
  // Check limits
  if data.len() >= MAX_KEYS_PER_SHARD: return Error("shard full")

  let entry_size = key.len() + value.len() + ENTRY_OVERHEAD
  if metrics.memory_bytes.load(Relaxed) + entry_size > MAX_MEMORY_PER_SHARD:
    return Error("OOM")

  // Compute expiry
  let expiry = ttl_secs.map(|s| Instant::now() + Duration::from_secs(s))

  // Update metrics for overwritten key
  if let Some(old_entry) = data.get(&key):
    metrics.memory_bytes.fetch_sub(key.len() + old_entry.value.len() + ENTRY_OVERHEAD, Relaxed)
    // key_count stays same (overwrite, not new key)
  else:
    metrics.key_count.fetch_add(1, Relaxed)

  // Insert or overwrite
  data.insert(key.clone(), Entry { value, expiry })
  metrics.memory_bytes.fetch_add(entry_size, Relaxed)

  // Push to TTL queue if TTL set
  if let Some(expiry_time) = expiry:
    ttl_queue.push(Expiry { expiry_time, key })

  return Ok
```

### DEL

```
del(keys):
  count = 0
  for key in keys:
    if let Some(entry) = data.remove(key):
      metrics.key_count.fetch_sub(1, Relaxed)
      metrics.memory_bytes.fetch_sub(key.len() + entry.value.len() + ENTRY_OVERHEAD, Relaxed)
      count += 1
  return Integer(count)
```

Note: TTL queue may still hold stale entries for deleted keys; these are cleaned up lazily in `expire_keys()`.

### EXPIRE

```
expire(key, ttl_secs):
  if !data.contains_key(key): return Integer(0)

  let expiry_time = Instant::now() + Duration::from_secs(ttl_secs)
  data.get_mut(key).unwrap().expiry = Some(expiry_time)
  ttl_queue.push(Expiry { expiry_time, key: key.clone() })
  return Integer(1)
```

### TTL

```
ttl(key):
  entry = data.get(key) else: return Integer(-2)  // not found
  match entry.expiry:
    None          → Integer(-1)   // exists, no TTL
    Some(expiry)  →
      let remaining = expiry.checked_duration_since(Instant::now())
      match remaining:
        None          → Integer(0)   // already expired (lazy not yet cleaned)
        Some(dur)     → Integer(dur.as_secs() as i64)
```

### Active TTL Expiration (expire_keys)

Runs every 100ms in the event loop. Processes up to 20 entries per call to bound latency impact.

```
expire_keys():
  now = Instant::now()
  processed = 0

  while processed < 20:
    peek = ttl_queue.peek() else: break
    if peek.expiry_time > now: break   // nothing more expired

    expiry = ttl_queue.pop().unwrap()
    processed += 1

    // Validate: key may have been deleted or TTL may have changed
    if let Some(entry) = data.get(&expiry.key):
      if entry.expiry == Some(expiry.expiry_time):
        data.remove(&expiry.key)
        metrics.key_count.fetch_sub(1, Relaxed)
        metrics.memory_bytes.fetch_sub(expiry.key.len() + ... , Relaxed)
        metrics.keys_expired.fetch_add(1, Relaxed)
    // else: stale queue entry, discard silently
```

### Shard Worker Event Loop

```
shard_worker_thread(shard_id, command_rx, snapshot_tx, config):
  pin_to_core(shard_id) if cpu_pinning enabled

  shard = Shard::new(shard_id, config)
  last_expire  = Instant::now()
  last_snapshot = Instant::now()

  loop:
    match command_rx.recv_timeout(Duration::from_millis(100)):
      Ok(cmd) →
        start = Instant::now()
        response = shard.execute(&cmd.op)
        latency = start.elapsed()
        cmd.reply_tx.send(response).ok()
        shard.metrics.record_command(latency)

      Err(RecvTimeoutError::Disconnected) → break

      Err(RecvTimeoutError::Timeout) → {}  // fall through to maintenance

    if last_expire.elapsed() >= 100ms:
      shard.expire_keys()
      last_expire = Instant::now()

    if last_snapshot.elapsed() >= config.snapshot_interval_secs:
      trigger_snapshot(&shard, &snapshot_tx)
      last_snapshot = Instant::now()
```

### Snapshot Trigger

```
trigger_snapshot(shard, snapshot_tx):
  data = persistence::serialize_shard(shard)
  match snapshot_tx.try_send(SnapshotRequest { shard_id: shard.id, data }):
    Ok(())                     → log debug "snapshot triggered"
    Err(TrySendError::Full)    →
      log warn "snapshot writer busy, skipping"
      shard.metrics.snapshots_skipped.fetch_add(1, Relaxed)
    Err(TrySendError::Disconnected) → log error "snapshot writer gone"
```

## 8. Persistence Model

The shard engine does not write to disk. It serializes its state in-memory and hands the bytes to the persistence crate via a channel. See `plan-persistence.md` for the write procedure.

On startup, the shard engine receives a pre-loaded `HashMap<Key, Entry>` (or an empty one) from the persistence crate; it does not read snapshots directly.

## 9. Concurrency Model

- The `Shard` struct is NOT `Send` (contains `HashMap` which is not `Send`). Only the shard thread may access it.
- `ShardMetrics` fields are atomic; the metrics exporter thread reads them concurrently without coordination with the shard thread.
- The `Mutex<Histogram>` in `ShardMetrics` is held by the shard thread only during `record_command` (< 1μs), and by the metrics exporter at scrape time. These windows do not overlap in practice.
- No locks exist on the `HashMap`, `BinaryHeap`, or entry read/write path.

## 10. Configuration

| Parameter | Source | Default | Notes |
|-----------|--------|---------|-------|
| `snapshot_interval_secs` | `Config` | 60 | Seconds between snapshots |
| `cpu_pinning` | `Config` | false | Pin shard thread to CPU core |
| `MAX_KEYS_PER_SHARD` | `common::config` | 100M | Compile-time limit |
| `MAX_MEMORY_PER_SHARD` | `common::config` | 20GB | Compile-time limit |

## 11. Observability

The shard updates `ShardMetrics` after each command:
- `commands_processed` — incremented per command
- `key_count` — updated on SET (new key), DEL, and expiration
- `memory_bytes` — updated on SET, DEL, and expiration
- `keys_expired` — incremented per active expiration
- `latency_histogram` — records per-command latency (μs)
- `snapshots_written` / `snapshots_skipped` — updated by trigger_snapshot

Structured log events:
- `INFO`: snapshot triggered per shard
- `WARN`: snapshot writer busy (skipped)
- `ERROR`: snapshot writer disconnected; shard panicked (caught by catch_unwind in server)

## 12. Testing Strategy

- **Unit tests**:
  - `test_set_get`: set a key, get it back, assert value equality
  - `test_get_missing_key`: get non-existent key, assert `BulkString(None)`
  - `test_set_overwrite`: set key twice, assert second value is returned
  - `test_del_existing`: set then del, assert count=1, get returns None
  - `test_del_missing`: del non-existent key, assert count=0
  - `test_del_multi_key`: set 3 keys, del all 3, assert count=3
  - `test_set_with_ttl_not_yet_expired`: set with 10s TTL, immediately get, assert value present
  - `test_set_with_ttl_expired_lazy`: set with 1ms TTL, sleep 2ms, get — assert lazy expiry returns None
  - `test_expire_command`: set key without TTL, call expire 1s, TTL returns ~1
  - `test_expire_nonexistent`: expire on missing key returns 0
  - `test_ttl_no_expiry`: set without TTL, TTL returns -1
  - `test_ttl_missing`: TTL on missing key returns -2
  - `test_ttl_with_expiry`: set with 10s TTL, TTL returns value between 1 and 10
  - `test_active_expiry`: push an entry with expired timestamp to ttl_queue, call expire_keys, assert removed
  - `test_key_count_limit`: insert keys up to MAX_KEYS_PER_SHARD, assert next SET returns Error
  - `test_memory_limit`: track memory bytes, assert OOM error when limit reached
  - `test_shard_worker_thread`: spawn thread, send GET command over channel, receive response
  - `test_shard_thread_shutdown`: drop sender, verify thread exits cleanly

- **Integration tests**:
  - `test_concurrent_clients_no_races`: 50 threads each doing 1000 SET/GET on same shard, no panics, ThreadSanitizer clean

## 13. Open Questions

None.
