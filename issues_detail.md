# issues_detail.md

This file tells you exactly how to fix issues in `code_review_issues.md` one-by-one.

Read `guide_to_review.md` first.

## How to Use This File
For each issue:
1. Open the referenced files.
2. Apply the fix steps.
3. Run the verification command(s).
4. Move to next issue only after passing verification.

---

## Phase 1: Core Data Safety and Startup/Shutdown (Critical)

### Issue 1 - Snapshot recovery not used at startup
Target:
- Server must load each shard snapshot before shard workers start.

Where:
- `src/server.rs`
- `src/shard/snapshot/loader.rs`
- `src/shard/worker.rs` (constructor path)

Fix steps:
1. In startup flow, call snapshot loader for every shard.
2. Pass loaded shard state into worker initialization.
3. If snapshot load fails, log and start empty shard.

Verify:
```bash
cargo test
# manual: set key, wait snapshot, restart, get key
```

### Issue 2 - Snapshot checksum offset/range mismatch
Target:
- Write and validate checksums at documented offsets and ranges.

Where:
- `src/shard/snapshot/serialize.rs`
- `src/shard/snapshot/deserialize.rs`
- `src/shard/persistence/format.rs`

Fix steps:
1. Align checksum offsets with format spec.
2. Compute header checksum using the exact required byte range.
3. Add round-trip + corruption tests.

Verify:
```bash
cargo test
```

### Issue 3 - Graceful shutdown incomplete
Target:
- Handle SIGTERM and guarantee drain + final snapshot + clean exit.

Where:
- `src/server.rs`
- frontend connection lifecycle files

Fix steps:
1. Add SIGTERM/SIGINT shutdown flag handling.
2. Stop accepting new connections.
3. Drain in-flight work with timeout.
4. Trigger final snapshot per shard.
5. Wait for workers/writers to complete safely.

Verify:
```bash
cargo test
# manual: run server, write key, SIGTERM, restart, key exists
```

### Issue 4 - Metrics data source mismatch
Target:
- Exporter must read the same shard metrics objects that workers update.

Where:
- `src/server.rs`
- `src/shard/shard.rs`
- worker startup path

Fix steps:
1. Create one shared `ShardMetrics` instance per shard.
2. Inject shared metrics into shard worker/shard object.
3. Ensure exporter references those same instances.

Verify:
```bash
cargo test
# manual: run commands, confirm metrics increase
```

### Issue 6 - Threading model mismatch
Target:
- Move toward required shard-thread architecture or align behavior with planned model.

Where:
- `src/server.rs`
- `src/shard/worker.rs`
- `src/frontend/handler.rs`

Fix steps:
1. Replace accidental async-task model for shard critical path with dedicated thread model (or documented equivalent with strict ownership).
2. Ensure one shard execution context per shard.
3. Confirm no shared mutable shard data across shards.

Verify:
```bash
cargo test
```

### Issue 7 - Failing sharding distribution test
Target:
- Fix routing/hash distribution check and make test pass consistently.

Where:
- `src/common/hash.rs`
- routing tests

Fix steps:
1. Revisit distribution test method/threshold correctness.
2. Keep deterministic hash implementation.
3. Use robust variance calculation aligned with requirement.

Verify:
```bash
cargo test
```

---

## Phase 2: Protocol + Command Correctness

### Issue 5 - INFO semantics wrong
Target:
- INFO must aggregate globally and include required fields.

Where:
- `src/frontend/handler.rs`
- `src/shard/shard.rs`
- metrics aggregation logic

Fix steps:
1. Move INFO response building to aggregation layer (not single shard).
2. Support sections: server/stats/memory.
3. Ensure full INFO returns concatenated sections.

Verify:
```bash
cargo test
# manual: redis-cli INFO, INFO server, INFO stats, INFO memory
```

### Issue 8 - Protocol-format errors not connection-fatal
Target:
- True protocol errors must close connection.

Where:
- `src/frontend/handler.rs`
- `src/protocol/command.rs`

Fix steps:
1. Distinguish command errors vs protocol framing errors.
2. Close connection on protocol framing violations.
3. Keep Redis-style error response for normal command validation errors.

Verify:
```bash
cargo test
# manual: send invalid frame, ensure disconnect
```

### Issue 16 - Error messages not Redis-compatible
Target:
- Use Redis-style error strings.

Where:
- `src/frontend/handler.rs`
- command parsing files

Fix steps:
1. Map each parser/command error to explicit Redis-style message.
2. Remove debug-format output from client errors.

Verify:
```bash
cargo test
```

### Issue 17 - PING wrong wire type
Target:
- `PING` without message returns simple string `+PONG`.

Where:
- `src/shard/shard.rs`
- response mapping in `src/frontend/handler.rs`

Fix steps:
1. Represent PING response as correct response variant.
2. Ensure serializer emits simple string format.

Verify:
```bash
redis-cli -p 6379 PING
```

### Issue 18 - INFO arg count validation missing
Target:
- Reject INFO with more than one argument.

Where:
- `src/protocol/command.rs`

