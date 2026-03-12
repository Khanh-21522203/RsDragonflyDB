# Implementation Order

## Purpose

This document defines the order in which to implement the plans, and what to verify after each step before moving on. Follow it sequentially — each step builds on the previous one.

**Golden rule**: at the end of every step, `cargo test --workspace` must pass with zero failures.

---

## Step 0 — Project Setup

**Plan**: `plan-project-setup.md`

**What to do**:
1. Create the workspace directory structure
2. Write all `Cargo.toml` files
3. Create empty `lib.rs` / `main.rs` stubs with `todo!()` bodies

**Verify**:
```bash
cargo check --workspace   # no errors (warnings OK)
cargo build               # binary produced at target/debug/rsdragonfly
```

**Done when**: `cargo check` reports zero errors.

---

## Step 1 — Common Types

**Plan**: `plan-common-types.md`

**What to do**:
Implement `crates/common/src/`:
- `types.rs` — `Key`, `Value`, `Entry`, `ShardId`, `Expiry` (with `Ord` impl for min-heap)
- `config.rs` — `SHARD_COUNT` constant (feature-gated), limit constants, `Config` struct
- `error.rs` — `Error` enum with `thiserror` derives
- `command.rs` — `Operation`, `Command`, `Response`, `SnapshotRequest`
- `metrics.rs` — `Histogram` (cumulative buckets), `HistogramSnapshot`, `ShardMetrics`
- `routing.rs` — `crc16()` and `route_key()`

**Verify**:
```bash
cargo test -p rsdragonfly-common
```

Expected passing tests:
- `test_value_as_bytes`
- `test_expiry_min_heap_order`
- `test_shard_count_is_power_of_two`
- `test_crc16_test_vector` → `crc16(b"123456789") == 0x31C3`
- `test_route_key_deterministic`
- `test_route_key_in_range`
- `test_histogram_record_cumulative` — verify that recording 50μs increments ALL 5 buckets (le=100, 500, 1000, 5000, +Inf)
- `test_error_display`

**Common mistakes**:
- Forgetting to implement `PartialOrd`, `PartialEq`, and `Eq` for `Expiry` (required by `BinaryHeap`)
- Making `Histogram` non-cumulative — see `plan-common-types.md` §5 for the correct `record()` loop
- Missing `#[derive(Clone)]` on `Value` and `Entry` (needed by engine)

---

## Step 2 — RESP2 Protocol

**Plan**: `plan-resp2-protocol.md`

**What to do**:
Implement `crates/protocol/src/`:
- `parser.rs` — `Resp2Parser`, `ParsedCommand`, `ParseResult`, `Resp2Parser::next_command()`
- `encoder.rs` — `Resp2Encoder::encode()`
- `lib.rs` — `command_to_operation()`

**Verify**:
```bash
cargo test -p rsdragonfly-protocol
```

Expected passing tests (write these alongside implementation):
- `test_parse_ping`
- `test_parse_set_with_ttl`
- `test_parse_del_multi_key`
- `test_parse_incomplete_frame`
- `test_parse_pipelining`
- `test_parse_unknown_command`
- `test_parse_wrong_arg_count`
- `test_encode_bulk_string`
- `test_encode_null_bulk_string`
- `test_encode_integer`
- `test_encode_ok`
- `test_encode_error`
- `test_encode_pong_no_message`

**Manual verification** (before moving on):
```bash
# In one terminal:
cargo run -- --port 6399 --snapshot-dir /tmp/rsd-test

# In another terminal (if redis-cli is installed):
redis-cli -p 6399 PING
# Expected: PONG
```
*Note*: this won't work yet — the server isn't wired. Skip for now; return here in Step 7.

**Common mistakes**:
- Off-by-two error: after reading `$N\r\n`, cursor must advance by `N + 2` (data bytes + `\r\n`), not `N`
- Not upcasing the verb: `GET` and `get` should both work → `args[0].to_ascii_uppercase()`
- Forgetting to handle `PING` with an optional message argument (0 or 1 args, not exactly 1)

---

## Step 3 — Shard Engine + TTL Expiration

**Plans**: `plan-shard-engine.md`, `plan-ttl-expiration.md`

These two plans are implemented together because TTL is part of `Shard` — same file, same struct.

**What to do**:
Implement `crates/engine/src/shard.rs`:
- `Shard::new()`
- `Shard::execute()` — dispatches to individual command methods
- `Shard::ping()`, `Shard::get()`, `Shard::set()`, `Shard::del()`, `Shard::expire()`, `Shard::ttl()`
- `Shard::expire_keys()` — active TTL sweep
- `shard_worker_thread()` — the event loop (without snapshot trigger for now; add in Step 4)

**Verify**:
```bash
cargo test -p rsdragonfly-engine
```

