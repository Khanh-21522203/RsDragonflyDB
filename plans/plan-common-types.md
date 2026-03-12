# Feature: Common Types

## 1. Purpose

The `crates/common` crate defines the shared vocabulary used by every other crate in the workspace. It owns the fundamental data types (keys, values, entries), shard identifiers, channel message types (Command, Response), error enums, and the compile-time `SHARD_COUNT` constant.

Every other crate depends on `common`; `common` depends on nothing else in the workspace.

## 2. Responsibilities

- Define `Key`, `Value`, `Entry` — the core in-memory data model
- Define `ShardId` — typed shard identifier
- Define `Command` / `Operation` / `Response` — message types passed over channels between frontend and shard threads
- Define `SnapshotRequest` — message type from shard thread to snapshot writer thread
- Define `Error` — unified error enum covering all error categories
- Define `ShardMetrics` — per-shard atomic counters and `Mutex<Histogram>` for latency
- Expose `SHARD_COUNT` compile-time constant (via Cargo feature flags)
- Define `Config` struct — parsed runtime configuration passed by `Arc<Config>` to all threads

## 3. Non-Responsibilities

- Does not parse RESP2 protocol
- Does not implement command logic
- Does not manage threads or channels
- Does not perform IO

## 4. Architecture Design

```
crates/common
    src/
        lib.rs          re-exports all public items
        types.rs        Key, Value, Entry, ShardId, Expiry
        command.rs      Command, Operation, Response, SnapshotRequest
        error.rs        Error enum
        metrics.rs      ShardMetrics, Histogram wrapper
        config.rs       Config, SHARD_COUNT constant
```

All other crates import `common`:
```
crates/protocol    → uses Key, Value, Error
crates/engine      → uses Key, Value, Entry, Command, Response, ShardMetrics, Config
crates/persistence → uses ShardId, Entry, Error, Config
crates/frontend    → uses Command, Operation, Response, ShardId, Config
crates/server      → uses Config, ShardMetrics, everything
```

## 5. Core Data Structures

```rust
// crates/common/src/types.rs

/// Binary key — arbitrary byte sequence, heap-allocated.
/// Max size: 512 bytes (enforced by frontend validation).
pub type Key = Vec<u8>;

/// All supported value types. MVP: String only.
pub enum Value {
    String(Vec<u8>),
}

impl Value {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Value::String(b) => b,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Value::String(b) => b.len(),
        }
    }
}

/// A single stored entry: value + optional expiry instant.
/// Stored inline in the HashMap (not boxed).
pub struct Entry {
    pub value: Value,
    pub expiry: Option<Instant>, // None = no TTL
}

/// Typed shard identifier. Range: 0..SHARD_COUNT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShardId(pub u16);

/// A TTL queue entry — min-heap ordered by expiry_time.
pub struct Expiry {
    pub expiry_time: Instant,
    pub key: Key,
}

// Min-heap: earlier expiry_time = higher priority
impl Ord for Expiry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.expiry_time.cmp(&self.expiry_time)
    }
}
impl PartialOrd for Expiry { fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) } }
impl PartialEq for Expiry { fn eq(&self, other: &Self) -> bool { self.expiry_time == other.expiry_time } }
impl Eq for Expiry {}
```

```rust
// crates/common/src/command.rs

use crossbeam::channel::Sender;

/// A command sent from a frontend thread to a shard thread.
pub struct Command {
    pub op: Operation,
    /// Bounded(1) channel; shard sends exactly one Response per Command.
    pub reply_tx: Sender<Response>,
}

/// All operations a shard can execute.
pub enum Operation {
    Ping { message: Option<Vec<u8>> },
    Get  { key: Key },
    Set  { key: Key, value: Value, ttl_secs: Option<u64> },
    Del  { keys: Vec<Key> },
    Expire { key: Key, ttl_secs: u64 },
    Ttl  { key: Key },
    Info { section: Option<Vec<u8>> },
    Shutdown,
}

/// A shard's reply to a Command.
pub enum Response {
    Pong(Option<Vec<u8>>),          // PING response
    BulkString(Option<Vec<u8>>),    // GET / nullable bulk string
    Integer(i64),                   // DEL count, EXPIRE 0/1, TTL seconds
    Ok,                             // SET success
    Info(String),                   // INFO output
    Error(String),                  // -ERR ...
}

/// Serialized shard state sent from a shard thread to its snapshot writer.
pub struct SnapshotRequest {
    pub shard_id: ShardId,
    pub data: Vec<u8>,  // fully serialized snapshot bytes (header + entries)
}
```

