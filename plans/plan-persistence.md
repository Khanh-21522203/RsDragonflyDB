# Feature: Persistence

## 1. Purpose

The `crates/persistence` crate implements snapshot-based crash recovery. Each shard serializes its in-memory state to a binary file (`shard{N}.snap`) on a configurable interval (default 60s) and on graceful shutdown. On startup, each shard's snapshot is read back and deserialized, filtering out expired keys.

Persistence is fully per-shard — there is no global snapshot coordinator. Each shard thread sends its serialized data to a dedicated snapshot writer thread; blocking disk IO never touches the shard thread.

## 2. Responsibilities

- Define the binary snapshot format (64-byte fixed header + variable-length entry stream)
- Implement `serialize_shard`: convert `Shard` data to owned `Vec<u8>` (called in shard thread)
- Implement the snapshot writer thread: receive `SnapshotRequest`, write to `.tmp`, rename atomically
- Implement `load_snapshot`: read and validate a snapshot file at startup; return `HashMap<Key, Entry>`
- Validate header and data checksums (CRC64) on load; reject corrupt files
- Filter expired keys during deserialization (TTL < now → skip)
- Clean up stale `.snap.tmp` files at startup
- Expose `cleanup_temp_files(dir)` for the server startup sequence

## 3. Non-Responsibilities

- Does not manage shard threads or channels
- Does not implement TTL logic (TTL expiry timestamps are provided by the engine)
- Does not implement AOF or incremental snapshots (post-MVP)
- Does not compress snapshot data (post-MVP)
- Does not retain multiple historical snapshots (only latest per shard)

## 4. Architecture Design

```
Shard thread
    │  serialize_shard(&shard) → Vec<u8>   (fast, ~1ms, in shard thread)
    │  try_send(SnapshotRequest { shard_id, data })
    │  crossbeam::channel::bounded(1)
    ▼
snapshot_writer_thread(shard_id, snapshot_rx, snapshot_dir)
    │
    ├─ File::create("shard{N}.snap.tmp")
    ├─ write_all(data)
    ├─ sync_all()                           (fsync)
    └─ fs::rename("shard{N}.snap.tmp", "shard{N}.snap")  (atomic)

On startup (main thread, parallel):
    │  load_snapshot("shard{N}.snap")
    │    ├─ validate magic, version, checksums
    │    ├─ parse entries
    │    └─ skip expired keys
    ▼
    HashMap<Key, Entry>  (handed to shard thread)
```

### File naming

```
/var/lib/rsdragonfly/
    shard0.snap       ← latest valid snapshot for shard 0
    shard0.snap.tmp   ← write-in-progress; renamed to .snap on success
    shard1.snap
    ...
    shard63.snap
```

## 5. Core Data Structures

```rust
// crates/persistence/src/format.rs

/// Snapshot file header — exactly 64 bytes, little-endian.
///
/// Offsets:
///  0: magic (4 bytes)        "RSDF"
///  4: version (4 bytes)      1
///  8: shard_id (2 bytes)
/// 10: reserved (2 bytes)     0
/// 12: key_count (8 bytes)
/// 20: timestamp (8 bytes)    Unix seconds
/// 28: data_checksum (8 bytes) CRC64 of all entry bytes
/// 36: header_checksum (8 bytes) CRC64 of bytes [0..36]
/// 44: reserved (20 bytes)    0
/// 64: first entry starts here
pub const HEADER_SIZE: usize = 64;
pub const MAGIC: &[u8; 4] = b"RSDF";
pub const FORMAT_VERSION: u32 = 1;

/// Per-entry layout (variable size):
///   key_len:   u32  (4 bytes)
///   key_data:  [u8; key_len]
///   val_len:   u32  (4 bytes)
///   val_data:  [u8; val_len]
///   has_expiry: u8  (1 byte, 0 or 1)
///   expiry_ts: u64  (8 bytes, only if has_expiry == 1)
pub struct EntryRecord<'a> {
    pub key: &'a [u8],
    pub value: &'a [u8],
    pub expiry_unix_secs: Option<u64>,
}
```

```rust
// crates/persistence/src/lib.rs

pub use writer::snapshot_writer_thread;
pub use serializer::serialize_shard;
pub use loader::{load_snapshot, cleanup_temp_files};
```

## 6. Public Interfaces

```rust
// Serialization (called in shard thread — must be fast)
pub fn serialize_shard(shard: &Shard) -> Vec<u8>

// Snapshot writer OS thread entry point
pub fn snapshot_writer_thread(
    shard_id: ShardId,
    snapshot_rx: Receiver<SnapshotRequest>,
    snapshot_dir: PathBuf,
    metrics: Arc<ShardMetrics>,
)

// Startup loading (called in main thread, parallel per shard)
pub fn load_snapshot(path: &Path) -> Result<HashMap<Key, Entry>, Error>

// Startup cleanup
pub fn cleanup_temp_files(snapshot_dir: &Path)
```

## 7. Internal Algorithms

### serialize_shard

