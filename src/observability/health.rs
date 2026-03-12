use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub fn health_check_handler(shard_status: &[AtomicU8]) -> String {
    for (i, status) in shard_status.iter().enumerate() {
        if status.load(Ordering::Relaxed) == SHARD_FAILED {
            return format!("ERR shard {} failed", i);
        }
    }
    "OK".to_string()
}

pub fn readiness_check_handler(
    snapshots_loaded: &AtomicBool,
    shard_status: &[AtomicU8],
) -> String {
    if !snapshots_loaded.load(Ordering::Relaxed) {
        return "ERR loading snapshots".to_string();
    }

    for (i, status) in shard_status.iter().enumerate() {
        if status.load(Ordering::Relaxed) != SHARD_READY {
            return format!("ERR shard {} not ready", i);
        }
    }

    "READY".to_string()
}

pub const SHARD_INITIALIZING: u8 = 0;
pub const SHARD_READY: u8 = 1;
pub const SHARD_FAILED: u8 = 2;