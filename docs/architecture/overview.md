# Architecture Overview

## Document Purpose
This document provides a high-level architectural overview of RsDragonflyDB, explaining the system's structure, component interactions, and design philosophy.

**Audience**: Senior engineers, architects, new contributors

---

## Design Philosophy

### Core Principle: Shard-Per-Thread Architecture

RsDragonflyDB is built on a single, non-negotiable architectural principle:

**Each shard is owned by exactly one thread. No shared mutable state exists between shards.**

This principle drives every design decision:
- **Ownership**: Rust's ownership system enforces shard isolation at compile time
- **Concurrency**: No locks on hot paths; shards operate independently
- **Scalability**: Linear performance scaling with CPU cores
- **Simplicity**: Clear boundaries eliminate entire classes of bugs

### Why This Matters

Traditional Redis uses a single-threaded event loop. Multi-threaded forks introduce locks and contention. RsDragonflyDB eliminates contention by partitioning data and computation:

```
Traditional Multi-Threaded DB:
┌─────────────────────────────┐
│   Shared Hash Table (Mutex) │  ← Contention bottleneck
└─────────────────────────────┘
   ↑      ↑      ↑      ↑
Thread1 Thread2 Thread3 Thread4

RsDragonflyDB:
┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐
│ Shard0 │ │ Shard1 │ │ Shard2 │ │ Shard3 │  ← No sharing
└────────┘ └────────┘ └────────┘ └────────┘
    ↑          ↑          ↑          ↑
 Thread0    Thread1    Thread2    Thread3
```

---

## System Architecture

### High-Level Component Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                         Clients                              │
│                    (redis-cli, apps)                         │
└────────────────────────┬────────────────────────────────────┘
                         │ TCP (RESP2)
                         ↓
┌─────────────────────────────────────────────────────────────┐
│                    Frontend Layer                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │  Acceptor    │  │  Connection  │  │   Router     │      │
│  │   Thread     │→ │   Handlers   │→ │  (Hash Key)  │      │
│  └──────────────┘  └──────────────┘  └──────────────┘      │
└────────────────────────┬────────────────────────────────────┘
                         │ Message Passing (Channels)
                         ↓
┌─────────────────────────────────────────────────────────────┐
│                     Shard Layer                              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │
│  │ Shard 0  │  │ Shard 1  │  │ Shard 2  │  │ Shard N  │   │
│  │ Thread 0 │  │ Thread 1 │  │ Thread 2 │  │ Thread N │   │
│  │          │  │          │  │          │  │          │   │
│  │ HashMap  │  │ HashMap  │  │ HashMap  │  │ HashMap  │   │
│  │ TTL Heap │  │ TTL Heap │  │ TTL Heap │  │ TTL Heap │   │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘   │
└───────┼─────────────┼─────────────┼─────────────┼──────────┘
        │             │             │             │
        ↓             ↓             ↓             ↓
┌─────────────────────────────────────────────────────────────┐
│                  Persistence Layer                           │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐   │
│  │Snapshot  │  │Snapshot  │  │Snapshot  │  │Snapshot  │   │
│  │Writer 0  │  │Writer 1  │  │Writer 2  │  │Writer N  │   │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘   │
└───────┼─────────────┼─────────────┼─────────────┼──────────┘
        ↓             ↓             ↓             ↓
    [shard0.snap] [shard1.snap] [shard2.snap] [shardN.snap]
