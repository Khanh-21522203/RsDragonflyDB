# Feature: RESP2 Protocol

## 1. Purpose

The `crates/protocol` crate implements the Redis Serialization Protocol version 2 (RESP2) parser and encoder. It is the wire-format layer between raw TCP byte streams and the structured `Operation` / `Response` types used by the rest of the system.

The parser is stateful and streaming: it handles partial reads (TCP fragmentation) and batched reads (pipelining). The encoder is stateless: it converts `Response` values to RESP2 bytes.

## 2. Responsibilities

- Parse RESP2 frames from a byte buffer into `ParsedCommand` structs (verb + raw argument bytes)
- Handle incomplete frames (return `Incomplete` without consuming bytes)
- Handle multiple commands in one buffer (pipelining)
- Validate RESP2 framing (correct lengths, `\r\n` terminators)
- Encode `Response` values into RESP2 wire bytes
- Validate command argument counts and produce `Error::WrongArgCount` / `Error::UnknownCommand`
- Convert parsed raw arguments to typed `Operation` values

## 3. Non-Responsibilities

- Does not execute commands (no shard interaction)
- Does not manage TCP sockets
- Does not perform routing or channel I/O
- Does not handle TLS

## 4. Architecture Design

```
TCP socket (raw bytes)
        │
        ▼
┌─────────────────────┐
│   Resp2Parser       │  stateful, holds partial read buffer
│   feed(bytes)       │
│   next_command()    │──→ ParsedCommand { verb, args: Vec<Vec<u8>> }
└─────────────────────┘
        │
        ▼
┌─────────────────────┐
│  command_to_op()    │  pure function; validates & converts to Operation
└─────────────────────┘
        │
        ▼
   Operation (to shard via channel)

        │  (shard response)
        ▼
┌─────────────────────┐
│  Resp2Encoder       │  stateless; Response → Vec<u8>
│  encode(response)   │
└─────────────────────┘
        │
        ▼
  TCP socket write
```

## 5. Core Data Structures

```rust
// crates/protocol/src/parser.rs

/// A successfully parsed RESP2 command from the client.
/// `verb` is the uppercased command name (e.g., b"GET").
/// `args` are the remaining arguments as raw byte vectors.
pub struct ParsedCommand {
    pub verb: Vec<u8>,
    pub args: Vec<Vec<u8>>,
}

/// Result of attempting to parse one command from the buffer.
pub enum ParseResult {
    /// A complete command was parsed; `consumed` bytes should be drained from the buffer.
    Complete { command: ParsedCommand, consumed: usize },
    /// Not enough data yet; caller should read more bytes and retry.
    Incomplete,
    /// The client sent invalid RESP2; the connection should be closed.
    Error(String),
}

/// Streaming RESP2 parser. Maintains a partial-read buffer across calls.
pub struct Resp2Parser {
    buf: Vec<u8>,
}

impl Resp2Parser {
    pub fn new() -> Self { Self { buf: Vec::with_capacity(4096) } }

    /// Append newly received bytes to the internal buffer.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Attempt to parse one complete command from the buffer.
    /// Returns `Complete` and advances the buffer, or `Incomplete` if more data needed.
    pub fn next_command(&mut self) -> ParseResult { /* see §7 */ }

    /// Drain all complete commands in a loop; returns a vec (for pipelining).
    pub fn drain_commands(&mut self) -> (Vec<ParsedCommand>, Option<String>) {
        let mut cmds = Vec::new();
        loop {
            match self.next_command() {
                ParseResult::Complete { command, consumed } => {
                    self.buf.drain(..consumed);
                    cmds.push(command);
                }
                ParseResult::Incomplete => return (cmds, None),
                ParseResult::Error(e)   => return (cmds, Some(e)),
            }
        }
    }
}
```

