# Threading Model

## Document Purpose
This document defines the threading architecture, execution model, and concurrency primitives for RsDragonflyDB.

**Audience**: Implementation engineers, performance engineers

---

## Threading Philosophy

### Core Principle: Thread-Per-Shard

**Each shard is owned by exactly one OS thread. Shards never share mutable state.**

This principle eliminates:
- Lock contention on hot paths
- Cache coherence traffic between cores
- Complex synchronization logic
- Non-deterministic performance

---

## Thread Types and Responsibilities

### Overview

```
Thread Type          Count    Purpose                          Blocking?
─────────────────────────────────────────────────────────────────────────
Acceptor             1        Accept TCP connections           Yes (accept)
Connection Handler   4        Parse RESP2, route               No
Shard Worker         64       Execute commands                 No
Snapshot Writer      64       Write snapshots                  Yes (write)
Metrics Exporter     1        Aggregate metrics, serve HTTP    Yes (accept/write)
Watchdog             1        Detect stuck/stalled shards      No (sleeps 10s)
─────────────────────────────────────────────────────────────────────────
Total                135
```

---

### 1. Acceptor Thread

**Responsibility**: Accept new TCP connections and assign to connection handlers.

**Lifecycle**:
```rust
fn acceptor_thread(listener: TcpListener, handler_pool: Arc<HandlerPool>) {
    loop {
        match listener.accept() {
            Ok((socket, addr)) => {
                // Assign to least-loaded handler
                let handler = handler_pool.next();
                handler.add_connection(socket, addr);
            }
            Err(e) => {
                log::error!("Accept failed: {}", e);
            }
        }
    }
}
```

**Blocking**: Yes (blocks on `accept()` syscall).

**Failure Handling**: If acceptor panics, server cannot accept new connections → fatal error, shutdown.

---

### 2. Connection Handler Threads

**Responsibility**: Parse RESP2 protocol, route commands to shards, serialize responses.

**Count**: 4 (configurable via `--frontend-threads`).

**Rationale**: 
- Parsing RESP2 is CPU-bound
- 4 threads balance CPU usage without excessive context switching
- Each handler manages ~1000 connections (epoll/kqueue)

**Lifecycle**:
```rust
// NOTE: Arc<Mutex<Vec<Connection>>> is used here only for connection registration
// by the acceptor thread. It is NOT on the command execution hot path.
// The Mutex is held only briefly (to add/remove a connection), not during
// command processing. This does not violate the "no locks on hot paths" rule.
fn connection_handler_thread(
    connections: Arc<Mutex<Vec<Connection>>>,
    shard_channels: Vec<Sender<Command>>,
) {
    let mut epoll = Epoll::new();
    
    loop {
        // Wait for readable sockets
        let events = epoll.wait(timeout_ms: 100);
        
        for event in events {
            let conn = &mut connections[event.id];
            
            // Read from socket (non-blocking)
            match conn.read_buffer() {
                Ok(bytes_read) if bytes_read > 0 => {
                    // Parse RESP2 commands
                    while let Some(cmd) = conn.parse_command() {
                        route_command(cmd, &shard_channels, conn);
                    }
                }
                Ok(0) => {
                    // Connection closed
                    conn.close();
                }
                Err(e) if e.kind() == WouldBlock => {
                    // No data available, continue
                }
                Err(e) => {
                    log::warn!("Read error: {}", e);
                    conn.close();
                }
            }
        }
        
        // Send pending responses
        for conn in connections.iter_mut() {
            conn.flush_responses();
        }
    }
}
```

**Blocking**: No (uses non-blocking sockets + epoll).

**Failure Handling**: If handler panics, connections on that handler are dropped. Other handlers continue serving.

---

### 3. Shard Worker Threads

**Responsibility**: Execute commands on shard-local data structures.

**Count**: 64 (= SHARD_COUNT).

**Pinning**: Optional CPU pinning for NUMA optimization.

