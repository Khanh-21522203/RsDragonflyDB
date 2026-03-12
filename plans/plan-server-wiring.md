# Feature: Server Wiring

## 1. Purpose

The `crates/server` crate is the main binary entry point. It owns the top-level startup and shutdown sequence: parsing configuration, loading snapshots in parallel, spawning all 135 OS threads (shard workers, snapshot writers, frontend handlers, metrics exporter, watchdog), wiring channels between them, handling SIGTERM/SIGINT, and coordinating the shutdown sequence.

This crate does not implement any feature logic — it composes all other crates into a running system.

## 2. Responsibilities

- Parse and validate `Config` from argv and environment
- Initialize logging
- Create all `crossbeam` channels (frontend→shard, shard→snapshot writer)
- Load snapshots in parallel (one per shard, concurrent)
- Spawn all thread types in the correct startup order
- Set `shards_ready` flag once all snapshots are loaded
- Register SIGTERM/SIGINT handler
- Block main thread until shutdown signal
- Execute graceful shutdown: drain commands → final snapshots → join threads → exit

## 3. Non-Responsibilities

- Does not implement command execution (engine crate)
- Does not implement persistence format (persistence crate)
- Does not implement RESP2 protocol (protocol crate)
- Does not implement metrics formatting (observability crate)

## 4. Architecture Design

```
main()
  │
  ├─ parse_config(argv, env)
  ├─ init_logging(config.log_level)
  ├─ cleanup_temp_snapshot_files(config.snapshot_dir)
  │
  ├─ create channels:
  │   for each shard:
  │     (cmd_tx, cmd_rx) = unbounded()     // frontend → shard
  │     (snap_tx, snap_rx) = bounded(1)    // shard → snapshot writer
  │
  ├─ load snapshots (parallel, one thread per shard):
  │   for each shard:
  │     initial_data = load_snapshot(shard_N.snap) or HashMap::new()
  │
  ├─ spawn shard worker threads (64):
  │   thread::spawn(shard_worker_thread(shard_id, cmd_rx, snap_tx, config))
  │
  ├─ spawn snapshot writer threads (64):
  │   thread::spawn(snapshot_writer_thread(shard_id, snap_rx, snapshot_dir, metrics))
  │
  ├─ set shards_ready = true
  │
  ├─ spawn metrics exporter thread (1):
  │   thread::spawn(metrics_exporter_thread(shard_metrics, config, ...))
  │
  ├─ spawn watchdog thread (1):
  │   thread::spawn(watchdog_thread(shard_metrics, shutdown_flag))
  │
  ├─ spawn frontend threads:
  │   ├─ spawn acceptor thread (1)
  │   └─ spawn connection handler threads (frontend_threads, default 4)
  │
  ├─ register SIGTERM / SIGINT handler → sets shutdown_flag
  │
  └─ main thread blocks: shutdown_flag.wait()
         │
         ▼
  graceful_shutdown(all handles, shard_txs, shard_metrics, snapshot_txs)
  process::exit(0)
```

### Thread Count Summary

| Thread Type | Count | Notes |
|-------------|-------|-------|
| Shard Worker | 64 | = SHARD_COUNT |
| Snapshot Writer | 64 | = SHARD_COUNT |
| Connection Handler | 4 | configurable via `--frontend-threads` |
| Acceptor | 1 | |
| Metrics Exporter | 1 | |
| Watchdog | 1 | |
| **Total** | **135** | for default SHARD_COUNT=64, frontend_threads=4 |

## 5. Core Data Structures

```rust
// crates/server/src/main.rs

use common::{Config, ShardMetrics, ShardId, Command, SnapshotRequest, SHARD_COUNT};
use crossbeam::channel::{unbounded, bounded, Sender, Receiver};
use std::sync::{Arc, atomic::{AtomicBool, AtomicU8, Ordering}};

/// Global shard status — set by shard panic handler.
static SHARD_STATUS: [AtomicU8; SHARD_COUNT] = /* ALIVE = 0, FAILED = 1 */;

const ALIVE: u8 = 0;
const FAILED: u8 = 1;

/// All handles and channels needed for a clean shutdown.
struct ServerHandles {
    shard_handles:    Vec<JoinHandle<()>>,
    snapshot_handles: Vec<JoinHandle<()>>,
    frontend_handles: Vec<JoinHandle<()>>,
    metrics_handle:   JoinHandle<()>,
    watchdog_handle:  JoinHandle<()>,
    shard_txs:        Vec<Sender<Command>>,
    snapshot_txs:     Vec<Sender<SnapshotRequest>>,
    shard_metrics:    Arc<[ShardMetrics; SHARD_COUNT]>,
    shutdown_flag:    Arc<AtomicBool>,
}
```

## 6. Public Interfaces

