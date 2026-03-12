use std::collections::{HashMap, BinaryHeap};
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::common::constants::{MAX_KEYS_PER_SHARD, MAX_KEY_SIZE, MAX_MEMORY_PER_SHARD, MAX_VALUE_SIZE};
use crate::common::shard_id::ShardId;
use crate::common::types::{Key, Value};
use crate::protocol::command::Command;
use crate::protocol::response::Response;
use super::entry::Entry;
use super::expiry::Expiry;

pub struct Shard {
    pub id: ShardId,
    pub data: HashMap<Key, Entry>,
    pub ttl_queue: BinaryHeap<Expiry>,
    pub metrics: ShardMetrics,
    pub max_memory_bytes: usize,
    pub max_keys: usize,
}

impl Shard {
    pub fn new(id: ShardId) -> Self {
        Shard {
            id,
            data: HashMap::with_capacity(1_000_000),
            ttl_queue: BinaryHeap::new(),
            metrics: ShardMetrics::new(),
            max_memory_bytes: MAX_MEMORY_PER_SHARD,
            max_keys: MAX_KEYS_PER_SHARD,
        }
    }

    pub fn execute(&mut self, command: Command) -> Response {
        let start = Instant::now();

        let response = match command {
            Command::Ping { message } => self.cmd_ping(message),
            Command::Set { key, value, ttl } => self.cmd_set(key, value, ttl),
            Command::Get { key } => self.cmd_get(&key),
            Command::Del { keys } => self.cmd_del(&keys),
            Command::Expire { key, seconds } => self.cmd_expire(&key, seconds),
            Command::Ttl { key } => self.cmd_ttl(&key),
            Command::Info { section } => self.cmd_info(section),
        };

        let latency = start.elapsed();
        self.metrics.record_command(latency);

        response
    }

    fn cmd_ping(&self, message: Option<Vec<u8>>) -> Response {
        match message {
            None => Response::Value(Some(Value::String(b"PONG".to_vec()))),
            Some(msg) => Response::Value(Some(Value::String(msg))),
        }
    }

    fn cmd_set(&mut self, key: Key, value: Value, ttl: Option<u64>) -> Response {
        if key.len() > MAX_KEY_SIZE {
            return Response::Error("ERR key too large".to_string());
        }

        if value.size_bytes() > MAX_VALUE_SIZE {
            return Response::Error("ERR value too large".to_string());
        }

        // Check key count limit
        if !self.data.contains_key(&key) && self.data.len() >= self.max_keys {
            return Response::Error("ERR shard full".to_string());
        }

        // Compute expiry
        let expiry = ttl.map(|secs| Instant::now() + Duration::from_secs(secs));

        // Compute memory delta precisely for overwrite vs insert
        let new_size = Self::entry_size_bytes(&key, &value);

        // If overwriting, subtract old size first
        let old_size = self
            .data
            .get(&key)
            .map(|e| Self::entry_size_bytes(&key, &e.value))
            .unwrap_or(0);

        // Check memory limit using delta (avoid double-count)
        let current_memory = self.metrics.memory_bytes.load(Ordering::Relaxed);
        let projected = current_memory + new_size - old_size;
        if projected > self.max_memory_bytes {
            return Response::Error(
                "ERR OOM command not allowed when used memory > 'maxmemory'".to_string(),
            );
        }

        // Insert entry
        let entry = Entry::new(value, expiry);
        self.data.insert(key.clone(), entry);

        // Update TTL queue
        if let Some(exp_time) = expiry {
            self.ttl_queue.push(Expiry {
                expiry_time: exp_time,
                key: key.clone(),
            });
        }

        // Update metrics (apply delta)
        if old_size > 0 {
            self.metrics
                .memory_bytes
                .fetch_sub(old_size, Ordering::Relaxed);
        }
        self.metrics
            .memory_bytes
            .fetch_add(new_size, Ordering::Relaxed);
        self.metrics.key_count.store(self.data.len(), Ordering::Relaxed);

        Response::Ok
    }

    fn cmd_get(&mut self, key: &Key) -> Response {
        // Peek first
        let expired = match self.data.get(key) {
            Some(entry) => entry.is_expired(),
            None => return Response::Value(None),
        };

        if expired {
            // Remove + update metrics
            self.remove_key_update_metrics(key);
            self.metrics.keys_expired.fetch_add(1, Ordering::Relaxed);
            return Response::Value(None);
        }

        // Safe to clone after expiry check
        let value = self.data.get(key).unwrap().value.clone();
        Response::Value(Some(value))
    }

