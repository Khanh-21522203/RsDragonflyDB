pub const SHARD_COUNT: usize = 64;

pub const FRONTEND_THREADS: usize = 4;

// Snapshot configuration
pub const SNAPSHOT_INTERVAL_SECS: u64 = 60;

// TTL configuration
pub const TTL_CHECK_INTERVAL_MS: u64 = 100;
pub const TTL_KEYS_PER_CHECK: usize = 20;

// Memory limits
pub const MAX_KEY_SIZE: usize = 512;
pub const MAX_VALUE_SIZE: usize = 512 * 1024 * 1024;  // 512MB
pub const MAX_KEYS_PER_SHARD: usize = 100_000_000;    // 100M
pub const MAX_MEMORY_PER_SHARD: usize = 20 * 1024 * 1024 * 1024;  // 20GB

// Channel configuration
pub const SNAPSHOT_CHANNEL_CAPACITY: usize = 1;

// Shutdown timeouts
pub const DRAIN_TIMEOUT_SECS: u64 = 10;
pub const SNAPSHOT_TIMEOUT_SECS: u64 = 30;