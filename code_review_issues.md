# Code Review Issues

This review compares the current implementation against `docs/requirements.md` and the implementation plans under `plans/`.

## Critical

1. **Snapshot recovery is never executed at startup (data loss on restart).**
- Spec: `docs/requirements.md` FR-5 requires loading the latest snapshot on startup and recovering non-expired keys.
- Evidence: Server startup creates empty shards only (`src/server.rs:89-98` and `src/shard/worker.rs:15`), while snapshot loading exists but is unused (`src/shard/snapshot/loader.rs:7-17`).
- Impact: Crash/restart recovery path is effectively absent.

2. **Snapshot checksum fields are written at wrong offsets; header checksum range is inconsistent with format spec.**
- Spec: `docs/persistence/persistence.md` and `plans/plan-persistence.md` define checksum offsets at header bytes 28 and 36, and header checksum over bytes `[0..36]`.
- Evidence: Serialization writes checksums starting at byte 32 (`src/shard/snapshot/serialize.rs:47-49`) and computes header checksum over `[0..56]` (`src/shard/snapshot/serialize.rs:43`); deserialization also validates `[0..56]` (`src/shard/snapshot/deserialize.rs:37`).
- Impact: Snapshot format diverges from docs/plans and risks incompatibility/corruption handling defects.

3. **Graceful shutdown is incomplete; final snapshots are not reliably guaranteed.**
- Spec: `docs/requirements.md` NFR-5 requires SIGTERM-triggered graceful shutdown with drain + final snapshots.
- Evidence: Only `ctrl_c()` is handled (`src/server.rs:140`), not SIGTERM; shutdown just sleeps and drops one sender vector (`src/server.rs:150-162`) without stopping connection tasks or joining worker/writer tasks.
- Impact: In-flight requests and final snapshots can be lost during shutdown.

4. **Metrics exporter reads a different metrics set than shard workers update.**
- Spec: `docs/requirements.md` FR-1 INFO and NFR-4 metrics require real aggregated shard stats.
- Evidence: Server creates `shard_metrics` for exporter (`src/server.rs:56-59`) but shard workers instantiate independent internal metrics in `Shard::new()` (`src/shard/shard.rs:22-31`) and never receive exporter metrics.
- Impact: Exported shard metrics and any INFO based on them are inaccurate (often all zeros).

5. **INFO command implementation does not match required global semantics or required fields.**
- Spec: `docs/requirements.md` FR-1: INFO must aggregate across all shards and include required `server/stats/memory` fields.
- Evidence: INFO is routed to shard 0 (`src/frontend/handler.rs:112-113`), and shard-local formatter emits only partial/non-compliant fields (`src/shard/shard.rs:196-225`).
- Impact: INFO output is not Redis-compatible per MVP spec and misrepresents server state.

6. **Core concurrency architecture diverges from required thread-per-shard model.**
- Spec: `docs/requirements.md` FR-6 + constraints: one OS thread per shard, sync-only shard execution path (no async runtime in shard threads).
- Evidence: Shards and snapshot writers are spawned as Tokio tasks (`src/server.rs:84-97`), with async/timer-driven shard loop (`src/shard/worker.rs:10-48`).
- Impact: Violates documented architecture guarantees and predictability assumptions.

7. **Current test suite already fails a sharding acceptance check.**
- Spec: `docs/requirements.md` FR-2 requires uniform key distribution (<5% variance).
- Evidence: `cargo test` currently fails `common::hash::tests::test_crc16_distribution` (`src/common/hash.rs:30-46`, failure observed in current run).
- Impact: Baseline quality gate fails; sharding distribution requirement is not met/verified.

8. **Protocol-format violations are not consistently connection-fatal.**
- Spec: `docs/requirements.md` FR-1 says protocol errors must close the connection (`docs/requirements.md:67`).
- Evidence: Non-array RESP values are mapped to `CommandError::InvalidFormat` (`src/protocol/command.rs:16-20`), but handler turns that into `ERR ...` and continues (`src/frontend/handler.rs:94-97`) instead of closing.
- Impact: Invalid wire-format clients can remain connected contrary to protocol error policy.

## High

