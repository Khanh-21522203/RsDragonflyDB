use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};
use common::channels::{SnapshotRequest, SnapshotSender};
use common::constants::{SNAPSHOT_INTERVAL_SECS, TTL_CHECK_INTERVAL_MS};
use common::shard_id::ShardId;
use protocol::channels::CommandReceiver;
use crate::shard::Shard;
use crate::snapshot::serialize::serialize_shard;

pub async fn run_shard_worker(
    shard_id: ShardId,
    mut command_rx: CommandReceiver,
    snapshot_tx: SnapshotSender,
) {
    let mut shard = Shard::new(shard_id);

    log::info!("Shard {} ready with {} keys", shard_id.0, shard.data.len());

    let mut ttl_ticker = interval(Duration::from_millis(TTL_CHECK_INTERVAL_MS));
    let mut snapshot_ticker = interval(Duration::from_secs(SNAPSHOT_INTERVAL_SECS));

    ttl_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    snapshot_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            cmd_opt = command_rx.recv() => {
                match cmd_opt {
                    Some(cmd_msg) => {
                        let response = shard.execute(cmd_msg.command);
                        let _ = cmd_msg.reply_tx.send(response);
                    }
                    None => {
                        log::info!("Shard {} shutting down (channel closed)", shard_id.0);
                        break;
                    }
                }
            }

            _ = ttl_ticker.tick() => {
                shard.expire_keys();
            }

            _ = snapshot_ticker.tick() => {
                trigger_snapshot(&mut shard, &snapshot_tx).await;
            }
        }
    }

    log::info!("Performing final snapshot for shard {}", shard_id.0);
    trigger_snapshot(&mut shard, &snapshot_tx).await;
}

async fn trigger_snapshot(shard: &mut Shard, snapshot_tx: &SnapshotSender) {
    let snapshot_data = serialize_shard(shard);

    let req = SnapshotRequest {
        shard_id: shard.id,
        data: snapshot_data,
    };

    match snapshot_tx.try_send(req) {
        Ok(()) => {
            log::debug!("Snapshot triggered for shard {}", shard.id.0);
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            log::warn!("Snapshot writer busy, skipping snapshot for shard {}", shard.id.0);
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            log::error!("Snapshot writer disconnected for shard {}", shard.id.0);
        }
    }
}