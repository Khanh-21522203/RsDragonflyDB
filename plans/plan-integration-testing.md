# Feature: Integration Testing

## 1. Purpose

Integration tests verify the system end-to-end: a real server process is started, a real TCP client connects, commands are sent, and responses are validated. They catch bugs that unit tests cannot — incorrect RESP2 encoding, wrong channel wiring, snapshot file corruption on real disk.

All integration tests live in `crates/server/tests/`. They use a `TestServer` harness that starts the server on a random port, waits for it to be ready, runs the test, and shuts it down cleanly.

## 2. Test Infrastructure

### Dependency: `redis` crate for the client

Add to `crates/server/Cargo.toml` under `[dev-dependencies]`:

```toml
[dev-dependencies]
redis = "0.27"         # Redis client library — speaks RESP2, compatible with our server
tempfile = "3"         # Temporary directory for snapshot files
```

### TestServer harness

Create `crates/server/tests/common/mod.rs`:

```rust
// crates/server/tests/common/mod.rs
//
// A TestServer starts the real rsdragonfly server in a background thread
// on a randomly chosen free port, waits until PING returns PONG (ready),
// and provides a helper to get a redis::Connection pointed at it.
// Drop the TestServer to shut it down.

use std::net::TcpListener as StdTcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct TestServer {
    pub port: u16,
    pub snapshot_dir: tempfile::TempDir,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestServer {
    /// Start a server on a random free port. Blocks until PING returns PONG.
    pub fn start() -> Self {
        // Pick a free port by binding to :0 and reading the assigned port
        let port = {
            let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let snapshot_dir = tempfile::tempdir().unwrap();
        let snapshot_path = snapshot_dir.path().to_path_buf();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        // Build Config and start the server in a background thread
        let thread = std::thread::spawn(move || {
            // Build argv as if calling: rsdragonfly --port N --snapshot-dir /tmp/...
            let argv = vec![
                "rsdragonfly".to_string(),
                "--port".to_string(),  port.to_string(),
                "--snapshot-dir".to_string(), snapshot_path.to_string_lossy().into(),
                "--log-level".to_string(), "warn".to_string(),
                "--metrics-port".to_string(), "0".to_string(),  // disable metrics server
                "--frontend-threads".to_string(), "2".to_string(),
            ];
            rsdragonfly_server::run_with_argv_and_shutdown(&argv, shutdown_clone);
        });

        let server = TestServer { port, snapshot_dir, shutdown, thread: Some(thread) };

        // Wait for the server to be ready (up to 5 seconds)
        server.wait_ready(Duration::from_secs(5));
        server
    }

    /// Block until PING returns PONG (server is ready to accept commands).
    fn wait_ready(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() > deadline {
                panic!("TestServer did not start within {:?}", timeout);
            }
            match self.connection() {
                Ok(mut conn) => {
                    let pong: redis::RedisResult<String> = redis::cmd("PING").query(&mut conn);
                    if pong.is_ok() { return; }
                }
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Get a new redis::Connection to this server.
    pub fn connection(&self) -> redis::RedisResult<redis::Connection> {
        let client = redis::Client::open(format!("redis://127.0.0.1:{}/", self.port))?;
        client.get_connection()
    }

    /// Convenience: get a connection or panic.
    pub fn conn(&self) -> redis::Connection {
        self.connection().expect("failed to connect to TestServer")
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Signal the server to shut down
        self.shutdown.store(true, Ordering::Release);
        // Join the server thread
        if let Some(thread) = self.thread.take() {
            thread.join().ok();
        }
    }
}
```

### Server entrypoint (needed for TestServer)

The server's `main.rs` calls `run_with_argv_and_shutdown` so tests can reuse it. Expose this as a library function from `crates/server`:

```rust
// crates/server/src/lib.rs  (add this file)

/// Run the server. Blocks until `shutdown` is set to true.
/// Called by `main()` and by integration tests via `TestServer`.
pub fn run_with_argv_and_shutdown(argv: &[String], shutdown: Arc<AtomicBool>) {
    // same body as main(), but using the provided shutdown flag
    // instead of registering a signal handler
    // ...
}
```