```

---

## Component Responsibilities

### Frontend Layer

**Purpose**: Accept client connections, parse RESP2 protocol, route commands to shards.

**Components**:

1. **Acceptor Thread**
   - Listens on TCP port (default: 6379)
   - Accepts new connections
   - Spawns connection handler or assigns to thread pool

2. **Connection Handlers** (Thread Pool, default: 4 threads)
   - Parse RESP2 protocol from client socket
   - Validate commands and arguments
   - Determine target shard(s) for each key
   - Send command to shard(s) via channel
   - Await response(s) and serialize RESP2 reply
   - Handle pipelining (multiple commands per read)

3. **Router**
   - Pure function: `route(key: &[u8]) -> ShardId`
   - Implementation: `CRC16(key) % SHARD_COUNT`
   - No state, no locks

**Key Design Decisions**:
- Frontend threads do NOT access shard data directly
- All shard interaction is via message passing
- Connection handlers are stateless (connection state in socket buffer)
- Pipelining is supported (batch multiple commands before awaiting responses)

---

### Shard Layer

**Purpose**: Own and manage a disjoint subset of the keyspace.

**Per-Shard Components**:

1. **Event Loop** (Sync, not async)
   - Receives commands from frontend via channel
   - Executes command on local data structures
   - Sends response back via channel
   - Runs TTL expiration background task
   - Triggers snapshot writes

2. **Data Structures**
   - `HashMap<Key, Value>`: Primary key-value store
   - `BinaryHeap<(Expiry, Key)>`: TTL expiration queue
   - `Metrics`: Per-shard counters (QPS, latency, key count)

3. **Ownership Rules**
   - Shard thread has exclusive mutable access to its HashMap
   - No other thread may read or write shard data
   - Enforced by Rust: HashMap is not `Send` across threads

**Key Design Decisions**:
- Synchronous execution (no async runtime overhead)
- Single-threaded per shard (no intra-shard locking)
- Copy-on-write for snapshots (via message to persistence thread)

---

### Persistence Layer

**Purpose**: Write periodic snapshots of shard state to disk for crash recovery.

**Per-Shard Components**:

1. **Snapshot Writer Thread**
   - Receives snapshot request from shard thread
   - Receives serialized snapshot data (owned, no shared references)
   - Writes to temporary file
   - Atomically renames to final snapshot file
   - Computes checksum

2. **Snapshot Reader** (Startup Only)
   - Reads snapshot file for each shard
   - Validates checksum
   - Deserializes into HashMap
   - Filters expired keys
   - Sends data to shard thread for initialization

**Key Design Decisions**:
- Snapshot is per-shard (no global snapshot coordination)
- Shard thread serializes data and sends owned copy to writer (no blocking IO)
- Writer thread does blocking IO (does not impact shard latency)
- Snapshot format is custom (not RDB-compatible)

---

## Data Flow

### Single-Key Command (GET)

```
1. Client sends: GET mykey\r\n

2. Frontend:
   - Parse command: GET, key="mykey"
   - Compute shard: CRC16("mykey") % 64 = 42
   - Send message to Shard 42: GetCommand { key: "mykey", reply_tx }

3. Shard 42:
   - Receive message from channel
   - Lookup key in HashMap
   - Check TTL (lazy expiration)
   - Send response: Some("value") or None

4. Frontend:
   - Receive response from Shard 42
   - Serialize RESP2: $5\r\nvalue\r\n or $-1\r\n
   - Write to client socket

5. Client receives: $5\r\nvalue\r\n
```

**Latency Breakdown**:
- Frontend parse: ~1μs
- Channel send: ~0.1μs
- Shard lookup: ~0.1μs (HashMap)
- Channel receive: ~0.1μs
- Frontend serialize: ~1μs
- **Total: ~2.3μs** (excluding network)

---

### Multi-Key Command (DEL key1 key2 key3)

```
1. Client sends: DEL key1 key2 key3\r\n

2. Frontend:
   - Parse command: DEL, keys=["key1", "key2", "key3"]
   - Compute shards:
     - key1 → Shard 10
     - key2 → Shard 10 (same shard)
     - key3 → Shard 25 (different shard)
   - Group by shard:
     - Shard 10: [key1, key2]
     - Shard 25: [key3]
   - Send messages:
     - Shard 10: DelCommand { keys: [key1, key2], reply_tx }
     - Shard 25: DelCommand { keys: [key3], reply_tx }

3. Shard 10 & 25 (in parallel):
   - Receive message
   - Delete keys from HashMap
   - Count deleted keys
   - Send response: DeletedCount(2) and DeletedCount(1)

