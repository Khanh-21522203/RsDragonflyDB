use crossbeam::channel::{self, Sender, Receiver};
use tokio::sync::oneshot;
use crate::command::Command;
use crate::constants::SNAPSHOT_CHANNEL_CAPACITY;
use crate::shard_id::ShardId;
use crate::types::Value;

// Frontend → Shard: Unbounded MPSC
pub type CommandSender = Sender<CommandMessage>;
pub type CommandReceiver = Receiver<CommandMessage>;

pub fn command_channel() -> (CommandSender, CommandReceiver) {
    channel::unbounded()
}

// Shard → Frontend: Oneshot
pub type ResponseSender = oneshot::Sender<Response>;
pub type ResponseReceiver = oneshot::Receiver<Response>;

pub fn response_channel() -> (ResponseSender, ResponseReceiver) {
    oneshot::channel()
}

// Shard → Snapshot Writer: Bounded SPSC
pub type SnapshotSender = Sender<SnapshotRequest>;
pub type SnapshotReceiver = Receiver<SnapshotRequest>;

pub fn snapshot_channel() -> (SnapshotSender, SnapshotReceiver) {
    channel::bounded(SNAPSHOT_CHANNEL_CAPACITY)
}

pub struct CommandMessage {
    pub command: Command,
    pub reply_tx: ResponseSender,
}

pub struct SnapshotRequest {
    pub shard_id: ShardId,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub enum Response {
    Value(Option<Value>),
    Integer(i64),
    Ok,
    Error(String),
}