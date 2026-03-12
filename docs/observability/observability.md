# Observability

## Document Purpose
This document defines the observability strategy for RsDragonflyDB, including metrics, logging, tracing, and health checks.

**Audience**: Operations engineers, SREs, developers

---

## Observability Philosophy

### Three Pillars

1. **Metrics**: Quantitative measurements (QPS, latency, memory)
2. **Logs**: Discrete events (errors, warnings, debug info)
3. **Traces**: Request flow across components (future)

**Goal**: Answer operational questions quickly:
- Is the system healthy?
- Where is the bottleneck?
- What caused this error?
- How do I debug this issue?

---

## Metrics

### Metrics Exposition

**Format**: Prometheus text format

**Endpoint**: `http://localhost:9090/metrics`

**Scrape Interval**: 15s (recommended)

---

### Global Metrics

**Server Metrics**:
```
# Server uptime
rsdragonfly_uptime_seconds 3600

# Total connections
rsdragonfly_connections_total 1234
rsdragonfly_connections_active 50

# Total commands processed
rsdragonfly_commands_total{command="GET"} 1000000
rsdragonfly_commands_total{command="SET"} 500000
rsdragonfly_commands_total{command="DEL"} 100000

# Command errors
rsdragonfly_command_errors_total{command="GET",error="key_not_found"} 1000
rsdragonfly_command_errors_total{command="SET",error="oom"} 5

# Global QPS
rsdragonfly_qps 15000

# Global memory usage
rsdragonfly_memory_bytes 10737418240
rsdragonfly_memory_fragmentation_ratio 1.15
```

---

### Per-Shard Metrics

**Shard Health**:
```
# Shard status (0=failed, 1=healthy)
rsdragonfly_shard_status{shard="0"} 1

# Commands processed per shard
rsdragonfly_shard_commands_total{shard="0"} 15625

# Keys per shard
rsdragonfly_shard_keys{shard="0"} 1000000

# Memory per shard
rsdragonfly_shard_memory_bytes{shard="0"} 167772160

# Queue depth (backpressure indicator)
rsdragonfly_shard_queue_depth{shard="0"} 10
```

---

**Shard Latency**:
```
# Latency histogram (microseconds)
rsdragonfly_shard_latency_us_bucket{shard="0",le="100"} 14000
rsdragonfly_shard_latency_us_bucket{shard="0",le="500"} 15500
rsdragonfly_shard_latency_us_bucket{shard="0",le="1000"} 15600
rsdragonfly_shard_latency_us_bucket{shard="0",le="5000"} 15625
rsdragonfly_shard_latency_us_bucket{shard="0",le="+Inf"} 15625
rsdragonfly_shard_latency_us_sum{shard="0"} 4687500
rsdragonfly_shard_latency_us_count{shard="0"} 15625

# Derived metrics (computed by Prometheus)
# p50 latency: histogram_quantile(0.5, rsdragonfly_shard_latency_us_bucket)
# p99 latency: histogram_quantile(0.99, rsdragonfly_shard_latency_us_bucket)
```

---

**TTL Metrics**:
```
# Keys expired (lazy + active)
rsdragonfly_shard_keys_expired_total{shard="0"} 5000

# TTL queue size
rsdragonfly_shard_ttl_queue_size{shard="0"} 100000

# Active expiration duration
rsdragonfly_shard_active_expiration_duration_us{shard="0"} 2000
```

---

**Persistence Metrics**:
```
# Snapshots written
rsdragonfly_shard_snapshots_written_total{shard="0"} 60

# Snapshot write duration
rsdragonfly_shard_snapshot_write_duration_seconds{shard="0"} 1.2

# Snapshot write errors
rsdragonfly_shard_snapshot_write_errors_total{shard="0"} 0

# Snapshots skipped (writer busy)
rsdragonfly_shard_snapshots_skipped_total{shard="0"} 2

# Snapshot size
rsdragonfly_shard_snapshot_size_bytes{shard="0"} 10485760

# Last snapshot timestamp
rsdragonfly_shard_last_snapshot_timestamp_seconds{shard="0"} 1706112000
```

