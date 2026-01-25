use std::collections::HashMap;
use crossbeam::channel::{self, Sender, Receiver};
use common::channels::{response_channel, CommandMessage, CommandSender, Response};
use common::types::Key;
use common::command::Command;
use crate::router::Router;

pub struct MultiKeyExecutor {
    shard_channels: Vec<CommandSender>,
}

impl MultiKeyExecutor {
    pub fn new(shard_channels: Vec<CommandSender>) -> Self {
        MultiKeyExecutor { shard_channels }
    }

    pub fn execute_del(&self, keys: Vec<Key>) -> Response {
        // Group keys by shard
        let grouped = Router::route_keys(&keys);
        let shard_count = grouped.len();

        if shard_count == 0 {
            return Response::Integer(0);
        }

        // Create aggregation channel
        let (agg_tx, agg_rx) = channel::bounded(shard_count);

        // Send to all shards in parallel
        for (shard_id, shard_keys) in grouped {
            let (reply_tx, reply_rx) = response_channel();

            let msg = CommandMessage {
                command: Command::Del { keys: shard_keys },
                reply_tx,
            };

            if let Err(e) = self.shard_channels[shard_id.as_usize()].send(msg) {
                log::error!("Failed to send to shard {}: {}", shard_id.0, e);
                let _ = agg_tx.send(Response::Error(format!("ERR shard {} unavailable", shard_id.0)));
                continue;
            }

            // Forward response to aggregator
            let agg_tx_clone = agg_tx.clone();
            std::thread::spawn(move || {
                match reply_rx.blocking_recv() {
                    Ok(response) => {
                        let _ = agg_tx_clone.send(response);
                    }
                    Err(_) => {
                        let _ = agg_tx_clone.send(Response::Error("ERR timeout".to_string()));
                    }
                }
            });
        }

        drop(agg_tx);

        // Aggregate responses
        let mut total_deleted = 0;
        let mut errors = Vec::new();

        for response in agg_rx {
            match response {
                Response::Integer(n) => total_deleted += n,
                Response::Error(e) => errors.push(e),
                _ => {}
            }
        }

        if errors.is_empty() {
            Response::Integer(total_deleted)
        } else {
            Response::Error(format!("ERR partial failure: deleted {} keys, errors: {}",
                                    total_deleted, errors.join(", ")))
        }
    }
}