9. **CLI surface is incomplete versus requirements; key runtime knobs are missing.**
- Spec: `docs/requirements.md` NFR-5 requires flags including `--snapshot-interval`, `--max-connections`, `--frontend-threads`, `--metrics-port`.
- Evidence: Args define only `port/bind/snapshot_dir/log_level/cpu_pinning` (`src/server.rs:21-36`), while metrics port is hardcoded to 9090 (`src/server.rs:116`).
- Impact: Operability requirements are not met.

10. **Environment variable overrides are not implemented.**
- Spec: `docs/requirements.md` NFR-5 requires env vars overriding flags.
- Evidence: No env override logic exists; config is a direct CLI mapping (`src/server.rs:174-183`).
- Impact: Deployment behavior diverges from docs and container/runbook expectations.

11. **Configuration validation is largely missing.**
- Spec: `docs/requirements.md` NFR-5 acceptance requires rejecting invalid values.
- Evidence: No explicit validation for log level, path writability, thread counts, etc.; startup uses `expect` on snapshot dir creation (`src/server.rs:49-52`).
- Impact: Invalid configs can panic or run with undefined behavior.

12. **`/health` and `/ready` endpoints are not implemented in the live exporter path.**
- Spec: `docs/requirements.md` NFR-4 requires `/health` and `/ready`.
- Evidence: Exporter always writes a 200 metrics response without parsing request path (`src/observability/prometheus.rs:39-57`); health helpers are unused (`src/observability/health.rs:3-27`).
- Impact: Kubernetes/readiness integration is blocked.

13. **Logging format is not structured JSON.**
- Spec: `docs/requirements.md` NFR-4 requires structured JSON logs.
- Evidence: `env_logger` default text formatter is used (`src/main.rs:18-20`), while JSON logging helpers are not integrated (`src/observability/logging.rs:1-53`).
- Impact: Log ingestion assumptions in docs/runbooks are invalid.

14. **Connection management does not enforce max connections or fixed frontend thread pool model.**
- Spec: `docs/requirements.md` FR-6/NFR-5 and `plans/plan-frontend.md` require acceptor + configurable handler pool and max connection enforcement.
- Evidence: Each accepted socket spawns an unbounded Tokio task (`src/frontend/handler.rs:31-42`); no max-connection checks exist.
- Impact: Resource exhaustion risk and architectural mismatch.

15. **Active expiration removes keys without adjusting memory usage metrics.**
- Spec: `plans/plan-ttl-expiration.md` and `plan-shard-engine.md` require key_count and memory metrics updates on expiration.
- Evidence: `expire_keys` removes from `data` but updates only `keys_expired` and `key_count` (`src/shard/expiration.rs:23-38`).
- Impact: `memory_bytes` drifts upward and can induce false OOM decisions.

16. **RESP command error messages are non-compliant with required Redis conventions.**
- Spec: `docs/requirements.md` FR-1 error handling mandates Redis-style strings.
- Evidence: Parse/command errors are returned as `ERR {:?}` enum debug output (`src/frontend/handler.rs:94-96`).
- Impact: Client compatibility tests expecting Redis-like errors will fail.

17. **`PING` response wire type is incorrect for no-argument form.**
- Spec: `docs/requirements.md` FR-1 expects `PING` to return `PONG`.
- Evidence: Shard returns `Response::Value("PONG")` (`src/shard/shard.rs:52-56`), serialized as bulk string (`src/frontend/handler.rs:201-205`) instead of simple string.
- Impact: Redis compatibility edge-case mismatch.

18. **`INFO` argument count validation is missing.**
- Spec: `docs/requirements.md` FR-1 requires Redis-style wrong-arg-count handling.
- Evidence: `parse_info` accepts any number of args and uses only the first (`src/protocol/command.rs:132-139`).
- Impact: Protocol compliance gap.

19. **Shard count is hardcoded; compile-time configurability from plans is missing.**
- Spec: `docs/requirements.md` FR-2 (compile-time configurable shard count) and `plans/plan-common-types.md` feature-gated constants.
- Evidence: `SHARD_COUNT` is fixed to `64` in code (`src/common/constants.rs:1`) with no feature gating.
- Impact: Cannot validate or run planned shard-count variants.

