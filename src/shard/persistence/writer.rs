use std::fs::{self, File};
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;
use crate::common::channels::SnapshotReceiver;
use crate::common::shard_id::ShardId;
use tokio::task;

pub async  fn run_snapshot_writer(
    shard_id: ShardId,
    mut snapshot_rx: SnapshotReceiver,
    snapshot_dir: PathBuf,
) {
    log::info!("Snapshot writer for shard {} started", shard_id.0);

    while let Some(request) = snapshot_rx.recv().await {
        let dir = snapshot_dir.clone();

        let write_result = task::spawn_blocking(move || {
            let start = Instant::now();
            let temp_path = dir.join(format!("shard{}.snap.tmp", request.shard_id.0));
            let final_path = dir.join(format!("shard{}.snap", request.shard_id.0));

            if let Err(e) = write_snapshot_file(&temp_path, &request.data) {
                log::error!("Failed to write snapshot temp file shard {}: {}", request.shard_id.0, e);
                return;
            }

            // Atomic rename
            match fs::rename(&temp_path, &final_path) {
                Ok(()) => {
                    let duration = start.elapsed();
                    log::info!(
                        "Snapshot saved: shard {} ({} bytes, {:.2}ms)",
                        request.shard_id.0,
                        request.data.len(),
                        duration.as_secs_f64() * 1000.0
                    );
                }
                Err(e) => {
                    log::error!("Failed to rename snapshot shard {}: {}", request.shard_id.0, e);
                }
            }
        }).await;

        if let Err(e) = write_result {
            log::error!("Snapshot writer task panicked: {}", e);
        }
    }

    log::info!("Snapshot writer for shard {} shutting down", shard_id.0);
}

fn write_snapshot_file(path: &PathBuf, data: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(data)?;
    file.sync_all()?;  // fsync
    Ok(())
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