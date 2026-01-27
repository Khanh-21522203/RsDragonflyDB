use std::io::{self, Cursor, Read};
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use byteorder::{LittleEndian, ReadBytesExt};
use crc::{Crc, CRC_64_ECMA_182};
use common::shard_id::ShardId;
use common::time::unix_timestamp_to_instant;
use common::types::Value;
use crate::shard::Shard;
use crate::entry::Entry;
use crate::expiry::Expiry;
use crate::persistence::format::{SnapshotHeader, HEADER_SIZE, MAGIC, VERSION};

const CRC64: Crc<u64> = Crc::<u64>::new(&CRC_64_ECMA_182);

pub fn deserialize_snapshot(data: &[u8]) -> Result<Shard, SnapshotError> {
    if data.len() < HEADER_SIZE {
        return Err(SnapshotError::TooSmall);
    }

    let mut cursor = Cursor::new(data);

    // Read header
    let header = SnapshotHeader::read(&mut cursor)?;

    // Validate magic
    if &header.magic != MAGIC {
        return Err(SnapshotError::InvalidMagic);
    }

    // Validate version
    if header.version != VERSION {
        return Err(SnapshotError::UnsupportedVersion(header.version));
    }

    // Verify header checksum
    let computed_header_checksum = CRC64.checksum(&data[0..56]);
    if header.header_checksum != computed_header_checksum {
        return Err(SnapshotError::HeaderChecksumMismatch);
    }

    // Verify data checksum
    let computed_data_checksum = CRC64.checksum(&data[HEADER_SIZE..]);
    if header.data_checksum != computed_data_checksum {
        return Err(SnapshotError::DataChecksumMismatch);
    }

    // Parse entries
    let mut shard = Shard::new(ShardId(header.shard_id));
    let now = Instant::now();
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    for _ in 0..header.key_count {
        // Read key
        let key_len = cursor.read_u32::<LittleEndian>()? as usize;
        let mut key = vec![0u8; key_len];
        cursor.read_exact(&mut key)?;

        // Read value
        let value_len = cursor.read_u32::<LittleEndian>()? as usize;
        let mut value = vec![0u8; value_len];
        cursor.read_exact(&mut value)?;

        // Read expiry
        let has_expiry = cursor.read_u8()? == 1;
        let expiry = if has_expiry {
            let expiry_timestamp = cursor.read_u64::<LittleEndian>()?;

            // Skip expired keys
            if expiry_timestamp <= now_unix {
                continue;
            }

            Some(unix_timestamp_to_instant(expiry_timestamp))
        } else {
            None
        };

        // Insert into shard
        let entry = Entry::new(Value::String(value), expiry);
        shard.data.insert(key.clone(), entry);

        // Add to TTL queue if has expiry
        if let Some(exp_time) = expiry {
            shard.ttl_queue.push(Expiry {
                expiry_time: exp_time,
                key,
            });
        }
    }

    shard.metrics.key_count.store(shard.data.len(), Ordering::Relaxed);

    log::info!("Loaded shard {}: {} keys (snapshot timestamp: {})",
        header.shard_id, shard.data.len(), header.timestamp);

    Ok(shard)
}

#[derive(Debug)]
pub enum SnapshotError {
    TooSmall,
    InvalidMagic,
    UnsupportedVersion(u32),
    HeaderChecksumMismatch,
    DataChecksumMismatch,
    Io(io::Error),
}

impl From<io::Error> for SnapshotError {
    fn from(e: io::Error) -> Self {
        SnapshotError::Io(e)
    }
}