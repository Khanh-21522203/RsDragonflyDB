# Testing Strategy

## Document Purpose
This document defines the comprehensive testing approach for RsDragonflyDB, including unit tests, integration tests, performance tests, and quality gates.

**Audience**: Engineering team, QA engineers

---

## Testing Philosophy

### Quality Goals

1. **Correctness**: All commands behave according to specification
2. **Reliability**: No crashes, panics, or data corruption
3. **Performance**: Meet latency and throughput targets
4. **Concurrency**: No data races or deadlocks
5. **Durability**: Crash recovery works correctly

---

## Test Pyramid

```
                    ┌─────────────┐
                    │   Manual    │  (Exploratory, ad-hoc)
                    │   Testing   │
                    └─────────────┘
                  ┌─────────────────┐
                  │  Performance    │  (Benchmarks, load tests)
                  │     Tests       │
                  └─────────────────┘
              ┌───────────────────────┐
              │   Integration Tests   │  (End-to-end, multi-component)
              └───────────────────────┘
          ┌───────────────────────────────┐
          │        Unit Tests             │  (Fast, isolated, many)
          └───────────────────────────────┘
```

**Distribution**:
- Unit tests: 70% (fast, isolated)
- Integration tests: 20% (realistic scenarios)
- Performance tests: 10% (benchmarks, load tests)

---

## Unit Tests

### Scope

**What to Test**:
- Individual functions and methods
- Data structure invariants
- Edge cases and error conditions
- Rust-specific concerns (ownership, lifetimes)

**What NOT to Test**:
- Integration between components (use integration tests)
- Performance (use benchmarks)
- External dependencies (mock or stub)

---

### Test Organization

```
crates/
├── protocol/
│   ├── src/
│   │   ├── parser.rs
│   │   └── serializer.rs
│   └── tests/
│       ├── parser_tests.rs
│       └── serializer_tests.rs
├── shard/
│   ├── src/
│   │   ├── shard.rs
│   │   └── ttl.rs
│   └── tests/
│       ├── shard_tests.rs
│       └── ttl_tests.rs
└── ...
```

**Convention**: Tests in `tests/` directory (integration-style unit tests).

---

### Example Unit Tests

**RESP2 Parser**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_string() {
        let input = b"+OK\r\n";
        let result = parse_resp2(input).unwrap();
        assert_eq!(result, RespValue::SimpleString("OK".to_string()));
    }

    #[test]
    fn test_parse_bulk_string() {
        let input = b"$5\r\nhello\r\n";
        let result = parse_resp2(input).unwrap();
        assert_eq!(result, RespValue::BulkString(Some(b"hello".to_vec())));
    }

    #[test]
    fn test_parse_array() {
        let input = b"*2\r\n$3\r\nGET\r\n$3\r\nkey\r\n";
        let result = parse_resp2(input).unwrap();
        assert_eq!(result, RespValue::Array(vec![
            RespValue::BulkString(Some(b"GET".to_vec())),
            RespValue::BulkString(Some(b"key".to_vec())),
        ]));
    }

    #[test]
    fn test_parse_invalid_input() {
        let input = b"invalid\r\n";
        let result = parse_resp2(input);
        assert!(result.is_err());
    }
}
```

---

**Shard Operations**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get() {
        let mut shard = Shard::new(ShardId(0));
        shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), None).unwrap();
        
        let value = shard.get(b"key").unwrap();
        assert_eq!(value, &Value::String(b"value".to_vec()));
    }

    #[test]
    fn test_get_nonexistent_key() {
        let shard = Shard::new(ShardId(0));
        let value = shard.get(b"key");
        assert!(value.is_none());
    }

    #[test]
    fn test_del() {
        let mut shard = Shard::new(ShardId(0));
        shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), None).unwrap();
        
        let deleted = shard.del(&[b"key".to_vec()]);
        assert_eq!(deleted, 1);
        
        let value = shard.get(b"key");
        assert!(value.is_none());
    }

    #[test]
    fn test_memory_limit() {
        let mut shard = Shard::new(ShardId(0));
        shard.max_memory_bytes = 1000;
        
        // Insert large value
        let large_value = vec![0u8; 2000];
        let result = shard.set(b"key".to_vec(), Value::String(large_value), None);
        
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), Error::OutOfMemory);
    }
}
```

---

