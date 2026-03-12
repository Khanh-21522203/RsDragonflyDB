# Feature: Observability

## 1. Purpose

The `crates/observability` crate implements the observability stack for RsDragonflyDB: structured JSON logging, a Prometheus-compatible metrics endpoint (`/metrics`), and HTTP health/readiness endpoints (`/health`, `/ready`).

The metrics exporter is a dedicated OS thread that aggregates per-shard atomic counters and exposes them via a minimal HTTP server on a configurable port (default: 9090). The logging subsystem is initialized at startup and writes structured JSON to stderr.

## 2. Responsibilities

- Initialize structured JSON logging (via `tracing` or `slog`); expose a global logger handle
- Define and expose a `MetricsExporter` thread that:
  - Serves `GET /metrics` (Prometheus text format)
  - Serves `GET /health` (200 if all shards alive)
  - Serves `GET /ready` (200 if all snapshots loaded)
- Aggregate per-shard `ShardMetrics` atomics across all shards on every scrape
- Read `Mutex<Histogram>` from each shard's `latency_histogram` under brief lock
- Format Prometheus text exposition for all defined metrics
- Track server uptime, total connections, instantaneous ops/sec

## 3. Non-Responsibilities

- Does not implement distributed tracing (post-MVP)
- Does not push metrics to Pushgateway (post-MVP)
- Does not aggregate logs centrally (operator responsibility)
- Does not configure alerting rules
- Does not implement log rotation

## 4. Architecture Design

```
All shard threads
  │  AtomicU64 / AtomicUsize  (no lock on hot path)
  │  Mutex<Histogram>          (locked only at scrape time, ~15s interval)
  ▼
Arc<[ShardMetrics; SHARD_COUNT]>
        │
        ▼
┌─────────────────────────────────────────────────┐
│  metrics_exporter_thread                        │
│                                                 │
│  loop:                                          │
│    accept HTTP request                          │
│    match request.path:                          │
│      "/metrics" → aggregate_and_format()        │
│      "/health"  → check_shards_alive()          │
│      "/ready"   → check_snapshots_loaded()      │
│    write response                               │
└─────────────────────────────────────────────────┘
        │  listening on metrics_port (default 9090)
        ▼
   Prometheus scraper / health check probe

Logging:
  any thread → log::info!(...) → JSON to stderr
```

## 5. Core Data Structures

```rust
// crates/observability/src/metrics.rs

/// Aggregated metrics across all shards — computed fresh on every scrape.
pub struct GlobalMetrics {
    // Stats
    pub total_commands_processed: u64,
    pub total_connections_received: u64,
    pub instantaneous_ops_per_sec: f64,
    pub total_keys_expired: u64,

    // Memory
    pub used_memory_bytes: usize,
    pub mem_fragmentation_ratio: f64,

    // Per-shard snapshots
    pub shards: Vec<ShardSnapshot>,
}

pub struct ShardSnapshot {
    pub shard_id: u16,
    pub key_count: usize,
    pub memory_bytes: usize,
    pub commands_processed: u64,
    pub keys_expired: u64,
    pub snapshots_written: u64,
    pub snapshots_skipped: u64,
    pub snapshot_write_errors: u64,
    pub queue_depth: usize,
    pub latency: HistogramSnapshot,  // from Mutex<Histogram> — brief lock
}
```

```rust
// crates/observability/src/exporter.rs

use common::{ShardMetrics, Config, SHARD_COUNT};

pub struct MetricsExporter {
    shard_metrics: Arc<[ShardMetrics; SHARD_COUNT]>,
    config: Arc<Config>,
    server_start: Instant,
    connection_count: Arc<AtomicUsize>,
    shards_ready: Arc<AtomicBool>,  // set true once all snapshots loaded
    prev_commands_processed: u64,   // for instantaneous QPS calculation
    prev_check_time: Instant,
}

impl MetricsExporter {
    pub fn new(
        shard_metrics: Arc<[ShardMetrics; SHARD_COUNT]>,
        config: Arc<Config>,
        connection_count: Arc<AtomicUsize>,
        shards_ready: Arc<AtomicBool>,
    ) -> Self

    /// Blocking entry point for the metrics exporter OS thread.
    pub fn run(mut self)
}
```

## 6. Public Interfaces

