use tokio::sync::{mpsc, oneshot};
use crate::command::Command;
use crate::response::Response;

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

pub struct CommandMessage {
    pub command: Command,
    pub reply_tx: ResponseSender,
}