---

### Metrics Implementation

**Per-Shard Metrics Struct**:
```rust
pub struct ShardMetrics {
    pub commands_processed: AtomicU64,
    pub keys_expired: AtomicU64,
    pub snapshots_written: AtomicU64,
    pub snapshots_skipped: AtomicU64,
    pub snapshot_write_errors: AtomicU64,
    pub key_count: AtomicUsize,
    pub memory_bytes: AtomicUsize,
    pub queue_depth: AtomicUsize,
    pub latency_histogram: Mutex<Histogram>, // Allowed exception: read only by metrics exporter thread, not on command hot path
}

impl ShardMetrics {
    pub fn record_command(&self, latency: Duration) {
        self.commands_processed.fetch_add(1, Ordering::Relaxed);
        self.latency_histogram.lock().unwrap().record(latency.as_micros() as u64);
    }
}
```

---

**Metrics Exporter**:
```rust
fn metrics_exporter_thread(
    shards: Arc<[ShardMetrics; SHARD_COUNT]>,
    global_metrics: Arc<GlobalMetrics>,
) {
    let listener = TcpListener::bind("0.0.0.0:9090").unwrap();
    
    loop {
        match listener.accept() {
            Ok((mut socket, _)) => {
                let metrics = format_prometheus_metrics(&shards, &global_metrics);
                
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
                    metrics.len(),
                    metrics
                );
                
                socket.write_all(response.as_bytes()).ok();
            }
            Err(e) => {
                log::error!("Metrics export failed: {}", e);
            }
        }
    }
}

fn format_prometheus_metrics(
    shards: &[ShardMetrics],
    global: &GlobalMetrics,
) -> String {
    let mut output = String::new();
    
    // Global metrics
    output.push_str(&format!("rsdragonfly_uptime_seconds {}\n", global.uptime_seconds()));
    output.push_str(&format!("rsdragonfly_connections_active {}\n", global.connections_active.load(Ordering::Relaxed)));
    
    // Per-shard metrics
    for (shard_id, shard) in shards.iter().enumerate() {
        output.push_str(&format!(
            "rsdragonfly_shard_commands_total{{shard=\"{}\"}} {}\n",
            shard_id,
            shard.commands_processed.load(Ordering::Relaxed)
        ));
        
        output.push_str(&format!(
            "rsdragonfly_shard_keys{{shard=\"{}\"}} {}\n",
            shard_id,
            shard.key_count.load(Ordering::Relaxed)
        ));
        
        // Latency histogram
        let histogram = shard.latency_histogram.lock().unwrap();
        for (le, count) in histogram.buckets() {
            output.push_str(&format!(
                "rsdragonfly_shard_latency_us_bucket{{shard=\"{}\",le=\"{}\"}} {}\n",
                shard_id, le, count
            ));
        }
    }
    
    output
}
```

---

## Logging

### Log Levels

| Level | Use Case | Example |
|-------|----------|---------|
| ERROR | Unrecoverable errors | Shard panic, snapshot corruption |
| WARN | Recoverable errors | Snapshot write failed, shard overload |
| INFO | Important events | Server start, snapshot written, graceful shutdown |
| DEBUG | Detailed events | Command execution, TTL expiration |
| TRACE | Very detailed | Protocol parsing, channel sends |

**Default Level**: INFO

**Configuration**: `--log-level` flag or `RSDRAGONFLY_LOG_LEVEL` env var

---

### Structured Logging

**Format**: JSON (for machine parsing)

**Library**: `slog` or `tracing`

**Example**:
```json
{
  "timestamp": "2026-01-24T12:00:00.123Z",
  "level": "INFO",
  "target": "rsdragonfly::shard",
  "message": "Snapshot written",
  "shard_id": 0,
  "key_count": 1000000,
  "size_bytes": 10485760,
  "duration_ms": 1200
}
```

---

### Log Categories