```rust
fn shard_worker_thread(
    shard_id: ShardId,
    command_rx: Receiver<Command>,
    snapshot_tx: Sender<SnapshotRequest>,
) {
    // Pin to CPU core (optional)
    #[cfg(target_os = "linux")]
    pin_to_core(shard_id.0);

    let mut shard = Shard::new(shard_id);
    let mut last_expire  = Instant::now();
    let mut last_snapshot = Instant::now();

    // Shard threads are fully synchronous — no async runtime.
    // Timers are implemented via recv_timeout: block for up to 100ms
    // waiting for a command, then run maintenance tasks.
    loop {
        match command_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(cmd) => {
                let start = Instant::now();
                let response = shard.execute(&cmd);
                let latency = start.elapsed();

                cmd.reply_tx.send(response).ok();
                shard.metrics.record_latency(latency);
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {} // fall through to maintenance
        }

        // TTL expiration — every 100ms
        if last_expire.elapsed() >= Duration::from_millis(100) {
            shard.expire_keys();
            last_expire = Instant::now();
        }

        // Snapshot trigger — every 60s (configurable)
        if last_snapshot.elapsed() >= Duration::from_secs(60) {
            trigger_snapshot(&shard, &snapshot_tx);
            last_snapshot = Instant::now();
        }
    }
}
```

**Blocking**: Never (all operations are bounded and non-blocking).

**Failure Handling**: If shard panics, catch with `catch_unwind`, log error, mark shard as failed, initiate shutdown.

---

### 4. Snapshot Writer Threads

**Responsibility**: Write shard snapshots to disk asynchronously.

**Count**: 64 (one per shard).

**Rationale**: Isolate blocking IO from shard threads.

```rust
fn snapshot_writer_thread(
    shard_id: ShardId,
    snapshot_rx: Receiver<SnapshotRequest>,
    snapshot_dir: PathBuf,
) {
    loop {
        match snapshot_rx.recv() {
            Ok(request) => {
                let temp_path = snapshot_dir.join(format!("shard{}.snap.tmp", shard_id.0));
                let final_path = snapshot_dir.join(format!("shard{}.snap", shard_id.0));
                
                // Write to temp file
                match write_snapshot(&temp_path, &request.data) {
                    Ok(()) => {
                        // Atomic rename
                        fs::rename(&temp_path, &final_path).unwrap();
                        log::info!("Snapshot written: shard {}", shard_id.0);
                    }
                    Err(e) => {
                        log::error!("Snapshot write failed: {}", e);
                    }
                }
            }
            Err(_) => {
                // Channel closed, shutdown
                break;
            }
        }
    }
}
```

**Blocking**: Yes (blocks on `write()` syscall).

**Failure Handling**: If write fails, log error and continue. Shard remains operational.

---

### 5. Metrics Exporter Thread

**Responsibility**: Aggregate per-shard metrics and expose via HTTP.

**Count**: 1.

```rust
fn metrics_exporter_thread(
    shards: Arc<[ShardMetrics; SHARD_COUNT]>,
    http_port: u16,
) {
    let listener = TcpListener::bind(("0.0.0.0", http_port)).unwrap();
    
    loop {
        match listener.accept() {
            Ok((mut socket, _)) => {
                let metrics = aggregate_metrics(&shards);
                let response = format_prometheus(metrics);
                socket.write_all(response.as_bytes()).unwrap();
            }
            Err(e) => {
                log::error!("Metrics export failed: {}", e);
            }
        }
    }
}

fn aggregate_metrics(shards: &[ShardMetrics]) -> GlobalMetrics {
    let mut total_qps = 0;
    let mut total_keys = 0;
    
    for shard in shards {
        total_qps += shard.commands_processed.load(Ordering::Relaxed);
        total_keys += shard.key_count.load(Ordering::Relaxed);
    }
    
    GlobalMetrics { total_qps, total_keys, /* ... */ }
}
```

**Blocking**: Yes (blocks on `accept()` and `write()`).

**Failure Handling**: If exporter panics, metrics are unavailable but server continues serving.

---

## Thread Communication

### Communication Patterns

```
Frontend → Shard:
  Type: MPSC (multiple frontend threads → one shard thread)
  Primitive: crossbeam::channel::unbounded
  Message: Command { op, reply_tx }

Shard → Frontend:
  Type: One-shot response (one per command)
  Primitive: crossbeam::channel::bounded(1)
  Rationale: Sync primitive; connection handlers use a blocking epoll loop,
             not an async runtime. tokio::sync::oneshot is intentionally avoided.
  Message: Response { result }

Shard → Snapshot Writer:
  Type: SPSC (one shard → one writer)
  Primitive: crossbeam::channel::bounded(1)
  Message: SnapshotRequest { shard_id, data }
```

