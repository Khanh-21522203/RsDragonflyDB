# Feature: Project Setup

## 1. Purpose

This document defines the complete project structure, all `Cargo.toml` files, external crate choices, and build commands. A developer should be able to follow this document from an empty directory to a compiling (but not yet functional) workspace.

**Read this document first before any other plan.**

## 2. Prerequisites

```bash
# Install Rust (1.75+)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable

# Verify
rustc --version   # should print 1.75.0 or higher
cargo --version

# Optional: faster linker (speeds up incremental builds)
sudo apt install clang lld   # Linux
# Then add to ~/.cargo/config.toml:
# [target.x86_64-unknown-linux-gnu]
# linker = "clang"
# rustflags = ["-C", "link-arg=-fuse-ld=lld"]
```

## 3. Directory Structure

```
RsDragonflyDB/
├── Cargo.toml              ← workspace root
├── Cargo.lock              ← committed to git
├── .cargo/
│   └── config.toml         ← optional: linker, rustflags
├── crates/
│   ├── common/             ← shared types, config, metrics
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types.rs
│   │       ├── command.rs
│   │       ├── error.rs
│   │       ├── metrics.rs
│   │       └── config.rs
│   ├── protocol/           ← RESP2 parser + encoder
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── parser.rs
│   │       └── encoder.rs
│   ├── engine/             ← shard storage + TTL expiration
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       └── shard.rs
│   ├── persistence/        ← snapshot write + read
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── serializer.rs
│   │       ├── loader.rs
│   │       └── writer.rs
│   ├── frontend/           ← TCP acceptor + connection handlers
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── acceptor.rs
│   │       ├── handler.rs
│   │       └── connection.rs
│   ├── observability/      ← metrics exporter + logging
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── exporter.rs
│   │       └── format.rs
│   └── server/             ← main binary + wiring
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           └── shutdown.rs
├── docs/                   ← architecture documentation
└── plans/                  ← implementation plans (this directory)
```

Create the skeleton:
```bash
mkdir -p RsDragonflyDB
cd RsDragonflyDB

for crate in common protocol engine persistence frontend observability server; do
  mkdir -p crates/$crate/src
done
```

## 4. Workspace Cargo.toml

```toml
# Cargo.toml (workspace root)
[workspace]
resolver = "2"
members  = [
    "crates/common",
    "crates/protocol",
    "crates/engine",
    "crates/persistence",
    "crates/frontend",
    "crates/observability",
    "crates/server",
]

# Shared dependency versions — all crates inherit from here
[workspace.dependencies]
# Error handling
thiserror = "1"

# Logging
log       = "0.4"
env_logger = "0.11"   # sets up log output to stderr from RUST_LOG env var

# Channels
crossbeam = "0.8"

# Non-blocking I/O (used by frontend)
mio = { version = "1", features = ["net", "os-poll"] }

# CRC checksums (used by persistence)
crc = "3"

# Signal handling (used by server)
signal-hook = "0.3"

# Parallel iteration (used by server for parallel snapshot loading)
rayon = "1"

# Property-based testing (dev dependency)
proptest = "1"
```

## 5. Per-Crate Cargo.toml Files

### crates/common/Cargo.toml

```toml
[package]
name    = "rsdragonfly-common"
version = "0.1.0"
edition = "2021"

# Cargo feature flags to select SHARD_COUNT at compile time.
# Default is shard_count_64 (64 shards).
# Usage: cargo build --features shard_count_128
[features]
default         = []
shard_count_16  = []
shard_count_32  = []
shard_count_128 = []
# Note: shard_count_64 is the default (no feature needed).
# Only one of these should be active at a time.

[dependencies]
thiserror = { workspace = true }
crossbeam = { workspace = true }
log       = { workspace = true }
```

### crates/protocol/Cargo.toml

```toml
[package]
name    = "rsdragonfly-protocol"
version = "0.1.0"
edition = "2021"

[dependencies]
rsdragonfly-common = { path = "../common" }
log                = { workspace = true }
```

### crates/engine/Cargo.toml

```toml
[package]
name    = "rsdragonfly-engine"
version = "0.1.0"
edition = "2021"

# Forward shard_count features so the engine knows SHARD_COUNT
[features]
default         = []
shard_count_16  = ["rsdragonfly-common/shard_count_16"]
shard_count_32  = ["rsdragonfly-common/shard_count_32"]
shard_count_128 = ["rsdragonfly-common/shard_count_128"]

[dependencies]
rsdragonfly-common      = { path = "../common" }
rsdragonfly-persistence = { path = "../persistence" }
log                     = { workspace = true }
crossbeam               = { workspace = true }
```

### crates/persistence/Cargo.toml

```toml
[package]
name    = "rsdragonfly-persistence"
version = "0.1.0"
edition = "2021"

[dependencies]
rsdragonfly-common = { path = "../common" }
log                = { workspace = true }
crc                = { workspace = true }
```

