use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use log::{info, error};
use crate::pool::HandlerPool;

pub fn acceptor_thread(
    bind_addr: String,
    port: u16,
    handler_pool: Arc<HandlerPool>,
) {
    let addr = format!("{}:{}", bind_addr, port);
    let listener = TcpListener::bind(&addr)
        .expect(&format!("Failed to bind to {}", addr));

    info!("Listening on {}", addr);

    loop {
        match listener.accept() {
            Ok((socket, addr)) => {
                log::debug!("Accepted connection from {}", addr);

                // Configure socket
                if let Err(e) = socket.set_nodelay(true) {
                    log::warn!("Failed to set TCP_NODELAY: {}", e);
                }

                // Assign to handler
                handler_pool.add_connection(socket, addr);
            }
            Err(e) => {
                error!("Accept failed: {}", e);
            }
        }
    }
}