**Server Lifecycle**:
```json
{"level": "INFO", "message": "Server starting", "port": 6379, "shards": 64}
{"level": "INFO", "message": "Server ready", "startup_duration_ms": 150}
{"level": "INFO", "message": "Graceful shutdown initiated"}
{"level": "INFO", "message": "Server stopped"}
```

---

**Command Execution** (DEBUG):
```json
{
  "level": "DEBUG",
  "message": "Command executed",
  "shard_id": 0,
  "command": "GET",
  "key": "user:123",
  "result": "hit",
  "latency_us": 50
}
```

---

**Errors** (ERROR/WARN):
```json
{
  "level": "ERROR",
  "message": "Shard panic",
  "shard_id": 0,
  "error": "index out of bounds",
  "backtrace": "..."
}

{
  "level": "WARN",
  "message": "Snapshot write failed",
  "shard_id": 0,
  "error": "Disk full",
  "path": "/var/lib/rsdragonfly/shard0.snap.tmp"
}
```

---

**Persistence** (INFO):
```json
{
  "level": "INFO",
  "message": "Snapshot written",
  "shard_id": 0,
  "key_count": 1000000,
  "size_bytes": 10485760,
  "duration_ms": 1200
}

{
  "level": "INFO",
  "message": "Snapshot loaded",
  "shard_id": 0,
  "key_count": 1000000,
  "expired_keys": 5000,
  "duration_ms": 5000
}
```

---

### Log Sampling

**Problem**: High QPS generates too many DEBUG logs.

**Solution**: Sample logs (e.g., log 1% of commands).

```rust
fn execute_command(&mut self, cmd: Command) -> Response {
    let start = Instant::now();
    let response = self.execute_internal(cmd);
    let latency = start.elapsed();
    
    // Sample 1% of commands
    if rand::random::<u8>() < 3 {  // 3/256 ≈ 1%
        log::debug!(
            "Command executed";
            "shard_id" => self.id.0,
            "command" => cmd.name(),
            "latency_us" => latency.as_micros(),
        );
    }
    
    response
}
```

---

## Health Checks

### Liveness Probe

**Endpoint**: `GET /health`

**Purpose**: Is the server process alive?

**Response**:
```
HTTP/1.1 200 OK
Content-Type: text/plain

OK
```

**Failure**: 500 if any shard is failed.

**Implementation**:
```rust
fn health_check_handler() -> Response {
    for shard in &SHARDS {
        if shard.status.load(Ordering::Relaxed) == FAILED {
            return Response::InternalServerError("Shard failed");
        }
    }
    Response::Ok("OK")
}
```

---

### Readiness Probe

**Endpoint**: `GET /ready`

**Purpose**: Is the server ready to accept traffic?

**Response**:
```
HTTP/1.1 200 OK
Content-Type: text/plain

READY
```

**Failure**: 503 if snapshots not loaded or shards not initialized.

**Implementation**:
```rust
fn readiness_check_handler() -> Response {
    if !SNAPSHOTS_LOADED.load(Ordering::Relaxed) {
        return Response::ServiceUnavailable("Loading snapshots");
    }
    
    for shard in &SHARDS {
        if !shard.initialized.load(Ordering::Relaxed) {
            return Response::ServiceUnavailable("Shard not initialized");
        }
    }
    
    Response::Ok("READY")
}
```

---

### Kubernetes Probes

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: 9090
  initialDelaySeconds: 10
  periodSeconds: 10
  timeoutSeconds: 5
  failureThreshold: 3

readinessProbe:
  httpGet:
    path: /ready
    port: 9090
  initialDelaySeconds: 5
  periodSeconds: 5
  timeoutSeconds: 3
  failureThreshold: 2
```

---

## Alerting

### Alert Rules (Prometheus)

**High Error Rate**:
```yaml
- alert: HighCommandErrorRate
  expr: rate(rsdragonfly_command_errors_total[5m]) > 10
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "High command error rate"
    description: "Error rate is {{ $value }} errors/sec"
```

---

**Shard Failure**:
```yaml
- alert: ShardFailed
  expr: rsdragonfly_shard_status == 0
  for: 1m
  labels:
    severity: critical
  annotations:
    summary: "Shard {{ $labels.shard }} failed"
    description: "Shard is unavailable"