```rust
// Entry point
fn main()

// Shutdown (called from signal handler or panic handler)
fn graceful_shutdown(handles: ServerHandles, timeout: Duration)

// Shard thread panic wrapper
fn spawn_shard_thread(
    shard_id: ShardId,
    cmd_rx: Receiver<Command>,
    snap_tx: Sender<SnapshotRequest>,
    config: Arc<Config>,
    initial_data: HashMap<Key, Entry>,
) -> JoinHandle<()>
```

## 7. Internal Algorithms

### Startup Sequence

```
main():
  // 1. Parse configuration
  let config = Arc::new(parse_config(env::args().collect(), &env::vars().collect())
    .unwrap_or_else(|e| { eprintln!("Configuration error: {}", e); process::exit(1) }))

  // 2. Initialize logging
  init_logging(&config.log_level)
  log INFO "starting RsDragonflyDB" { version, shard_count: SHARD_COUNT, port: config.port }

  // 3. Clean up stale temp files
  cleanup_temp_files(&config.snapshot_dir)

  // 4. Create channels for all shards
  let (cmd_txs, cmd_rxs): (Vec<_>, Vec<_>) = (0..SHARD_COUNT)
    .map(|_| unbounded::<Command>())
    .unzip()
  let (snap_txs, snap_rxs): (Vec<_>, Vec<_>) = (0..SHARD_COUNT)
    .map(|_| bounded::<SnapshotRequest>(1))
    .unzip()

  // 5. Allocate shared metrics array
  let shard_metrics: Arc<[ShardMetrics; SHARD_COUNT]> = Arc::new(/* array of ShardMetrics::new() */)

  // 6. Load snapshots in parallel
  let initial_data: Vec<HashMap<Key, Entry>> = (0..SHARD_COUNT)
    .into_par_iter()  // rayon parallel iterator (or manual thread::scope)
    .map(|shard_id| {
      let path = config.snapshot_dir.join(format!("shard{}.snap", shard_id))
      match load_snapshot(&path):
        Ok(data) → { log INFO "loaded shard {}", shard_id; data }
        Err(e)   → { log error "failed to load shard {}: {}, starting empty", shard_id, e; HashMap::new() }
    })
    .collect()

  // 7. Spawn shard worker threads
  let shard_handles: Vec<_> = (0..SHARD_COUNT).map(|i| {
    spawn_shard_thread(ShardId(i as u16), cmd_rxs[i], snap_txs[i].clone(),
                       config.clone(), initial_data[i].take())
  }).collect()

  // 8. Spawn snapshot writer threads
  let snapshot_handles: Vec<_> = (0..SHARD_COUNT).map(|i| {
    thread::Builder::new()
      .name(format!("snapshot-{}", i))
      .spawn(move || snapshot_writer_thread(ShardId(i as u16), snap_rxs[i],
                                            config.snapshot_dir.clone(), shard_metrics.clone()))
      .unwrap()
  }).collect()

  // 9. Set ready flag
  let shards_ready = Arc::new(AtomicBool::new(true))
  log INFO "all shards ready, accepting connections"

  // 10. Spawn metrics exporter thread
  let metrics_handle = start_metrics_exporter(shard_metrics.clone(), config.clone(), ...)

  // 11. Spawn watchdog thread
  let watchdog_handle = thread::Builder::new()
    .name("watchdog")
    .spawn(move || watchdog_thread(shard_metrics.clone(), shutdown_flag.clone()))
    .unwrap()

  // 12. Spawn frontend threads
  let frontend_handle = start_frontend(config.clone(), Arc::new(cmd_txs), shard_metrics.clone())

  // 13. Register signal handler
  let shutdown_flag = Arc::new(AtomicBool::new(false))
  signal_hook::flag::register(SIGTERM, Arc::clone(&shutdown_flag))?
  signal_hook::flag::register(SIGINT,  Arc::clone(&shutdown_flag))?

  // 14. Wait for shutdown
  while !shutdown_flag.load(Acquire):
    thread::sleep(Duration::from_millis(100))

  // 15. Graceful shutdown
  graceful_shutdown(ServerHandles { ... })
```

### spawn_shard_thread (with panic recovery)

```
spawn_shard_thread(shard_id, cmd_rx, snap_tx, config, initial_data):
  thread::Builder::new()
    .name(format!("shard-{}", shard_id.0))
    .spawn(move || {
      let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        shard_worker_thread(shard_id, cmd_rx, snap_tx, config, initial_data)
      }))

      if let Err(e) = result:
        log error "shard {} panicked: {:?}", shard_id.0, e
        SHARD_STATUS[shard_id.0 as usize].store(FAILED, Release)
        SHUTDOWN_FLAG.store(true, Release)  // trigger global shutdown
    })
    .unwrap()
```

### Graceful Shutdown