```rust
// crates/observability/src/lib.rs

/// Initialize the global structured JSON logger.
/// Call once at startup before spawning any threads.
pub fn init_logging(log_level: &str)

/// Spawn the metrics exporter thread.
/// Returns a handle to join on shutdown.
pub fn start_metrics_exporter(
    shard_metrics: Arc<[ShardMetrics; SHARD_COUNT]>,
    config: Arc<Config>,
    connection_count: Arc<AtomicUsize>,
    shards_ready: Arc<AtomicBool>,
) -> JoinHandle<()>
```

## 7. Internal Algorithms

### Metrics Exporter Event Loop

```
MetricsExporter::run():
  listener = TcpListener::bind(("0.0.0.0", config.metrics_port))?
  log INFO "metrics server listening on port {}", config.metrics_port

  loop:
    match listener.accept():
      Ok((mut socket, addr)) →
        // Read HTTP request line (minimal HTTP/1.1 parser)
        request_line = read_line(&socket)
        path = parse_path(request_line)

        (status, body) = match path:
          "/metrics" → (200, format_prometheus_metrics(self.aggregate()))
          "/health"  → check_health()
          "/ready"   → check_ready()
          _          → (404, "Not Found")

        http_response = format_http_response(status, body)
        socket.write_all(http_response.as_bytes()).ok()

      Err(e) → log error "metrics accept error: {}", e
```

### aggregate()

```
aggregate():
  // Compute instantaneous QPS
  total_cmds = sum of shard_metrics[i].commands_processed.load(Relaxed)
  now = Instant::now()
  elapsed = (now - prev_check_time).as_secs_f64()
  ops_per_sec = (total_cmds - prev_commands_processed) as f64 / elapsed
  prev_commands_processed = total_cmds
  prev_check_time = now

  shards = for each shard_metrics[i]:
    latency = shard_metrics[i].latency_histogram.lock().unwrap().snapshot()
    ShardSnapshot {
      shard_id: i as u16,
      key_count: shard_metrics[i].key_count.load(Relaxed),
      memory_bytes: shard_metrics[i].memory_bytes.load(Relaxed),
      commands_processed: shard_metrics[i].commands_processed.load(Relaxed),
      keys_expired: shard_metrics[i].keys_expired.load(Relaxed),
      snapshots_written: shard_metrics[i].snapshots_written.load(Relaxed),
      snapshots_skipped: shard_metrics[i].snapshots_skipped.load(Relaxed),
      snapshot_write_errors: shard_metrics[i].snapshot_write_errors.load(Relaxed),
      queue_depth: shard_metrics[i].queue_depth.load(Relaxed),
      latency,
    }

  GlobalMetrics {
    total_commands_processed: total_cmds,
    instantaneous_ops_per_sec: ops_per_sec,
    total_keys_expired: sum(shards[i].keys_expired),
    used_memory_bytes: sum(shards[i].memory_bytes),
    mem_fragmentation_ratio: get_process_resident() / used_memory_bytes,
    shards,
  }
```

### format_prometheus_metrics

Produces Prometheus text format. Each metric is written with `# HELP`, `# TYPE`, and one or more value lines:

```
# HELP rsdragonfly_shard_commands_processed_total Total commands processed per shard.
# TYPE rsdragonfly_shard_commands_processed_total counter
rsdragonfly_shard_commands_processed_total{shard="0"} 1500000
rsdragonfly_shard_commands_processed_total{shard="1"} 1480000
...

# HELP rsdragonfly_shard_key_count Current number of keys in shard.
# TYPE rsdragonfly_shard_key_count gauge
rsdragonfly_shard_key_count{shard="0"} 15625
...

# HELP rsdragonfly_shard_latency_us_bucket Latency histogram (microseconds).
# TYPE rsdragonfly_shard_latency_us_bucket histogram
rsdragonfly_shard_latency_us_bucket{shard="0",le="100"} 14000
rsdragonfly_shard_latency_us_bucket{shard="0",le="500"} 15500
rsdragonfly_shard_latency_us_bucket{shard="0",le="1000"} 15600
rsdragonfly_shard_latency_us_bucket{shard="0",le="5000"} 15625
rsdragonfly_shard_latency_us_bucket{shard="0",le="+Inf"} 15625
rsdragonfly_shard_latency_us_sum{shard="0"} 4687500
rsdragonfly_shard_latency_us_count{shard="0"} 15625
...

# HELP rsdragonfly_shard_memory_bytes_total Memory used by shard (bytes).
# TYPE rsdragonfly_shard_memory_bytes_total gauge
rsdragonfly_shard_memory_bytes_total{shard="0"} 167772160
...
```

