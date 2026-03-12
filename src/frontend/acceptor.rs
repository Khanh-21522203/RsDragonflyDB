use std::sync::Arc;
use log::{info};
use tokio::net::TcpListener;
use super::handler::ConnectionHandler;


pub async fn acceptor_thread(
    bind_addr: String,
    port: u16,
    handler: Arc<ConnectionHandler>,
) -> std::io::Result<()>{
    let addr = format!("{}:{}", bind_addr, port);
    let listener = TcpListener::bind(&addr).await?;

    info!("Listening on {}", addr);

    let _ = handler.run(listener).await;

    Ok(())
}