# RsDragonflyDB

## What is RsDragonflyDB?

RsDragonflyDB is a high-performance, in-memory key-value database written in Rust, designed for extreme throughput and predictable low latency. It implements a Redis-compatible RESP2 protocol subset and uses a strict **per-shard, thread-per-core architecture** to eliminate lock contention and maximize multi-core utilization.

## Core Design Philosophy

**One Shard, One Thread, One Owner**

Every shard in RsDragonflyDB is:
- Owned by exactly one OS thread
- Responsible for a disjoint subset of the keyspace
- Free from shared mutable state with other shards
- Capable of operating independently without global locks

This architecture delivers:
- **Predictable latency**: No lock contention on hot paths
- **Linear scalability**: Performance scales with CPU cores
- **Simple reasoning**: Clear ownership boundaries
- **Production reliability**: Fewer race conditions and deadlocks

## MVP Feature Set

### Supported Commands
- `PING` - Connection health check
- `SET key value [EX seconds]` - Store key-value with optional TTL
- `GET key` - Retrieve value by key
- `DEL key [key ...]` - Delete one or more keys
- `EXPIRE key seconds` - Set TTL on existing key
- `TTL key` - Query remaining TTL
- `INFO [section]` - Server statistics (minimal)

### Architecture Highlights
- **Fixed sharding**: 64 shards (configurable at compile time)
- **Hash-based routing**: CRC16 key hashing for shard assignment
- **Snapshot persistence**: Periodic full-state snapshots for crash recovery
- **No cross-shard atomicity**: Multi-key commands execute independently per shard
- **Lazy + active TTL expiration**: Hybrid approach for memory efficiency

## Quick Start

### Prerequisites
- Rust 1.75+ (2021 edition)
- Linux or macOS (Windows support not tested)
- 4+ CPU cores recommended

### Build
```bash
cargo build --release
```

### Run
```bash
./target/release/rsdragonfly --port 6379
```

> Shard count is set at compile time (default: 64). See [architecture/overview.md](architecture/overview.md#compile-time-configuration) to change it.

### Connect
```bash
redis-cli -p 6379
127.0.0.1:6379> PING
PONG
127.0.0.1:6379> SET mykey "Hello, RsDragonflyDB!"
OK
127.0.0.1:6379> GET mykey
"Hello, RsDragonflyDB!"
```

## Performance Characteristics

### Expected MVP Performance (64 shards, 64-core machine)
- **Throughput**: 1M+ QPS for GET/SET workloads
- **Latency**: p99 < 1ms for single-key operations
- **Memory**: ~50 bytes overhead per key-value pair
- **Persistence**: Snapshot every 60s (configurable)

### Scaling Properties
- Throughput scales linearly with shard count up to physical core count
- Single-shard operations have zero cross-shard coordination
- Multi-key operations scale with number of unique shards touched

## Non-Goals (MVP)

RsDragonflyDB MVP explicitly does NOT support:
- Full Redis command compatibility (only subset listed above)
- Redis Cluster protocol
- Transactions (MULTI/EXEC)
- Lua scripting
- Pub/Sub
- Streams
- Strong cross-shard atomicity
- Online resharding
- Replication or high availability
- Fine-grained ACLs

See [roadmap.md](roadmap.md) for post-MVP features.

## Project Structure

```
rsdragonfly/
├── crates/
│   ├── protocol/       # RESP2 parser and serializer
│   ├── frontend/       # Connection handling and routing
│   ├── shard/          # Per-shard storage engine
│   ├── persistence/    # Snapshot writer/reader
│   └── common/         # Shared utilities (no mutable state)
├── docs/               # This documentation
├── tests/              # Integration tests
└── benches/            # Performance benchmarks
```

## Documentation

See [INDEX.md](INDEX.md) for complete documentation structure.

Key documents:
- [requirements.md](requirements.md) - What we're building
- [architecture/overview.md](architecture/overview.md) - How it works
- [testing/testing-strategy.md](testing/testing-strategy.md) - Quality assurance
- [devops/deployment.md](devops/deployment.md) - Running in production

## Development

### Setup

```bash
# Clone and enter the repo
git clone <repo-url>
cd rsdragonfly

# Build (debug mode, faster compile)
cargo build

# Build (release mode, optimized)
cargo build --release
```

### Running Tests

```bash
# Unit tests only (fast, no server required)
cargo test --lib

# All tests including integration tests
cargo test

# Run a specific test
cargo test test_set_and_get

# Run with ThreadSanitizer (requires nightly)
RUSTFLAGS="-Z sanitizer=thread" cargo +nightly test --target x86_64-unknown-linux-gnu
```

### Code Quality

```bash
# Format code
cargo fmt

# Check formatting without changing files
cargo fmt --check

# Lint
cargo clippy -- -D warnings

# Run benchmarks
cargo bench
```

### Coverage

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --out Html --output-dir coverage
open coverage/tarpaulin-report.html
```

### Fuzzing

```bash
cargo install cargo-fuzz
cargo fuzz run resp2_parser
```

---

## Contributing

RsDragonflyDB is currently in MVP development. Contributions must:
- Respect the per-shard, thread-per-core architecture
- Include tests (unit + integration)
- Pass `cargo fmt` and `cargo clippy`
- Not introduce shared mutable state across shards

## License

[To be determined - suggest Apache 2.0 or MIT]

## Acknowledgments

RsDragonflyDB is inspired by the architectural ideas of DragonflyDB but is an independent, original implementation. We do not copy DragonflyDB source code or internal implementation details.

## Contact

[Project maintainer contact information]
