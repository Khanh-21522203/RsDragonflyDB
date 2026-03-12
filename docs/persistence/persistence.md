# Persistence Strategy

## Document Purpose
This document defines the snapshot-based persistence mechanism for crash recovery, including snapshot format, write/read procedures, and durability guarantees.

**Audience**: Implementation engineers, operations engineers

---

## Persistence Philosophy

### Snapshot-Based Persistence

**Decision**: RsDragonflyDB MVP uses snapshot-only persistence (no AOF).

**Rationale**:
- **Simplicity**: Single persistence mechanism, easier to implement and test
- **Performance**: No write amplification (no fsync per command)
- **Recovery Speed**: Fast recovery (single read per shard)
- **Use Case**: Suitable for cache workloads where some data loss is acceptable

**Trade-Off**: Data loss window (up to snapshot interval, default 60s).

---

### Durability Guarantees

**What RsDragonflyDB Guarantees**:
- Data persisted in last successful snapshot survives crash
- Snapshots are atomic per shard (all keys or none)
- Corrupt snapshots are detected and rejected

**What RsDragonflyDB Does NOT Guarantee**:
- No data loss (writes between snapshots are lost)
- No durability for individual commands (no fsync per write)
- No point-in-time recovery (only latest snapshot)

**Suitable For**:
- Cache workloads (session storage, API caching)
- Non-critical data (can tolerate 60s data loss)

**NOT Suitable For**:
- Financial transactions
- Critical user data
- Systems requiring strong durability

---

## Snapshot Format

### File Format Specification

```
Snapshot File: shard{N}.snap

┌─────────────────────────────────────────────────────────────┐
│ HEADER (64 bytes)                                           │
├─────────────────────────────────────────────────────────────┤
│ Magic Number: "RSDF" (4 bytes)                              │
│ Format Version: u32 (4 bytes) = 1                           │
│ Shard ID: u16 (2 bytes)                                     │
│ Reserved: u16 (2 bytes) = 0                                 │
│ Key Count: u64 (8 bytes)                                    │
│ Snapshot Timestamp: u64 (8 bytes, Unix seconds)             │
│ Data Checksum: u64 (8 bytes, CRC64 of entries)             │
│ Header Checksum: u64 (8 bytes, CRC64 of header)            │
│ Reserved: [u8; 20] (20 bytes) = 0                           │
├─────────────────────────────────────────────────────────────┤
│ ENTRY 1                                                     │
├─────────────────────────────────────────────────────────────┤
│ Key Length: u32 (4 bytes)                                   │
│ Key Data: [u8; key_len]                                     │
│ Value Length: u32 (4 bytes)                                 │
│ Value Data: [u8; value_len]                                 │
│ Has Expiry: u8 (1 byte) = 0 or 1                           │
│ Expiry Timestamp: u64 (8 bytes, Unix seconds, if has_expiry)│
├─────────────────────────────────────────────────────────────┤
│ ENTRY 2                                                     │
│ ...                                                         │
├─────────────────────────────────────────────────────────────┤
│ ENTRY N                                                     │
└─────────────────────────────────────────────────────────────┘
```

**Design Decisions**:
- **Fixed header size**: Simplifies parsing
- **CRC64 checksums**: Detects corruption (stronger than CRC32)
- **Little-endian**: Standard for x86_64
- **No compression**: Simplicity (future: add optional compression)

---

### Header Fields

| Field | Type | Size | Description |
|-------|------|------|-------------|
| Magic | [u8; 4] | 4 | "RSDF" (0x52534446) |
| Version | u32 | 4 | Format version (1 for MVP) |
| Shard ID | u16 | 2 | Shard identifier (0-63) |
| Reserved | u16 | 2 | Future use |
| Key Count | u64 | 8 | Number of entries |
| Timestamp | u64 | 8 | Snapshot creation time (Unix seconds) |
| Data Checksum | u64 | 8 | CRC64 of all entries |
| Header Checksum | u64 | 8 | CRC64 of header (excluding this field) |
| Reserved | [u8; 20] | 20 | Future use (total header = 64 bytes) |

---

### Entry Format