**TTL Expiration**:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lazy_expiration() {
        let mut shard = Shard::new(ShardId(0));
        let expiry = Instant::now() + Duration::from_millis(100);
        shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), Some(expiry)).unwrap();
        
        // Key exists before expiration
        assert!(shard.get(b"key").is_some());
        
        // Wait for expiration
        std::thread::sleep(Duration::from_millis(150));
        
        // Key is deleted on access
        assert!(shard.get(b"key").is_none());
    }

    #[test]
    fn test_active_expiration() {
        let mut shard = Shard::new(ShardId(0));
        let expiry = Instant::now() + Duration::from_millis(100);
        shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), Some(expiry)).unwrap();
        
        // Wait for expiration
        std::thread::sleep(Duration::from_millis(150));
        
        // Run active expiration
        shard.expire_keys();
        
        // Key is deleted
        assert_eq!(shard.data.len(), 0);
    }
}
```

---

### Test Coverage

**Target**: 80% line coverage for core logic.

**Measurement**:
```bash
cargo install cargo-tarpaulin
cargo tarpaulin --out Html --output-dir coverage
```

**Exclusions**:
- Main function
- Logging statements
- Error formatting (Display impl)

---

## Integration Tests

### Scope

**What to Test**:
- End-to-end command execution
- Multi-component interaction (frontend → shard → persistence)
- Redis protocol compatibility
- Crash recovery
- Concurrent client access

---

### Test Organization

```
tests/
├── integration/
│   ├── basic_commands.rs
│   ├── ttl_commands.rs
│   ├── multi_key_commands.rs
│   ├── persistence.rs
│   ├── concurrency.rs
│   └── redis_compatibility.rs
└── common/
    └── test_server.rs
```

---

### Test Harness

```rust
// tests/common/test_server.rs
pub struct TestServer {
    process: Child,
    port: u16,
    snapshot_dir: TempDir,
}

impl TestServer {
    pub fn start() -> Self {
        let snapshot_dir = TempDir::new().unwrap();
        let port = find_free_port();
        
        let process = Command::new("target/debug/rsdragonfly")
            .arg("--port").arg(port.to_string())
            .arg("--snapshot-dir").arg(snapshot_dir.path())
            .arg("--snapshot-interval").arg("5")  // Fast snapshots for testing
            .spawn()
            .unwrap();
        
        // Wait for server to start
        std::thread::sleep(Duration::from_millis(500));
        
        TestServer { process, port, snapshot_dir }
    }
    
    pub fn connect(&self) -> redis::Connection {
        let client = redis::Client::open(format!("redis://127.0.0.1:{}", self.port)).unwrap();
        client.get_connection().unwrap()
    }
    
    pub fn restart(&mut self) {
        self.process.kill().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        
        self.process = Command::new("target/debug/rsdragonfly")
            .arg("--port").arg(self.port.to_string())
            .arg("--snapshot-dir").arg(self.snapshot_dir.path())
            .spawn()
            .unwrap();
        
        std::thread::sleep(Duration::from_millis(500));
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.process.kill().ok();
    }
}
```

---

### Example Integration Tests

**Basic Commands**:
```rust
#[test]
fn test_ping() {
    let server = TestServer::start();
    let mut conn = server.connect();
    
    let result: String = redis::cmd("PING").query(&mut conn).unwrap();
    assert_eq!(result, "PONG");
}

#[test]
fn test_set_and_get() {
    let server = TestServer::start();
    let mut conn = server.connect();
    
    redis::cmd("SET").arg("mykey").arg("myvalue").execute(&mut conn);
    let value: String = redis::cmd("GET").arg("mykey").query(&mut conn).unwrap();
    
    assert_eq!(value, "myvalue");
}

#[test]
fn test_del() {
    let server = TestServer::start();
    let mut conn = server.connect();
    
    redis::cmd("SET").arg("key1").arg("value1").execute(&mut conn);
    redis::cmd("SET").arg("key2").arg("value2").execute(&mut conn);
    
    let deleted: i32 = redis::cmd("DEL").arg("key1").arg("key2").query(&mut conn).unwrap();
    assert_eq!(deleted, 2);
    
    let value: Option<String> = redis::cmd("GET").arg("key1").query(&mut conn).unwrap();
    assert!(value.is_none());
}
```

---

**TTL Commands**:
```rust
#[test]
fn test_set_with_ex() {
    let server = TestServer::start();
    let mut conn = server.connect();
    
    redis::cmd("SET").arg("key").arg("value").arg("EX").arg(2).execute(&mut conn);
    
    let value: String = redis::cmd("GET").arg("key").query(&mut conn).unwrap();
    assert_eq!(value, "value");
    
    std::thread::sleep(Duration::from_secs(3));
    
    let value: Option<String> = redis::cmd("GET").arg("key").query(&mut conn).unwrap();
    assert!(value.is_none());
}