Full list of metrics:

| Metric name | Type | Labels | Description |
|------------|------|--------|-------------|
| `rsdragonfly_shard_commands_processed_total` | counter | shard | Commands executed |
| `rsdragonfly_shard_keys_expired_total` | counter | shard | Keys expired |
| `rsdragonfly_shard_snapshots_written_total` | counter | shard | Successful snapshots |
| `rsdragonfly_shard_snapshots_skipped_total` | counter | shard | Skipped snapshots |
| `rsdragonfly_shard_snapshot_write_errors_total` | counter | shard | Failed snapshot writes |
| `rsdragonfly_shard_key_count` | gauge | shard | Current key count |
| `rsdragonfly_shard_memory_bytes` | gauge | shard | Memory usage |
| `rsdragonfly_shard_queue_depth` | gauge | shard | Command queue depth |
| `rsdragonfly_shard_latency_us_bucket` | histogram | shard, le | Latency histogram |
| `rsdragonfly_total_commands_processed` | counter | — | Global command count |
| `rsdragonfly_total_connections_received` | counter | — | Lifetime connections |
| `rsdragonfly_instantaneous_ops_per_sec` | gauge | — | Current QPS |
| `rsdragonfly_used_memory_bytes` | gauge | — | Total memory |
| `rsdragonfly_mem_fragmentation_ratio` | gauge | — | Resident / allocated |

### /health endpoint

```
check_health():
  // All shards must be in the alive set (not panicked/failed)
  // Check via SHARD_STATUS atomics (set by shard panic handler in crates/server)
  all_alive = SHARD_STATUS.iter().all(|s| s.load(Relaxed) == ALIVE)
  if all_alive: (200, "OK")
  else: (503, "DEGRADED: one or more shards failed")
```

### /ready endpoint

```
check_ready():
  if shards_ready.load(Relaxed): (200, "OK")
  else: (503, "NOT READY: loading snapshots")
```

## 8. Persistence Model

None. Metrics are in-memory counters; they reset on process restart. Log output goes to stderr; rotation is the operator's responsibility.

## 9. Concurrency Model

- The metrics exporter thread is the only reader of `Mutex<Histogram>`. The shard thread is the only writer. The lock is held for < 1μs by either party. No deadlock risk.
- All other `ShardMetrics` fields are `AtomicU64` / `AtomicUsize` — lockless reads.
- `shards_ready: Arc<AtomicBool>` is set by the main thread after all snapshots are loaded; read by the exporter.
- The HTTP listener is single-threaded in the exporter; one scrape at a time. Prometheus scrapes every 15s by default, so this is not a bottleneck.
- Logging macros (`log::info!`) use a global logger that is thread-safe by design.

## 10. Configuration

| Parameter | Source | Default |
|-----------|--------|---------|
| `metrics_port` | `Config` | 9090 |
| `log_level` | `Config` | `info` |

Log format is always JSON (no configuration). Log destination is always stderr.

## 11. Observability

The observability crate is itself observed minimally:
- `INFO` log when the metrics server starts: `"metrics server listening" { port: 9090 }`
- `WARN` log if a scrape request fails (socket write error)
- `ERROR` log if `listener.accept()` fails (should not happen in normal operation)

## 12. Testing Strategy

- **Unit tests**:
  - `test_aggregate_sums_all_shards`: create mock ShardMetrics with known values, call aggregate, assert totals correct
  - `test_prometheus_format_counter`: format a counter metric, assert lines match Prometheus text format spec
  - `test_prometheus_format_histogram`: format a histogram, assert bucket, sum, and count lines correct
  - `test_health_all_alive`: all SHARD_STATUS = ALIVE → 200
  - `test_health_one_failed`: one SHARD_STATUS = FAILED → 503
  - `test_ready_not_ready`: shards_ready = false → 503
  - `test_ready_ready`: shards_ready = true → 200
  - `test_instantaneous_qps_computed_correctly`: simulate two aggregations 1s apart with a known command delta

- **Integration tests**:
  - `test_metrics_endpoint_responds`: start server, `GET :9090/metrics`, assert 200 and Prometheus format
  - `test_metrics_contains_expected_labels`: scrape and assert all SHARD_COUNT shards appear with `shard="N"` label
  - `test_health_returns_200_on_startup`: `GET :9090/health` returns 200 after server starts
  - `test_ready_returns_503_before_snapshots_loaded`: check /ready before shards_ready is set

## 13. Open Questions

None.