Expected passing tests:
- `test_set_get`
- `test_get_missing_key`
- `test_set_overwrite`
- `test_del_existing`
- `test_del_missing`
- `test_del_multi_key`
- `test_set_with_ttl_not_yet_expired`
- `test_set_with_ttl_expired_lazy`
- `test_expire_command`
- `test_expire_nonexistent`
- `test_ttl_no_expiry`
- `test_ttl_missing`
- `test_ttl_with_expiry`
- `test_active_expiry_removes_key`
- `test_active_expiry_stale_queue_entry`
- `test_active_expiry_max_20_entries`
- `test_key_count_limit`
- `test_shard_worker_thread` — spawn thread, send GET, receive response
- `test_shard_thread_shutdown` — drop sender, verify thread exits

**Common mistakes**:
- In `get()`, forgetting to remove the key from the HashMap on lazy expiry (returning `None` is correct but the key lingers)
- In `expire_keys()`, not validating the stale queue entry — `entry.expiry == Some(expiry.expiry_time)` check is required
- In `set()`, forgetting to update `memory_bytes` metric for overwritten keys (subtract old, add new)
- In `ttl()`, returning `Integer(0)` for already-expired-but-not-yet-cleaned keys is correct — do not remove them here; that's `get()`'s job

---

## Step 4 — Persistence

**Plan**: `plan-persistence.md`

**What to do**:
Implement `crates/persistence/src/`:
- `serializer.rs` — `serialize_shard()` → `Vec<u8>` (header + entries + CRC64 checksums)
- `loader.rs` — `load_snapshot()`, `cleanup_temp_files()`
- `writer.rs` — `snapshot_writer_thread()`
- `lib.rs` — re-export all three

Also wire snapshot trigger into `shard_worker_thread()` (add the `snapshot_tx` channel and `trigger_snapshot()` call).

**Verify**:
```bash
cargo test -p rsdragonfly-persistence
```

Expected passing tests:
- `test_serialize_deserialize_roundtrip`
- `test_serialize_expired_keys_skipped`
- `test_checksum_corruption_detected`
- `test_header_checksum_corruption_detected`
- `test_wrong_magic_rejected`
- `test_file_too_small_rejected`
- `test_atomic_rename` — write `.tmp`, verify old `.snap` unchanged on simulated failure
- `test_cleanup_temp_files`
- `test_expiry_unix_roundtrip` — `Instant` → Unix secs → `Instant`, assert < 1s error

**Common mistakes**:
- Computing `header_checksum` over the wrong range — it must cover bytes `[0..36]` (everything before offset 36, which is where the `header_checksum` field itself lives)
- Computing `data_checksum` over `buf[0..]` instead of `buf[64..]` — entries start at byte 64
- Writing checksums at the wrong offset — use `write_u64_at(&mut buf, 28, data_crc)` and `write_u64_at(&mut buf, 36, header_crc)`
- Using `Instant::now().elapsed()` for the expiry timestamp (this gives ~0 seconds, not the Unix time)

  The correct conversion:
  ```rust
  fn expiry_to_unix_secs(expiry: Instant) -> u64 {
      let remaining = expiry.saturating_duration_since(Instant::now());
      let now_unix = SystemTime::now()
          .duration_since(UNIX_EPOCH)
          .unwrap()
          .as_secs();
      now_unix + remaining.as_secs()
  }
  ```

---

## Step 5 — Frontend

**Plan**: `plan-frontend.md`

**What to do**:
Implement `crates/frontend/src/`:
- `connection.rs` — `Connection` struct wrapping `mio::net::TcpStream` + `Resp2Parser` + write buffer
- `handler.rs` — `ConnectionHandler` using `mio::Poll` event loop
- `acceptor.rs` — `Acceptor` using `mio::net::TcpListener`
- `lib.rs` — `start_frontend()` that spawns all frontend threads

**Verify**:
```bash
cargo test -p rsdragonfly-frontend
```

Expected passing tests:
- `test_route_single_key_command`
- `test_scatter_gather_del_two_shards`
- `test_scatter_gather_del_same_shard`
- `test_info_aggregation_server_section`
- `test_backpressure_rejects_command`

The big end-to-end verification happens in Step 7.

**Common mistakes**:
- Calling `reply_rx.recv()` (blocking forever) instead of `reply_rx.recv_timeout(5s)` — this blocks the entire handler thread if a shard is stuck
- Forgetting to re-register a `TcpStream` with mio after reading — with edge-triggered events you must drain until `WouldBlock`
- Not resetting the `mio::Token` counter, causing token collisions when connections close and reopen

---

## Step 6 — Observability

**Plan**: `plan-observability.md`