```
serialize_shard(shard):
  buf = Vec::with_capacity(HEADER_SIZE + estimate_entries_size(shard))

  // Write placeholder header (checksums = 0 initially)
  buf.extend_from_slice(MAGIC)            // [0..4]
  write_u32(&mut buf, FORMAT_VERSION)     // [4..8]
  write_u16(&mut buf, shard.id.0)         // [8..10]
  write_u16(&mut buf, 0)                  // [10..12] reserved
  write_u64(&mut buf, shard.data.len() as u64)  // [12..20]
  write_u64(&mut buf, unix_now_secs())    // [20..28]
  write_u64(&mut buf, 0)                  // [28..36] data_checksum placeholder
  write_u64(&mut buf, 0)                  // [36..44] header_checksum placeholder
  buf.extend_from_slice(&[0u8; 20])       // [44..64] reserved

  data_start = buf.len()  // = 64

  // Write entries
  for (key, entry) in &shard.data:
    write_u32(&mut buf, key.len() as u32)
    buf.extend_from_slice(key)

    let val_bytes = entry.value.as_bytes()
    write_u32(&mut buf, val_bytes.len() as u32)
    buf.extend_from_slice(val_bytes)

    match entry.expiry:
      None →
        buf.push(0)   // has_expiry = 0
      Some(expiry) →
        buf.push(1)   // has_expiry = 1
        write_u64(&mut buf, expiry_to_unix_secs(expiry))

  // Compute and back-fill checksums
  data_checksum   = crc64(&buf[data_start..])
  header_checksum = crc64(&buf[0..36])     // covers header fields at [0..36]

  write_u64_at(&mut buf, 28, data_checksum)
  write_u64_at(&mut buf, 36, header_checksum)

  return buf
```

### snapshot_writer_thread

```
snapshot_writer_thread(shard_id, snapshot_rx, snapshot_dir, metrics):
  loop:
    match snapshot_rx.recv():
      Ok(request) →
        temp_path  = snapshot_dir / "shard{N}.snap.tmp"
        final_path = snapshot_dir / "shard{N}.snap"
        start = Instant::now()

        match write_snapshot_file(temp_path, request.data):
          Ok(()) →
            match fs::rename(temp_path, final_path):
              Ok(()) →
                log INFO "snapshot_written" { shard_id, key_count, size_bytes, duration_ms }
                metrics.snapshots_written.fetch_add(1, Relaxed)
              Err(e) → log ERROR "snapshot_rename_failed" { e }; metrics.snapshot_write_errors++
          Err(e) →
            log ERROR "snapshot_write_failed" { e, path: temp_path }
            metrics.snapshot_write_errors.fetch_add(1, Relaxed)

      Err(_) → break  // channel closed (shutdown)

write_snapshot_file(path, data):
  file = File::create(path)?
  file.write_all(data)?
  file.sync_all()?   // fsync — ensures data is on disk before rename
  Ok(())
```

### load_snapshot

```
load_snapshot(path):
  data = fs::read(path)?

  // Minimum size check
  if data.len() < HEADER_SIZE: return Err(InvalidSnapshot("file too small"))

  // Validate magic
  if data[0..4] != b"RSDF": return Err(InvalidSnapshot("bad magic"))

  // Validate version
  version = read_u32(&data, 4)
  if version != 1: return Err(InvalidSnapshot("unsupported version"))

  shard_id  = read_u16(&data, 8)
  key_count = read_u64(&data, 12)
  timestamp = read_u64(&data, 20)
  data_checksum   = read_u64(&data, 28)
  header_checksum = read_u64(&data, 36)

  // Verify header checksum (covers bytes [0..36])
  computed_header_cksum = crc64(&data[0..36])
  if header_checksum != computed_header_cksum:
    return Err(CorruptSnapshot("header checksum mismatch"))

  // Verify data checksum (covers bytes [64..])
  computed_data_cksum = crc64(&data[HEADER_SIZE..])
  if data_checksum != computed_data_cksum:
    return Err(CorruptSnapshot("data checksum mismatch"))

  // Parse entries
  map = HashMap::with_capacity(key_count as usize)
  cursor = HEADER_SIZE
  now_unix = unix_now_secs()

  for _ in 0..key_count:
    key_len = read_u32(&data, cursor) as usize; cursor += 4
    key = data[cursor..cursor+key_len].to_vec(); cursor += key_len

    val_len = read_u32(&data, cursor) as usize; cursor += 4
    val = data[cursor..cursor+val_len].to_vec(); cursor += val_len

    has_expiry = data[cursor]; cursor += 1

    expiry = if has_expiry == 1:
      expiry_ts = read_u64(&data, cursor); cursor += 8
      if expiry_ts <= now_unix: continue   // skip already-expired keys
      Some(unix_secs_to_instant(expiry_ts))
    else: None

    map.insert(key, Entry { value: Value::String(val), expiry })

  log INFO "snapshot_loaded" { shard_id, key_count: map.len(), expired_skipped, duration_ms }
  return Ok(map)
```