4. Frontend:
   - Await both responses
   - Aggregate: 2 + 1 = 3
   - Serialize RESP2: :3\r\n
   - Write to client socket

5. Client receives: :3\r\n
```

**Key Points**:
- No cross-shard coordination
- Shards execute independently
- Partial failures possible (one shard fails, other succeeds)
- No atomicity guarantee across shards

---

## Concurrency Model

### Thread Types

| Thread Type | Count | Purpose | Blocking? |
|-------------|-------|---------|-----------|
| Acceptor | 1 | Accept connections | Yes (accept syscall) |
| Connection Handler | 4 (configurable) | Parse RESP2, route commands | No (non-blocking sockets) |
| Shard Worker | 64 (= shard count) | Execute commands, manage data | No |
| Snapshot Writer | 64 (= shard count) | Write snapshots to disk | Yes (write syscall) |
| Metrics Exporter | 1 | Aggregate metrics, serve /metrics | Yes (HTTP accept/write) |
| Watchdog | 1 | Detect stuck/stalled shards | No (sleeps 10s) |

**Total Threads**: 1 + 4 + 64 + 64 + 1 + 1 = 135 threads (for 64 shards)

### Communication Primitives

**Frontend → Shard**:
- Type: `crossbeam::channel::unbounded` (MPSC)
- Message: `Command { op: Operation, reply_tx: crossbeam::channel::Sender<Response> }`
- Backpressure: None (unbounded queue)

**Shard → Frontend**:
- Type: `crossbeam::channel::bounded(1)` (one-shot response per command)
- Message: `Response { result: Result<Value, Error> }`
- Rationale: bounded(1) is sync, matches the connection handler's sync epoll loop; no async runtime dependency needed

**Shard → Snapshot Writer**:
- Type: `crossbeam::channel::bounded(1)` (SPSC)
- Message: `SnapshotData { shard_id: u16, data: Vec<u8> }`
- Backpressure: If writer is slow, shard skips snapshot

### Synchronization Rules

**Allowed**:
- Message passing via channels
- Atomic counters for metrics (e.g., `AtomicU64` for QPS)
- Immutable shared data (e.g., configuration)

**Forbidden**:
- `Mutex<HashMap>` or any shared mutable data structure on the command hot path
- `RwLock` on hot path
- Shared references to shard data

**Allowed Exception**:
- `Mutex<Histogram>` for latency histograms — the metrics exporter thread reads histograms from outside the shard thread. The Mutex is held briefly (only during metrics scrape, not during command execution). This is acceptable because metric scrapes are infrequent (~15s intervals) and do not block the shard's command loop.

**Enforcement**:
- Rust's type system prevents accidental sharing
- Code review enforces architectural rules
- Integration tests verify no lock contention (via profiling)

---

## Error Handling Strategy

### Error Categories

1. **Client Errors** (user's fault)
   - Invalid command syntax
   - Wrong argument count
   - Type mismatch
   - **Handling**: Return RESP2 error, keep connection open

2. **Transient Errors** (retry-able)
   - Shard channel full (backpressure)
   - Temporary IO error
   - **Handling**: Return error, log warning, continue serving

3. **Fatal Errors** (unrecoverable)
   - Shard thread panic
   - Snapshot corruption on startup
   - Out of memory
   - **Handling**: Log error, initiate graceful shutdown

### Error Propagation

```
Client Error:
  Frontend → RESP2 error → Client
  (no shard involvement)

Shard Error:
  Shard → Response::Err → Frontend → RESP2 error → Client

Fatal Error:
  Any thread → log::error! → Shutdown signal → Graceful shutdown