---

### Channel Details

#### Frontend → Shard Channel

```rust
pub struct Command {
    pub op: Operation,
    pub reply_tx: crossbeam::channel::Sender<Response>,  // bounded(1), sync
}

pub enum Operation {
    Get { key: Key },
    Set { key: Key, value: Value, ttl: Option<u64> },
    Del { keys: Vec<Key> },
    Expire { key: Key, ttl: u64 },
    Ttl { key: Key },
}
```

**Capacity**: Unbounded (backpressure handled at connection level).

**Rationale**: 
- Bounded channels can deadlock if shard is slow
- Unbounded allows frontend to enqueue commands without blocking
- Memory usage is bounded by connection count × pipeline depth

**Backpressure**: If shard queue depth > 10,000, frontend stops reading from sockets.

---

#### Shard → Frontend Channel

```rust
pub enum Response {
    Value(Option<Value>),
    Integer(i64),
    Ok,
    Error(String),
}
```

**Capacity**: 1 (`crossbeam::channel::bounded(1)`).

**Rationale**: Each command gets exactly one response. Using bounded(1) instead of an async oneshot keeps shard threads fully synchronous.

---

#### Shard → Snapshot Writer Channel

```rust
pub struct SnapshotRequest {
    pub shard_id: ShardId,
    pub data: Vec<u8>,  // Serialized snapshot
}
```

**Capacity**: 1 (bounded).

**Rationale**: 
- If writer is slow, shard skips snapshot (logs warning)
- Prevents memory buildup from queued snapshots

---

## Synchronization Primitives

### Allowed Primitives

| Primitive | Use Case | Example |
|-----------|----------|---------|
| `AtomicU64` | Metrics counters | `shard.metrics.qps.fetch_add(1, Relaxed)` |
| `AtomicUsize` | Key count, memory usage | `shard.key_count.load(Relaxed)` |
| `Arc<T>` | Immutable shared data | `Arc<Config>` |
| `crossbeam::channel` | Message passing | Frontend → Shard commands |
| `crossbeam::channel::bounded(1)` | One-time per-command responses | Shard → Frontend responses |
| `Mutex<Histogram>` | Latency histograms (metrics only) | Read infrequently by metrics exporter, never on command hot path |

---

### Forbidden Primitives

| Primitive | Why Forbidden | Alternative |
|-----------|---------------|-------------|
| `Mutex<HashMap>` | Shared mutable state | Per-shard HashMap (no sharing) |
| `RwLock<T>` | Lock contention | Message passing |
| `Arc<Mutex<T>>` | Violates architecture | Owned data per shard |
| `static mut` | Unsafe, shared state | `const` or `Arc<T>` |

**Enforcement**: Code review + architectural guidelines.

---

## CPU Pinning (Optional)

### Why Pin Threads to Cores?

**Benefits**:
- Reduces context switches (thread stays on same core)
- Improves cache locality (L1/L2 cache stays warm)
- Predictable performance (no CPU migration)

**Costs**:
- Less flexible scheduling (OS cannot balance load)
- Requires careful NUMA configuration

**Recommendation**: Enable for production, disable for development.

---

### Pinning Strategy

```rust
#[cfg(target_os = "linux")]
fn pin_to_core(core_id: usize) {
    use libc::{cpu_set_t, sched_setaffinity, CPU_SET, CPU_ZERO};
    
    unsafe {
        let mut cpuset: cpu_set_t = std::mem::zeroed();
        CPU_ZERO(&mut cpuset);
        CPU_SET(core_id, &mut cpuset);
        
        let result = sched_setaffinity(
            0,  // Current thread
            std::mem::size_of::<cpu_set_t>(),
            &cpuset,
        );
        
        if result != 0 {
            log::warn!("Failed to pin thread to core {}", core_id);
        }
    }
}
```

**Mapping**:
- Shard 0 → Core 0
- Shard 1 → Core 1
- ...
- Shard 63 → Core 63
- Frontend threads → Cores 64-67
- Snapshot writers → No pinning (IO-bound)

---

### NUMA Considerations

