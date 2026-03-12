# Guide to Review the Codebase

This guide is for a junior engineer who has no prior context on this project.

Goal: understand the current codebase quickly and correctly before fixing issues.

---

## 1. What This Project Is

`RsDragonflyDB` is intended to be a Redis-like in-memory key-value server in Rust.

Supported MVP commands (expected):
- `PING`
- `SET key value [EX seconds]`
- `GET key`
- `DEL key [key ...]`
- `EXPIRE key seconds`
- `TTL key`
- `INFO [section]`

The project also aims to support:
- shard-based ownership (`SHARD_COUNT` shards)
- TTL expiration (lazy + active)
- snapshot persistence
- Prometheus metrics and health/readiness endpoints

---

## 2. What You Should Read First (In Order)

Read these in this exact sequence:

1. `docs/requirements.md`
- This defines WHAT must be true.

2. `plans/plan-implementation-order.md`
- This defines expected build sequence and major milestones.

3. `code_review_issues.md`
- This is the current gap list between expected vs implemented behavior.

4. Source code, in request flow order:
- `src/main.rs`
- `src/server.rs`
- `src/frontend/handler.rs`
- `src/protocol/parser.rs`
- `src/protocol/command.rs`
- `src/frontend/router.rs`
- `src/shard/shard.rs`
- `src/shard/worker.rs`
- `src/shard/expiration.rs`
- `src/shard/snapshot/serialize.rs`
- `src/shard/snapshot/deserialize.rs`
- `src/observability/prometheus.rs`

Why this order: it matches runtime behavior from client request to storage to observability.

---

## 3. Codebase Map (Current Implementation)

### Root-level
- `Cargo.toml`: single-crate package config
- `docs/`: target requirements and architecture docs
- `plans/`: implementation plan docs
- `src/`: all runtime code (current code is monolithic, not multi-crate)

### `src/` modules
- `main.rs`: process entrypoint, logging init, server start
- `server.rs`: config parsing (`Args`), startup, worker/task spawning, shutdown

- `common/`: shared constants/types/helpers
  - `constants.rs`: shard count, limits, default intervals
  - `types.rs`: key/value types
  - `hash.rs`: CRC16
  - `channels.rs`: snapshot channel types
  - `time.rs`: instant <-> unix timestamp conversion

- `protocol/`: RESP layer + command parsing
  - `parser.rs`: raw RESP2 parser
  - `resp.rs`: RESP value enum
  - `serializer.rs`: RESP encoding
  - `command.rs`: map RESP values -> internal `Command`
  - `response.rs`: internal `Response`
  - `channels.rs`: frontend <-> shard command message channels

- `frontend/`: TCP request handling and routing
  - `handler.rs`: connection loop + command processing
  - `router.rs`: key -> shard routing
  - `multi_key.rs`: scatter-gather helper for DEL (currently not fully integrated)
  - `error_handling.rs`: multi-key result wrapper

- `shard/`: storage engine + TTL + persistence
  - `shard.rs`: main in-memory key-value operations
  - `worker.rs`: shard event loop, periodic TTL + snapshot trigger
  - `expiration.rs`: active TTL expiration logic
  - `entry.rs`: stored record structure
  - `expiry.rs`: TTL queue item ordering
  - `snapshot/`: snapshot serialize/deserialize/load
  - `persistence/`: snapshot header format and disk writer

- `observability/`: metrics/logging/health helpers
  - `metrics.rs`: global metric container
  - `prometheus.rs`: HTTP metrics endpoint
  - `health.rs`: health/readiness helper logic (currently not wired into exporter)
  - `logging.rs`: JSON log helper functions (currently not wired as main logger)

---

## 4. End-to-End Request Flow

Use this model to reason about behavior.

1. Client opens TCP connection.
- `src/frontend/handler.rs` accepts connection.

2. Bytes are read and parsed as RESP.
- `RespParser` in `src/protocol/parser.rs`

3. RESP frame converted to internal `Command`.
- `Command::from_resp` in `src/protocol/command.rs`