| Field | Type | Size | Description |
|-------|------|------|-------------|
| Key Length | u32 | 4 | Length of key in bytes |
| Key Data | [u8] | key_len | Key bytes |
| Value Length | u32 | 4 | Length of value in bytes |
| Value Data | [u8] | value_len | Value bytes |
| Has Expiry | u8 | 1 | 0 = no TTL, 1 = has TTL |
| Expiry | u64 | 8 | Unix timestamp (only if has_expiry = 1) |

**Variable Size**: Each entry size = 9 + key_len + value_len (+ 8 if has TTL).

---

## Snapshot Write Procedure

### Trigger Conditions

**Periodic**: Every 60 seconds (configurable via `--snapshot-interval`).

**Manual**: SIGTERM (graceful shutdown triggers final snapshot).

**Not Triggered By**: Individual commands (no fsync per write).

---

### Write Algorithm

```rust
fn trigger_snapshot(shard: &mut Shard, snapshot_tx: &Sender<SnapshotRequest>) {
    // 1. Serialize shard data (in shard thread)
    let snapshot_data = serialize_shard(shard);
    
    // 2. Send to snapshot writer thread (non-blocking)
    match snapshot_tx.try_send(SnapshotRequest {
        shard_id: shard.id,
        data: snapshot_data,
    }) {
        Ok(()) => {
            log::debug!("Snapshot triggered for shard {}", shard.id.0);
        }
        Err(TrySendError::Full(_)) => {
            log::warn!("Snapshot writer busy, skipping snapshot for shard {}", shard.id.0);
            shard.metrics.snapshots_skipped.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {
            log::error!("Snapshot writer disconnected for shard {}", shard.id.0);
        }
    }
}

fn serialize_shard(shard: &Shard) -> Vec<u8> {
    let mut buf = Vec::with_capacity(estimate_snapshot_size(shard));
    
    // Write header (placeholder checksums)
    write_header(&mut buf, shard.id, shard.data.len(), 0, 0);
    
    let data_start = buf.len();
    
    // Write entries
    for (key, entry) in &shard.data {
        write_u32(&mut buf, key.len() as u32);
        buf.extend_from_slice(key);
        
        let value_bytes = entry.value.as_bytes();
        write_u32(&mut buf, value_bytes.len() as u32);
        buf.extend_from_slice(value_bytes);
        
        if let Some(expiry) = entry.expiry {
            buf.push(1);
            let unix_timestamp = expiry_to_unix_timestamp(expiry);
            write_u64(&mut buf, unix_timestamp);
        } else {
            buf.push(0);
        }
    }
    
    // Compute checksums
    let data_checksum = crc64(&buf[data_start..]);
    // Header checksum covers bytes 0-35 (all fields before the checksum field at offset 36)
    let header_checksum = crc64(&buf[0..36]);

    // Update checksums in header at their specified offsets
    write_u64_at(&mut buf, 28, data_checksum);   // Data Checksum: offset 28
    write_u64_at(&mut buf, 36, header_checksum); // Header Checksum: offset 36
    
    buf
}

fn snapshot_writer_thread(
    shard_id: ShardId,
    snapshot_rx: Receiver<SnapshotRequest>,
    snapshot_dir: PathBuf,
) {
    loop {
        match snapshot_rx.recv() {
            Ok(request) => {
                let temp_path = snapshot_dir.join(format!("shard{}.snap.tmp", shard_id.0));
                let final_path = snapshot_dir.join(format!("shard{}.snap", shard_id.0));
                
                // Write to temp file
                match write_snapshot_file(&temp_path, &request.data) {
                    Ok(()) => {
                        // Atomic rename
                        if let Err(e) = fs::rename(&temp_path, &final_path) {
                            log::error!("Failed to rename snapshot: {}", e);
                        } else {
                            log::info!("Snapshot written: shard {}", shard_id.0);
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to write snapshot: {}", e);
                    }
                }
            }
            Err(_) => {
                // Channel closed, shutdown
                break;
            }
        }
    }
}

fn write_snapshot_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(data)?;
    file.sync_all()?;  // fsync
    Ok(())
}
```

---

### Copy-on-Write Semantics

**Problem**: Serializing shard data blocks shard thread.

**Solution**: Serialize in shard thread (fast, ~1ms for 1M keys), then send owned data to writer thread.