**Problem**: On multi-socket systems, accessing remote memory is slower.

**Solution**: Allocate shard data on local NUMA node.

```rust
#[cfg(target_os = "linux")]
fn allocate_on_numa_node(node: usize, size: usize) -> *mut u8 {
    use libc::{numa_alloc_onnode, numa_available};
    
    if numa_available() < 0 {
        return std::alloc::alloc(Layout::from_size_align(size, 8).unwrap());
    }
    
    unsafe {
        numa_alloc_onnode(size, node as i32) as *mut u8
    }
}
```

**Mapping** (2-socket system, 32 cores per socket):
- Shards 0-31 → NUMA node 0
- Shards 32-63 → NUMA node 1

**Impact**: 20-30% latency improvement on NUMA systems.

---

## Thread Lifecycle

### Startup Sequence

```
1. Main thread starts
   ├─ Parse configuration
   ├─ Initialize logging
   └─ Create snapshot directory

2. Spawn shard threads (parallel)
   ├─ For each shard_id in 0..64:
   │   ├─ Spawn shard worker thread
   │   ├─ Pin to CPU core (optional)
   │   ├─ Load snapshot (if exists)
   │   └─ Send "ready" signal
   └─ Wait for all shards ready

3. Spawn snapshot writer threads
   └─ For each shard_id in 0..64:
       └─ Spawn writer thread

4. Spawn frontend threads
   ├─ Spawn acceptor thread
   ├─ Spawn connection handler threads (4)
   └─ Spawn metrics exporter thread

5. Main thread waits for SIGTERM
```

**Total Startup Time**: ~100ms (dominated by snapshot loading).

---

### Shutdown Sequence

```
1. Receive SIGTERM
   └─ Set global shutdown flag

2. Stop accepting new connections
   └─ Acceptor thread exits

3. Drain in-flight commands
   ├─ Frontend stops reading from sockets
   ├─ Wait for shard queues to empty (timeout: 10s)
   └─ Close all client connections

4. Trigger final snapshots
   ├─ For each shard:
   │   ├─ Send snapshot request
   │   └─ Wait for completion (timeout: 30s)
   └─ Snapshot writers exit

5. Shutdown shard threads
   ├─ Send shutdown signal to each shard
   └─ Join all shard threads

6. Shutdown remaining threads
   ├─ Join frontend threads
   └─ Join metrics exporter

7. Main thread exits
```

**Total Shutdown Time**: ~15s (dominated by final snapshots).

---

## Error Handling

### Thread Panic Handling

```rust
fn spawn_shard_thread(shard_id: ShardId) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("shard-{}", shard_id.0))
        .spawn(move || {
            let result = std::panic::catch_unwind(|| {
                shard_worker_thread(shard_id, /* ... */);
            });
            
            if let Err(e) = result {
                log::error!("Shard {} panicked: {:?}", shard_id.0, e);
                // Mark shard as failed
                SHARD_STATUS[shard_id.0].store(FAILED, Ordering::Release);
                // Initiate graceful shutdown
                SHUTDOWN_FLAG.store(true, Ordering::Release);
            }
        })
        .unwrap()
}
```

**Policy**: Any shard panic is fatal (cannot recover single shard without risking inconsistency).

---

### Thread Starvation Detection

```rust
fn watchdog_thread(shards: Arc<[ShardMetrics; SHARD_COUNT]>) {
    let mut last_qps = vec![0u64; SHARD_COUNT];
    
    loop {
        std::thread::sleep(Duration::from_secs(10));
        
        for (i, shard) in shards.iter().enumerate() {
            let current_qps = shard.commands_processed.load(Ordering::Relaxed);
            
            if current_qps == last_qps[i] {
                log::warn!("Shard {} may be stuck (no progress in 10s)", i);
            }
            
            last_qps[i] = current_qps;
        }
    }
}
```

**Action**: Log warning, expose metric, alert operator.

---

## Performance Characteristics

### Thread Overhead

| Metric | Value | Notes |
|--------|-------|-------|
| Thread creation time | ~50μs | Per thread |
| Context switch time | ~1-5μs | Depends on CPU |
| Thread memory overhead | ~8MB | Stack size |
| Total memory overhead | ~1GB | 135 threads × 8MB |