### cleanup_temp_files

```
cleanup_temp_files(snapshot_dir):
  for entry in fs::read_dir(snapshot_dir):
    if entry.path().extension() == "tmp":
      log INFO "removing stale temp file" { path }
      fs::remove_file(entry.path()).ok()
```

### CRC64 algorithm

Uses the `crc` crate with the `CRC_64_ECMA_182` polynomial (standard CRC-64 for data integrity). Input is a byte slice; output is `u64`.

## 8. Persistence Model

This crate IS the persistence model. Key design decisions:
- **Atomic write**: write to `.tmp`, then rename. If the process crashes mid-write, the old `.snap` remains valid.
- **fsync before rename**: guarantees data is on disk before the new snapshot becomes visible.
- **One snapshot per shard**: on rename, the old file is atomically replaced. No snapshot history.
- **No cross-shard coordination**: each shard's snapshot is independent; there is no global consistent snapshot timestamp.

## 9. Concurrency Model

- `serialize_shard` runs in the shard thread (no shared state needed — shard owns its data)
- `snapshot_writer_thread` runs in a dedicated OS thread per shard (blocking IO isolated here)
- Channel `bounded(1)` between shard and writer: if writer is slow, shard skips the snapshot (logged as `snapshots_skipped`)
- `load_snapshot` runs at startup in the main thread, one call per shard, parallel across shards (no shared state between shard loads)

## 10. Configuration

| Parameter | Source | Default | Notes |
|-----------|--------|---------|-------|
| `snapshot_dir` | `Config` | `/var/lib/rsdragonfly` | Directory for `.snap` files |
| `snapshot_interval_secs` | `Config` | 60 | Periodic trigger interval in shard thread |

## 11. Observability

Per-shard metrics (updated by snapshot writer thread via `Arc<ShardMetrics>`):
- `snapshots_written` — successful writes
- `snapshot_write_errors` — failed writes
- `snapshots_skipped` — channel full, write skipped

Prometheus metrics exposed by the metrics exporter:
```
rsdragonfly_shard_snapshots_written_total{shard="0"}
rsdragonfly_shard_snapshot_write_errors_total{shard="0"}
rsdragonfly_shard_snapshots_skipped_total{shard="0"}
```

Structured log events (JSON):
```json
// Success
{ "level":"info", "event":"snapshot_written", "shard_id":0, "key_count":1000000, "size_bytes":10485760, "duration_ms":1200 }
// Failure
{ "level":"error", "event":"snapshot_write_failed", "shard_id":0, "error":"No space left on device", "path":"/var/lib/rsdragonfly/shard0.snap.tmp" }
// Load success
{ "level":"info", "event":"snapshot_loaded", "shard_id":0, "key_count":999500, "expired_skipped":500, "duration_ms":5000 }
// Load failure
{ "level":"error", "event":"snapshot_load_failed", "shard_id":0, "error":"header checksum mismatch", "path":"/var/lib/rsdragonfly/shard0.snap" }
```

Alerts:
- `snapshot_write_errors_total > 0` → WARNING
- `snapshot_last_write_timestamp > 120s` ago → WARNING (snapshot lag)

## 12. Testing Strategy

- **Unit tests**:
  - `test_serialize_deserialize_roundtrip`: create a Shard with 100 keys (some with TTL), serialize, deserialize, assert all non-expired keys present
  - `test_serialize_expired_keys_skipped`: set a key with TTL in the past, serialize, deserialize, assert key absent
  - `test_checksum_corruption_detected`: serialize shard, flip one byte in entries area, load, assert CorruptSnapshot error
  - `test_header_checksum_corruption_detected`: flip one byte in header area (before offset 36), load, assert CorruptSnapshot
  - `test_wrong_magic_rejected`: write file with bad magic, load, assert InvalidSnapshot
  - `test_unsupported_version_rejected`: write file with version=99, load, assert InvalidSnapshot
  - `test_file_too_small_rejected`: write 10-byte file, load, assert InvalidSnapshot
  - `test_atomic_rename`: simulate crash mid-write (write .tmp but don't rename), verify old .snap unchanged
  - `test_cleanup_temp_files`: create 3 .tmp files in dir, call cleanup_temp_files, assert all removed
  - `test_expiry_unix_roundtrip`: set expiry 1h from now, convert to unix, back to Instant, assert < 1s error
  - `test_snapshot_writer_thread_busy`: send two requests before writer finishes first, assert second is dropped (try_send fails)

- **Integration tests**:
  - `test_crash_recovery`: start server, set 1000 keys, wait 61s for snapshot, SIGKILL, restart, verify all 1000 keys present
  - `test_restart_expired_keys_gone`: set 100 keys with EX 5, wait for snapshot, wait 6s, SIGKILL, restart, verify all 100 gone
  - `test_corrupt_snapshot_starts_empty`: corrupt a shard snapshot file, restart server, verify shard starts empty (no crash)

## 13. Open Questions

None.
