pub type Key = Vec<u8>;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(Vec<u8>),
    // TODO: Future: Integer(i64), List(Vec<Value>), etc.
}

impl Value {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Value::String(bytes) => bytes,
        }
    }

    pub fn size_bytes(&self) -> usize {
        match self {
            Value::String(bytes) => bytes.len(),
        }
    }
}