### crates/frontend/Cargo.toml

```toml
[package]
name    = "rsdragonfly-frontend"
version = "0.1.0"
edition = "2021"

[features]
default         = []
shard_count_16  = ["rsdragonfly-common/shard_count_16"]
shard_count_32  = ["rsdragonfly-common/shard_count_32"]
shard_count_128 = ["rsdragonfly-common/shard_count_128"]

[dependencies]
rsdragonfly-common   = { path = "../common" }
rsdragonfly-protocol = { path = "../protocol" }
log                  = { workspace = true }
crossbeam            = { workspace = true }
mio                  = { workspace = true }
```

### crates/observability/Cargo.toml

```toml
[package]
name    = "rsdragonfly-observability"
version = "0.1.0"
edition = "2021"

[features]
default         = []
shard_count_16  = ["rsdragonfly-common/shard_count_16"]
shard_count_32  = ["rsdragonfly-common/shard_count_32"]
shard_count_128 = ["rsdragonfly-common/shard_count_128"]

[dependencies]
rsdragonfly-common = { path = "../common" }
log                = { workspace = true }
env_logger         = { workspace = true }
```

### crates/server/Cargo.toml

```toml
[package]
name    = "rsdragonfly"       # final binary name: `rsdragonfly`
version = "0.1.0"
edition = "2021"

[features]
default         = []
shard_count_16  = [
    "rsdragonfly-common/shard_count_16",
    "rsdragonfly-engine/shard_count_16",
    "rsdragonfly-frontend/shard_count_16",
    "rsdragonfly-observability/shard_count_16",
]
shard_count_32  = [
    "rsdragonfly-common/shard_count_32",
    "rsdragonfly-engine/shard_count_32",
    "rsdragonfly-frontend/shard_count_32",
    "rsdragonfly-observability/shard_count_32",
]
shard_count_128 = [
    "rsdragonfly-common/shard_count_128",
    "rsdragonfly-engine/shard_count_128",
    "rsdragonfly-frontend/shard_count_128",
    "rsdragonfly-observability/shard_count_128",
]

[dependencies]
rsdragonfly-common      = { path = "../common" }
rsdragonfly-engine      = { path = "../engine" }
rsdragonfly-persistence = { path = "../persistence" }
rsdragonfly-frontend    = { path = "../frontend" }
rsdragonfly-observability = { path = "../observability" }
log                     = { workspace = true }
env_logger              = { workspace = true }
crossbeam               = { workspace = true }
signal-hook             = { workspace = true }
rayon                   = { workspace = true }
```

## 6. Minimal src/lib.rs Stubs

Create these empty files so the workspace compiles from the start. Fill them in as you implement each plan.

```rust
// crates/common/src/lib.rs
pub mod types;
pub mod command;
pub mod error;
pub mod metrics;
pub mod config;
pub mod routing;

pub use types::{Key, Value, Entry, ShardId, Expiry};
pub use command::{Command, Operation, Response, SnapshotRequest};
pub use error::Error;
pub use metrics::{ShardMetrics, Histogram, HistogramSnapshot};
pub use config::{Config, SHARD_COUNT, MAX_KEYS_PER_SHARD, MAX_MEMORY_PER_SHARD,
                 MAX_KEY_SIZE, MAX_VALUE_SIZE, ENTRY_OVERHEAD};
pub use routing::route_key;
```

```rust
// crates/protocol/src/lib.rs
mod parser;
mod encoder;

pub use parser::{Resp2Parser, ParseResult, ParsedCommand};
pub use encoder::Resp2Encoder;

use rsdragonfly_common::{Operation, Error};
pub fn command_to_operation(cmd: ParsedCommand) -> Result<Operation, Error> {
    todo!()
}
```

Each other crate follows the same pattern: create a `lib.rs` with `pub mod X;` declarations, leaving implementations as `todo!()`. This lets the whole workspace compile (with warnings, not errors) from day one.

## 7. Build Commands

```bash
# Check all crates compile (fast, no codegen)
cargo check --workspace

# Build debug binary
cargo build

# Build release binary (optimized)
cargo build --release

# Build with 128 shards
cargo build --release --features shard_count_128

# Run the server (debug build)
cargo run -- --port 6379 --snapshot-dir /tmp/rsdragonfly

# Run all tests
cargo test --workspace

# Run tests for one crate only
cargo test -p rsdragonfly-engine

# Run a specific test
cargo test -p rsdragonfly-engine test_set_get

# Format code
cargo fmt --all

# Lint (catch common mistakes)
cargo clippy --workspace -- -D warnings

# Check for unused dependencies (install once: cargo install cargo-udeps)
cargo +nightly udeps --workspace
```

## 8. External Crate Reference

These are the only external crates used. No others should be added without discussion.