Fix steps:
1. Enforce arg count exactly 0 or 1.
2. Return proper wrong-arg-count error.

Verify:
```bash
cargo test
```

### Issue 34 - INFO without section incomplete
Target:
- Bare `INFO` returns server + stats + memory.

Where:
- INFO formatting logic

Fix steps:
1. Build all sections then concatenate in required order.
2. Keep section headers and CRLF formatting exact.

Verify:
```bash
redis-cli -p 6379 INFO
```

### Issue 35 - INFO section case sensitivity
Target:
- Section handling should be case-insensitive.

Where:
- `src/protocol/command.rs` and INFO dispatcher

Fix steps:
1. Normalize section value (lowercase) before match.
2. Keep output section names in canonical format.

Verify:
```bash
redis-cli -p 6379 INFO SERVER
redis-cli -p 6379 INFO server
```

### Issue 36 - RESP array length unbounded
Target:
- Add safe upper bound for array length.

Where:
- `src/protocol/parser.rs`

Fix steps:
1. Add max array element count constant.
2. Reject oversized arrays with parse error.
3. Add tests for large counts.

Verify:
```bash
cargo test
```

---

## Phase 3: Operability and Configuration

### Issue 9 - Missing CLI options
Target:
- Support required flags (snapshot interval, max connections, frontend threads, metrics port).

Where:
- `src/server.rs` Args + Config

Fix steps:
1. Add missing CLI flags and defaults.
2. Wire flags through runtime config usage.

Verify:
```bash
cargo run -- --help
```

### Issue 10 - Missing env overrides
Target:
- Environment variables override CLI values.

Where:
- config parse/build flow

Fix steps:
1. Implement `RSDRAGONFLY_*` env parsing.
2. Apply env values after parsing args.
3. Validate precedence rules in tests.

Verify:
```bash
cargo test
```

### Issue 11 - Missing config validation
Target:
- Reject invalid config cleanly.

Where:
- config init path

Fix steps:
1. Validate ports, log level, thread counts, snapshot interval.
2. Validate snapshot dir writable.
3. Replace panic/expect with proper error return.

Verify:
```bash
cargo test
```

### Issue 12 - /health and /ready not live
Target:
- Metrics server must route `/metrics`, `/health`, `/ready` correctly.

Where:
- `src/observability/prometheus.rs`
- `src/observability/health.rs`

Fix steps:
1. Parse request path from incoming HTTP request.
2. Return proper 200/503 responses for health/readiness.
3. Keep `/metrics` behavior unchanged.

Verify:
```bash
curl -i http://localhost:9090/metrics
curl -i http://localhost:9090/health
curl -i http://localhost:9090/ready
```

### Issue 13 - Logs not structured JSON
Target:
- Output structured JSON logs.

Where:
- logging initialization and log usage

Fix steps:
1. Switch logger formatting to JSON output.
2. Include required context fields where available.

Verify:
```bash
cargo run -- --port 6379
# inspect logs format
```

### Issue 21 - cpu_pinning flag unused
Target:
- Either implement CPU pinning behavior or remove misleading flag.

Where:
- worker spawn path + OS pinning logic

Fix steps:
1. If implementing: apply thread affinity when flag is set.
2. If not implementing now: block flag with explicit “not supported” error.

Verify:
```bash
cargo test
```

### Issue 39 - snapshot dir default mismatch
Target:
- Default must match docs/plan or docs must be updated consistently.

Where:
- `src/server.rs` Args default

Fix steps:
1. Choose single canonical default.
2. Apply consistently in code/docs.

Verify:
```bash
cargo run -- --help
```

### Issue 40 - EMFILE/resource exhaustion behavior missing
Target:
- Handle accept failures gracefully under FD pressure.

Where:
- `src/frontend/handler.rs`

Fix steps:
1. Add explicit handling for `EMFILE`/`ENFILE` errors.
2. Avoid busy error loops; throttle logging/retry strategy.

Verify:
```bash
cargo test
```

---

## Phase 4: Observability Completeness

### Issue 29 - snapshot metrics never updated
Target:
- Increment snapshot success/failure/skip counters in correct paths.

Where:
- `src/shard/worker.rs`
- `src/shard/persistence/writer.rs`

Fix steps:
1. Increment skip on channel full.
2. Increment write error on write/rename failures.
3. Increment written on successful commit.

Verify:
```bash
cargo test
```

### Issue 30 - queue_depth never maintained
Target:
- queue_depth reflects actual pending shard queue length.

Where:
- channel send/receive boundaries

Fix steps:
1. Increment on enqueue, decrement on dequeue.
2. Keep updates exception-safe on send failures/timeouts.

Verify:
```bash
cargo test
```

### Issue 31 - global counters never incremented
Target:
- Update global connection/command/error counters.

Where:
- frontend accept path
- command processing path

Fix steps:
1. Increment connections_total on accept.
2. Maintain active connection count on open/close.
3. Increment command counters during processing.

Verify:
```bash
cargo test
```

### Issue 32 - latency histogram missing
Target:
- Implement per-shard latency histogram and recording.

Where:
- `src/shard/shard.rs`
- metrics structures/export formatting