20. **Routing formula differs from requirements text and assumes power-of-two shard counts without guardrails.**
- Spec: `docs/requirements.md` FR-2 states `CRC16(key) % shard_count`.
- Evidence: Routing uses bitmask `(hash as usize) & (SHARD_COUNT - 1)` (`src/frontend/router.rs:10-12`) and no assertion that shard count stays power-of-two.
- Impact: Behavior can silently break if shard count changes.

21. **CPU pinning flag is accepted but unused.**
- Spec: `docs/requirements.md` NFR-5 includes CPU pinning option.
- Evidence: Flag is parsed into config (`src/server.rs:34-35`, `src/server.rs:171`) but never applied to shard worker execution.
- Impact: Config surface is misleading and not functional.

## Medium

22. **Planned workspace crate architecture is not implemented.**
- Spec: `plans/plan-project-setup.md` defines a multi-crate workspace (`crates/common`, `crates/protocol`, `crates/engine`, etc.).
- Evidence: Repository is a single crate with monolithic `src/` modules (`Cargo.toml:1-19`).
- Impact: Divergence from planned build boundaries and dependency structure.

23. **Several modules still contain placeholder/stub scaffolding and dead code paths.**
- Evidence: Example placeholder test helpers (`src/protocol/mod.rs:8-21`, `src/shard/mod.rs:9-22`), TODOs in hot-path modules (`src/protocol/parser.rs:124`, `src/frontend/handler.rs:192`, `src/shard/shard.rs:275`).
- Impact: Indicates incomplete implementation and increases maintenance risk.

24. **Integration test suite from plans/docs is missing.**
- Spec: `docs/testing/testing-strategy.md` and `plans/plan-integration-testing.md` define integration tests for protocol, TTL, persistence, multi-key behavior.
- Evidence: No `tests/` directory in repository root; only limited unit tests under modules.
- Impact: Major behavioral requirements are unverified end-to-end.

25. **Binary/package naming diverges from docs and plan conventions.**
- Spec: docs/plans consistently refer to runtime binary `rsdragonfly`.
- Evidence: Package and bin are currently named `RsDragonflyDB` (`Cargo.toml:2`, `Cargo.toml:8`).
- Impact: Operational commands in docs/runbooks won’t work as written.

## Second-Pass Additional Findings

26. **Connection shutdown path can hang indefinitely because writer task is awaited while sender is still alive.**
- Evidence: `handle_connection` creates `response_tx` (`src/frontend/handler.rs:54`), then awaits `writer_handle` at function end (`src/frontend/handler.rs:106`) without dropping `response_tx` first.
- Impact: Connection tasks can leak/hang on disconnect, consuming resources indefinitely.

27. **Shard channel closure on shutdown is unreliable because per-connection tasks retain cloned senders.**
- Evidence: Each accepted connection clones shard sender vector (`src/frontend/handler.rs:35`), while shutdown only drops the server-owned sender vector (`src/server.rs:156-157`).
- Impact: Shard worker loops may never observe channel closure, preventing deterministic final snapshot trigger/exit.

28. **Active expiration’s 20-entries-per-pass bound is implemented incorrectly for stale TTL entries.**
- Spec: `plans/plan-ttl-expiration.md` requires processing up to 20 heap pops per pass (`plans/plan-ttl-expiration.md:166-176`).
- Evidence: Loop bound increments only when a key is actually deleted (`src/shard/expiration.rs:24-27`), so stale entries can be popped without bound in one tick.
- Impact: Worst-case latency spikes under stale-heavy TTL heaps.

29. **Snapshot-related shard metrics are defined but never updated on success/skip/failure paths.**
- Spec: Observability docs require snapshot counters (`docs/observability/observability.md:122-140`).
- Evidence: Counters exist (`src/shard/shard.rs:249-251`) but no increment on writer success/failure (`src/shard/persistence/writer.rs:25-43`) and no increment on skip/disconnect in trigger path (`src/shard/worker.rs:62-71`).
- Impact: Snapshot health signals are inaccurate.

30. **`queue_depth` metric is never maintained, invalidating backpressure/monitoring signals.**
- Spec: Queue depth is required (`docs/observability/observability.md:84-86`, `plans/plan-frontend.md:415-416`).
- Evidence: Field exists (`src/shard/shard.rs:254`) but there are no runtime updates to it.
- Impact: Any overload decisions or dashboards based on queue depth are meaningless.