**Why Not True COW?**:
- Rust HashMap does not support COW (no built-in fork/clone)
- Cloning HashMap is expensive (O(n))
- Serialization is fast enough (~1ms)

**Blocking Time**: ~1ms per snapshot (acceptable).

---

### Atomic Rename

**Problem**: Crash during snapshot write leaves corrupt file.

**Solution**: Write to temp file, then atomic rename.

```rust
// Write to temp file
write_snapshot_file("shard0.snap.tmp", data)?;

// Atomic rename (POSIX guarantees atomicity)
fs::rename("shard0.snap.tmp", "shard0.snap")?;
```

**Guarantee**: Snapshot file is either old (complete) or new (complete), never partial.

---

## Snapshot Read Procedure

### Startup Sequence

```rust
fn load_snapshots(snapshot_dir: &Path, shard_count: usize) -> Vec<Shard> {
    let mut shards = Vec::with_capacity(shard_count);
    
    for shard_id in 0..shard_count {
        let snapshot_path = snapshot_dir.join(format!("shard{}.snap", shard_id));
        
        let shard = if snapshot_path.exists() {
            match load_snapshot(&snapshot_path) {
                Ok(shard) => {
                    log::info!("Loaded snapshot for shard {}: {} keys", shard_id, shard.data.len());
                    shard
                }
                Err(e) => {
                    log::error!("Failed to load snapshot for shard {}: {}", shard_id, e);
                    log::warn!("Starting shard {} with empty state", shard_id);
                    Shard::new(ShardId(shard_id as u16))
                }
            }
        } else {
            log::info!("No snapshot found for shard {}, starting empty", shard_id);
            Shard::new(ShardId(shard_id as u16))
        };
        
        shards.push(shard);
    }
    
    shards
}

fn load_snapshot(path: &Path) -> Result<Shard, Error> {
    let data = fs::read(path)?;
    
    if data.len() < 64 {
        return Err(Error::InvalidSnapshot("File too small"));
    }
    
    // Parse header
    let magic = &data[0..4];
    if magic != b"RSDF" {
        return Err(Error::InvalidSnapshot("Invalid magic number"));
    }
    
    let version = read_u32(&data, 4);
    if version != 1 {
        return Err(Error::InvalidSnapshot("Unsupported version"));
    }
    
    let shard_id = read_u16(&data, 8);
    let key_count = read_u64(&data, 12);
    let timestamp = read_u64(&data, 20);
    let data_checksum = read_u64(&data, 28);
    let header_checksum = read_u64(&data, 36);
    
    // Verify header checksum: covers bytes 0-35 (all fields before offset 36)
    let computed_header_checksum = crc64(&data[0..36]);
    if header_checksum != computed_header_checksum {
        return Err(Error::CorruptSnapshot("Header checksum mismatch"));
    }
    
    // Verify data checksum
    let computed_data_checksum = crc64(&data[64..]);
    if data_checksum != computed_data_checksum {
        return Err(Error::CorruptSnapshot("Data checksum mismatch"));
    }
    
    // Parse entries
    let mut shard = Shard::new(ShardId(shard_id));
    let mut cursor = 64;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    
    for _ in 0..key_count {
        let key_len = read_u32(&data, cursor) as usize;
        cursor += 4;
        let key = data[cursor..cursor + key_len].to_vec();
        cursor += key_len;
        
        let value_len = read_u32(&data, cursor) as usize;
        cursor += 4;
        let value = data[cursor..cursor + value_len].to_vec();
        cursor += value_len;
        
        let has_expiry = data[cursor] == 1;
        cursor += 1;
        
        let expiry = if has_expiry {
            let expiry_timestamp = read_u64(&data, cursor);
            cursor += 8;
            
            // Skip expired keys
            if expiry_timestamp <= now {
                continue;
            }
            
            Some(unix_timestamp_to_instant(expiry_timestamp))
        } else {
            None
        };
        
        shard.set(key, Value::String(value), expiry);
    }
    
    log::info!("Loaded shard {}: {} keys (snapshot timestamp: {})", shard_id, shard.data.len(), timestamp);
    
    Ok(shard)
}
```

---

### Error Handling

**Corrupt Snapshot**:
- Checksum mismatch → Reject snapshot, start with empty shard
- Invalid format → Reject snapshot, start with empty shard
- Partial file → Reject snapshot, start with empty shard