| Crate | Version | Used by | Purpose |
|-------|---------|---------|---------|
| `thiserror` | 1 | common | Derive `Error` trait with display messages |
| `log` | 0.4 | all | Logging facade (`log::info!`, `log::warn!`, etc.) |
| `env_logger` | 0.11 | server, observability | Initialize logging from `RUST_LOG` env var |
| `crossbeam` | 0.8 | common, engine, frontend, server | `channel::unbounded`, `channel::bounded` |
| `mio` | 1 | frontend | Non-blocking I/O event loop (epoll on Linux) |
| `crc` | 3 | persistence | CRC64 checksum for snapshot integrity |
| `signal-hook` | 0.3 | server | Register SIGTERM/SIGINT handler |
| `rayon` | 1 | server | Parallel snapshot loading at startup |
| `proptest` | 1 | all (dev) | Property-based testing |

### How to use each crate (quick examples)

**thiserror:**
```rust
use thiserror::Error;
#[derive(Debug, Error)]
pub enum MyError {
    #[error("key too long: {len} bytes")]
    KeyTooLong { len: usize },
}
```

**log + env_logger:**
```rust
// In main.rs (once):
env_logger::init();   // reads RUST_LOG environment variable

// Anywhere in the codebase:
log::info!("server started on port {}", port);
log::warn!("shard {} queue depth is high: {}", shard_id, depth);
log::error!("shard {} panicked", shard_id);

// Run with: RUST_LOG=debug cargo run
// Or: RUST_LOG=rsdragonfly_engine=debug cargo run   (single crate)
```

**crossbeam channels:**
```rust
use crossbeam::channel;

// Unbounded (frontend → shard):
let (tx, rx) = channel::unbounded::<Command>();
tx.send(cmd).unwrap();
let cmd = rx.recv().unwrap();

// Bounded-1 (one-shot response):
let (reply_tx, reply_rx) = channel::bounded::<Response>(1);
reply_tx.send(response).unwrap();
let response = reply_rx.recv_timeout(Duration::from_secs(5)).unwrap();

// Non-blocking send (for snapshot trigger):
match snap_tx.try_send(request) {
    Ok(()) => {}
    Err(channel::TrySendError::Full(_)) => { /* skip */ }
    Err(channel::TrySendError::Disconnected(_)) => { /* shutdown */ }
}

// Recv with timeout (shard event loop):
match cmd_rx.recv_timeout(Duration::from_millis(100)) {
    Ok(cmd) => { /* process */ }
    Err(channel::RecvTimeoutError::Timeout) => { /* maintenance */ }
    Err(channel::RecvTimeoutError::Disconnected) => break,
}
```

**mio (non-blocking I/O):**
```rust
use mio::{Events, Interest, Poll, Token};
use mio::net::{TcpListener, TcpStream};

// See plan-frontend.md for full usage.
```

**crc:**
```rust
use crc::{Crc, CRC_64_ECMA_182};

const CRC64: Crc<u64> = Crc::<u64>::new(&CRC_64_ECMA_182);

pub fn crc64(data: &[u8]) -> u64 {
    CRC64.checksum(data)
}
```

**signal-hook:**
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

let shutdown = Arc::new(AtomicBool::new(false));
signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))?;
signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))?;

// Poll in main loop:
while !shutdown.load(Ordering::Acquire) {
    std::thread::sleep(Duration::from_millis(100));
}
```

**rayon (parallel snapshot load):**
```rust
use rayon::prelude::*;

let results: Vec<_> = (0..SHARD_COUNT)
    .into_par_iter()
    .map(|i| load_snapshot(path_for_shard(i)))
    .collect();
```

## 9. IDE Setup (VS Code)

Install the `rust-analyzer` extension. Add to `.vscode/settings.json`:

```json
{
    "rust-analyzer.cargo.features": [],
    "rust-analyzer.checkOnSave.command": "clippy",
    "editor.formatOnSave": true,
    "[rust]": {
        "editor.defaultFormatter": "rust-lang.rust-analyzer"
    }
}
```

## 10. Verifying the Setup

After creating all `Cargo.toml` files and stub `lib.rs` / `main.rs` files:

```bash
cargo check --workspace
# Expected: compiles with warnings ("function is never called", "unused import", etc.)
# NOT expected: any errors
```

If you see errors, check:
- All paths in `[dependencies]` match actual directory names
- Feature flag names are spelled consistently across all Cargo.toml files
- Every `pub mod foo;` in `lib.rs` has a corresponding `foo.rs` file

## 11. Testing the Build Pipeline

After setup, run this to confirm everything works end-to-end:

```bash
# 1. Format check
cargo fmt --all -- --check

# 2. Lint (expect warnings, not errors)
cargo clippy --workspace

# 3. Test (all todo!() stubs will pass since no tests call them yet)
cargo test --workspace

# 4. Build release
cargo build --release
ls -lh target/release/rsdragonfly   # binary should exist
```

## 12. Open Questions

None. This document must be complete before any implementation begins.
