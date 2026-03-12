use std::io::{Cursor};
use byteorder::{LittleEndian, WriteBytesExt};
use crc::{Crc, CRC_64_ECMA_182};
use crate::common::time::instant_to_unix_timestamp;
use super::super::persistence::format::{SnapshotHeader, HEADER_SIZE};
use super::super::shard::Shard;

const CRC64: Crc<u64> = Crc::<u64>::new(&CRC_64_ECMA_182);

pub fn serialize_shard(shard: &Shard) -> Vec<u8> {
    let estimated_size = estimate_snapshot_size(shard);
    let mut buf = Vec::with_capacity(estimated_size);

    // Write placeholder header
    let header = SnapshotHeader::new(shard.id.0, shard.data.len() as u64);
    header.write(&mut buf).unwrap();

    let data_start = buf.len();

    // Write entries
    for (key, entry) in &shard.data {
        // Key length and data
        buf.write_u32::<LittleEndian>(key.len() as u32).unwrap();
        buf.extend_from_slice(key);

        // Value length and data
        let value_bytes = entry.value.as_bytes();
        buf.write_u32::<LittleEndian>(value_bytes.len() as u32).unwrap();
        buf.extend_from_slice(value_bytes);

        // Expiry
        if let Some(expiry) = entry.expiry {
            buf.write_u8(1).unwrap();
            let unix_timestamp = instant_to_unix_timestamp(expiry);
            buf.write_u64::<LittleEndian>(unix_timestamp).unwrap();
        } else {
            buf.write_u8(0).unwrap();
        }
    }

    // Compute checksums
    let data_checksum = CRC64.checksum(&buf[data_start..]);
    let header_checksum = CRC64.checksum(&buf[0..56]);  // Exclude checksums

    // Update checksums in header
    let mut cursor = Cursor::new(&mut buf[..]);
    cursor.set_position(32);
    cursor.write_u64::<LittleEndian>(data_checksum).unwrap();
    cursor.write_u64::<LittleEndian>(header_checksum).unwrap();

    buf
}

fn estimate_snapshot_size(shard: &Shard) -> usize {
    let avg_key_size = 20;
    let avg_value_size = 100;
    let entry_overhead = 9;  // lengths + expiry flag

    HEADER_SIZE + shard.data.len() * (avg_key_size + avg_value_size + entry_overhead)
}