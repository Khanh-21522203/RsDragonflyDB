use std::fs;
use std::path::PathBuf;
use crate::common::shard_id::ShardId;
use super::deserialize::{deserialize_snapshot, SnapshotError};
use super::super::shard::Shard;

pub fn load_snapshot(snapshot_dir: &PathBuf, shard_id: ShardId) -> Result<Shard, SnapshotError> {
    let snapshot_path = snapshot_dir.join(format!("shard{}.snap", shard_id.0));

    if !snapshot_path.exists() {
        log::info!("No snapshot found for shard {}, starting empty", shard_id.0);
        return Ok(Shard::new(shard_id));
    }

    let data = fs::read(&snapshot_path)?;
    deserialize_snapshot(&data)
}