```rust
// crates/common/src/error.rs

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown command: {0}")]
    UnknownCommand(String),

    #[error("wrong number of arguments for '{0}'")]
    WrongArgCount(String),

    #[error("value is not an integer or out of range")]
    NotAnInteger,

    #[error("shard {0} is full (key count limit exceeded)")]
    ShardFull(u16),

    #[error("out of memory")]
    OutOfMemory,

    #[error("shard unavailable")]
    ShardUnavailable,

    #[error("invalid snapshot: {0}")]
    InvalidSnapshot(&'static str),

    #[error("corrupt snapshot: {0}")]
    CorruptSnapshot(&'static str),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("key too long (max 512 bytes)")]
    KeyTooLong,

    #[error("value too large (max 512 MB)")]
    ValueTooLarge,
}
```

```rust
// crates/common/src/metrics.rs

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Simple histogram that tracks a count per latency bucket (microseconds).
/// Bucket boundaries: [100, 500, 1000, 5000, ∞]
pub struct Histogram {
    buckets: [u64; 5],  // le=100, 500, 1000, 5000, +Inf
    sum_us: u64,
    count: u64,
}

impl Histogram {
    pub fn new() -> Self { Self { buckets: [0; 5], sum_us: 0, count: 0 } }

    pub fn record(&mut self, latency_us: u64) {
        self.sum_us += latency_us;
        self.count += 1;

        // IMPORTANT: Prometheus requires CUMULATIVE buckets.
        // Each bucket le=X must count ALL observations with value <= X — not just
        // observations that fall exactly in that bucket's range.
        //
        // This loop checks every bound independently (no `break`).
        // Example: latency_us = 50
        //   50 <= 100  → buckets[0]++ (le=100)
        //   50 <= 500  → buckets[1]++ (le=500)
        //   50 <= 1000 → buckets[2]++ (le=1000)
        //   50 <= 5000 → buckets[3]++ (le=5000)
        //   50 <= MAX  → buckets[4]++ (le=+Inf)
        // All five buckets are incremented — this is correct.
        //
        // DO NOT add a `break` here. That would make buckets non-cumulative
        // and violate the Prometheus data model.
        const BOUNDS: [u64; 5] = [100, 500, 1000, 5000, u64::MAX];
        for (bucket, &bound) in self.buckets.iter_mut().zip(BOUNDS.iter()) {
            if latency_us <= bound {
                *bucket += 1;
            }
        }
    }

    pub fn snapshot(&self) -> HistogramSnapshot {
        HistogramSnapshot {
            buckets: self.buckets,
            sum_us: self.sum_us,
            count: self.count,
        }
    }
}

pub struct HistogramSnapshot {
    pub buckets: [u64; 5],
    pub sum_us: u64,
    pub count: u64,
}

/// Per-shard metrics. All fields are atomics except latency_histogram.
/// The Mutex<Histogram> is an allowed exception to the no-shared-state rule:
/// it is read only by the metrics exporter thread at ~15s intervals, never
/// on the command hot path. The Mutex contention is negligible.
pub struct ShardMetrics {
    pub commands_processed: AtomicU64,
    pub keys_expired: AtomicU64,
    pub snapshots_written: AtomicU64,
    pub snapshots_skipped: AtomicU64,
    pub snapshot_write_errors: AtomicU64,
    pub key_count: AtomicUsize,
    pub memory_bytes: AtomicUsize,
    pub queue_depth: AtomicUsize,
    pub latency_histogram: Mutex<Histogram>,
}

impl ShardMetrics {
    pub fn new() -> Self {
        Self {
            commands_processed: AtomicU64::new(0),
            keys_expired: AtomicU64::new(0),
            snapshots_written: AtomicU64::new(0),
            snapshots_skipped: AtomicU64::new(0),
            snapshot_write_errors: AtomicU64::new(0),
            key_count: AtomicUsize::new(0),
            memory_bytes: AtomicUsize::new(0),
            queue_depth: AtomicUsize::new(0),
            latency_histogram: Mutex::new(Histogram::new()),
        }
    }

    pub fn record_command(&self, latency: std::time::Duration) {
        self.commands_processed.fetch_add(1, Ordering::Relaxed);
        self.latency_histogram
            .lock()
            .unwrap()
            .record(latency.as_micros() as u64);
    }
}
```

