#[derive(Debug, Clone, PartialEq)]
pub enum RespValue {
    SimpleString(String),
    Error(String),
    Integer(i64),
    BulkString(Option<Vec<u8>>),  // None = null bulk string
    Array(Option<Vec<RespValue>>),
}

impl RespValue {
    pub fn ok() -> Self {
        RespValue::SimpleString("OK".to_string())
    }

    pub fn pong() -> Self {
        RespValue::SimpleString("PONG".to_string())
    }

    pub fn null() -> Self {
        RespValue::BulkString(None)
    }

    pub fn error(msg: impl Into<String>) -> Self {
        RespValue::Error(msg.into())
    }
}