```rust
// crates/server/src/main.rs
fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let shutdown = Arc::new(AtomicBool::new(false));

    // Register SIGTERM / SIGINT to set the flag
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown)).unwrap();
    signal_hook::flag::register(signal_hook::consts::SIGINT,  Arc::clone(&shutdown)).unwrap();

    rsdragonfly_server::run_with_argv_and_shutdown(&argv, shutdown);
}
```

## 3. Test File Layout

```
crates/server/
└── tests/
    ├── common/
    │   └── mod.rs        ← TestServer harness (above)
    ├── test_basic_commands.rs
    ├── test_ttl.rs
    ├── test_persistence.rs
    ├── test_multi_key.rs
    └── test_info.rs
```

Each test file begins with:
```rust
mod common;
use common::TestServer;
```

## 4. Test Implementations

### test_basic_commands.rs

```rust
mod common;
use common::TestServer;
use redis::Commands;

#[test]
fn test_ping() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let pong: String = redis::cmd("PING").query(&mut conn).unwrap();
    assert_eq!(pong, "PONG");
}

#[test]
fn test_ping_with_message() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let echo: String = redis::cmd("PING").arg("hello").query(&mut conn).unwrap();
    assert_eq!(echo, "hello");
}

#[test]
fn test_set_and_get() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let _: () = conn.set("mykey", "myvalue").unwrap();
    let val: String = conn.get("mykey").unwrap();
    assert_eq!(val, "myvalue");
}

#[test]
fn test_get_missing_key_returns_nil() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let val: Option<String> = conn.get("nonexistent").unwrap();
    assert!(val.is_none());
}

#[test]
fn test_set_overwrites_existing_key() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let _: () = conn.set("k", "v1").unwrap();
    let _: () = conn.set("k", "v2").unwrap();
    let val: String = conn.get("k").unwrap();
    assert_eq!(val, "v2");
}

#[test]
fn test_del_single_key() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let _: () = conn.set("k", "v").unwrap();
    let count: i64 = conn.del("k").unwrap();
    assert_eq!(count, 1);
    let val: Option<String> = conn.get("k").unwrap();
    assert!(val.is_none());
}

#[test]
fn test_del_missing_key_returns_zero() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let count: i64 = conn.del("ghost").unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_del_multi_key() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let _: () = conn.set("k1", "v1").unwrap();
    let _: () = conn.set("k2", "v2").unwrap();
    let _: () = conn.set("k3", "v3").unwrap();
    let count: i64 = conn.del(&["k1", "k2", "k3"]).unwrap();
    assert_eq!(count, 3);
}

#[test]
fn test_unknown_command_returns_error() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let result: redis::RedisResult<String> = redis::cmd("FOOBAR").query(&mut conn);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("unknown command"));
}

#[test]
fn test_pipelining() {
    let server = TestServer::start();
    let mut conn = server.conn();

    // Send 5 SET commands in a pipeline (all in one TCP write)
    let mut pipe = redis::pipe();
    for i in 0..5 {
        pipe.set(format!("pipe:{}", i), format!("val{}", i));
    }
    let _: () = pipe.query(&mut conn).unwrap();

    // Verify all 5 keys are present
    for i in 0..5 {
        let val: String = conn.get(format!("pipe:{}", i)).unwrap();
        assert_eq!(val, format!("val{}", i));
    }
}
```

### test_ttl.rs