```rust
// crates/protocol/src/encoder.rs

use common::{Response, Value};

/// Stateless RESP2 encoder. Converts a Response to wire bytes.
pub struct Resp2Encoder;

impl Resp2Encoder {
    /// Encode a Response into RESP2 bytes. Result is ready to write to socket.
    pub fn encode(response: &Response) -> Vec<u8> { /* see §7 */ }
}
```

```rust
// crates/protocol/src/lib.rs

/// Convert a ParsedCommand to a typed Operation.
/// Validates argument count and parses numeric arguments.
/// Returns Err if the command is unknown or arguments are invalid.
pub fn command_to_operation(cmd: ParsedCommand) -> Result<Operation, Error> { /* see §7 */ }
```

## 6. Public Interfaces

```rust
// Parser
pub fn Resp2Parser::new() -> Self
pub fn Resp2Parser::feed(&mut self, bytes: &[u8])
pub fn Resp2Parser::next_command(&mut self) -> ParseResult
pub fn Resp2Parser::drain_commands(&mut self) -> (Vec<ParsedCommand>, Option<String>)

// Encoder
pub fn Resp2Encoder::encode(response: &Response) -> Vec<u8>

// Conversion
pub fn command_to_operation(cmd: ParsedCommand) -> Result<Operation, Error>
```

## 7. Internal Algorithms

### RESP2 Parser

RESP2 clients send commands as Arrays of Bulk Strings:

```
*3\r\n           ← array of 3 elements
$3\r\n           ← bulk string of 3 bytes
SET\r\n
$5\r\n           ← bulk string of 5 bytes
mykey\r\n
$7\r\n           ← bulk string of 7 bytes
myvalue\r\n
```

```
next_command(buf):
  if buf is empty: return Incomplete

  if buf[0] != b'*':
    return Error("expected array prefix '*'")

  find first '\r\n' in buf → position `p`
  if not found: return Incomplete

  n_args = parse_integer(buf[1..p])
  if n_args < 1: return Error("array length < 1")

  cursor = p + 2  // skip '*N\r\n'
  args = []

  for i in 0..n_args:
    if cursor >= buf.len(): return Incomplete
    if buf[cursor] != b'$': return Error("expected bulk string '$'")

    find '\r\n' starting at cursor+1 → position `q`
    if not found: return Incomplete

    arg_len = parse_integer(buf[cursor+1..q])
    cursor = q + 2  // skip '$N\r\n'

    if cursor + arg_len + 2 > buf.len(): return Incomplete

    arg = buf[cursor..cursor+arg_len].to_vec()
    cursor += arg_len + 2  // skip arg bytes + '\r\n'
    args.push(arg)

  verb = args[0].to_ascii_uppercase()
  return Complete { command: ParsedCommand { verb, args: args[1..] }, consumed: cursor }
```

**Inline commands** (e.g., `PING\r\n` without RESP2 framing) are not supported in MVP; connections sending inline format receive a protocol error.

### RESP2 Encoder

```
encode(response):
  match response:
    Pong(None)          → "+PONG\r\n"
    Pong(Some(msg))     → "$N\r\n<msg>\r\n"  (bulk string echo)
    BulkString(Some(b)) → "$N\r\n<bytes>\r\n"
    BulkString(None)    → "$-1\r\n"          (null bulk string)
    Integer(n)          → ":N\r\n"
    Ok                  → "+OK\r\n"
    Info(s)             → "$N\r\n<s>\r\n"    (bulk string)
    Error(msg)          → "-ERR <msg>\r\n"
```

### command_to_operation