**Missing Snapshot**:
- No snapshot file → Start with empty shard (normal for first run)

**Expired Keys**:
- Keys with TTL < now → Skip during load (not inserted into shard)

---

## Crash Recovery

### Recovery Scenarios

**Scenario 1: Clean Shutdown (SIGTERM)**
```
1. Receive SIGTERM
2. Trigger final snapshot for all shards
3. Wait for snapshot completion (timeout: 30s)
4. Exit

Recovery:
- Load latest snapshot (includes all data up to shutdown)
- Data loss: 0 (final snapshot captured everything)
```

---

**Scenario 2: Unclean Shutdown (SIGKILL, crash)**
```
1. Process killed (no final snapshot)

Recovery:
- Load latest periodic snapshot
- Data loss: Up to 60s (since last snapshot)
```

---

**Scenario 3: Corrupt Snapshot**
```
1. Snapshot file is corrupt (checksum mismatch)

Recovery:
- Reject corrupt snapshot
- Start shard with empty state
- Data loss: All data for that shard
```

---

**Scenario 4: Disk Full During Snapshot**
```
1. Snapshot write fails (disk full)
2. Temp file is incomplete
3. Rename fails (temp file does not exist or is corrupt)

Recovery:
- Old snapshot remains intact (atomic rename not executed)
- Load old snapshot
- Data loss: Since last successful snapshot
```

---

### Recovery Time

**Factors**:
- Snapshot file size (depends on key count and value sizes)
- Disk read speed (SSD: ~500 MB/s, HDD: ~100 MB/s)
- Deserialization overhead (~100 MB/s)

**Estimate**:
```
Snapshot size: 10 GB (1M keys × 10 KB average)
Disk read: 10 GB / 500 MB/s = 20s
Deserialization: 10 GB / 100 MB/s = 100s
Total: ~120s per shard

Parallel recovery (64 shards): ~120s (limited by disk bandwidth)
```

**Optimization** (future):
- Compress snapshots (reduce disk read time)
- Parallel deserialization (multi-threaded)

---

## Snapshot Management

### Snapshot Retention

**MVP**: Keep only latest snapshot per shard.

**Rationale**: Simplicity (no backup/restore logic).

**Future**: Keep N snapshots for point-in-time recovery.

---

### Snapshot Cleanup

**Old Snapshots**: Deleted on successful new snapshot.

```rust
fn write_snapshot_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let temp_path = path.with_extension("tmp");
    
    // Write to temp file
    let mut file = File::create(&temp_path)?;
    file.write_all(data)?;
    file.sync_all()?;
    
    // Atomic rename (overwrites old snapshot)
    fs::rename(&temp_path, path)?;
    
    Ok(())
}
```

**Temp Files**: Cleaned up on startup.

```rust
fn cleanup_temp_files(snapshot_dir: &Path) {
    for entry in fs::read_dir(snapshot_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension() == Some(OsStr::new("tmp")) {
            log::info!("Removing stale temp file: {:?}", path);
            fs::remove_file(path).ok();
        }
    }
}
```

---

### Snapshot Compression (Future)

**Algorithm**: LZ4 (fast compression, ~500 MB/s).

**Trade-Off**:
- Pros: Smaller files, faster disk IO
- Cons: CPU overhead, slower deserialization

**Estimate**:
- Compression ratio: 3x (typical for key-value data)
- Snapshot size: 10 GB → 3.3 GB
- Disk read time: 20s → 6.6s
- Decompression time: 3.3 GB / 500 MB/s = 6.6s
- Total: ~13s (vs 120s uncompressed)

**Decision**: Not in MVP (complexity vs benefit).

---

## Monitoring and Observability

### Metrics

**Per-Shard Metrics**:
```
shard_snapshot_writes_total{shard="0"}
shard_snapshot_write_duration_seconds{shard="0"}
shard_snapshot_write_errors_total{shard="0"}
shard_snapshot_skipped_total{shard="0"}
shard_snapshot_size_bytes{shard="0"}
```

**Global Metrics**:
```
snapshot_writes_total
snapshot_write_errors_total
snapshot_last_write_timestamp_seconds
```

**Alerting**:
- Alert if `snapshot_write_errors_total > 0`
- Alert if `snapshot_last_write_timestamp_seconds` is stale (> 120s)

