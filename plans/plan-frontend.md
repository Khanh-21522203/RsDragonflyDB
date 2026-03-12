# Feature: Frontend

## 1. Purpose

The `crates/frontend` crate implements the network-facing layer of RsDragonflyDB: the TCP acceptor, the connection handler thread pool, and the command router. It bridges raw TCP sockets (RESP2 bytes) to the shard layer (typed `Command` messages) and sends serialized RESP2 responses back to clients.

The frontend handles pipelining (multiple commands in one read), backpressure (stops reading when a shard queue is overloaded), and INFO command aggregation (reads from all shards' metrics atomics without sending a Command to any shard thread).

## 2. Responsibilities

- Accept new TCP connections on a configured port and bind address
- Assign connections to connection handler threads (round-robin or least-loaded)
- Parse RESP2 frames from client sockets using `crates/protocol`
- Validate key / value size limits (`MAX_KEY_SIZE`, `MAX_VALUE_SIZE`)
- Route single-key commands to the owning shard via `crossbeam::channel::unbounded`
- Implement scatter-gather for multi-key DEL: group keys by shard, send in parallel, aggregate
- Receive `Response` from shards via `crossbeam::channel::bounded(1)` and serialize to RESP2
- Handle INFO command locally by aggregating `ShardMetrics` atomics across all shards
- Apply backpressure: stop reading from a socket if target shard queue depth > 10,000
- Handle connection close (client EOF, read error, protocol error)
- Enforce maximum concurrent connections (`max_connections`)

## 3. Non-Responsibilities

- Does not execute command logic (all execution is in shard threads)
- Does not manage shard data structures
- Does not perform disk IO
- Does not implement TLS
- Does not implement AUTH
- Does not handle the RESP3 protocol (post-MVP)

## 4. Architecture Design

```
                     Clients (redis-cli, apps)
                           │ TCP
                           ▼
                    ┌──────────────┐
                    │   Acceptor   │   1 OS thread, blocks on accept()
                    │   Thread     │   enforces max_connections limit
                    └──────┬───────┘
                           │ assigns Connection to handler pool (round-robin)
              ┌────────────┼────────────┐
              ▼            ▼            ▼
     ┌─────────────┐ ┌─────────────┐ ┌─────────────┐
     │  Handler 0  │ │  Handler 1  │ │  Handler N  │  N = frontend_threads (default 4)
     │  epoll loop │ │  epoll loop │ │  epoll loop │
     │             │ │             │ │             │
     │ Resp2Parser │ │ Resp2Parser │ │ Resp2Parser │  one parser per connection
     └──────┬──────┘ └──────┬──────┘ └──────┬──────┘
            │               │               │
            └───────────────┼───────────────┘
                            │ crossbeam::channel::unbounded
                            │ Command { op, reply_tx: Sender<Response> (bounded(1)) }
              ┌─────────────┼─────────────┐
              ▼             ▼             ▼
         [Shard 0]     [Shard 1]  ... [Shard 63]
```

## 5. I/O Model: mio

The frontend uses [`mio`](https://docs.rs/mio) for non-blocking I/O on Linux (epoll under the hood). This is why the connection handlers never block on a single socket — they multiplex thousands of connections on one thread.

### Key mio concepts

**`mio::Poll`** — the central event loop object. Call `poll.poll(&mut events, timeout)` to block until at least one registered source is ready.

**`mio::Token`** — a `usize` tag you assign when registering a source. Events come back with the same token so you know which socket is ready.

**`Interest`** — `Interest::READABLE` means notify me when data is available to read. `Interest::WRITABLE` means notify me when the socket can accept writes.

**Edge-triggered vs level-triggered** — mio uses edge-triggered mode by default on Linux. This means you are notified once when a socket becomes readable. You must then drain it completely (read until `WouldBlock`) or you won't get another notification even if more data arrives.

### mio cheat sheet for this project

```rust
use mio::{Events, Interest, Poll, Token};
use mio::net::{TcpListener, TcpStream};
use std::io::{self, Read, Write, ErrorKind};

// --- Setup (once per handler thread) ---

let mut poll = Poll::new().unwrap();
let mut events = Events::with_capacity(1024);

// Register the acceptor's listener (Token 0 is reserved for the listener)
const LISTENER_TOKEN: Token = Token(0);
poll.registry()
    .register(&mut listener, LISTENER_TOKEN, Interest::READABLE)
    .unwrap();

// --- Registering a new client connection ---
// Each connection gets a unique token (Token(1), Token(2), ...)
let token = Token(next_token_id);
next_token_id += 1;

poll.registry()
    .register(&mut stream, token, Interest::READABLE)
    .unwrap();

// --- The event loop ---
loop {
    poll.poll(&mut events, Some(Duration::from_millis(100))).unwrap();

    for event in events.iter() {
        match event.token() {
            LISTENER_TOKEN => {
                // New connection is available
                match listener.accept() {
                    Ok((mut stream, addr)) => {
                        let token = Token(next_token_id);
                        next_token_id += 1;
                        poll.registry()
                            .register(&mut stream, token, Interest::READABLE)
                            .unwrap();
                        connections.insert(token, Connection::new(stream, addr));
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => { log::error!("accept error: {}", e); }
                }
            }
            token => {
                // Data available on an existing connection
                let conn = match connections.get_mut(&token) {
                    Some(c) => c,
                    None => continue,
                };

                // IMPORTANT: drain until WouldBlock (edge-triggered)
                loop {
                    let mut buf = [0u8; 4096];
                    match conn.stream.read(&mut buf) {
                        Ok(0) => {
                            // Client closed connection
                            conn.closed = true;
                            break;
                        }
                        Ok(n) => {
                            conn.parser.feed(&buf[..n]);
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(e) => {
                            log::warn!("read error: {}", e);
                            conn.closed = true;
                            break;
                        }
                    }
                }

                // Parse and dispatch all complete commands
                let (cmds, proto_err) = conn.parser.drain_commands();
                for cmd in cmds {
                    dispatch_command(conn, cmd, &shard_txs, &shard_metrics, &config);
                }
                if proto_err.is_some() {
                    conn.closed = true;
                }
            }
        }
    }

    // Flush write buffers for all connections
    for conn in connections.values_mut() {
        flush_connection(conn);
    }

    // Remove closed connections
    connections.retain(|token, conn| {
        if conn.closed {
            poll.registry().deregister(&mut conn.stream).ok();
            false
        } else {
            true
        }
    });
}

// --- Writing a response ---
// Append bytes to a per-connection write buffer, then flush:
fn flush_connection(conn: &mut Connection) {
    while !conn.write_buf.is_empty() {
        match conn.stream.write(&conn.write_buf) {
            Ok(n) => { conn.write_buf.drain(..n); }
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => { log::warn!("write error: {}", e); conn.closed = true; break; }
        }
    }
}
```

### Token management

A simple incrementing counter works for MVP. When a connection closes and its token is retired, do NOT reuse the token immediately — wait until the slot is removed from the connections map. Use a `HashMap<Token, Connection>` keyed by token.

```rust
// Simple token allocator — just increment forever (u64 won't overflow in practice)
struct TokenAllocator(usize);
impl TokenAllocator {
    fn next(&mut self) -> Token {
        let t = Token(self.0);
        self.0 += 1;
        t
    }
}
```

---

## 5. Core Data Structures

```rust
// crates/frontend/src/connection.rs

use common::{Key, Value, ShardId, Command, Operation, Response, Error, Config,
             MAX_KEY_SIZE, MAX_VALUE_SIZE, SHARD_COUNT};
use protocol::{Resp2Parser, Resp2Encoder, command_to_operation, ParseResult};
use crossbeam::channel::{Sender, Receiver};

/// State for one active client connection.
pub struct Connection {
    pub id: usize,
    socket: TcpStream,        // non-blocking
    parser: Resp2Parser,      // per-connection RESP2 state machine
    write_buf: Vec<u8>,       // outgoing bytes not yet flushed
    peer_addr: SocketAddr,
    closed: bool,
}

impl Connection {
    pub fn new(id: usize, socket: TcpStream, peer_addr: SocketAddr) -> Self {
        socket.set_nonblocking(true).unwrap();
        Self {
            id,
            socket,
            parser: Resp2Parser::new(),
            write_buf: Vec::with_capacity(4096),
            peer_addr,
            closed: false,
        }
    }

    /// Non-blocking read: fill parser buffer from socket.
    pub fn read(&mut self) -> io::Result<usize>

    /// Drain complete RESP2 commands from the parser.
    pub fn drain_commands(&mut self) -> (Vec<ParsedCommand>, Option<String>)

    /// Queue RESP2-encoded bytes for write.
    pub fn enqueue_response(&mut self, data: Vec<u8>)

    /// Flush write buffer to socket (non-blocking).
    pub fn flush(&mut self) -> io::Result<()>

    pub fn close(&mut self) { self.closed = true; }
    pub fn is_closed(&self) -> bool { self.closed }
}
```

```rust
// crates/frontend/src/handler.rs

/// One connection handler thread manages multiple connections via epoll.
pub struct ConnectionHandler {
    id: usize,
    connections: Vec<Option<Connection>>,  // indexed by fd / slot
    epoll: Epoll,
    shard_txs: Arc<[Sender<Command>; SHARD_COUNT]>,
    shard_metrics: Arc<[ShardMetrics; SHARD_COUNT]>,
    config: Arc<Config>,
    connection_count: Arc<AtomicUsize>,    // shared with acceptor, global counter
}

impl ConnectionHandler {
    pub fn add_connection(&mut self, socket: TcpStream, addr: SocketAddr)
    pub fn run(&mut self)   // main event loop; blocking
}
```

```rust
// crates/frontend/src/acceptor.rs

pub struct Acceptor {
    listener: TcpListener,
    handlers: Vec<Sender<(TcpStream, SocketAddr)>>,  // one channel per handler thread
    next_handler: usize,     // round-robin index
    connection_count: Arc<AtomicUsize>,
    config: Arc<Config>,
}

impl Acceptor {
    pub fn run(&mut self)
}
```

## 6. Public Interfaces

```rust
// crates/frontend/src/lib.rs

/// Spawn the acceptor thread and N connection handler threads.
/// Returns a shutdown handle.
pub fn start_frontend(
    config: Arc<Config>,
    shard_txs: Arc<[Sender<Command>; SHARD_COUNT]>,
    shard_metrics: Arc<[ShardMetrics; SHARD_COUNT]>,
) -> FrontendHandle

pub struct FrontendHandle {
    /// Signal the frontend to stop accepting new connections and drain.
    pub fn shutdown(self)
    /// Wait for all frontend threads to exit.
    pub fn join(self)
}
```

## 7. Internal Algorithms

### Acceptor Thread

```
acceptor.run():
  loop:
    match listener.accept():
      Ok((socket, addr)) →
        if connection_count.load(Relaxed) >= config.max_connections:
          // Reject: immediately send "-ERR max connections reached\r\n" and close
          drop(socket)
          continue

        connection_count.fetch_add(1, Relaxed)
        handler_idx = next_handler % frontend_threads
        next_handler += 1
        handlers[handler_idx].send((socket, addr)).ok()

      Err(e) if e.kind() == WouldBlock → {}  // OS-level transient
      Err(e) → log error "accept failed"; break
```

### Connection Handler Event Loop

```
handler.run():
  loop:
    events = epoll.wait(timeout_ms: 100)

    // Accept new connections sent from acceptor
    while let Ok((socket, addr)) = new_conn_rx.try_recv():
      conn = Connection::new(next_slot, socket, addr)
      connections[next_slot] = Some(conn)
      epoll.add(conn.fd, next_slot)
      next_slot += 1

    // Process readable sockets
    for event in &events:
      conn = &mut connections[event.id]

      // Read bytes
      match conn.read():
        Ok(0) → conn.close(); continue
        Err(WouldBlock) → continue
        Err(e) → log warn "read error"; conn.close(); continue
        Ok(_) → {}

      // Parse and dispatch commands (pipelining: drain all complete commands)
      let (cmds, proto_err) = conn.drain_commands()
      for parsed_cmd in cmds:
        dispatch_command(conn, parsed_cmd, &shard_txs, &shard_metrics, &config)
      if let Some(e) = proto_err:
        log warn "protocol error from {}: {}", conn.peer_addr, e
        conn.close()

    // Flush write buffers
    for conn in &mut connections:
      conn.flush().ok()

    // Remove closed connections
    connections.retain(|c| !c.is_closed());
    for closed in closed_conns: connection_count.fetch_sub(1, Relaxed)
```

### Command Dispatch

```
dispatch_command(conn, parsed_cmd, shard_txs, shard_metrics, config):
  op = match command_to_operation(parsed_cmd):
    Err(e) → conn.enqueue_response(Resp2Encoder::encode(&Response::Error(e.to_string()))); return
    Ok(op) → op

  // Validate sizes
  match &op:
    Set { key, value, .. } →
      if key.len() > MAX_KEY_SIZE:
        conn.enqueue_response(encode(Error("key too long"))); return
      if value.len() > MAX_VALUE_SIZE:
        conn.enqueue_response(encode(Error("value too large"))); return
    _ → {}

  match &op:
    // INFO: handled locally — no shard message needed
    Info { section } →
      info_str = aggregate_info(shard_metrics, config, section)
      conn.enqueue_response(encode(Response::Info(info_str))); return

    // DEL with multiple keys: scatter-gather
    Del { keys } if keys.len() > 1 →
      response = scatter_gather_del(keys, shard_txs)
      conn.enqueue_response(encode(response)); return

    // Single-shard commands: route to one shard
    _ →
      key = op.primary_key()
      shard_id = route_key(key)

      // Backpressure check
      if shard_metrics[shard_id].queue_depth.load(Relaxed) > 10_000:
        conn.enqueue_response(encode(Error("shard overloaded"))); return

      let (reply_tx, reply_rx) = crossbeam::channel::bounded(1)
      shard_txs[shard_id].send(Command { op, reply_tx }).ok()

      // Block until response (connection handler is sync — epoll loop handles other fds)
      match reply_rx.recv_timeout(Duration::from_secs(5)):
        Ok(resp) → conn.enqueue_response(encode(resp))
        Err(_)   → conn.enqueue_response(encode(Error("shard timeout")))
```

### Scatter-Gather DEL

```
scatter_gather_del(keys, shard_txs):
  // Group keys by shard
  groups: HashMap<ShardId, Vec<Key>> = {}
  for key in keys:
    shard_id = route_key(key)
    groups[shard_id].push(key)

  // Send to each shard in parallel, collect reply channels
  reply_rxs = []
  for (shard_id, shard_keys) in groups:
    let (reply_tx, reply_rx) = crossbeam::channel::bounded(1)
    shard_txs[shard_id].send(Command {
      op: Operation::Del { keys: shard_keys },
      reply_tx,
    }).ok()
    reply_rxs.push(reply_rx)

  // Aggregate responses
  total = 0
  for reply_rx in reply_rxs:
    match reply_rx.recv_timeout(5s):
      Ok(Response::Integer(n)) → total += n
      Ok(Response::Error(e))  → log warn "del shard error: {}", e
      Err(_) → log warn "del shard timeout"

  return Response::Integer(total)
```

### INFO Aggregation

```
aggregate_info(shard_metrics, config, section):
  // Aggregate atomics across all shards
  total_cmds = 0; total_keys_expired = 0; total_memory = 0; total_keys = 0
  for m in shard_metrics:
    total_cmds          += m.commands_processed.load(Relaxed)
    total_keys_expired  += m.keys_expired.load(Relaxed)
    total_memory        += m.memory_bytes.load(Relaxed)
    total_keys          += m.key_count.load(Relaxed)

  uptime = server_start_time.elapsed().as_secs()
  ops_per_sec = ... // rate computed from last interval

  server_section = format!(
    "# server\r\nversion:{}\r\nuptime_in_seconds:{}\r\ntcp_port:{}\r\nshard_count:{}\r\nos:{}\r\n\r\n",
    env!("CARGO_PKG_VERSION"), uptime, config.port, SHARD_COUNT, std::env::consts::OS
  )
  stats_section = format!(
    "# stats\r\ntotal_commands_processed:{}\r\n...\r\nkeys_expired:{}\r\n\r\n",
    total_cmds, total_keys_expired
  )
  memory_section = format!(
    "# memory\r\nused_memory:{}\r\nused_memory_human:{}\r\nmem_fragmentation_ratio:{}\r\n\r\n",
    total_memory, format_bytes(total_memory), fragmentation_ratio()
  )

  match section:
    None          → server_section + stats_section + memory_section
    Some(b"server")  → server_section
    Some(b"stats")   → stats_section
    Some(b"memory")  → memory_section
    Some(_)       → Error("unknown INFO section")
```

## 8. Persistence Model

None. The frontend is stateless per request — all state lives in the shard threads.

## 9. Concurrency Model

- One `Acceptor` thread blocks on `accept()`. It is the only writer to the connection counter and the only sender to handler-assignment channels.
- Each `ConnectionHandler` thread owns its `Vec<Connection>` exclusively — no sharing with other handler threads.
- `shard_txs` (one channel sender per shard) is wrapped in `Arc` and cloned into each handler thread. Channels are MPSC-safe.
- `shard_metrics` is `Arc<[ShardMetrics; SHARD_COUNT]>` — atomic fields are read without a lock.
- `connection_count` is a single `Arc<AtomicUsize>` shared between the acceptor (increments) and handlers (decrements on close).

**No mutex on the command dispatch path.**

## 10. Configuration

| Parameter | Source | Default |
|-----------|--------|---------|
| `port` | `Config` | 6379 |
| `bind` | `Config` | 0.0.0.0 |
| `max_connections` | `Config` | 10000 |
| `frontend_threads` | `Config` | 4 |

## 11. Observability

Structured log events:
- `INFO`: new connection accepted `{ peer_addr }`
- `WARN`: protocol error from client `{ peer_addr, error }`
- `WARN`: shard response timeout `{ shard_id, peer_addr }`
- `WARN`: max connections reached, rejecting `{ peer_addr }`

Metrics (contributed to global metrics by the metrics exporter):
- `total_connections_received` — lifetime count of accepted connections
- `instantaneous_ops_per_sec` — computed from commands_processed delta over 1s
- These are part of the INFO stats section output

## 12. Testing Strategy

- **Unit tests**:
  - `test_route_single_key_command`: route GET command, verify correct shard channel receives it
  - `test_scatter_gather_del_two_shards`: mock two shard channels; send DEL key1 key2 where keys are on different shards; assert total = 2
  - `test_scatter_gather_del_same_shard`: all 3 keys on same shard; assert one message sent, total = 3
  - `test_info_aggregation_server_section`: mock ShardMetrics with known values, call aggregate_info("server"), assert version and shard_count correct
  - `test_info_aggregation_stats_section`: assert total_commands and keys_expired are summed correctly
  - `test_backpressure_rejects_command`: set queue_depth > 10000 in mock metrics, assert Error response
  - `test_max_connections_rejects_new`: set connection_count to max_connections, acceptor should reject next

- **Integration tests**:
  - `test_ping_pong`: connect redis client, PING → PONG
  - `test_set_get_via_tcp`: SET mykey value, GET mykey → "value"
  - `test_pipelining`: send 5 commands in one TCP write, verify 5 responses received in order
  - `test_protocol_error_closes_connection`: send garbage bytes, verify connection closed
  - `test_concurrent_100_clients`: 100 clients simultaneously SET and GET different keys, no errors
  - `test_info_command`: INFO server → contains version, shard_count, tcp_port

## 13. Open Questions

None.
