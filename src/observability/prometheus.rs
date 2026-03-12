
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use crate::shard::shard::ShardMetrics;
use super::metrics::GlobalMetrics;

pub async fn run_metrics_exporter(
    port: u16,
    global_metrics: Arc<GlobalMetrics>,
    shard_metrics: Vec<Arc<ShardMetrics>>,
) -> std::io::Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    let listener = TcpListener::bind(&addr).await?;


    log::info!("Metrics endpoint listening on {}", addr);

    loop {
        match listener.accept().await {
            Ok((mut socket, _)) => {
                let g_metrics = global_metrics.clone();
                let s_metrics = shard_metrics.clone();

                tokio::spawn(async move {
                    if let Err(e) = handle_metrics_request(&mut socket, &g_metrics, &s_metrics).await {
                        log::debug!("Metrics write error: {}", e);
                    }
                });
            }
            Err(e) => {
                log::error!("Metrics export accept failed: {}", e);
            }
        }
    }
}

async fn handle_metrics_request(
    socket: &mut TcpStream,
    global: &GlobalMetrics,
    shards: &[Arc<ShardMetrics>]
) -> std::io::Result<()> {
    let metrics_text = format_prometheus_metrics(global, shards);

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
        metrics_text.len(),
        metrics_text
    );

    // Async write
    socket.write_all(response.as_bytes()).await?;
    socket.flush().await?;

    Ok(())
}

fn format_prometheus_metrics(
    global: &GlobalMetrics,
    shards: &[Arc<ShardMetrics>],
) -> String {
    let mut output = String::new();

    // Global metrics
    output.push_str("# HELP rsdragonfly_uptime_seconds Server uptime in seconds\n");
    output.push_str("# TYPE rsdragonfly_uptime_seconds gauge\n");
    output.push_str(&format!("rsdragonfly_uptime_seconds {}\n", global.uptime_seconds()));

    output.push_str("# HELP rsdragonfly_connections_active Active client connections\n");
    output.push_str("# TYPE rsdragonfly_connections_active gauge\n");
    output.push_str(&format!("rsdragonfly_connections_active {}\n",
                             global.connections_active.load(Ordering::Relaxed)));

    output.push_str("# HELP rsdragonfly_connections_total Total connections accepted\n");
    output.push_str("# TYPE rsdragonfly_connections_total counter\n");
    output.push_str(&format!("rsdragonfly_connections_total {}\n",
                             global.connections_total.load(Ordering::Relaxed)));

    // Per-shard metrics
    output.push_str("# HELP rsdragonfly_shard_commands_total Commands processed per shard\n");
    output.push_str("# TYPE rsdragonfly_shard_commands_total counter\n");
    for (i, shard) in shards.iter().enumerate() {
        output.push_str(&format!("rsdragonfly_shard_commands_total{{shard=\"{}\"}} {}\n",
                                 i, shard.commands_processed.load(Ordering::Relaxed)));
    }

    output.push_str("# HELP rsdragonfly_shard_keys Current key count per shard\n");
    output.push_str("# TYPE rsdragonfly_shard_keys gauge\n");
    for (i, shard) in shards.iter().enumerate() {
        output.push_str(&format!("rsdragonfly_shard_keys{{shard=\"{}\"}} {}\n",
                                 i, shard.key_count.load(Ordering::Relaxed)));
    }

    output.push_str("# HELP rsdragonfly_shard_memory_bytes Memory usage per shard\n");
    output.push_str("# TYPE rsdragonfly_shard_memory_bytes gauge\n");
    for (i, shard) in shards.iter().enumerate() {
        output.push_str(&format!("rsdragonfly_shard_memory_bytes{{shard=\"{}\"}} {}\n",
                                 i, shard.memory_bytes.load(Ordering::Relaxed)));
    }

    output.push_str("# HELP rsdragonfly_shard_keys_expired_total Keys expired per shard\n");
    output.push_str("# TYPE rsdragonfly_shard_keys_expired_total counter\n");
    for (i, shard) in shards.iter().enumerate() {
        output.push_str(&format!("rsdragonfly_shard_keys_expired_total{{shard=\"{}\"}} {}\n",
                                 i, shard.keys_expired.load(Ordering::Relaxed)));
    }

    output.push_str("# HELP rsdragonfly_shard_snapshots_written_total Snapshots written per shard\n");
    output.push_str("# TYPE rsdragonfly_shard_snapshots_written_total counter\n");
    for (i, shard) in shards.iter().enumerate() {
        output.push_str(&format!("rsdragonfly_shard_snapshots_written_total{{shard=\"{}\"}} {}\n",
                                 i, shard.snapshots_written.load(Ordering::Relaxed)));
    }

    output.push_str("# HELP rsdragonfly_shard_snapshot_write_errors_total Snapshot write errors per shard\n");
    output.push_str("# TYPE rsdragonfly_shard_snapshot_write_errors_total counter\n");
    for (i, shard) in shards.iter().enumerate() {
        output.push_str(&format!("rsdragonfly_shard_snapshot_write_errors_total{{shard=\"{}\"}} {}\n",
                                 i, shard.snapshot_write_errors.load(Ordering::Relaxed)));
    }

    output
}