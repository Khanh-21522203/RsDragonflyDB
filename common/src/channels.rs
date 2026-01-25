use tokio::sync::{mpsc, oneshot};
use crate::command::Command;
use crate::constants::SNAPSHOT_CHANNEL_CAPACITY;
use crate::shard_id::ShardId;
use crate::types::Value;

// Frontend → Shard: MPSC
pub type CommandSender = mpsc::Sender<CommandMessage>;
pub type CommandReceiver = mpsc::Receiver<CommandMessage>;

// TODO: add 1024 to config
pub fn command_channel() -> (CommandSender, CommandReceiver) {
    mpsc::channel(1024)
}
// Shard → Frontend: Oneshot
pub type ResponseSender = oneshot::Sender<Response>;
pub type ResponseReceiver = oneshot::Receiver<Response>;

pub fn response_channel() -> (ResponseSender, ResponseReceiver) {
    oneshot::channel()
}

// Shard → Snapshot Writer: Bounded SPSC
pub type SnapshotSender = mpsc::Sender<SnapshotRequest>;
pub type SnapshotReceiver = mpsc::Receiver<SnapshotRequest>;

pub fn snapshot_channel() -> (SnapshotSender, SnapshotReceiver) {
    mpsc::channel(SNAPSHOT_CHANNEL_CAPACITY)
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