use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;
use crate::common::types::Key;
use crate::protocol::channels::{CommandMessage, CommandSender};
use crate::protocol::command::Command;
use crate::protocol::response::Response;
use super::error_handling::MultiKeyResult;
use super::router::Router;


const SHARD_TIMEOUT: Duration = Duration::from_millis(500);
const SEND_TIMEOUT: Duration = Duration::from_millis(100);

pub struct MultiKeyExecutor {
    shard_channels: Vec<CommandSender>,
}

impl MultiKeyExecutor {
    pub fn new(shard_channels: Vec<CommandSender>) -> Self {
        MultiKeyExecutor { shard_channels }
    }

    pub async fn execute_del(&self, keys: Vec<Key>) -> MultiKeyResult {
        let grouped = Router::route_keys(&keys);

        if grouped.is_empty() {
            return MultiKeyResult::Success(0);
        }

        let mut futures = Vec::new();

        // Send to all shards in parallel
        for (shard_id, shard_keys) in grouped {
            let cmd = Command::Del { keys: shard_keys };
            let sender = self.shard_channels[shard_id.as_usize()].clone();

            futures.push(async move {
                let (reply_tx, reply_rx) = oneshot::channel();
                let msg = CommandMessage { command: cmd, reply_tx };

                if let Err(_) = timeout(SEND_TIMEOUT, sender.send(msg)).await {
                    return (shard_id, Err("Shard overloaded (Send timeout)".to_string()));
                }

                match timeout(SHARD_TIMEOUT, reply_rx).await {
                    Ok(Ok(Response::Integer(n))) => (shard_id, Ok(n)),
                    Ok(Ok(Response::Error(e))) => (shard_id, Err(e)),
                    Ok(Err(_)) => (shard_id, Err("Shard disconnected".to_string())),
                    Err(_) => (shard_id, Err("Shard response timeout".to_string())),
                    _ => (shard_id, Err("Unexpected response type".to_string())),
                }
            });
        }

        let results = futures::future::join_all(futures).await;

        // Aggregate responses
        let mut total_deleted = 0;
        let mut failed_shards = Vec::new();
        let mut errors = Vec::new();

        for (shard_id, res) in results {
            match res {
                Ok(n) => total_deleted += n,
                Err(e) => {
                    failed_shards.push(shard_id);
                    errors.push(format!("Shard {}: {}", shard_id.0, e));
                }
            }
        }

        if errors.is_empty() {
            MultiKeyResult::Success(total_deleted)
        } else if total_deleted > 0 {
            MultiKeyResult::PartialFailure {
                succeeded: total_deleted,
                failed_shards,
                errors,
            }
        } else {
            MultiKeyResult::TotalFailure(errors.join("; "))
        }
    }
}