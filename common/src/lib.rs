pub mod types;
pub mod shard_id;
pub mod constants;
pub mod hash;
pub mod threads;
pub mod channels;
pub mod cpu_pinning;
pub mod time;
pub mod resp;
pub mod command;

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = add(2, 2);
        assert_eq!(result, 4);
    }
}