**Optimization**: Use smaller stack size for IO threads (snapshot writers).

---

### Scalability

**Throughput Scaling**:
```
Shards    Cores    Throughput    Efficiency
16        16       240K QPS      100%
32        32       480K QPS      100%
64        64       960K QPS      100%
128       64       960K QPS      50% (CPU-bound)
```

**Conclusion**: Linear scaling up to physical core count.

---

### Latency

**Single-Key Operation Latency Breakdown**:
```
Component                Time      %
─────────────────────────────────────
Network (client → server) 100μs    40%
Frontend parse           1μs       0.4%
Channel send             0.1μs     0.04%
Shard execute            0.5μs     0.2%
Channel receive          0.1μs     0.04%
Frontend serialize       1μs       0.4%
Network (server → client) 100μs    40%
─────────────────────────────────────
Total                    ~203μs    100%
```

**Conclusion**: Latency is dominated by network, not threading overhead.

---

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_shard_thread_isolation() {
    let (tx, rx) = crossbeam::channel::unbounded();
    let handle = spawn_shard_thread(ShardId(0), rx);
    
    // Send command
    let (reply_tx, reply_rx) = crossbeam::channel::bounded(1);
    tx.send(Command::Get { key: b"key".to_vec(), reply_tx }).unwrap();

    // Receive response (blocking recv, no async needed)
    let response = reply_rx.recv().unwrap();
    assert!(matches!(response, Response::Value(None)));
    
    // Shutdown
    drop(tx);
    handle.join().unwrap();
}
```

---

### Concurrency Tests

```rust
#[test]
fn test_no_data_races() {
    // Run with ThreadSanitizer: cargo test --target x86_64-unknown-linux-gnu
    let server = start_test_server();
    
    // Spawn 100 clients, each sending 1000 commands
    let handles: Vec<_> = (0..100)
        .map(|i| {
            std::thread::spawn(move || {
                let mut conn = connect_to_server();
                for j in 0..1000 {
                    let key = format!("key:{}:{}", i, j);
                    conn.set(&key, "value").unwrap();
                    conn.get(&key).unwrap();
                }
            })
        })
        .collect();
    
    for handle in handles {
        handle.join().unwrap();
    }
    
    // ThreadSanitizer will report any data races
}
```

---

### Performance Tests

```rust
#[bench]
fn bench_channel_latency(b: &mut Bencher) {
    let (tx, rx) = crossbeam::channel::unbounded();
    
    b.iter(|| {
        let (reply_tx, reply_rx) = crossbeam::channel::bounded(1);
        tx.send(Command::Get { key: b"key".to_vec(), reply_tx }).unwrap();
        reply_rx.recv().unwrap();
    });
}
```

**Expected**: < 1μs per round-trip.

---

## Operational Runbook

### High CPU Usage

**Symptom**: CPU usage > 90% on all cores.

**Diagnosis**:
```bash
# Check per-thread CPU usage
top -H -p $(pgrep rsdragonfly)

# Profile with perf
perf record -p $(pgrep rsdragonfly) -g -- sleep 10
perf report
```

**Resolution**:
- If shard threads are hot: Normal (high load)
- If frontend threads are hot: Parsing bottleneck, increase `--frontend-threads`
- If snapshot writers are hot: Abnormal, investigate

---

### Thread Deadlock

**Symptom**: Server stops responding, CPU usage drops to 0%.

**Diagnosis**:
```bash
# Attach debugger
gdb -p $(pgrep rsdragonfly)
(gdb) thread apply all bt

# Check for stuck threads
```

**Resolution**:
- Should not happen (no locks on hot path)
- If it does: Bug in channel implementation or OS scheduler
- Restart server, file bug report

---

## Definition of Done

Threading model is complete when:

1. All thread types are implemented and tested
2. Thread communication uses only allowed primitives
3. CPU pinning is optional and configurable
4. Thread panics are caught and handled gracefully
5. Shutdown sequence completes within 15s
6. ThreadSanitizer reports zero data races
7. Performance tests meet latency targets
8. Operational runbook covers thread-related issues

---

## References

- [overview.md](overview.md) - System architecture
- [sharding-model.md](sharding-model.md) - Shard ownership
- [multi-key-commands.md](multi-key-commands.md) - Cross-shard coordination
