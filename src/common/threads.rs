use std::thread::{self, JoinHandle};
use super::shard_id::ShardId;

// TODO: remove
pub enum ThreadType {
    Acceptor,
    ConnectionHandler(usize),
    ShardWorker(ShardId),
    SnapshotWriter(ShardId),
    MetricsExporter,
}

impl ThreadType {
    pub fn name(&self) -> String {
        match self {
            ThreadType::Acceptor => "acceptor".to_string(),
            ThreadType::ConnectionHandler(id) => format!("conn-handler-{}", id),
            ThreadType::ShardWorker(shard_id) => format!("shard-{}", shard_id.0),
            ThreadType::SnapshotWriter(shard_id) => format!("snapshot-{}", shard_id.0),
            ThreadType::MetricsExporter => "metrics".to_string(),
        }
    }
}

pub fn spawn_thread<F, T>(thread_type: ThreadType, f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    thread::Builder::new()
        .name(thread_type.name())
        .spawn(f)
        .expect("Failed to spawn thread")
}