4. Command routed to shard(s).
- Single-key commands: `Router::route_key` (`src/frontend/router.rs`)
- Multi-key DEL: grouped by shard (`src/frontend/handler.rs`)

5. Shard executes operation.
- `Shard::execute` in `src/shard/shard.rs`

6. `Response` sent back and encoded to RESP.
- response mapping in `src/frontend/handler.rs`
- serializer in `src/protocol/serializer.rs`

7. Background operations continue.
- active expiration in `src/shard/expiration.rs`
- snapshot trigger in `src/shard/worker.rs`
- snapshot write in `src/shard/persistence/writer.rs`

---

## 5. Key Data Structures You Must Understand

### `Command` (`src/protocol/command.rs`)
Represents parsed client operation and arguments.

### `Response` (`src/protocol/response.rs`)
Represents execution result before wire encoding.

### `Shard` (`src/shard/shard.rs`)
Owns:
- `data: HashMap<Key, Entry>`
- `ttl_queue: BinaryHeap<Expiry>`
- metrics counters

### `Entry` (`src/shard/entry.rs`)
Stores:
- `value`
- optional `expiry`

### `Expiry` (`src/shard/expiry.rs`)
Used in `BinaryHeap` to pop earliest expiry first.

---

## 6. Critical Runtime Invariants

When reading or changing code, protect these invariants:

1. Single-key command touches only one owning shard.
2. Multi-key DEL may touch multiple shards; not cross-shard atomic.
3. Expired keys must not be returned from GET.
4. TTL queue may contain stale entries; expiration logic must tolerate this.
5. Snapshot format read/write must match exactly (offsets/checksum ranges).
6. Metrics should reflect actual runtime state, not stale copies.

---

## 7. Where Juniors Usually Get Confused

1. `Response::Value(Some("PONG"))` vs RESP simple string `+PONG`.
- Same text, different wire semantics.

2. INFO behavior.
- Spec requires aggregated global sections; current code has shard-local behavior.

3. Snapshot loading exists but may not be wired into startup path.
- Check call sites, not just helper existence.

4. Metrics structs can be duplicated accidentally.
- Ensure exporter and shards use same instances.

5. Async tasks vs OS thread model.
- Requirements/plans discuss thread-per-shard architecture.

---

## 8. Practical Review Method (Per Module)

For each module:
1. Open docs requirement related to it.
2. Read module top-down.
3. Write down:
- current behavior
- expected behavior
- exact delta
4. Only then change code.

Do not patch first and reason later.

---

## 9. How to Run and Debug Locally

### Build and test
```bash
cargo fmt
cargo test
```

### Run server
```bash
cargo run -- --port 6379 --snapshot-dir /tmp/rsd
```

### Basic manual checks
```bash
redis-cli -p 6379 PING
redis-cli -p 6379 SET mykey value
redis-cli -p 6379 GET mykey
redis-cli -p 6379 DEL mykey
redis-cli -p 6379 INFO
curl http://localhost:9090/metrics
```

### Persistence check
```bash
redis-cli -p 6379 SET pkey pval
# wait for snapshot interval or trigger shutdown flow
# restart server
redis-cli -p 6379 GET pkey
```

---

## 10. Suggested Learning Path Before Fixing

Do this once:

1. Trace one `GET` request end-to-end in code.
2. Trace one `SET EX` request end-to-end.
3. Trace one `DEL key1 key2` end-to-end.
4. Trace one snapshot write from shard worker to disk writer.
5. Trace one metrics scrape path.

If you can explain these five flows clearly, you are ready to fix issues.

---

## 11. Definition of “Understood the Codebase”

You should be able to answer all of these without guessing:

1. Where does routing happen?
2. Where does TTL lazy expiration happen?
3. Where does active expiration happen?
4. Where is snapshot data serialized?
5. Where is snapshot data deserialized?
6. Where are INFO fields built?
7. Where do health/readiness responses come from?
8. Where are global and per-shard metrics updated?

If any answer is unclear, go back and re-read relevant module before touching code.

---

## 12. Next File

After finishing this guide, use:
- `issues_detail.md`

That file gives issue-by-issue instructions for implementation.