```

### Panic Handling

- Shard thread panic: Catch with `std::panic::catch_unwind`, log, mark shard as failed
- Frontend thread panic: Catch, log, close connection, continue serving
- Acceptor thread panic: Fatal, initiate shutdown

---

## Configuration

### Compile-Time Configuration

```rust
// crates/common/src/config.rs — selected via Cargo feature flag
#[cfg(feature = "shard_count_16")]  pub const SHARD_COUNT: usize = 16;
#[cfg(feature = "shard_count_32")]  pub const SHARD_COUNT: usize = 32;
#[cfg(feature = "shard_count_64")]  pub const SHARD_COUNT: usize = 64;  // default
#[cfg(feature = "shard_count_128")] pub const SHARD_COUNT: usize = 128;
```

To build with a different shard count:
```bash
cargo build --release --features shard_count_128
```

> Changing `SHARD_COUNT` after data has been written requires deleting existing snapshots because key-to-shard routing changes.

**Rationale**: Only `SHARD_COUNT` is compile-time. It enables const-generic optimizations (fixed-size arrays, bitwise modulo). All other values (`frontend_threads`, `snapshot_interval`, etc.) are runtime-configurable via CLI flags.

| Parameter | Compile-time? | Default | Reason |
|-----------|--------------|---------|--------|
| `SHARD_COUNT` | Yes | 64 | Enables const-generic array sizing |
| `frontend_threads` | No | 4 | Can tune without rebuild |
| `snapshot_interval` | No | 60s | Can tune without rebuild |
| `max_connections` | No | 10000 | Can tune without rebuild |

### Runtime Configuration

```bash
rsdragonfly \
  --port 6379 \
  --bind 0.0.0.0 \
  --snapshot-dir /var/lib/rsdragonfly \
  --snapshot-interval 60 \
  --log-level info \
  --max-connections 10000 \
  --frontend-threads 4 \
  --metrics-port 9090 \
  --cpu-pinning
