mod common;
mod protocol;
mod shard;
mod frontend;
mod observability;
mod server;

use clap::Parser;
use env_logger::Env;
use log::info;
use common::constants::SHARD_COUNT;
use server::{Args, Server};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    env_logger::Builder::from_env(Env::default().default_filter_or(&args.log_level))
        .format_timestamp_millis()
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
