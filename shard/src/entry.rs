use std::time::Instant;
use common::types::Value;

pub struct Entry {
    pub value: Value,
    pub expiry: Option<Instant>,
    // TODO: Metadata for future features (LRU, LFU)
    pub metadata: Metadata,
}

pub struct Metadata {
    pub created_at: Instant,
    pub last_accessed: Instant,  // For LRU eviction (future)
    pub access_count: u64,        // For LFU eviction (future)
}

impl Entry {
    pub fn new(value: Value, expiry: Option<Instant>) -> Self {
        let now = Instant::now();
        Entry {
            value,
            expiry,
            metadata: Metadata {
                created_at: now,
                last_accessed: now,
                access_count: 0,
            },
        }
    }

    pub fn is_expired(&self) -> bool {
        self.expiry.map_or(false, |exp| Instant::now() >= exp)
    }

    pub fn size_bytes(&self) -> usize {
        let base = std::mem::size_of::<Entry>();
        let value_size = self.value.size_bytes();
        base + value_size
    }
}