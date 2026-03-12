use crate::common::shard_id::ShardId;
use serde_json::json;

pub fn log_command_executed(
    shard_id: ShardId,
    command: &str,
    latency_us: u64,
    result: &str,
) {
    log::debug!("{}", json!({
        "event": "command_executed",
        "shard_id": shard_id.0,
        "command": command,
        "latency_us": latency_us,
        "result": result,
    }));
}

pub fn log_snapshot_written(
    shard_id: ShardId,
    key_count: usize,
    size_bytes: usize,
    duration_ms: f64,
) {
    log::info!("{}", json!({
        "event": "snapshot_written",
        "shard_id": shard_id.0,
        "key_count": key_count,
        "size_bytes": size_bytes,
        "duration_ms": duration_ms,
    }));
}

pub fn log_snapshot_load_failed(
    shard_id: ShardId,
    error: &str,
    path: &str,
) {
    log::error!("{}", json!({
        "event": "snapshot_load_failed",
        "shard_id": shard_id.0,
        "error": error,
        "path": path,
    }));
}

pub fn log_shard_panic(
    shard_id: ShardId,
    error: &str,
) {
    log::error!("{}", json!({
        "event": "shard_panic",
        "shard_id": shard_id.0,
        "error": error,
    }));
}