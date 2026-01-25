use clap::Parser;
use env_logger::Env;
use log::info;
use common::constants::SHARD_COUNT;
use server::Server;
mod server;

#[derive(Parser)]
#[command(name = "rs_dragonfly_db")]
struct Args {
    #[arg(long, default_value = "6379")]
    port: u16,

    #[arg(long, default_value = "0.0.0.0")]
    bind: String,

    #[arg(long, default_value = "./data")]
    snapshot_dir: String,

    #[arg(long, default_value = "info")]
    log_level: String,

    #[arg(long, default_value = "false")]
    cpu_pinning: bool,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    env_logger::Builder::from_env(Env::default().default_filter_or(&args.log_level))
        .format_timestamp_millis() // Thêm timestamp
        .init();

    info!("Starting RsDragonflyDB v{}", env!("CARGO_PKG_VERSION"));
    info!("Config: Port={}, Shards={}, Bind={}", args.port, SHARD_COUNT, args.bind);

    let server = Server::new(args).await;

    if let Err(e) = server.run().await {
        log::error!("Server exited with error: {}", e);
        std::process::exit(1);
    }

    info!("Server stopped gracefully. Goodbye!");
    Ok(())
}