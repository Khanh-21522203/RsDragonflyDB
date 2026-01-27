use tokio::sync::mpsc;
use crate::constants::SNAPSHOT_CHANNEL_CAPACITY;
use crate::shard_id::ShardId;

// Shard → Snapshot Writer: Bounded SPSC
pub type SnapshotSender = mpsc::Sender<SnapshotRequest>;
pub type SnapshotReceiver = mpsc::Receiver<SnapshotRequest>;

pub fn snapshot_channel() -> (SnapshotSender, SnapshotReceiver) {
    mpsc::channel(SNAPSHOT_CHANNEL_CAPACITY)
}

pub struct SnapshotRequest {
    pub shard_id: ShardId,
    pub data: Vec<u8>,
}