#[test]
fn test_expire_and_ttl() {
    let server = TestServer::start();
    let mut conn = server.connect();
    
    redis::cmd("SET").arg("key").arg("value").execute(&mut conn);
    redis::cmd("EXPIRE").arg("key").arg(10).execute(&mut conn);
    
    let ttl: i64 = redis::cmd("TTL").arg("key").query(&mut conn).unwrap();
    assert!(ttl >= 9 && ttl <= 10);
}
```

---

**Persistence**:
```rust
#[test]
fn test_crash_recovery() {
    let mut server = TestServer::start();
    let mut conn = server.connect();
    
    // Write data
    for i in 0..1000 {
        redis::cmd("SET").arg(format!("key{}", i)).arg("value").execute(&mut conn);
    }
    
    // Wait for snapshot
    std::thread::sleep(Duration::from_secs(6));
    
    // Restart server
    server.restart();
    let mut conn = server.connect();
    
    // Verify data recovered
    for i in 0..1000 {
        let value: String = redis::cmd("GET").arg(format!("key{}", i)).query(&mut conn).unwrap();
        assert_eq!(value, "value");
    }
}

#[test]
fn test_ttl_persists_across_restart() {
    let mut server = TestServer::start();
    let mut conn = server.connect();
    
    redis::cmd("SET").arg("key").arg("value").arg("EX").arg(3600).execute(&mut conn);
    
    // Wait for snapshot
    std::thread::sleep(Duration::from_secs(6));
    
    // Restart server
    server.restart();
    let mut conn = server.connect();
    
    // Verify TTL persists
    let ttl: i64 = redis::cmd("TTL").arg("key").query(&mut conn).unwrap();
    assert!(ttl > 3500 && ttl <= 3600);
}
```

---

**Concurrency**:
```rust
#[test]
fn test_concurrent_clients() {
    let server = TestServer::start();
    
    let handles: Vec<_> = (0..10)
        .map(|client_id| {
            let port = server.port;
            std::thread::spawn(move || {
                let client = redis::Client::open(format!("redis://127.0.0.1:{}", port)).unwrap();
                let mut conn = client.get_connection().unwrap();
                
                for i in 0..100 {
                    let key = format!("key:{}:{}", client_id, i);
                    redis::cmd("SET").arg(&key).arg("value").execute(&mut conn);
                    let value: String = redis::cmd("GET").arg(&key).query(&mut conn).unwrap();
                    assert_eq!(value, "value");
                }
            })
        })
        .collect();
    
    for handle in handles {
        handle.join().unwrap();
    }
}
```

---

## Concurrency Tests

### ThreadSanitizer

**Purpose**: Detect data races.

**Usage**:
```bash
# Install Rust nightly
rustup install nightly

# Run tests with ThreadSanitizer
RUSTFLAGS="-Z sanitizer=thread" cargo +nightly test --target x86_64-unknown-linux-gnu
```

**Expected**: Zero data races reported.

---

### Loom (Concurrency Testing)

**Purpose**: Exhaustively test concurrent code.

**Usage**:
```rust
#[cfg(loom)]
mod loom_tests {
    use loom::sync::Arc;
    use loom::thread;

    #[test]
    fn test_concurrent_shard_access() {
        loom::model(|| {
            let shard = Arc::new(Mutex::new(Shard::new(ShardId(0))));
            
            let shard1 = shard.clone();
            let t1 = thread::spawn(move || {
                shard1.lock().unwrap().set(b"key".to_vec(), Value::String(b"value1".to_vec()), None);
            });
            
            let shard2 = shard.clone();
            let t2 = thread::spawn(move || {
                shard2.lock().unwrap().set(b"key".to_vec(), Value::String(b"value2".to_vec()), None);
            });
            
            t1.join().unwrap();
            t2.join().unwrap();
        });
    }
}
```

**Note**: Loom is expensive (exhaustive search). Use sparingly.

---

## Performance Tests

### Benchmarks

**Framework**: Criterion.rs

**Organization**:
```
benches/
├── shard_ops.rs
├── protocol_parsing.rs
└── end_to_end.rs
```

---

**Example Benchmark**:
```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_shard_get(c: &mut Criterion) {
    let mut shard = Shard::new(ShardId(0));
    shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), None).unwrap();
    
    c.bench_function("shard_get", |b| {
        b.iter(|| {
            shard.get(black_box(b"key"))
        });
    });
}

fn bench_shard_set(c: &mut Criterion) {
    let mut shard = Shard::new(ShardId(0));
    
    c.bench_function("shard_set", |b| {
        b.iter(|| {
            shard.set(
                black_box(b"key".to_vec()),
                black_box(Value::String(b"value".to_vec())),
                None
            )
        });
    });
}

