use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::time::sleep;
use common::channels::{snapshot_channel};
use common::constants::{DRAIN_TIMEOUT_SECS, SHARD_COUNT};
use common::shard_id::ShardId;
use frontend::handler::ConnectionHandler;
use observability::metrics::GlobalMetrics;
use observability::prometheus::{run_metrics_exporter};
use protocol::channels::{command_channel, CommandSender};
use shard::persistence::writer::{cleanup_temp_files, run_snapshot_writer};
use shard::shard::ShardMetrics;
use shard::worker::run_shard_worker;
use crate::Args;

pub struct Server {
    config: Config,
    shard_channels: Vec<CommandSender>,
    global_metrics: Arc<GlobalMetrics>,
    shard_metrics: Vec<Arc<ShardMetrics>>,
}

impl Server {
    pub async fn new(args: Args) -> Self {
        let config = Config::from_args(args);

        // Create snapshot directory
        fs::create_dir_all(&config.snapshot_dir)
            .expect("Failed to create snapshot directory");
        cleanup_temp_files(&config.snapshot_dir);

        // Initialize metrics
        let global_metrics = GlobalMetrics::new();
        let mut shard_metrics = Vec::new();
        for _ in 0..SHARD_COUNT {
            shard_metrics.push(Arc::new(ShardMetrics::new()));
        }

        // Create channels
        let mut shard_channels = Vec::new();
        let mut command_receivers = Vec::new();
        for _ in 0..SHARD_COUNT {
            let (tx, rx) = command_channel();
            shard_channels.push(tx);
            command_receivers.push(rx);
        }

        let mut snapshot_senders = Vec::new();
        let mut snapshot_receivers = Vec::new();
        for _ in 0..SHARD_COUNT {
            let (tx, rx) = snapshot_channel();
            snapshot_senders.push(tx);
            snapshot_receivers.push(rx);
        }

        // Spawn Snapshot Writers
        for shard_id in 0..SHARD_COUNT {
            let shard_id = ShardId(shard_id as u16);
            let snapshot_rx = snapshot_receivers.remove(0);
            let snapshot_dir = config.snapshot_dir.clone();

            tokio::spawn(async move {
                run_snapshot_writer(shard_id, snapshot_rx, snapshot_dir).await;
            });
        }

        // Spawn Shard Workers
        for shard_id in 0..SHARD_COUNT {
            let shard_id = ShardId(shard_id as u16);
            let command_rx = command_receivers.remove(0);
            let snapshot_tx = snapshot_senders[shard_id.as_usize()].clone();

            tokio::spawn(async move {
                run_shard_worker(shard_id, command_rx, snapshot_tx).await;
            });
        }

        // Wait for shards to be ready
        sleep(Duration::from_millis(100)).await;

        Server {
            config,
            shard_channels,
            global_metrics,
            shard_metrics,
        }
    }

    pub async fn run(self) -> std::io::Result<()> {
        let addr = format!("{}:{}", self.config.bind, self.config.port);
        log::info!("Server starting on {}", addr);

        // 1. Start Metrics Exporter (Background)
        let metrics_port = 9090;
        let g_metric = self.global_metrics.clone();
        let s_metrics = self.shard_metrics.clone();

        tokio::spawn(async move {
            // Nếu run_metrics_exporter chưa async, bọc nó lại:
            // tokio::task::spawn_blocking(move || run_metrics_exporter(...));
            // Nếu đã async thì gọi trực tiếp:
            if let Err(e) = run_metrics_exporter(metrics_port, g_metric, s_metrics).await {
                log::error!("Metrics exporter failed: {}", e);
            }
        });

        // 2. Start TCP Listener & Handler
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        let handler = ConnectionHandler::new(self.shard_channels.clone());

        // 3. Main Loop: Chờ Accept hoặc Shutdown Signal
        tokio::select! {
            res = handler.run(listener) => {
                if let Err(e) = res {
                    log::error!("Server error: {}", e);
                }
            }
            _ = signal::ctrl_c() => {
                log::info!("Shutdown signal received (Ctrl+C)");
            }
        }

        // 4. Graceful Shutdown
        self.shutdown().await;
        Ok(())
    }

    async fn shutdown(self) {
        log::info!("Initiating graceful shutdown...");

        log::info!("Draining in-flight commands ({}s)...", DRAIN_TIMEOUT_SECS);
        sleep(Duration::from_secs(DRAIN_TIMEOUT_SECS)).await;

        log::info!("Closing shard channels...");
        drop(self.shard_channels);

        log::info!("Waiting for workers to finish final snapshot...");
        sleep(Duration::from_secs(2)).await;

        log::info!("Shutdown complete.");
    }
}

pub struct Config {
    pub port: u16,
    pub bind: String,
    pub snapshot_dir: PathBuf,
    pub log_level: String,
    pub cpu_pinning: bool,
}

impl Config {
    pub fn from_args(args: Args) -> Self {
        Config {
            port: args.port,
            bind: args.bind,
            snapshot_dir: PathBuf::from(args.snapshot_dir),
            log_level: args.log_level,
            cpu_pinning: args.cpu_pinning,
        }
    }
}