    fn cmd_del(&mut self, keys: &[Key]) -> Response {
        let mut deleted = 0i64;

        for key in keys {
            if self.remove_key_update_metrics(key) {
                deleted += 1;
            }
        }

        Response::Integer(deleted)
    }

    fn cmd_expire(&mut self, key: &Key, seconds: u64) -> Response {
        match self.data.get_mut(key) {
            Some(entry) => {
                let expiry_time = Instant::now() + Duration::from_secs(seconds);
                entry.expiry = Some(expiry_time);

                self.ttl_queue.push(Expiry {
                    expiry_time,
                    key: key.clone(),
                });

                Response::Integer(1)
            }
            None => Response::Integer(0),
        }
    }

    fn cmd_ttl(&mut self, key: &Key) -> Response {
        // Redis-like:
        // -2: key does not exist
        // -1: key exists but no expiry
        // >=0 remaining seconds
        let expiry = match self.data.get(key) {
            None => return Response::Integer(-2),
            Some(entry) => entry.expiry,
        };

        match expiry {
            None => Response::Integer(-1),
            Some(exp) => {
                let now = Instant::now();
                if now >= exp {
                    // Expired => delete it (important!) and return -2
                    self.remove_key_update_metrics(key);
                    self.metrics.keys_expired.fetch_add(1, Ordering::Relaxed);
                    Response::Integer(-2)
                } else {
                    let remaining = exp.duration_since(now);
                    Response::Integer(remaining.as_secs() as i64)
                }
            }
        }
    }

    fn cmd_info(&self, section: Option<String>) -> Response {
        let mut info = String::new();

        match section.as_deref() {
            None | Some("server") => {
                info.push_str("# Server\r\n");
                info.push_str(&format!("shard_id:{}\r\n", self.id.0));
                info.push_str(&format!("uptime_seconds:{}\r\n",
                                       self.metrics.uptime_seconds()));
            }
            Some("stats") => {
                info.push_str("# Stats\r\n");
                info.push_str(&format!("total_commands_processed:{}\r\n",
                                       self.metrics.commands_processed.load(Ordering::Relaxed)));
                info.push_str(&format!("keys_expired:{}\r\n",
                                       self.metrics.keys_expired.load(Ordering::Relaxed)));
            }
            Some("memory") => {
                info.push_str("# Memory\r\n");
                info.push_str(&format!("used_memory:{}\r\n",
                                       self.metrics.memory_bytes.load(Ordering::Relaxed)));
                info.push_str(&format!("key_count:{}\r\n",
                                       self.metrics.key_count.load(Ordering::Relaxed)));
            }
            Some(_) => {
                return Response::Error("ERR unknown section".to_string());
            }
        }

        Response::Value(Some(Value::String(info.into_bytes())))
    }

    #[inline]
    fn entry_size_bytes(key: &Key, value: &Value) -> usize {
        key.len() + value.size_bytes() + size_of::<Entry>()
    }

    #[inline]
    fn remove_key_update_metrics(&mut self, key: &Key) -> bool {
        if let Some(old) = self.data.remove(key) {
            let old_size = Self::entry_size_bytes(key, &old.value);
            self.metrics.memory_bytes.fetch_sub(old_size, Ordering::Relaxed);
            self.metrics.key_count.store(self.data.len(), Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

pub struct ShardMetrics {
    pub commands_processed: AtomicU64,
    pub keys_expired: AtomicU64,
    pub snapshots_written: AtomicU64,
    pub snapshots_skipped: AtomicU64,
    pub snapshot_write_errors: AtomicU64,
    pub key_count: AtomicUsize,
    pub memory_bytes: AtomicUsize,
    pub queue_depth: AtomicUsize,
    start_time: Instant,
}

impl ShardMetrics {
    pub fn new() -> Self {
        ShardMetrics {
            commands_processed: AtomicU64::new(0),
            keys_expired: AtomicU64::new(0),
            snapshots_written: AtomicU64::new(0),
            snapshots_skipped: AtomicU64::new(0),
            snapshot_write_errors: AtomicU64::new(0),
            key_count: AtomicUsize::new(0),
            memory_bytes: AtomicUsize::new(0),
            queue_depth: AtomicUsize::new(0),
            start_time: Instant::now(),
        }
    }

    pub fn record_command(&self, latency: Duration) {
        self.commands_processed.fetch_add(1, Ordering::Relaxed);
        // TODO: Record latency histogram
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }
}