---

### Logging

**Snapshot Write Success**:
```json
{
  "level": "info",
  "shard_id": 0,
  "event": "snapshot_written",
  "key_count": 1000000,
  "size_bytes": 10485760,
  "duration_ms": 1200
}
```

**Snapshot Write Failure**:
```json
{
  "level": "error",
  "shard_id": 0,
  "event": "snapshot_write_failed",
  "error": "Disk full",
  "path": "/var/lib/rsdragonfly/shard0.snap.tmp"
}
```

**Snapshot Load Success**:
```json
{
  "level": "info",
  "shard_id": 0,
  "event": "snapshot_loaded",
  "key_count": 1000000,
  "expired_keys": 5000,
  "duration_ms": 5000
}
```

**Snapshot Load Failure**:
```json
{
  "level": "error",
  "shard_id": 0,
  "event": "snapshot_load_failed",
  "error": "Checksum mismatch",
  "path": "/var/lib/rsdragonfly/shard0.snap"
}
```

---

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_snapshot_serialization() {
    let mut shard = Shard::new(ShardId(0));
    shard.set(b"key1".to_vec(), Value::String(b"value1".to_vec()), None);
    shard.set(b"key2".to_vec(), Value::String(b"value2".to_vec()), Some(Instant::now() + Duration::from_secs(3600)));
    
    let data = serialize_shard(&shard);
    let loaded_shard = deserialize_snapshot(&data).unwrap();
    
    assert_eq!(loaded_shard.data.len(), 2);
    assert_eq!(loaded_shard.get(b"key1"), Some(&Value::String(b"value1".to_vec())));
}

#[test]
fn test_snapshot_checksum_validation() {
    let mut shard = Shard::new(ShardId(0));
    shard.set(b"key".to_vec(), Value::String(b"value".to_vec()), None);
    
    let mut data = serialize_shard(&shard);
    
    // Corrupt data
    data[100] ^= 0xFF;
    
    let result = deserialize_snapshot(&data);
    assert!(result.is_err());
}
```

---

### Integration Tests

```rust
#[test]
fn test_crash_recovery() {
    let server = start_test_server();
    
    // Write data
    for i in 0..1000 {
        set_key(&server, &format!("key{}", i), "value");
    }
    
    // Wait for snapshot
    std::thread::sleep(Duration::from_secs(61));
    
    // Kill server (SIGKILL)
    kill_server(&server);
    
    // Restart server
    let server = start_test_server();
    
    // Verify data recovered
    for i in 0..1000 {
        assert_eq!(get_key(&server, &format!("key{}", i)), Some("value"));
    }
}
```

---

## Operational Runbook

### Snapshot Write Failures

**Symptom**: `snapshot_write_errors_total` metric increasing.

**Diagnosis**:
```bash
# Check disk space
df -h /var/lib/rsdragonfly

# Check snapshot logs
journalctl -u rsdragonfly | grep snapshot_write_failed
```

**Resolution**:
1. If disk full: Free up space or increase disk size
2. If permission error: Fix file permissions
3. If IO error: Check disk health (smartctl)

---

### Slow Snapshot Writes

**Symptom**: `shard_snapshot_write_duration_seconds` is high (> 10s).

**Diagnosis**:
```bash
# Check disk IO
iostat -x 1

# Check snapshot size
ls -lh /var/lib/rsdragonfly/*.snap
```

**Resolution**:
1. If disk is slow: Upgrade to SSD
2. If snapshot is large: Reduce key count or value sizes
3. If CPU is bottleneck: Enable compression (future)

---

## Definition of Done

Persistence is complete when:

1. Snapshot format is defined and documented
2. Snapshot write/read procedures are implemented and tested
3. Checksums detect corruption
4. Atomic rename ensures consistency
5. Crash recovery is tested (SIGKILL, corrupt snapshot, disk full)
6. Metrics and logging are in place
7. Operational runbook covers common issues
8. Performance meets targets (< 2ms write, < 120s recovery)

---

## References

- [data-model.md](../engine/data-model.md) - In-memory data structures
- [ttl-expiration.md](../engine/ttl-expiration.md) - TTL in snapshots
- [overview.md](../architecture/overview.md) - System architecture
