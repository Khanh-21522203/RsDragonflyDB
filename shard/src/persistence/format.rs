use std::io::{self, Read, Write, Cursor};
use std::time::{SystemTime, UNIX_EPOCH};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

pub const MAGIC: &[u8; 4] = b"RSDF";
pub const VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 64;

pub struct SnapshotHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub shard_id: u16,
    pub reserved1: u16,
    pub key_count: u64,
    pub timestamp: u64,
    pub data_checksum: u64,
    pub header_checksum: u64,
    pub reserved2: [u8; 20],
}

impl SnapshotHeader {
    pub fn new(shard_id: u16, key_count: u64) -> Self {
        SnapshotHeader {
            magic: *MAGIC,
            version: VERSION,
            shard_id,
            reserved1: 0,
            key_count,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            data_checksum: 0,
            header_checksum: 0,
            reserved2: [0; 20],
        }
    }

    pub fn write<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(&self.magic)?;
        writer.write_u32::<LittleEndian>(self.version)?;
        writer.write_u16::<LittleEndian>(self.shard_id)?;
        writer.write_u16::<LittleEndian>(self.reserved1)?;
        writer.write_u64::<LittleEndian>(self.key_count)?;
        writer.write_u64::<LittleEndian>(self.timestamp)?;
        writer.write_u64::<LittleEndian>(self.data_checksum)?;
        writer.write_u64::<LittleEndian>(self.header_checksum)?;
        writer.write_all(&self.reserved2)?;
        Ok(())
    }

    pub fn read<R: Read>(reader: &mut R) -> io::Result<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;

        let version = reader.read_u32::<LittleEndian>()?;
        let shard_id = reader.read_u16::<LittleEndian>()?;
        let reserved1 = reader.read_u16::<LittleEndian>()?;
        let key_count = reader.read_u64::<LittleEndian>()?;
        let timestamp = reader.read_u64::<LittleEndian>()?;
        let data_checksum = reader.read_u64::<LittleEndian>()?;
        let header_checksum = reader.read_u64::<LittleEndian>()?;

        let mut reserved2 = [0u8; 20];
        reader.read_exact(&mut reserved2)?;

        Ok(SnapshotHeader {
            magic,
            version,
            shard_id,
            reserved1,
            key_count,
            timestamp,
            data_checksum,
            header_checksum,
            reserved2,
        })
    }
}