```
graceful_shutdown(handles):
  log INFO "initiating graceful shutdown"

  // Step 1: signal frontend to stop reading new commands
  handles.frontend_handles.signal_shutdown()

  // Step 2: drain in-flight commands (up to 10s)
  let deadline = Instant::now() + Duration::from_secs(10)
  loop:
    let all_empty = handles.shard_metrics.iter()
      .all(|m| m.queue_depth.load(Relaxed) == 0)
    if all_empty || Instant::now() >= deadline:
      if !all_empty: log warn "drain timeout after 10s; some commands may be lost"
      break
    thread::sleep(100ms)

  // Step 3: trigger final snapshot for each shard
  for shard_tx in &handles.shard_txs:
    let (reply_tx, _) = bounded(1)
    shard_tx.send(Command { op: Operation::Shutdown, reply_tx }).ok()
    // Shard thread triggers snapshot before exiting its loop

  // Step 4: join shard threads (30s timeout)
  for handle in handles.shard_handles:
    handle.join_timeout(30s).ok()

  // Step 5: join snapshot writer threads
  drop(snap_txs)  // close channels; writers exit their recv() loop
  for handle in handles.snapshot_handles:
    handle.join_timeout(30s).ok()

  // Step 6: join remaining threads
  handles.metrics_handle.join().ok()
  handles.watchdog_handle.join().ok()

  log INFO "shutdown complete"
```

### Watchdog Thread

```
watchdog_thread(shard_metrics, shutdown_flag):
  let mut last_qps = vec![0u64; SHARD_COUNT]

  loop:
    thread::sleep(Duration::from_secs(10))
    if shutdown_flag.load(Acquire): break

    for (i, m) in shard_metrics.iter().enumerate():
      let current = m.commands_processed.load(Relaxed)
      if SHARD_STATUS[i].load(Acquire) == FAILED:
        log error "shard {} is failed", i
      else if current == last_qps[i] && m.queue_depth.load(Relaxed) > 0:
        log warn "shard {} may be stuck (no progress in 10s, queue_depth={})", i, m.queue_depth.load(Relaxed)
      last_qps[i] = current
```

## 8. Persistence Model

`crates/server` calls `cleanup_temp_files()` at startup (from `crates/persistence`) and orchestrates the final snapshot trigger on shutdown. The actual persistence format lives in `crates/persistence`.

## 9. Concurrency Model

All inter-thread communication uses:
- `crossbeam::channel::unbounded` — frontend → shard (MPSC)
- `crossbeam::channel::bounded(1)` — shard → snapshot writer (SPSC)
- `crossbeam::channel::bounded(1)` — shard → frontend response (one-shot)
- `Arc<AtomicBool>` — shutdown flag (written by signal handler, read by all threads)
- `Arc<[ShardMetrics; SHARD_COUNT]>` — shared metrics (atomic fields + Mutex<Histogram>)
- `Arc<Config>` — immutable, safe to share

No `Mutex<HashMap>` or any lock on the command hot path.

## 10. Configuration

All configuration comes from `Arc<Config>` (see `plan-configuration.md`). The server binary has no configuration of its own.

Cargo workspace:
```toml
[workspace]
members = [
  "crates/common",
  "crates/protocol",
  "crates/engine",
  "crates/persistence",
  "crates/frontend",
  "crates/observability",
  "crates/server",
]

[workspace.dependencies]
crossbeam = "0.8"
thiserror = "1"
log       = "0.4"
env_logger = "0.11"
signal-hook = "0.3"
```

## 11. Observability

- `INFO`: server started `{ version, port, shard_count, frontend_threads }`
- `INFO`: each shard loaded (or started empty) at startup
- `INFO`: all shards ready, accepting connections
- `INFO`: SIGTERM received, beginning shutdown
- `INFO`: shutdown complete
- `ERROR`: shard N panicked (triggers shutdown)
- `WARN`: drain timeout reached before all shard queues empty

## 12. Testing Strategy

- **Unit tests**:
  - `test_spawn_shard_thread_sends_and_receives`: spawn one shard thread, send a PING, receive PONG, drop channel, join
  - `test_shutdown_flag_triggers_shard_exit`: set shutdown_flag, verify shard thread exits within 200ms
  - `test_shard_panic_caught_and_flag_set`: spawn shard that panics immediately, assert SHARD_STATUS[shard] == FAILED and SHUTDOWN_FLAG set

- **Integration tests**:
  - `test_full_startup_and_shutdown`: start server with all threads, send PING, receive PONG, send SIGTERM, verify clean exit
  - `test_startup_loads_snapshots`: write snapshot files for 3 shards, start server, verify keys from snapshots are accessible
  - `test_concurrent_load`: 50 clients, 10K SET/GET each, no errors, ThreadSanitizer clean
  - `test_graceful_shutdown_writes_final_snapshot`: write key, send SIGTERM, restart server, verify key recovered

## 13. Open Questions

None.