```
command_to_operation(ParsedCommand { verb, args }):
  match verb.as_slice():
    b"PING"   →
      if args.len() > 1: Err(WrongArgCount("PING"))
      Ok(Operation::Ping { message: args.into_iter().next() })

    b"GET"    →
      if args.len() != 1: Err(WrongArgCount("GET"))
      Ok(Operation::Get { key: args[0] })

    b"SET"    →
      if args.len() < 2 || args.len() > 4: Err(WrongArgCount("SET"))
      let key = args[0], value = Value::String(args[1])
      let ttl_secs = if args.len() == 4 && args[2].eq_ignore_ascii_case(b"EX"):
        Some(parse_u64(args[3])?)
      else: None
      Ok(Operation::Set { key, value, ttl_secs })

    b"DEL"    →
      if args.is_empty(): Err(WrongArgCount("DEL"))
      Ok(Operation::Del { keys: args })

    b"EXPIRE" →
      if args.len() != 2: Err(WrongArgCount("EXPIRE"))
      Ok(Operation::Expire { key: args[0], ttl_secs: parse_u64(args[1])? })

    b"TTL"    →
      if args.len() != 1: Err(WrongArgCount("TTL"))
      Ok(Operation::Ttl { key: args[0] })

    b"INFO"   →
      if args.len() > 1: Err(WrongArgCount("INFO"))
      Ok(Operation::Info { section: args.into_iter().next() })

    _         → Err(UnknownCommand(String::from_utf8_lossy(&verb).into()))
```

## 8. Persistence Model

None. The parser and encoder are pure in-memory functions; no disk IO.

## 9. Concurrency Model

- `Resp2Parser` is NOT `Send` — it must not be shared across threads. One parser instance per connection, owned exclusively by the connection handler thread that manages that socket.
- `Resp2Encoder::encode` is a pure function — thread-safe by default (no shared state).
- `command_to_operation` is a pure function — thread-safe.

## 10. Configuration

No configuration. Parser behavior (e.g., max frame size) will be bounded by upstream key/value size limits from `common::config` (`MAX_KEY_SIZE`, `MAX_VALUE_SIZE`).

## 11. Observability

- No metrics emitted directly from this crate
- The frontend layer logs protocol errors (malformed RESP2) at WARN level with client address
- Parse errors cause connection closure (logged by connection handler)

## 12. Testing Strategy

- **Unit tests**:
  - `test_parse_ping`: parse `*1\r\n$4\r\nPING\r\n`, assert verb=PING, args empty
  - `test_parse_set_with_ttl`: parse `*5\r\n$3\r\nSET\r\n$3\r\nfoo\r\n$3\r\nbar\r\n$2\r\nEX\r\n$2\r\n10\r\n`, assert Operation::Set with ttl_secs=10
  - `test_parse_del_multi_key`: parse DEL with 3 keys, assert all keys present
  - `test_parse_incomplete_frame`: feed half a frame, assert Incomplete; feed rest, assert Complete
  - `test_parse_pipelining`: feed 3 commands in one buffer, assert drain_commands returns all 3
  - `test_parse_unknown_command`: parse verb `FOOBAR`, assert Error::UnknownCommand
  - `test_parse_wrong_arg_count`: parse `GET` with 0 args, assert Error::WrongArgCount
  - `test_parse_invalid_integer_arg`: parse `EXPIRE key abc`, assert Error::NotAnInteger
  - `test_encode_bulk_string`: encode `Response::BulkString(Some(b"hello"))`, assert `$5\r\nhello\r\n`
  - `test_encode_null_bulk_string`: encode `Response::BulkString(None)`, assert `$-1\r\n`
  - `test_encode_integer`: encode `Response::Integer(42)`, assert `:42\r\n`
  - `test_encode_error`: encode `Response::Error("ERR bad")`, assert `-ERR ERR bad\r\n`
  - `test_encode_ok`: encode `Response::Ok`, assert `+OK\r\n`
  - `test_encode_pong_no_message`: encode `Response::Pong(None)`, assert `+PONG\r\n`
  - `test_encode_pong_with_message`: encode `Response::Pong(Some(b"hello"))`, assert `$5\r\nhello\r\n`

- **Fuzz tests** (cargo-fuzz):
  - `fuzz_parser`: feed arbitrary bytes, assert no panic and no undefined behavior
  - Verify all parser paths return `Incomplete` or `Error` — never panic on malformed input

## 13. Open Questions

None.
