use std::collections::HashMap;
use common::constants::SHARD_COUNT;
use common::hash::crc16;
use common::shard_id::ShardId;

pub struct Router;

impl Router {
    pub fn route_key(key: &[u8]) -> ShardId {
        let hash = crc16(key);
        let shard_id = (hash as usize) & (SHARD_COUNT - 1);
        ShardId(shard_id as u16)
    }

    pub fn route_keys(keys: &[Vec<u8>]) -> HashMap<ShardId, Vec<Vec<u8>>> {
        let mut grouped: HashMap<ShardId, Vec<Vec<u8>>> = HashMap::new();

        for key in keys {
            let shard_id = Self::route_key(key);
            grouped.entry(shard_id)
                .or_insert_with(Vec::new)
                .push(key.clone());
        }

        grouped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_key_deterministic() {
        let key = b"mykey";
        let shard1 = Router::route_key(key);
        let shard2 = Router::route_key(key);
        assert_eq!(shard1, shard2);
    }

    #[test]
    fn test_route_keys_grouping() {
        let keys = vec![
            b"key1".to_vec(),
            b"key2".to_vec(),
            b"key3".to_vec(),
        ];

        let grouped = Router::route_keys(&keys);

        // Verify all keys are grouped
        let total: usize = grouped.values().map(|v| v.len()).sum();
        assert_eq!(total, 3);
    }
}