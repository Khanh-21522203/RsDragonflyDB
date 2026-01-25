// TODO: pre-calculate CRC16 for all keys and store in CRC16_TABLE
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc16_deterministic() {
        let key = b"mykey";
        let hash1 = crc16(key);
        let hash2 = crc16(key);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_crc16_distribution() {
        let mut counts = vec![0; 64];
        for i in 0..1_000_000 {
            let key = format!("key:{}", i);
            let hash = crc16(key.as_bytes());
            let shard = (hash as usize) & 63;
            counts[shard] += 1;
        }

        let mean = 1_000_000 / 64;
        let variance: f64 = counts.iter()
            .map(|&c| (c as f64 - mean as f64).powi(2))
            .sum::<f64>() / 64.0;
        let std_dev = variance.sqrt();

        assert!(std_dev < 200.0, "Distribution variance too high: {}", std_dev);
    }
}