use common::types::Value;

#[derive(Debug, Clone)]
pub enum Response {
    Value(Option<Value>),
    Integer(i64),
    Ok,
    Error(String),
}

impl Response {
    pub fn ok() -> Self {
        Response::Ok
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Response::Error(msg.into())
    }
}