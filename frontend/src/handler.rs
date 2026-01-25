use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::sync::{mpsc, oneshot};
use std::time::Duration;
use tokio::time::timeout;
use common::channels::{CommandMessage, CommandSender, Response};
use common::command::Command;
use common::resp::RespValue;
use common::shard_id::ShardId;
use common::types::Key;
use protocol::parser::{RespParser};
use protocol::serializer::serialize_resp2;
use crate::router::Router;

const SHARD_TIMEOUT: Duration = Duration::from_millis(500);

pub struct ConnectionHandler {
    shard_channels: Vec<CommandSender>,
}

impl ConnectionHandler {
    pub fn new(shard_channels: Vec<CommandSender>) -> Self {
        ConnectionHandler { shard_channels }
    }

    pub async fn run(&self, listener: TcpListener) {
        log::info!("Connection handler running...");

        loop {
            // Chấp nhận kết nối mới (Async)
            match listener.accept().await {
                Ok((socket, addr)) => {
                    log::debug!("New connection: {}", addr);
                    let channels = self.shard_channels.clone();

                    // Spawn một task riêng cho mỗi kết nối (Green Thread)
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(socket, channels).await {
                            log::error!("Connection error: {}", e);
                        }
                    });
                }
                Err(e) => log::error!("Accept error: {}", e),
            }
        }
    }
}

async fn handle_connection(mut socket: TcpStream, shard_channels: Vec<CommandSender>) -> std::io::Result<()> {
    let (mut reader, writer) = socket.into_split();

    // Wrap writer by BufWriter to reduce number of syscall
    let mut writer = BufWriter::new(writer);

    let (response_tx, mut response_rx) = mpsc::channel::<Response>(32);

    // --- Writer task ---
    let writer_handle = tokio::spawn(async move {
        while let Some(response) = response_rx.recv().await {
            let resp_value = response_to_resp(response);
            let bytes = serialize_resp2(&resp_value);

            if let Err(e) = writer.write_all(&bytes).await {
                return Err(e);
            }

            if let Err(e) = writer.flush().await {
                return Err(e);
            }
        }
        Ok::<(), std::io::Error>(())
    });

    // --- Reader Loop ---
    let mut buf = [0u8; 8 * 1024]; // 8KB
    let mut parser = RespParser::new();

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 { break; } // EOF

        parser.feed(&buf[..n]);

        loop {
            match parser.parse() {
                Ok(Some(resp_value)) => {
                    match Command::from_resp(resp_value) {
                        Ok(cmd) => {
                            let response = process_command(cmd, &shard_channels).await;

                            if response_tx.send(response).await.is_err(){
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = response_tx.send(Response::Error(format!("ERR {:?}", e))).await;
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{:?}", e)))
            }
        }
        parser.compact();
    }

    let _ = writer_handle.await;
    Ok(())
}

async fn process_command(command: Command, shards: &[CommandSender]) -> Response {
    match command {
        Command::Ping { .. } | Command::Info { .. } => {
            send_to_shard(ShardId(0), command, shards).await
        }
        Command::Get { ref key } | Command::Set { ref key, .. } |
        Command::Expire { ref key, .. } | Command::Ttl { ref key } => {
            let shard_id = Router::route_key(key);
            send_to_shard(shard_id, command, shards).await
        }
        Command::Del { keys } => {
            process_multi_key_del(keys, shards).await
        }
    }
}

async fn send_to_shard(shard_id: ShardId, command: Command, shards: &[CommandSender]) -> Response {
    let (reply_tx, reply_rx) = oneshot::channel();
    let msg = CommandMessage { command, reply_tx };

    if let Some(sender) = shards.get(shard_id.as_usize()) {
        let send_result = timeout(Duration::from_millis(100), sender.send(msg)).await;

        if send_result.is_err() || send_result.unwrap().is_err() {
            return Response::Error("ERR shard overloaded".to_string());
        }

        match timeout(SHARD_TIMEOUT, reply_rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Response::Error("ERR shard closed connection".to_string()),
            Err(_) => Response::Error("ERR shard timeout".to_string()), // Hết giờ
        }
    } else {
        Response::Error("ERR invalid shard id".to_string())
    }
}

async fn process_multi_key_del(keys: Vec<Key>, shards: &[CommandSender]) -> Response {
    let grouped = Router::route_keys(&keys);
    let mut futures = Vec::new();

    for (shard_id, shard_keys) in grouped {
        let cmd = Command::Del { keys: shard_keys };
        let sender = shards[shard_id.as_usize()].clone();

        futures.push(async move {
            let (reply_tx, reply_rx) = oneshot::channel();
            let msg = CommandMessage { command: cmd, reply_tx };

            // Logic gửi tương tự send_to_shard, thêm timeout ngắn gọn
            if timeout(Duration::from_millis(100), sender.send(msg)).await.is_err() {
                return Response::Error("ERR shard overloaded".to_string());
            }

            match timeout(SHARD_TIMEOUT, reply_rx).await {
                Ok(Ok(res)) => res,
                _ => Response::Error("ERR timeout".to_string()),
            }
        });
    }

    let responses = futures::future::join_all(futures).await;

    let mut total_deleted = 0;
    let mut errors = Vec::new();

    for res in responses {
        match res {
            Response::Integer(n) => total_deleted += n,
            Response::Error(e) => errors.push(e),
            _ => {}
        }
    }

    if errors.is_empty() {
        Response::Integer(total_deleted)
    } else {
        Response::Error(format!("ERR partial failure: {}", errors.join(", ")))
    }
}

fn response_to_resp(response: Response) -> RespValue {
    match response {
        Response::Value(Some(value)) => {
            RespValue::BulkString(Some(value.as_bytes().to_vec()))
        }
        Response::Value(None) => RespValue::BulkString(None),
        Response::Integer(n) => RespValue::Integer(n),
        Response::Ok => RespValue::SimpleString("OK".to_string()),
        Response::Error(msg) => RespValue::Error(msg),
    }
}