Fix steps:
1. Add histogram data structure.
2. Record latency in `record_command`.
3. Expose histogram buckets/sum/count.

Verify:
```bash
cargo test
curl http://localhost:9090/metrics | grep latency
```

### Issue 33 - Prometheus schema incomplete
Target:
- Emit all required documented metrics.

Where:
- `src/observability/prometheus.rs`

Fix steps:
1. Add missing global and per-shard metrics lines.
2. Use stable metric names matching docs.
3. Ensure HELP/TYPE are valid.

Verify:
```bash
cargo test
curl http://localhost:9090/metrics
```

---

## Phase 5: Engine/TTL/Routing Robustness

### Issue 14 - max connections/thread pool mismatch
Target:
- Enforce max connections and align with planned frontend model.

Where:
- `src/frontend/handler.rs`
- startup configuration wiring

Fix steps:
1. Add active connection counting and max cap.
2. Reject extra connections cleanly.
3. Keep handler behavior predictable under load.

Verify:
```bash
cargo test
```

### Issue 15 - active expiry does not reduce memory_bytes
Target:
- Memory metrics must update when active expiration deletes keys.

Where:
- `src/shard/expiration.rs`

Fix steps:
1. On expired key delete, subtract entry memory from `memory_bytes`.
2. Keep key_count and keys_expired updates consistent.

Verify:
```bash
cargo test
```

### Issue 19 - compile-time shard count configurability missing
Target:
- Support compile-time shard count variants (or plan-compatible equivalent).

Where:
- `src/common/constants.rs`
- build config

Fix steps:
1. Add feature-gated shard count constants.
2. Keep routing/tests compatible across variants.

Verify:
```bash
cargo test
cargo test --features shard_count_32
```

### Issue 20 - routing formula mismatch risk
Target:
- Make routing behavior explicitly safe and documented.

Where:
- `src/frontend/router.rs`
- constants/tests

Fix steps:
1. Decide modulo vs bitmask strategy consistent with requirements/plans.
2. Add compile-time/runtime guards if using bitmask.
3. Add equivalence tests.

Verify:
```bash
cargo test
```

### Issue 26 - connection writer task hang
Target:
- Ensure writer task exits cleanly when connection loop ends.

Where:
- `src/frontend/handler.rs`

Fix steps:
1. Drop/close response sender before awaiting writer task.
2. Ensure no await deadlock path remains.

Verify:
```bash
cargo test
```

### Issue 27 - shard sender clones block shutdown
Target:
- Ensure shutdown can terminate all command producers.

Where:
- `src/frontend/handler.rs`
- `src/server.rs`

Fix steps:
1. Introduce shutdown signal in connection tasks.
2. Stop request loop and drop shard sender clones.
3. Confirm shard receivers get closed eventually.

Verify:
```bash
cargo test
```

### Issue 28 - TTL sweep bound incorrect for stale entries
Target:
- Sweep must cap by processed pops, not only deletes.

Where:
- `src/shard/expiration.rs`

Fix steps:
1. Increment processed counter on each pop.
2. Stop after configured max pops.

Verify:
```bash
cargo test
```

### Issue 37 - panic-style error handling in startup/snapshot encode
Target:
- Replace `expect/unwrap` on recoverable paths with error propagation.

Where:
- `src/server.rs`
- `src/shard/snapshot/serialize.rs`

Fix steps:
1. Return/propagate errors instead of panic.
2. Log with context and continue when safe.

Verify:
```bash
cargo test
```

### Issue 38 - snapshot shard ID consistency check missing
Target:
- Validate snapshot header shard id matches expected shard file.

Where:
- `src/shard/snapshot/loader.rs`
- `src/shard/snapshot/deserialize.rs`

Fix steps:
1. Pass expected shard id into deserialize or post-validate.
2. Reject mismatched snapshot.

Verify:
```bash
cargo test
```

---

## Phase 6: Project Structure/Testing/Housekeeping

### Issue 22 - architecture mismatch (single crate vs planned workspace)
Action:
- Do not do full migration first.
- First stabilize behavior.
- Then plan workspace migration as separate controlled refactor.

### Issue 23 - dead code/stubs/TODOs in hot paths
Action:
1. Remove placeholder add() functions/modules.
2. Resolve TODOs tied to behavior.
3. Keep TODOs only for true non-MVP items.

### Issue 24 - missing integration tests
Action:
1. Create integration tests for protocol/TTL/persistence/multi-key/INFO.
2. Follow scenarios in `docs/testing/testing-strategy.md`.

### Issue 25 - binary naming mismatch
Action:
1. Align package/bin names with docs (`rsdragonfly`) or update docs consistently.

---

## Final Completion Checklist
All issues can be considered fixed only when:
1. All behavior matches docs/plans (or documented deviation approved).
2. `cargo fmt && cargo test` passes.
3. Manual smoke tests pass:
- PING/SET/GET/DEL/EXPIRE/TTL/INFO
- `/metrics` `/health` `/ready`
- shutdown + restart + snapshot recovery
4. `code_review_issues.md` items are checked/closed in your task tracker.

