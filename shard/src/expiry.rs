use std::cmp::Ordering;
use std::time::Instant;
use common::types::Key;

#[derive(Debug, Clone)]
pub struct Expiry {
    pub expiry_time: Instant,
    pub key: Key,
}

// Min-heap: earliest expiry first
impl Ord for Expiry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap
        other.expiry_time.cmp(&self.expiry_time)
    }
}

impl PartialOrd for Expiry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Expiry {
    fn eq(&self, other: &Self) -> bool {
        self.expiry_time == other.expiry_time
    }
}

impl Eq for Expiry {}