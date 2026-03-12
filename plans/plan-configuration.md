# Feature: Configuration

## 1. Purpose

The configuration module handles all runtime configuration for RsDragonflyDB: parsing CLI flags, reading environment variable overrides, validating values, and constructing the `Config` struct that is shared (via `Arc<Config>`) across all threads.

It also owns **graceful shutdown**: registering the SIGTERM handler, draining in-flight commands, triggering final snapshots, and coordinating the shutdown sequence across all thread types.

## 2. Responsibilities

- Parse CLI flags using a lightweight argument parser (no heavy framework dependency)
- Override flag values with environment variables (e.g., `RSDRAGONFLY_PORT` overrides `--port`)
- Validate all configuration values (port range, positive integers, writable directory, etc.)
- Construct and expose `Arc<Config>` to all threads
- Register a SIGTERM signal handler
- On SIGTERM:
  1. Stop accepting new connections
  2. Drain in-flight shard commands (timeout: 10s)
  3. Trigger final snapshot for each shard (timeout: 30s)
  4. Join all threads cleanly
  5. Exit process with code 0

## 3. Non-Responsibilities

- Does not implement runtime config reload (post-MVP; restart required)
- Does not implement per-key or per-connection configuration
- Does not manage thread spawning (that is `crates/server`'s job)
- Does not validate `SHARD_COUNT` at runtime (it is compile-time only)

## 4. Architecture Design

```
argv + environment
        │
        ▼
┌─────────────────────┐
│  parse_cli(argv)    │  raw string map
└─────────────────────┘
        │
        ▼
┌─────────────────────┐
│  apply_env_overrides│  check RSDRAGONFLY_* vars
└─────────────────────┘
        │
        ▼
┌─────────────────────┐
│  validate_config()  │  return Err on invalid values
└─────────────────────┘
        │
        ▼
   Arc<Config>
        │  distributed to all threads
        ▼

SIGTERM / SIGINT
        │
        ▼
┌────────────────────────────────┐
│  shutdown_coordinator          │
│  1. set SHUTDOWN_FLAG          │
│  2. wait for shard queues      │
│  3. trigger final snapshots    │
│  4. join all thread handles    │
└────────────────────────────────┘
```

## 5. Core Data Structures

```rust
// crates/common/src/config.rs  (Config is defined here; re-exported)

pub struct Config {
    pub port: u16,                         // --port (default: 6379)
    pub bind: std::net::IpAddr,            // --bind (default: 0.0.0.0)
    pub snapshot_dir: std::path::PathBuf,  // --snapshot-dir
    pub snapshot_interval_secs: u64,       // --snapshot-interval (default: 60)
    pub log_level: String,                 // --log-level (default: "info")
    pub max_connections: usize,            // --max-connections (default: 10000)
    pub frontend_threads: usize,           // --frontend-threads (default: 4)
    pub metrics_port: u16,                 // --metrics-port (default: 9090)
    pub cpu_pinning: bool,                 // --cpu-pinning (boolean flag)
}
```

```rust
// crates/server/src/config_parser.rs

/// Parse argv and env into a validated Config.
pub fn parse_config(argv: &[String]) -> Result<Config, ConfigError>

#[derive(Debug)]
pub enum ConfigError {
    UnknownFlag(String),
    MissingValue(String),
    InvalidValue { flag: String, value: String, reason: String },
    SnapshotDirNotWritable(PathBuf),
}

impl fmt::Display for ConfigError {
    // human-readable error message for startup failure
}
```

## 6. Public Interfaces

```rust
pub fn parse_config(argv: &[String]) -> Result<Config, ConfigError>

// Shutdown coordination (in crates/server)
pub struct ShutdownCoordinator {
    pub flag: Arc<AtomicBool>,
}

impl ShutdownCoordinator {
    pub fn new() -> Self
    pub fn is_shutdown(&self) -> bool
    pub fn trigger(&self)  // sets flag + notifies all waiters
}
```

## 7. Internal Algorithms

### CLI Parsing

No external crate required — the CLI surface is small and stable. A simple token-by-token parser:

```
parse_cli(argv):
  i = 1  // skip program name
  raw = HashMap<String, Option<String>>

  while i < argv.len():
    arg = argv[i]
    if arg starts with "--":
      flag = arg[2..]
      if flag is boolean (cpu-pinning):
        raw[flag] = None
        i += 1
      else:
        if i+1 >= argv.len(): return Err(MissingValue(flag))
        raw[flag] = Some(argv[i+1])
        i += 2
    else:
      return Err(UnknownFlag(arg))

  return raw
```

### Environment Variable Override

```
apply_env_overrides(raw):
  mapping = {
    "RSDRAGONFLY_PORT"              → "port",
    "RSDRAGONFLY_BIND"              → "bind",
    "RSDRAGONFLY_SNAPSHOT_DIR"      → "snapshot-dir",
    "RSDRAGONFLY_SNAPSHOT_INTERVAL" → "snapshot-interval",
    "RSDRAGONFLY_LOG_LEVEL"         → "log-level",
    "RSDRAGONFLY_MAX_CONNECTIONS"   → "max-connections",
    "RSDRAGONFLY_FRONTEND_THREADS"  → "frontend-threads",
    "RSDRAGONFLY_METRICS_PORT"      → "metrics-port",
  }
  for (env_var, flag) in mapping:
    if let Ok(val) = std::env::var(env_var):
      raw.insert(flag, Some(val))  // env overrides CLI
```

### Validation

```
validate_config(raw):
  port = parse_u16(raw["port"] ?? "6379")?
  if port == 0: return Err(invalid "port must be 1-65535")

  bind = parse_ip_addr(raw["bind"] ?? "0.0.0.0")?

  snapshot_dir = PathBuf::from(raw["snapshot-dir"] ?? "/var/lib/rsdragonfly")
  if !snapshot_dir.exists(): fs::create_dir_all(&snapshot_dir)?
  test_write_access(&snapshot_dir)?  // create and remove a temp file

  snapshot_interval_secs = parse_u64(raw["snapshot-interval"] ?? "60")?
  if snapshot_interval_secs == 0: return Err(invalid "must be >= 1")

  log_level = raw["log-level"] ?? "info"
  if log_level not in ["error","warn","info","debug","trace"]: return Err(invalid)

  max_connections = parse_usize(raw["max-connections"] ?? "10000")?
  if max_connections == 0: return Err(invalid)

  frontend_threads = parse_usize(raw["frontend-threads"] ?? "4")?
  if frontend_threads == 0: return Err(invalid)

  metrics_port = parse_u16(raw["metrics-port"] ?? "9090")?

  cpu_pinning = raw.contains_key("cpu-pinning")

  return Ok(Config { port, bind, snapshot_dir, snapshot_interval_secs, log_level,
                     max_connections, frontend_threads, metrics_port, cpu_pinning })
```

### Graceful Shutdown Sequence

Registered via `ctrlc` crate or `signal-hook` for SIGTERM and SIGINT:

```
on_sigterm():
  log INFO "received SIGTERM, initiating graceful shutdown"
  SHUTDOWN_FLAG.store(true, Release)

  // Step 1: stop acceptor (acceptor checks SHUTDOWN_FLAG each iteration)
  // (acceptor exits its loop on next iteration)

  // Step 2: drain in-flight commands (wait up to 10s)
  deadline = Instant::now() + Duration::from_secs(10)
  loop:
    all_empty = shard_metrics.iter().all(|m| m.queue_depth.load(Relaxed) == 0)
    if all_empty || Instant::now() > deadline: break
    thread::sleep(100ms)

  // Step 3: trigger final snapshot for each shard
  // Send SIGTERM-triggered snapshot request to each shard's event loop
  // Shard event loops check SHUTDOWN_FLAG and call trigger_snapshot on exit
  // Wait for all snapshot writers to drain (up to 30s)
  deadline = Instant::now() + Duration::from_secs(30)
  // (join each snapshot_writer_thread handle with timeout)

  // Step 4: send shutdown Operation to each shard
  for shard_tx in shard_txs:
    let (reply_tx, _) = channel::bounded(1)
    shard_tx.send(Command { op: Operation::Shutdown, reply_tx }).ok()

  // Step 5: join all thread handles (shard workers, snapshot writers, frontend, metrics)
  for handle in all_thread_handles:
    handle.join().ok()

  log INFO "shutdown complete"
  process::exit(0)
```

## 8. Persistence Model

None. Configuration is read at startup from argv/env and never written to disk.

## 9. Concurrency Model

- `Arc<Config>` is immutable after construction — safe to share across any number of threads with no synchronization
- `SHUTDOWN_FLAG: Arc<AtomicBool>` is set by the signal handler (a background thread or callback) and polled by the acceptor and shard threads; `Ordering::Release` on write, `Ordering::Acquire` on read

## 10. Configuration (Self-Referential)

The CLI reference (from `docs/requirements.md`):

| Flag | Default | Description |
|------|---------|-------------|
| `--port <port>` | 6379 | Listen port |
| `--bind <addr>` | 0.0.0.0 | Bind address |
| `--snapshot-dir <path>` | `/var/lib/rsdragonfly` | Snapshot directory |
| `--snapshot-interval <secs>` | 60 | Snapshot interval |
| `--log-level <level>` | info | error\|warn\|info\|debug\|trace |
| `--max-connections <n>` | 10000 | Max concurrent connections |
| `--frontend-threads <n>` | 4 | Connection handler threads |
| `--metrics-port <port>` | 9090 | Prometheus metrics port |
| `--cpu-pinning` | off | Enable CPU pinning (boolean flag) |

Environment variable overrides: prefix `RSDRAGONFLY_`, uppercase, underscores for hyphens.
E.g.: `RSDRAGONFLY_MAX_CONNECTIONS=50000 rsdragonfly --port 6379`

## 11. Observability

- `INFO` log on startup: parsed config summary (port, bind, snapshot_dir, shard_count, frontend_threads)
- `ERROR` log + exit(1) on config validation failure with human-readable message
- `INFO` log at each shutdown step

## 12. Testing Strategy

- **Unit tests**:
  - `test_defaults_applied`: parse empty argv, assert all defaults correct
  - `test_flag_port`: parse `--port 7777`, assert Config.port == 7777
  - `test_flag_cpu_pinning_boolean`: parse `--cpu-pinning`, assert cpu_pinning == true; no `--cpu-pinning` → false
  - `test_env_override_port`: set `RSDRAGONFLY_PORT=8080`, parse empty argv, assert port == 8080
  - `test_env_overrides_cli`: set env `RSDRAGONFLY_PORT=8080`, CLI `--port 9000`, assert port == 8080
  - `test_invalid_port_zero`: parse `--port 0`, assert ConfigError::InvalidValue
  - `test_invalid_log_level`: parse `--log-level verbose`, assert ConfigError::InvalidValue
  - `test_unknown_flag`: parse `--shards 64`, assert ConfigError::UnknownFlag("shards") (not a runtime flag)
  - `test_missing_value`: parse `--port` with no following argument, assert ConfigError::MissingValue
  - `test_snapshot_dir_created`: parse `--snapshot-dir /tmp/rsdragonfly-test`, verify dir created
  - `test_snapshot_dir_not_writable`: parse dir where write is forbidden, assert SnapshotDirNotWritable

- **Integration tests**:
  - `test_graceful_shutdown`: start server, send 100 concurrent SET commands, send SIGTERM mid-flight, verify no panics and all keys recoverable from final snapshot

## 13. Open Questions

None.