**What to do**:
Implement `crates/observability/src/`:
- `lib.rs` — `init_logging()` (call `env_logger::Builder::from_env(...)`)
- `exporter.rs` — `MetricsExporter`, `aggregate()`, `check_health()`, `check_ready()`
- `format.rs` — `format_prometheus_metrics()` (plain string building, no external crate needed)

**Verify**:
```bash
cargo test -p rsdragonfly-observability
```

Expected passing tests:
- `test_aggregate_sums_all_shards`
- `test_prometheus_format_counter`
- `test_prometheus_format_histogram`
- `test_health_all_alive`
- `test_ready_not_ready`

**Common mistakes**:
- Prometheus histogram format requires `_bucket`, `_sum`, and `_count` suffixes on the same metric name — the base name is `rsdragonfly_shard_latency_us`
- The `le="+Inf"` label must be literal string `"+Inf"` (not a number) — Prometheus requires it

---

## Step 7 — Server Wiring

**Plan**: `plan-server-wiring.md`

**What to do**:
Implement `crates/server/src/`:
- `main.rs` — full startup sequence: parse config, init logging, create channels, load snapshots, spawn all threads
- `shutdown.rs` — `graceful_shutdown()` + SIGTERM registration

**Verify** (manual, no automated test yet):
```bash
# Start the server
RUST_LOG=info cargo run -- --port 6399 --snapshot-dir /tmp/rsd-test

# In another terminal — test all commands:
redis-cli -p 6399 PING
# → PONG

redis-cli -p 6399 SET hello world
# → OK

redis-cli -p 6399 GET hello
# → "world"

redis-cli -p 6399 SET expkey value EX 3
# → OK

redis-cli -p 6399 TTL expkey
# → 3 (or 2)

sleep 4
redis-cli -p 6399 GET expkey
# → (nil)

redis-cli -p 6399 DEL hello
# → (integer) 1

redis-cli -p 6399 INFO server
# → multiline output with version, port, shard_count

curl http://localhost:9090/metrics
# → Prometheus text format

curl http://localhost:9090/health
# → OK

curl http://localhost:9090/ready
# → OK
```

**Verify shutdown**:
```bash
# Start server in background
cargo run -- --port 6399 --snapshot-dir /tmp/rsd-test &
SERVER_PID=$!

redis-cli -p 6399 SET persistent_key my_value
sleep 65    # wait for periodic snapshot

kill -TERM $SERVER_PID    # graceful shutdown
wait $SERVER_PID

# Restart — key should survive
cargo run -- --port 6399 --snapshot-dir /tmp/rsd-test &
sleep 2
redis-cli -p 6399 GET persistent_key
# → "my_value"
kill -TERM $!
```

---

## Step 8 — Integration Tests

**Plan**: `plan-integration-testing.md`

**What to do**:
- Create `crates/server/tests/` directory
- Write integration tests using the `TestServer` harness

**Verify**:
```bash
cargo test --workspace
# All unit tests + integration tests must pass
```

---

## Step 9 — Performance Validation

After all tests pass, validate against the NFR targets from `docs/requirements.md`:

```bash
# Install redis-benchmark (ships with redis-tools package)
sudo apt install redis-tools

# Start release build
cargo build --release
./target/release/rsdragonfly --port 6399 --snapshot-dir /tmp/rsd-bench

# Benchmark: 50 clients, 1M total requests, GET/SET mix
redis-benchmark -p 6399 -c 50 -n 1000000 -t get,set -q

# Targets:
# GET: >100,000 requests/sec (on dev machine; 1M QPS on 64-core production)
# SET: >100,000 requests/sec
# p99 latency: < 1ms
```

---

## Dependency Graph (Visual)

```
common
  ├── protocol
  ├── engine
  │     └── persistence
  ├── persistence
  ├── frontend
  │     └── protocol
  ├── observability
  └── server
        ├── engine
        ├── persistence
        ├── frontend
        └── observability
```

Build order follows the dependency graph bottom-up:
`common` → `protocol` + `persistence` → `engine` → `frontend` + `observability` → `server`

---

## Time Estimates (for planning only)

| Step | Crate | Rough effort |
|------|-------|-------------|
| 0 | Setup | 1–2 hours |
| 1 | common | 3–4 hours |
| 2 | protocol | 4–6 hours |
| 3 | engine + TTL | 6–8 hours |
| 4 | persistence | 4–6 hours |
| 5 | frontend | 8–12 hours (mio is the hardest part) |
| 6 | observability | 3–4 hours |
| 7 | server wiring | 4–6 hours |
| 8 | integration tests | 3–4 hours |
| 9 | perf validation | 1–2 hours |

The frontend (Step 5) is the hardest step for a junior. If blocked, read `mio` examples in the crate's repository before continuing.