```

> `--cpu-pinning` is a boolean flag (no value required). Omit it to disable CPU pinning.

**Environment Variables** (override CLI flags):
- `RSDRAGONFLY_PORT`
- `RSDRAGONFLY_BIND`
- `RSDRAGONFLY_SNAPSHOT_DIR`
- `RSDRAGONFLY_SNAPSHOT_INTERVAL`
- `RSDRAGONFLY_LOG_LEVEL`
- `RSDRAGONFLY_MAX_CONNECTIONS`
- `RSDRAGONFLY_FRONTEND_THREADS`
- `RSDRAGONFLY_METRICS_PORT`

---

## Performance Characteristics

### Scalability

**Single-Shard Throughput**: ~15K QPS per shard (GET/SET mix)
**Total Throughput**: 15K × 64 = ~960K QPS (on 64-core machine)

**Scaling Law**:
```
Throughput = SHARD_COUNT × Single_Shard_QPS
(up to physical core count)
```

**Bottlenecks**:
- Frontend parsing (mitigated by pipelining)
- Network bandwidth (not CPU)
- Memory bandwidth (for large values)

### Latency

**Single-Key Operation**:
- p50: 0.3ms (mostly network)
- p99: 0.8ms
- p99.9: 2ms (GC pauses, context switches)

**Multi-Key Operation**:
- Latency = max(latency of slowest shard)
- p99: 1ms (for 2-3 shards)

**Tail Latency Mitigation**:
- No GC (Rust)
- No locks (shard-per-thread)
- CPU pinning (reduces context switches)

---

## Trade-Offs and Limitations

### Trade-Off 1: Fixed Sharding vs Dynamic Resharding

**Decision**: Fixed shard count at startup.

**Pros**:
- Simple implementation
- Predictable performance
- No resharding downtime

**Cons**:
- Cannot scale beyond initial shard count
- Uneven load if key distribution is skewed

**Mitigation**:
- Choose shard count = 2× expected core count
- Use good hash function (CRC16) for uniform distribution

---

### Trade-Off 2: No Cross-Shard Atomicity

**Decision**: Multi-key commands are not atomic across shards.

**Pros**:
- No distributed transactions
- No cross-shard coordination
- Predictable latency

**Cons**:
- Partial failures possible
- Not suitable for transactional workloads

**Mitigation**:
- Document behavior clearly
- Recommend single-shard keys for atomic operations
- Future: Add optional 2PC for critical operations

---

### Trade-Off 3: Snapshot vs AOF Persistence

**Decision**: Snapshot-only persistence.

**Pros**:
- Simple implementation
- No write amplification
- Fast recovery (single read per shard)

**Cons**:
- Data loss window (up to 60s)
- Snapshot write spikes (IO and memory)

**Mitigation**:
- Configurable snapshot interval
- Copy-on-write to avoid blocking shard
- Future: Add AOF for durability-critical workloads

---

## Security Considerations

### MVP Security Posture

**Included**:
- Input validation (prevent buffer overflows)
- Resource limits (max connections, max key size)
- Graceful handling of malformed RESP2

**Not Included** (post-MVP):
- Authentication (no AUTH command)
- TLS encryption
- ACLs (all clients have full access)
- Rate limiting per client

**Deployment Recommendation**:
- Run behind firewall or VPN
- Use network policies (Kubernetes)
- Do NOT expose directly to internet

---

## Observability Architecture

### Metrics Collection

**Per-Shard Metrics** (in shard thread):
```rust
struct ShardMetrics {
    qps: AtomicU64,
    latency_histogram: Mutex<Histogram>, // Mutex needed: metrics exporter reads from separate thread
    key_count: AtomicUsize,
    memory_bytes: AtomicUsize,
}
```

**Aggregation** (in metrics exporter thread):
- Poll each shard's atomic counters
- Aggregate into global metrics
- Expose via `/metrics` endpoint (Prometheus format)

**No Shared State**: Metrics use atomics, not mutexes.

---

## Deployment Architecture

### Single-Node Deployment

```
┌─────────────────────────────────────┐
│         Kubernetes Pod              │
│                                     │
│  ┌───────────────────────────────┐ │
│  │   rsdragonfly container       │ │
│  │   - 64 shards                 │ │
│  │   - CPU: 64 cores             │ │
│  │   - Memory: 128GB             │ │
│  └───────────────────────────────┘ │
│                                     │
│  ┌───────────────────────────────┐ │
│  │   Persistent Volume           │ │
│  │   /var/lib/rsdragonfly        │ │
│  │   - shard0.snap               │ │
│  │   - shard1.snap               │ │
│  │   - ...                       │ │
│  └───────────────────────────────┘ │
└─────────────────────────────────────┘
```

**Resource Requirements**:
- CPU: 1 core per shard + 5 cores for frontend/persistence
- Memory: Depends on dataset size + 50 bytes overhead per key
- Disk: 2× memory size (for snapshots)

---

## Future Architecture Evolution

### Post-MVP Features (Roadmap)

1. **Replication**
   - Primary-replica model
   - Per-shard replication (independent)
   - Async replication (no cross-shard coordination)

2. **Clustering**
   - Multiple nodes, each with fixed shards
   - Client-side routing (Redis Cluster protocol)
   - No automatic resharding (manual migration)

3. **Advanced Persistence**
   - AOF for durability
   - Incremental snapshots
   - Compression

4. **Operational Features**
   - Online resharding (complex, requires coordination)
   - Hot configuration reload
   - TLS and authentication

**Architectural Compatibility**:
All future features MUST respect the shard-per-thread principle. Any feature requiring cross-shard coordination must use explicit message passing, not shared state.

---

## Definition of Done

This architecture is complete when:

1. All components are defined with clear responsibilities
2. Data flow is documented for all command types
3. Concurrency model is explicit and enforceable
4. Trade-offs are documented with rationale
5. Performance characteristics are measurable
6. Security posture is understood and documented
7. Observability is built-in, not bolted-on
8. Deployment model is validated on Kubernetes

---

## References

- [sharding-model.md](sharding-model.md) - Detailed sharding design
- [threading-model.md](threading-model.md) - Thread lifecycle and communication
- [multi-key-commands.md](multi-key-commands.md) - Cross-shard operation semantics
- [data-model.md](../engine/data-model.md) - In-memory data structures
