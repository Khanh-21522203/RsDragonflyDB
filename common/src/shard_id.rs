use crate::constants::SHARD_COUNT;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShardId(pub u16);

impl ShardId {
    pub fn new(id: u16) -> Self {
        assert!(id < SHARD_COUNT as u16, "Invalid shard ID");
        ShardId(id)
    }

    pub fn as_usize(&self) -> usize {
        self.0 as usize
    }
}