# RsDragonflyDB Requirements Specification

## Document Purpose
This document defines the functional and non-functional requirements for the RsDragonflyDB MVP. All requirements are committed and non-negotiable for MVP completion.

**Audience**: Engineering team, product stakeholders, QA engineers

## Functional Requirements

### FR-1: RESP2 Protocol Compatibility

**Requirement**: RsDragonflyDB MUST implement a subset of the Redis RESP2 protocol.

**Supported Commands**:

| Command | Syntax | Behavior |
|---------|--------|----------|
| PING | `PING [message]` | Returns `PONG` or echoes message |
| SET | `SET key value [EX seconds]` | Stores key-value, optionally with TTL |
| GET | `GET key` | Returns value or nil if not found |
| DEL | `DEL key [key ...]` | Deletes keys, returns count deleted |
| EXPIRE | `EXPIRE key seconds` | Sets TTL, returns 1 if key exists, 0 otherwise |
| TTL | `TTL key` | Returns remaining seconds, -1 if no TTL, -2 if not found |
| INFO | `INFO [section]` | Returns server stats; sections: `server`, `stats`, `memory` (see below) |

**INFO Command Output**:

`INFO` (no section) returns all sections concatenated. Each section header is `# <section>\r\n` followed by `field:value\r\n` pairs.

| Section | Fields |
|---------|--------|
| `server` | `version`, `uptime_in_seconds`, `tcp_port`, `shard_count`, `os` |
| `stats` | `total_commands_processed`, `total_connections_received`, `instantaneous_ops_per_sec`, `keys_expired` |
| `memory` | `used_memory`, `used_memory_human`, `mem_fragmentation_ratio` |

Example response (bulk string):
```
# server
version:0.1.0
uptime_in_seconds:3600
tcp_port:6379
shard_count:64
os:Linux

# stats
total_commands_processed:1000000
total_connections_received:5000
instantaneous_ops_per_sec:15000
keys_expired:500

# memory
used_memory:10737418240
used_memory_human:10.00G
mem_fragmentation_ratio:1.15
```

`INFO server`, `INFO stats`, `INFO memory` return only that section.

**Note**: INFO aggregates metrics across all shards. Each shard's atomic counters are read independently, so the result is a point-in-time snapshot with no cross-shard ordering guarantee — not a globally consistent view.

---

**Error Handling**:
- Invalid command: `-ERR unknown command`
- Wrong argument count: `-ERR wrong number of arguments`
- Invalid integer: `-ERR value is not an integer or out of range`
- Protocol errors: Close connection

**Acceptance Criteria**:
- All commands pass redis-cli compatibility tests
- Error messages match Redis conventions
- RESP2 encoding/decoding is correct per Redis specification

---

### FR-2: Sharding Model

**Requirement**: RsDragonflyDB MUST use fixed hash-based sharding with per-shard ownership.

**Specifications**:
- **Shard count**: 64 (configurable at compile time via const generic)
- **Hash function**: CRC16(key) % shard_count
- **Routing**: Frontend routes each command to owning shard(s)
- **Ownership**: Each key belongs to exactly one shard; only that shard's thread may access it

**Key Distribution**:
- Hash function MUST distribute keys uniformly
- No dynamic resharding in MVP
- Shard assignment is deterministic and stable across restarts

**Acceptance Criteria**:
- Key distribution variance < 5% across shards for random keys
- Zero cross-shard data access on single-key commands
- Shard ownership verified via unit tests

---

### FR-3: Multi-Key Command Semantics

**Requirement**: Multi-key commands (DEL) MUST execute independently per shard without cross-shard atomicity.

**Behavior**:
- `DEL key1 key2 key3`:
  - Frontend determines shard for each key
  - Sends delete request to each shard independently
  - Aggregates results (count of deleted keys)
  - No ordering guarantees across shards
  - Partial failures possible (some shards succeed, others fail)

**Atomicity Guarantees**:
- **Single-shard operations**: Atomic within shard
- **Cross-shard operations**: NOT atomic, best-effort

**Error Handling**:
- If any shard fails, return error after attempting all shards
- Partial deletions are NOT rolled back

**Acceptance Criteria**:
- DEL with keys on same shard is atomic
- DEL with keys on different shards has no atomicity guarantee
- Integration tests verify partial failure behavior

---

### FR-4: TTL and Expiration

**Requirement**: RsDragonflyDB MUST support per-key TTL with hybrid expiration strategy.

**TTL Semantics**:
- TTL precision: 1 second
- TTL storage: Unix timestamp (absolute expiration time)
- Commands: SET with EX, EXPIRE, TTL