```rust
mod common;
use common::TestServer;
use redis::Commands;
use std::time::Duration;

#[test]
fn test_set_with_ex_expires_after_ttl() {
    let server = TestServer::start();
    let mut conn = server.conn();

    redis::cmd("SET").arg("expkey").arg("value").arg("EX").arg(1)
        .execute(&mut conn);

    // Immediately readable
    let val: String = conn.get("expkey").unwrap();
    assert_eq!(val, "value");

    // Wait for expiry
    std::thread::sleep(Duration::from_secs(2));

    let expired: Option<String> = conn.get("expkey").unwrap();
    assert!(expired.is_none(), "key should have expired");
}

#[test]
fn test_expire_command() {
    let server = TestServer::start();
    let mut conn = server.conn();

    let _: () = conn.set("k", "v").unwrap();
    let set_result: i64 = conn.expire("k", 2).unwrap();
    assert_eq!(set_result, 1, "EXPIRE should return 1 for existing key");

    std::thread::sleep(Duration::from_secs(3));
    let val: Option<String> = conn.get("k").unwrap();
    assert!(val.is_none(), "key should have expired after EXPIRE");
}

#[test]
fn test_expire_on_missing_key_returns_zero() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let result: i64 = conn.expire("ghost", 10).unwrap();
    assert_eq!(result, 0);
}

#[test]
fn test_ttl_no_expiry_returns_minus_one() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let _: () = conn.set("k", "v").unwrap();
    let ttl: i64 = conn.ttl("k").unwrap();
    assert_eq!(ttl, -1);
}

#[test]
fn test_ttl_missing_key_returns_minus_two() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let ttl: i64 = conn.ttl("ghost").unwrap();
    assert_eq!(ttl, -2);
}

#[test]
fn test_ttl_with_expiry_returns_remaining_seconds() {
    let server = TestServer::start();
    let mut conn = server.conn();
    redis::cmd("SET").arg("k").arg("v").arg("EX").arg(100).execute(&mut conn);
    let ttl: i64 = conn.ttl("k").unwrap();
    assert!(ttl > 95 && ttl <= 100, "TTL should be ~100 seconds, got {}", ttl);
}
```

### test_persistence.rs

```rust
mod common;
use common::TestServer;
use redis::Commands;
use std::time::Duration;

#[test]
fn test_data_survives_restart() {
    // Use a persistent temp dir that survives server restart
    let snapshot_dir = tempfile::tempdir().unwrap();
    let snapshot_path = snapshot_dir.path().to_path_buf();

    // Start server, write data, wait for snapshot, stop
    {
        let server = TestServer::start_with_snapshot_dir(snapshot_path.clone());
        let mut conn = server.conn();
        for i in 0..100 {
            let _: () = conn.set(format!("key:{}", i), format!("val:{}", i)).unwrap();
        }
        // Force snapshot (wait for the 60s interval is too slow for tests;
        // use a short interval in TestServer::start_with_snapshot_dir)
        std::thread::sleep(Duration::from_secs(2));
        // server is dropped here → graceful shutdown → final snapshot written
    }

    // Restart server from same snapshot dir
    {
        let server = TestServer::start_with_snapshot_dir(snapshot_path);
        let mut conn = server.conn();
        for i in 0..100 {
            let val: String = conn.get(format!("key:{}", i)).unwrap();
            assert_eq!(val, format!("val:{}", i));
        }
    }
}

#[test]
fn test_expired_keys_not_recovered() {
    let snapshot_dir = tempfile::tempdir().unwrap();
    let snapshot_path = snapshot_dir.path().to_path_buf();

    {
        let server = TestServer::start_with_snapshot_dir(snapshot_path.clone());
        let mut conn = server.conn();
        // Set with 1 second TTL
        redis::cmd("SET").arg("expiring").arg("value").arg("EX").arg(1)
            .execute(&mut conn);
        std::thread::sleep(Duration::from_secs(3)); // wait for key to expire AND snapshot
    }

    {
        let server = TestServer::start_with_snapshot_dir(snapshot_path);
        let mut conn = server.conn();
        let val: Option<String> = conn.get("expiring").unwrap();
        assert!(val.is_none(), "expired key should not be loaded from snapshot");
    }
}
```

Extend `TestServer` to support `start_with_snapshot_dir`:

```rust
impl TestServer {
    pub fn start_with_snapshot_dir(path: std::path::PathBuf) -> Self {
        // Same as start() but:
        // 1. Use the provided path instead of a fresh tempdir
        // 2. Use --snapshot-interval 1 for fast snapshots in tests
        // ... (implementation follows same pattern as start())
    }
}
```

### test_multi_key.rs