```

---

**High Latency**:
```yaml
- alert: HighLatency
  expr: histogram_quantile(0.99, rsdragonfly_shard_latency_us_bucket) > 5000
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "High p99 latency on shard {{ $labels.shard }}"
    description: "p99 latency is {{ $value }}us"
```

---

**Snapshot Failures**:
```yaml
- alert: SnapshotWriteFailures
  expr: rate(rsdragonfly_shard_snapshot_write_errors_total[10m]) > 0
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "Snapshot write failures on shard {{ $labels.shard }}"
    description: "Snapshots are failing"
```

---

**Memory Pressure**:
```yaml
- alert: HighMemoryUsage
  expr: rsdragonfly_memory_bytes / rsdragonfly_memory_limit_bytes > 0.9
  for: 5m
  labels:
    severity: warning
  annotations:
    summary: "High memory usage"
    description: "Memory usage is {{ $value | humanizePercentage }}"
```

---

## Dashboards

### Grafana Dashboard

**Panels**:

1. **Overview**
   - Total QPS (graph)
   - Active connections (gauge)
   - Memory usage (gauge)
   - Uptime (stat)

2. **Latency**
   - p50/p99/p99.9 latency (graph)
   - Latency heatmap (heatmap)

3. **Per-Shard Metrics**
   - QPS per shard (graph, stacked)
   - Key count per shard (graph)
   - Memory per shard (graph)

4. **Persistence**
   - Snapshot write duration (graph)
   - Snapshot size (graph)
   - Snapshot errors (counter)

5. **Errors**
   - Error rate by command (graph)
   - Error rate by type (graph)

---

### Example Queries

**Total QPS**:
```promql
sum(rate(rsdragonfly_commands_total[1m]))
```

**p99 Latency**:
```promql
histogram_quantile(0.99, sum(rate(rsdragonfly_shard_latency_us_bucket[1m])) by (le))
```

**Memory Usage**:
```promql
rsdragonfly_memory_bytes / 1024 / 1024 / 1024
```

**Shard Imbalance**:
```promql
stddev(rsdragonfly_shard_keys)
```

---

## Tracing (Future)

### Distributed Tracing

**Library**: OpenTelemetry

**Use Case**: Trace request flow across components.

**Example Trace**:
```
Span: handle_connection (10ms)
  ├─ Span: parse_resp2 (0.5ms)
  ├─ Span: route_command (0.1ms)
  ├─ Span: shard_execute (1ms)
  │   ├─ Span: hashmap_lookup (0.05ms)
  │   └─ Span: ttl_check (0.02ms)
  └─ Span: serialize_response (0.3ms)
```

**Not in MVP**: Complexity vs benefit.

---

## Profiling

### CPU Profiling

**Tool**: `perf` (Linux)

**Usage**:
```bash
# Record profile
perf record -F 99 -p $(pgrep rsdragonfly) -g -- sleep 30

# View report
perf report
```

**Flamegraph**:
```bash
# Generate flamegraph
perf script | stackcollapse-perf.pl | flamegraph.pl > flamegraph.svg
```

---

### Memory Profiling

**Tool**: jemalloc profiling

**Enable**:
```bash
export MALLOC_CONF="prof:true,prof_prefix:jeprof.out"
./rsdragonfly
```

**Analyze**:
```bash
jeprof --pdf rsdragonfly jeprof.out.*.heap > heap.pdf
```

---

## Definition of Done

Observability is complete when:

1. All metrics are exposed via `/metrics` endpoint
2. Structured logging is implemented (JSON format)
3. Health and readiness probes are implemented
4. Prometheus alert rules are defined
5. Grafana dashboard is created
6. Profiling tools are documented
7. Operational runbook references observability tools

---

## References

- [overview.md](../architecture/overview.md) - System architecture
- [testing-strategy.md](../testing/testing-strategy.md) - Testing approach
- [runbook.md](../runbooks/runbook.md) - Operational procedures