31. **Global metrics counters are never incremented in request handling path.**
- Spec: Global connections/commands/QPS metrics are required (`docs/requirements.md:262-265`).
- Evidence: Counters are declared (`src/observability/metrics.rs:8-11`) but no code updates them in frontend/server paths.
- Impact: Global observability and INFO stats cannot be accurate.

32. **Latency histogram required by plans/docs is not implemented in shard metrics.**
- Spec: `docs/observability/observability.md` requires `latency_histogram` with bucket metrics (`docs/observability/observability.md:90-104`, `docs/observability/observability.md:158-165`).
- Evidence: `record_command` has TODO for histogram (`src/shard/shard.rs:273-276`), and no histogram fields are exported.
- Impact: p50/p99 latency visibility is missing.

33. **Prometheus metric schema is materially incomplete versus documented metrics.**
- Spec: docs expect metrics including command/error totals by labels, QPS, queue depth, latency histogram, TTL queue size, snapshot duration/size/timestamps (`docs/observability/observability.md:50-63`, `docs/observability/observability.md:84-118`, `docs/observability/observability.md:124-140`).
- Evidence: Exporter emits only a small subset (`src/observability/prometheus.rs:65-123`).
- Impact: Runbook/alerting queries in docs cannot be satisfied.

34. **`INFO` with no section returns only server fragment instead of concatenated server+stats+memory.**
- Spec: `docs/requirements.md` requires all sections for bare `INFO` (`docs/requirements.md:28`, `docs/requirements.md:57`).
- Evidence: `None | Some(\"server\")` branch writes only server fields (`src/shard/shard.rs:199-205`), no concatenation logic exists.
- Impact: Non-compliant INFO semantics.

35. **`INFO` section parsing/dispatch is case-sensitive; Redis-style case-insensitive behavior is not preserved.**
- Evidence: Parser stores section as-is (`src/protocol/command.rs:136-138`), shard matches exact lowercase literals (`src/shard/shard.rs:199-220`).
- Impact: `INFO SERVER` and similar case variants fail unexpectedly.

36. **RESP parser has no upper bound on array length (`*N`), leaving a memory/CPU abuse vector.**
- Evidence: Explicit TODO for missing bound check (`src/protocol/parser.rs:124`).
- Impact: Malicious clients can force excessive allocation/work with oversized multibulk lengths.

37. **Startup path still uses panic-style error handling on filesystem setup.**
- Spec: Reliability requires avoiding panics on operational errors (`docs/requirements.md:249-252`).
- Evidence: Snapshot dir creation uses `expect` (`src/server.rs:49-51`); several snapshot encode paths use `unwrap` (`src/shard/snapshot/serialize.rs:16`, `src/shard/snapshot/serialize.rs:23`, `src/shard/snapshot/serialize.rs:28`).
- Impact: Recoverable IO/path issues can crash the server.

38. **Snapshot load path does not validate shard identity consistency against target file.**
- Spec: Persistence format includes shard metadata and corruption detection (`docs/requirements.md:157-160`).
- Evidence: Loader selects `shard{N}.snap` (`src/shard/snapshot/loader.rs:8`) but deserializer accepts whatever `header.shard_id` contains (`src/shard/snapshot/deserialize.rs:49`) without enforcing expected shard ID.
- Impact: Swapped/misplaced snapshot files can silently hydrate wrong shard IDs.

39. **Default snapshot directory diverges from planned configuration defaults.**
- Spec: plan/config docs use `/var/lib/rsdragonfly` as default snapshot path (`plans/plan-configuration.md:72`, `plans/plan-configuration.md:284-295`).
- Evidence: Current CLI default is `./data` (`src/server.rs:28-29`).
- Impact: Operational docs and manifests can drift from runtime behavior.

40. **Resource-exhaustion behavior for accept path (e.g., EMFILE) is not explicitly handled as required.**
- Spec: Operability requires graceful handling of file descriptor exhaustion (`docs/requirements.md:313-314`).
- Evidence: Accept loop logs generic errors only (`src/frontend/handler.rs:43`) and continues without specific rejection/mitigation policy.
- Impact: Under FD pressure, behavior is unspecified and likely noisy/degraded.
