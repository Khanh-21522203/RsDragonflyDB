use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::JoinHandle;
use common::channels::CommandSender;
use common::threads::{spawn_thread, ThreadType};
use crate::handler::ConnectionHandler;

pub struct HandlerPool {
    handlers: Vec<Arc<Mutex<ConnectionHandler>>>,
    next_handler: AtomicUsize,
}

impl HandlerPool {
    pub fn new(size: usize, shard_channels: Vec<CommandSender>) -> Arc<Self> {
        let mut handlers = Vec::new();

        for i in 0..size {
            let handler = ConnectionHandler::new(i, shard_channels.clone());
            handlers.push(Arc::new(Mutex::new(handler)));
        }

        Arc::new(HandlerPool {
            handlers,
            next_handler: AtomicUsize::new(0),
        })
    }

    pub fn add_connection(&self, socket: TcpStream, addr: SocketAddr) {
        let idx = self.next_handler.fetch_add(1, Ordering::Relaxed) % self.handlers.len();
        let mut handler = self.handlers[idx].lock().unwrap();
        handler.add_connection(socket, addr);
    }

    pub fn spawn_threads(&self) -> Vec<JoinHandle<()>> {
        self.handlers.iter().map(|handler| {
            let handler = Arc::clone(handler);
            spawn_thread(ThreadType::ConnectionHandler(0), move || {
                let mut h = handler.lock().unwrap();
                h.run();
            })
        }).collect()
    }
}