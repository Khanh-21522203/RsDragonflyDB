use std::fs::{self, File};
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;
use common::channels::SnapshotReceiver;
use common::shard_id::ShardId;
use shard::shard::Shard;
use crate::deserialize::{deserialize_snapshot, SnapshotError};

pub fn snapshot_writer_thread(
    shard_id: ShardId,
    snapshot_rx: SnapshotReceiver,
    snapshot_dir: PathBuf,
) {
    log::info!("Snapshot writer for shard {} started", shard_id.0);

    loop {
        match snapshot_rx.recv() {
            Ok(request) => {
                let start = Instant::now();

                let temp_path = snapshot_dir.join(format!("shard{}.snap.tmp", shard_id.0));
                let final_path = snapshot_dir.join(format!("shard{}.snap", shard_id.0));

                match write_snapshot_file(&temp_path, &request.data) {
                    Ok(()) => {
                        // Atomic rename
                        match fs::rename(&temp_path, &final_path) {
                            Ok(()) => {
                                let duration = start.elapsed();
                                log::info!("Snapshot written: shard {} ({} bytes, {:.2}ms)",
                                    shard_id.0, request.data.len(), duration.as_secs_f64() * 1000.0);
                            }
                            Err(e) => {
                                log::error!("Failed to rename snapshot for shard {}: {}", shard_id.0, e);
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to write snapshot for shard {}: {}", shard_id.0, e);
                    }
                }
            }
            Err(_) => {
                // Channel closed, shutdown
                log::info!("Snapshot writer for shard {} shutting down", shard_id.0);
                break;
            }
        }
    }
}

fn write_snapshot_file(path: &PathBuf, data: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(data)?;
    file.sync_all()?;  // fsync
    Ok(())
}

pub fn load_snapshot(snapshot_dir: &PathBuf, shard_id: ShardId) -> Result<Shard, SnapshotError> {
    let snapshot_path = snapshot_dir.join(format!("shard{}.snap", shard_id.0));

    if !snapshot_path.exists() {
        log::info!("No snapshot found for shard {}, starting empty", shard_id.0);
        return Ok(Shard::new(shard_id));
    }

    let data = fs::read(&snapshot_path)?;
    deserialize_snapshot(&data)
}

pub fn cleanup_temp_files(snapshot_dir: &PathBuf) {
    if let Ok(entries) = fs::read_dir(snapshot_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("tmp") {
                log::info!("Removing stale temp file: {:?}", path);
                let _ = fs::remove_file(path);
            }
        }
    }
}