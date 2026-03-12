use std::sync::atomic::Ordering;
use std::time::Instant;
use crate::common::constants::TTL_KEYS_PER_CHECK;
use super::shard::Shard;

impl Shard {
    pub fn expire_keys(&mut self) {
        let now = Instant::now();
        let mut expired_count = 0;

        // Pop expired keys from TTL queue
        while let Some(expiry) = self.ttl_queue.peek() {
            if expiry.expiry_time > now {
                break;  // No more expired keys
            }

            let expiry = self.ttl_queue.pop().unwrap();

            // Verify key still exists and has same expiry
            if let Some(entry) = self.data.get(&expiry.key) {
                if entry.expiry == Some(expiry.expiry_time) {
                    // Key exists with matching expiry, delete it
                    self.data.remove(&expiry.key);
                    expired_count += 1;

                    if expired_count >= TTL_KEYS_PER_CHECK {
                        break;  // Limit work per iteration
                    }
                }
                // Else: expiry was updated, ignore stale entry
            }
            // Else: key already deleted, ignore stale entry
        }

        if expired_count > 0 {
            self.metrics.keys_expired.fetch_add(expired_count as u64, Ordering::Relaxed);
            self.metrics.key_count.store(self.data.len(), Ordering::Relaxed);
            log::debug!("Shard {} expired {} keys", self.id.0, expired_count);
        }
    }
}