criterion_group!(benches, bench_shard_get, bench_shard_set);
criterion_main!(benches);
```

**Run**:
```bash
cargo bench
```

---

### Load Tests

**Tool**: redis-benchmark (included with Redis)

**Scenarios**:

**1. GET/SET Mix**:
```bash
redis-benchmark -h localhost -p 6379 -t get,set -n 1000000 -c 50 -d 100
```

**Expected**:
- Throughput: > 1M QPS
- Latency p99: < 1ms

---

**2. Pipeline**:
```bash
redis-benchmark -h localhost -p 6379 -t get,set -n 1000000 -c 50 -P 10
```

**Expected**:
- Throughput: > 2M QPS (with pipelining)

---

**3. Large Values**:
```bash
redis-benchmark -h localhost -p 6379 -t set -n 100000 -c 50 -d 10000
```

**Expected**:
- Throughput: > 100K QPS (10KB values)

---

### Stress Tests

**Purpose**: Find breaking points.

**Scenarios**:

**1. Memory Exhaustion**:
```bash
# Fill memory until OOM
redis-benchmark -h localhost -p 6379 -t set -n 100000000 -c 50 -d 1000
```

**Expected**: Server rejects writes with `-ERR OOM`, does not crash.

---

**2. Connection Exhaustion**:
```bash
# Open 10,000 connections
for i in {1..10000}; do
    redis-cli -h localhost -p 6379 PING &
done
```

**Expected**: Server handles gracefully (may reject new connections).

---

**3. Snapshot During Load**:
```bash
# Run load test while snapshots are being written
redis-benchmark -h localhost -p 6379 -t get,set -n 10000000 -c 50 &
# Snapshots run every 60s
```

**Expected**: No latency spikes > 10ms during snapshot.

---

## Chaos Tests

### Chaos Engineering

**Purpose**: Test resilience under failures.

**Scenarios**:

**1. Random SIGKILL**:
```rust
#[test]
fn test_random_crashes() {
    let mut server = TestServer::start();
    
    for _ in 0..10 {
        // Write data
        let mut conn = server.connect();
        for i in 0..1000 {
            redis::cmd("SET").arg(format!("key{}", i)).arg("value").execute(&mut conn);
        }
        
        // Random delay
        std::thread::sleep(Duration::from_millis(rand::random::<u64>() % 5000));
        
        // Kill server
        server.process.kill().unwrap();
        
        // Restart
        server.restart();
    }
    
    // Verify no corruption
    let mut conn = server.connect();
    redis::cmd("PING").query::<String>(&mut conn).unwrap();
}
```

---

**2. Disk Full**:
```rust
#[test]
fn test_disk_full_during_snapshot() {
    // Use a small tmpfs mount
    let snapshot_dir = "/tmp/small_disk";  // 10MB tmpfs
    
    let server = TestServer::start_with_dir(snapshot_dir);
    let mut conn = server.connect();
    
    // Fill disk
    for i in 0..100000 {
        redis::cmd("SET").arg(format!("key{}", i)).arg("x".repeat(1000)).execute(&mut conn);
    }
    
    // Wait for snapshot (will fail due to disk full)
    std::thread::sleep(Duration::from_secs(61));
    
    // Verify server still responsive
    let result: String = redis::cmd("PING").query(&mut conn).unwrap();
    assert_eq!(result, "PONG");
}
```

---

## Fuzzing

### Protocol Fuzzing

**Tool**: cargo-fuzz

**Setup**:
```bash
cargo install cargo-fuzz
cargo fuzz init
```

**Fuzz Target**:
```rust
// fuzz/fuzz_targets/resp2_parser.rs
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = parse_resp2(data);
});
```

**Run**:
```bash
cargo fuzz run resp2_parser
```

**Expected**: No panics, no crashes.

---

## CI/CD Quality Gates

### Pre-Commit Checks

```bash
# Format check
cargo fmt --check

# Lint
cargo clippy -- -D warnings

# Unit tests
cargo test --lib

# Build
cargo build --release
```

---

### CI Pipeline

```yaml
# .github/workflows/ci.yml
name: CI

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      
      - name: Format check
        run: cargo fmt --check
      
      - name: Clippy
        run: cargo clippy -- -D warnings
      
      - name: Unit tests
        run: cargo test --lib
      
      - name: Integration tests
        run: cargo test --test '*'
      
      - name: Benchmarks (smoke test)
        run: cargo bench --no-run
  
  coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - name: Coverage
        run: |
          cargo install cargo-tarpaulin
          cargo tarpaulin --out Xml
      - name: Upload coverage
        uses: codecov/codecov-action@v2
```

---

## Definition of Done

Testing strategy is complete when:

1. Unit tests cover 80% of core logic
2. Integration tests cover all MVP commands
3. Concurrency tests pass with ThreadSanitizer
4. Performance tests meet targets (1M QPS, p99 < 1ms)
5. Chaos tests verify crash recovery
6. Fuzzing finds no crashes
7. CI pipeline enforces quality gates
8. All tests are documented and runnable

---

## References

- [requirements.md](../requirements.md) - Functional requirements
- [overview.md](../architecture/overview.md) - System architecture
- [observability.md](../observability/observability.md) - Metrics and logging