```rust
// crates/common/src/config.rs

/// Compile-time shard count. Selected via Cargo feature flags.
/// Supported: shard_count_16, shard_count_32, shard_count_64 (default), shard_count_128.
/// MUST be a power of 2 (bitwise routing: CRC16(key) & (SHARD_COUNT - 1)).
#[cfg(feature = "shard_count_16")]  pub const SHARD_COUNT: usize = 16;
#[cfg(feature = "shard_count_32")]  pub const SHARD_COUNT: usize = 32;
#[cfg(not(any(feature = "shard_count_16", feature = "shard_count_32", feature = "shard_count_128")))]
pub const SHARD_COUNT: usize = 64;  // default
#[cfg(feature = "shard_count_128")] pub const SHARD_COUNT: usize = 128;

/// Per-shard hard limits.
pub const MAX_KEYS_PER_SHARD: usize = 100_000_000;   // 100M keys
pub const MAX_MEMORY_PER_SHARD: usize = 20 * 1024 * 1024 * 1024; // 20 GB
pub const MAX_KEY_SIZE: usize = 512;
pub const MAX_VALUE_SIZE: usize = 512 * 1024 * 1024; // 512 MB
pub const ENTRY_OVERHEAD: usize = 48; // bytes of fixed overhead per entry (key vec + value vec + Option<Instant>)

/// Parsed runtime configuration. Shared via Arc<Config>.
pub struct Config {
    pub port: u16,
    pub bind: std::net::IpAddr,
    pub snapshot_dir: std::path::PathBuf,
    pub snapshot_interval_secs: u64,
    pub log_level: String,
    pub max_connections: usize,
    pub frontend_threads: usize,
    pub metrics_port: u16,
    pub cpu_pinning: bool,
}
```

## 6. Public Interfaces

```rust
// Everything above is the public interface.
// Key re-exports from lib.rs:
pub use types::{Key, Value, Entry, ShardId, Expiry};
pub use command::{Command, Operation, Response, SnapshotRequest};
pub use error::Error;
pub use metrics::{ShardMetrics, Histogram, HistogramSnapshot};
pub use config::{Config, SHARD_COUNT, MAX_KEYS_PER_SHARD, MAX_MEMORY_PER_SHARD, MAX_KEY_SIZE, MAX_VALUE_SIZE, ENTRY_OVERHEAD};
```

## 7. Internal Algorithms

No algorithms — this crate is pure data type definitions.

## 8. Persistence Model

None. All types here are in-memory. Snapshot serialization lives in `crates/persistence`.

## 9. Concurrency Model

- `ShardMetrics` fields are `AtomicU64` / `AtomicUsize` — safe to share across threads via `Arc`
- `Mutex<Histogram>` is the only non-atomic field; held briefly by the metrics exporter at scrape time, never by shard threads during command execution
- `Config` is immutable after construction; safely shared via `Arc<Config>`
- All other types are `Send + Sync` by construction (owned data, no interior mutability)

## 10. Configuration

Cargo.toml feature flags:
```toml
[features]
default = []
shard_count_16  = []
shard_count_32  = []
shard_count_128 = []
```

`Config` is constructed in `crates/server` from parsed CLI flags + env vars.

## 11. Observability

- No metrics or logs emitted from this crate
- `ShardMetrics` is the metrics *container* for other crates to populate

## 12. Testing Strategy

- **Unit tests**:
  - `test_value_as_bytes`: assert `Value::String(b"hello".to_vec()).as_bytes() == b"hello"`
  - `test_expiry_min_heap_order`: push 3 `Expiry` entries to `BinaryHeap`, verify pop order is earliest first
  - `test_shard_metrics_concurrent_increment`: spawn 10 threads each doing `fetch_add(1)`, assert final count is 10
  - `test_histogram_record_and_snapshot`: record latencies, verify bucket counts and sum are correct
  - `test_shard_count_is_power_of_two`: `assert!(SHARD_COUNT.is_power_of_two())`
  - `test_error_display`: verify each `Error` variant produces expected message string

## 13. Open Questions

None.