**Expiration Strategy**:
- **Lazy expiration**: Check TTL on GET/access, delete if expired
- **Active expiration**: Background task per shard scans 20 keys/100ms, deletes expired
- **Memory pressure**: No special handling in MVP (active expiration prevents unbounded growth)

**Acceptance Criteria**:
- Keys expire within 1 second of TTL
- Expired keys return nil on GET
- Active expiration runs without blocking shard operations
- TTL persists across restarts (via snapshots)

---

### FR-5: Persistence and Recovery

**Requirement**: RsDragonflyDB MUST support snapshot-based persistence for crash recovery.

**Snapshot Strategy**:
- **Format**: Custom binary format (not RDB-compatible)
- **Frequency**: Every 60 seconds (configurable)
- **Trigger**: Background task per shard
- **Atomicity**: Per-shard snapshot is atomic (copy-on-write semantics)

**Snapshot Contents**:
- All keys, values, TTLs in shard
- Shard metadata (shard ID, timestamp)
- Checksum for corruption detection

**Recovery**:
- On startup, load latest snapshot for each shard
- Discard keys with expired TTLs during load
- If snapshot missing or corrupt, start with empty shard

**Durability Guarantees**:
- **NOT** durable writes (no fsync per command)
- Data loss window: up to 60 seconds
- Suitable for cache workloads, not transactional systems

**Acceptance Criteria**:
- Restart recovers all non-expired keys from last snapshot
- Corrupt snapshots are detected and rejected
- Snapshot write does not block shard operations

---

### FR-6: Concurrency and Threading

**Requirement**: RsDragonflyDB MUST use a per-shard, thread-per-core architecture with no shared mutable state.

**Threading Model**:
- **Shard threads**: One OS thread per shard, pinned to CPU core (optional)
- **Frontend threads**: Configurable pool (default: 4) for connection handling
- **Persistence threads**: One background thread per shard for snapshots

**Concurrency Rules**:
- Shard threads MUST NOT share mutable data structures
- Cross-shard communication via message passing (channels)
- No global locks on command execution path

**Acceptance Criteria**:
- ThreadSanitizer reports zero data races
- Shard threads never block on each other for single-key commands
- CPU utilization scales linearly with shard count

---

## Non-Functional Requirements

### NFR-1: Performance

**Throughput**:
- Target: 1M QPS on 64-core machine with 64 shards
- Measurement: redis-benchmark with 50 concurrent clients

**Latency**:
- p50: < 0.5ms
- p99: < 1ms
- p99.9: < 5ms
- Measurement: Single-key GET/SET operations

**Scalability**:
- Throughput MUST scale linearly with shard count up to physical core count
- Single-shard latency MUST remain constant regardless of total shard count

**Acceptance Criteria**:
- Benchmark results meet targets on reference hardware (64-core AMD EPYC)
- Latency does not degrade under sustained load

---

### NFR-2: Memory Efficiency

**Overhead**:
- Target: < 50 bytes per key-value pair (excluding value size)
- Includes: Key storage, value pointer, TTL, metadata

**Fragmentation**:
- Use jemalloc allocator (default on Linux)
- Monitor fragmentation ratio (allocated / resident)
- Target: < 1.2x fragmentation under steady-state workload

**Acceptance Criteria**:
- Memory profiling confirms overhead target
- Long-running tests show stable memory usage

---

### NFR-3: Reliability

**Crash Recovery**:
- MUST recover from unclean shutdown (SIGKILL)
- MUST detect and reject corrupt snapshots
- MUST start with empty state if no valid snapshot

**Error Handling**:
- MUST NOT panic on invalid client input
- MUST log errors with context (shard ID, key, operation)
- MUST continue serving requests after non-fatal errors

**Acceptance Criteria**:
- Chaos testing (random SIGKILL) results in successful recovery
- Fuzzing protocol parser finds no panics
- Error injection tests verify graceful degradation

---

### NFR-4: Observability

**Metrics**:
- Per-shard: QPS, latency histogram, memory usage, key count
- Global: Connection count, total QPS, snapshot lag
- Exposure: Prometheus-compatible endpoint on `/metrics`

**Logging**:
- Structured JSON logs
- Levels: ERROR, WARN, INFO, DEBUG
- Context: shard_id, client_addr, command, latency_us

**Health Checks**:
- `/health` endpoint returns 200 if all shards responsive
- `/ready` endpoint returns 200 if snapshots loaded

