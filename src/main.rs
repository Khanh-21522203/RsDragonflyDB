use clap::Parser;
use env_logger::Env;
use log::info;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "6379")]
    port: u16,

    #[arg(long, default_value = "0.0.0.0")]
    bind: String,

    #[arg(long, default_value = "/var/lib/rsdragonfly")]
    snapshot_dir: String,

    #[arg(long, default_value = "60")]
    snapshot_interval: u64,

    #[arg(long, default_value = "info")]
    log_level: String,

    #[arg(long, default_value = "false")]
    cpu_pinning: bool,
}

fn main() {
    let args = Args::parse();

    env_logger::Builder::from_env(Env::default().default_filter_or(&args.log_level))
        .init();

    info!("Starting RsDragonflyDB v{}", env!("CARGO_PKG_VERSION"));
    info!("Port: {}, Shards: {}", args.port, SHARD_COUNT);

    let server = Server::new(args);

    server.run();

    info!("Server stopped");
}