```rust
mod common;
use common::TestServer;
use redis::Commands;

#[test]
fn test_del_keys_across_shards() {
    let server = TestServer::start();
    let mut conn = server.conn();

    // These keys will likely land on different shards
    let keys = vec!["alpha", "beta", "gamma", "delta", "epsilon"];
    for k in &keys {
        let _: () = conn.set(*k, "val").unwrap();
    }

    let count: i64 = conn.del(keys.as_slice()).unwrap();
    assert_eq!(count, 5);

    for k in &keys {
        let v: Option<String> = conn.get(*k).unwrap();
        assert!(v.is_none(), "key {} should be deleted", k);
    }
}

#[test]
fn test_del_partial_existing() {
    let server = TestServer::start();
    let mut conn = server.conn();
    let _: () = conn.set("exists", "val").unwrap();
    // "missing" does not exist
    let count: i64 = conn.del(&["exists", "missing"]).unwrap();
    assert_eq!(count, 1);
}
```

### test_info.rs

```rust
mod common;
use common::TestServer;

#[test]
fn test_info_server_section() {
    let server = TestServer::start();
    let mut conn = server.conn();

    let info: String = redis::cmd("INFO").arg("server").query(&mut conn).unwrap();
    assert!(info.contains("version:"));
    assert!(info.contains("tcp_port:"));
    assert!(info.contains("shard_count:"));
    assert!(info.contains("uptime_in_seconds:"));
}

#[test]
fn test_info_stats_section() {
    let server = TestServer::start();
    let mut conn = server.conn();

    let info: String = redis::cmd("INFO").arg("stats").query(&mut conn).unwrap();
    assert!(info.contains("total_commands_processed:"));
    assert!(info.contains("instantaneous_ops_per_sec:"));
    assert!(info.contains("keys_expired:"));
}

#[test]
fn test_info_memory_section() {
    let server = TestServer::start();
    let mut conn = server.conn();

    let info: String = redis::cmd("INFO").arg("memory").query(&mut conn).unwrap();
    assert!(info.contains("used_memory:"));
    assert!(info.contains("mem_fragmentation_ratio:"));
}

#[test]
fn test_info_all_sections() {
    let server = TestServer::start();
    let mut conn = server.conn();

    // INFO with no argument returns all sections
    let info: String = redis::cmd("INFO").query(&mut conn).unwrap();
    assert!(info.contains("# server"));
    assert!(info.contains("# stats"));
    assert!(info.contains("# memory"));
}
```

## 5. Running Integration Tests

```bash
# Run all integration tests (server must NOT be already running on the test ports)
cargo test --test '*' -p rsdragonfly

# Run one test file
cargo test --test test_basic_commands -p rsdragonfly

# Run one specific test
cargo test --test test_ttl test_set_with_ex_expires_after_ttl -p rsdragonfly

# Run with output (useful for debugging failures)
cargo test --test test_basic_commands -p rsdragonfly -- --nocapture

# Run all tests (unit + integration) at once
cargo test --workspace
```

## 6. Test Isolation Rules

Each test MUST:
1. Create its own `TestServer` — never share between tests
2. Use a fresh `tempfile::tempdir()` for snapshots — never use a fixed path
3. Use a randomly assigned port — never hardcode port 6379

Each `TestServer` binds on a random free port, so tests are safe to run in parallel.

If tests are flaky (intermittent failures), the most common causes are:
- `wait_ready()` timeout too short (server slow to start under load)
- `std::thread::sleep()` too short for TTL tests (CI machines can be slow)
- Port reuse before previous TestServer has fully stopped

## 7. Concurrency / Load Tests

```rust
// crates/server/tests/test_concurrent.rs

mod common;
use common::TestServer;
use redis::Commands;

#[test]
fn test_concurrent_clients_no_data_races() {
    let server = TestServer::start();

    let handles: Vec<_> = (0..20).map(|thread_id| {
        let port = server.port;
        std::thread::spawn(move || {
            let client = redis::Client::open(format!("redis://127.0.0.1:{}/", port)).unwrap();
            let mut conn = client.get_connection().unwrap();

            for i in 0..1000 {
                let key = format!("t{}:k{}", thread_id, i);
                let _: () = conn.set(&key, "val").unwrap();
                let val: String = conn.get(&key).unwrap();
                assert_eq!(val, "val");
            }
        })
    }).collect();

    for h in handles { h.join().unwrap(); }
}
```

Run with ThreadSanitizer to catch data races:
```bash
RUSTFLAGS="-Z sanitizer=thread" cargo +nightly test \
    --target x86_64-unknown-linux-gnu \
    --test test_concurrent \
    -p rsdragonfly
```

## 8. Open Questions

None.