**Acceptance Criteria**:
- Metrics scrape succeeds in Prometheus
- Logs parse correctly in ELK stack
- Health checks integrate with Kubernetes probes

---

### NFR-5: Operability

**Configuration**:
- Command-line flags for: port, shard count, snapshot interval, log level, max connections, CPU pinning
- Environment variables override flags (e.g., `RSDRAGONFLY_MAX_CONNECTIONS`)
- No runtime reconfiguration (restart required)

**Full CLI Reference**:
```
--port <port>                   Listen port (default: 6379)
--bind <addr>                   Bind address (default: 0.0.0.0)
--snapshot-dir <path>           Directory for snapshot files
--snapshot-interval <secs>      Snapshot interval in seconds (default: 60)
--log-level <level>             Log level: error|warn|info|debug|trace (default: info)
--max-connections <n>           Max concurrent client connections (default: 10000)
--frontend-threads <n>          Number of connection handler threads (default: 4)
--metrics-port <port>           Prometheus metrics endpoint port (default: 9090)
--cpu-pinning                   Enable CPU pinning for shard threads (Linux only)
```

> **Shard count is NOT a runtime flag.** It is a compile-time constant (`SHARD_COUNT`). To change it, rebuild the binary. See [architecture/overview.md](architecture/overview.md#compile-time-configuration).

**Graceful Shutdown**:
- SIGTERM triggers graceful shutdown
- Drain in-flight requests (timeout: 10s)
- Write final snapshot for each shard
- Close connections cleanly

**Resource Limits**:
- MUST respect memory limits (OOM kills are acceptable)
- MUST NOT exceed file descriptor limits
- MUST handle EMFILE gracefully (reject new connections)

**Acceptance Criteria**:
- Graceful shutdown completes within 15s
- Configuration validation rejects invalid values
- Resource exhaustion does not crash server

---

## Constraints

### Technical Constraints
- **Language**: Rust 1.75+, edition 2021
- **Platform**: Linux x86_64 (primary), macOS (development only)
- **Dependencies**: Minimize external crates, prefer std library
- **Async**: No async runtime in shard threads (sync-only for predictability)

### Architectural Constraints
- **No shared mutable state**: Enforced by Rust ownership system
- **No global locks**: On command execution path
- **Fixed sharding**: No dynamic resharding in MVP

### Operational Constraints
- **Deployment**: Single-node only (no clustering)
- **Persistence**: Snapshot-only (no AOF)
- **Replication**: Not supported in MVP

---

## Out of Scope (MVP)

The following are explicitly NOT required for MVP:

1. **Redis Compatibility**:
   - Full command set (only subset above)
   - RDB/AOF format compatibility
   - Redis Cluster protocol
   - Pub/Sub, Streams, Sorted Sets

2. **Advanced Features**:
   - Transactions (MULTI/EXEC)
   - Lua scripting
   - Modules/plugins
   - TLS encryption

3. **High Availability**:
   - Replication
   - Automatic failover
   - Sentinel integration

4. **Operational Features**:
   - Online resharding
   - Hot configuration reload
   - Fine-grained ACLs
   - Audit logging

5. **Performance Optimizations**:
   - NUMA-aware allocation
   - Huge pages
   - DPDK networking

See [roadmap.md](roadmap.md) for post-MVP features.

---

## Definition of Done

The MVP is complete when:

1. All functional requirements (FR-1 through FR-6) are implemented and tested
2. All non-functional requirements (NFR-1 through NFR-5) are met and verified
3. Integration tests pass with redis-cli
4. Benchmark targets are achieved on reference hardware
5. Documentation is complete and reviewed
6. Docker image builds and runs successfully
7. Kubernetes deployment manifests are validated
8. Runbooks cover common operational scenarios

---

## Acceptance Testing

### Test Scenarios

**TS-1: Basic Operations**
```
SET key1 "value1"
GET key1 -> "value1"
DEL key1 -> 1
GET key1 -> nil
```

**TS-2: TTL Expiration**
```
SET key2 "value2" EX 2
GET key2 -> "value2"
[wait 3 seconds]
GET key2 -> nil
```

**TS-3: Multi-Key Delete**
```
SET key3 "v3"
SET key4 "v4"
DEL key3 key4 -> 2
```

**TS-4: Crash Recovery**
```
SET key5 "persistent"
[kill -9 server]
[restart server]
GET key5 -> "persistent"
```

**TS-5: Concurrent Load**
```
[50 clients, 10k requests each]
[verify: no errors, p99 < 1ms]
```

---

## Revision History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0 | 2026-01-24 | Initial | MVP requirements baseline |
