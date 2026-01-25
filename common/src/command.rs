use crate::resp::RespValue;
use crate::types::{Key, Value};

pub enum Command {
    Ping { message: Option<Vec<u8>> },
    Set { key: Key, value: Value, ttl: Option<u64> },
    Get { key: Key },
    Del { keys: Vec<Key> },
    Expire { key: Key, seconds: u64 },
    Ttl { key: Key },
    Info { section: Option<String> },
}

impl Command {
    pub fn from_resp(value: RespValue) -> Result<Self, CommandError> {
        let array = match value {
            RespValue::Array(Some(arr)) => arr,
            RespValue::Array(None) => return Err(CommandError::EmptyCommand),
            _ => return Err(CommandError::InvalidFormat),
        };

        if array.is_empty() {
            return Err(CommandError::EmptyCommand);
        }

        let cmd_name = match &array[0] {
            RespValue::BulkString(Some(bytes)) => {
                String::from_utf8_lossy(bytes).to_uppercase()
            }
            _ => return Err(CommandError::InvalidFormat),
        };

        match cmd_name.as_str() {
            "PING" => Self::parse_ping(&array[1..]),
            "SET" => Self::parse_set(&array[1..]),
            "GET" => Self::parse_get(&array[1..]),
            "DEL" => Self::parse_del(&array[1..]),
            "EXPIRE" => Self::parse_expire(&array[1..]),
            "TTL" => Self::parse_ttl(&array[1..]),
            "INFO" => Self::parse_info(&array[1..]),
            _ => Err(CommandError::UnknownCommand(cmd_name)),
        }
    }

    fn parse_ping(args: &[RespValue]) -> Result<Self, CommandError> {
        match args.len() {
            0 => Ok(Command::Ping { message: None }),
            1 => {
                let msg = extract_bulk_string(&args[0])?;
                Ok(Command::Ping { message: Some(msg) })
            }
            _ => Err(CommandError::WrongArgCount),
        }
    }

    fn parse_set(args: &[RespValue]) -> Result<Self, CommandError> {
        if args.len() < 2 {
            return Err(CommandError::WrongArgCount);
        }

        let key = extract_bulk_string(&args[0])?;
        let value = extract_bulk_string(&args[1])?;

        let mut ttl = None;
        let mut i = 2;
        while i < args.len() {
            let option = extract_bulk_string(&args[i])?;
            let option_str = String::from_utf8_lossy(&option).to_uppercase();

            match option_str.as_str() {
                "EX" => {
                    if i + 1 >= args.len() {
                        return Err(CommandError::WrongArgCount);
                    }
                    let seconds = extract_integer(&args[i + 1])?;
                    if seconds < 0 {
                        return Err(CommandError::InvalidArgument);
                    }
                    ttl = Some(seconds as u64);
                    i += 2;
                }
                _ => return Err(CommandError::InvalidArgument),
            }
        }

        Ok(Command::Set {
            key,
            value: Value::String(value),
            ttl,
        })
    }

    fn parse_get(args: &[RespValue]) -> Result<Self, CommandError> {
        if args.len() != 1 {
            return Err(CommandError::WrongArgCount);
        }
        let key = extract_bulk_string(&args[0])?;
        Ok(Command::Get { key })
    }

    fn parse_del(args: &[RespValue]) -> Result<Self, CommandError> {
        if args.is_empty() {
            return Err(CommandError::WrongArgCount);
        }
        let mut keys = Vec::new();
        for arg in args {
            keys.push(extract_bulk_string(arg)?);
        }
        Ok(Command::Del { keys })
    }

    fn parse_expire(args: &[RespValue]) -> Result<Self, CommandError> {
        if args.len() != 2 {
            return Err(CommandError::WrongArgCount);
        }
        let key = extract_bulk_string(&args[0])?;
        let seconds = extract_integer(&args[1])?;
        if seconds < 0 {
            return Err(CommandError::InvalidArgument);
        }
        Ok(Command::Expire { key, seconds: seconds as u64 })
    }

    fn parse_ttl(args: &[RespValue]) -> Result<Self, CommandError> {
        if args.len() != 1 {
            return Err(CommandError::WrongArgCount);
        }
        let key = extract_bulk_string(&args[0])?;
        Ok(Command::Ttl { key })
    }

    fn parse_info(args: &[RespValue]) -> Result<Self, CommandError> {
        let section = if args.is_empty() {
            None
        } else {
            let s = extract_bulk_string(&args[0])?;
            Some(String::from_utf8_lossy(&s).into_owned())
        };
        Ok(Command::Info { section })
    }
}

fn extract_bulk_string(value: &RespValue) -> Result<Vec<u8>, CommandError> {
    match value {
        RespValue::BulkString(Some(bytes)) => Ok(bytes.clone()),
        RespValue::BulkString(None) => Err(CommandError::InvalidArgument),
        _ => Err(CommandError::InvalidFormat),
    }
}

fn extract_integer(value: &RespValue) -> Result<i64, CommandError> {
    match value {
        RespValue::Integer(n) => Ok(*n),
        RespValue::BulkString(Some(bytes)) => {
            let s = String::from_utf8_lossy(bytes);
            s.parse::<i64>().map_err(|_| CommandError::InvalidArgument)
        }
        RespValue::BulkString(None) => Err(CommandError::InvalidArgument),
        _ => Err(CommandError::InvalidFormat),
    }
}

#[derive(Debug)]
pub enum CommandError {
    InvalidFormat,
    EmptyCommand,
    UnknownCommand(String),
    WrongArgCount,
    InvalidArgument,
}