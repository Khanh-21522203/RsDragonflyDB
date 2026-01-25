use shard::shard::ShardMetrics;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

pub struct GlobalMetrics {
    pub uptime_start: Instant,
    pub connections_total: AtomicU64,
    pub connections_active: AtomicUsize,
    pub commands_total: AtomicU64,
    pub command_errors_total: AtomicU64,
}

impl GlobalMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(GlobalMetrics {
            uptime_start: Instant::now(),
            connections_total: AtomicU64::new(0),
            connections_active: AtomicUsize::new(0),
            commands_total: AtomicU64::new(0),
            command_errors_total: AtomicU64::new(0),
        })
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.uptime_start.elapsed().as_secs()
    }
}

pub fn aggregate_shard_metrics(shards: &[Arc<ShardMetrics>]) -> AggregatedMetrics {
    let mut total_commands = 0;
    let mut total_keys = 0;
    let mut total_memory = 0;
    let mut total_expired = 0;

    for shard in shards {
        total_commands += shard.commands_processed.load(Ordering::Relaxed);
        total_keys += shard.key_count.load(Ordering::Relaxed);
        total_memory += shard.memory_bytes.load(Ordering::Relaxed);
        total_expired += shard.keys_expired.load(Ordering::Relaxed);
    }

    AggregatedMetrics {
        total_commands,
        total_keys,
        total_memory,
        total_expired,
    }
}

pub struct AggregatedMetrics {
    pub total_commands: u64,
    pub total_keys: usize,
    pub total_memory: usize,
    pub total_expired: u64,
}