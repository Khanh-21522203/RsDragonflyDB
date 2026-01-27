use crate::resp::RespValue;

pub fn serialize_resp2(value: &RespValue) -> Vec<u8> {
    let mut buf = Vec::new();
    serialize_value(value, &mut buf);
    buf
}

fn serialize_value(value: &RespValue, buf: &mut Vec<u8>) {
    match value {
        RespValue::SimpleString(s) => {
            buf.push(b'+');
            buf.extend_from_slice(s.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        RespValue::Error(s) => {
            buf.push(b'-');
            buf.extend_from_slice(s.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        RespValue::Integer(n) => {
            buf.push(b':');
            buf.extend_from_slice(n.to_string().as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        RespValue::BulkString(None) => {
            buf.extend_from_slice(b"$-1\r\n");
        }
        RespValue::BulkString(Some(data)) => {
            buf.push(b'$');
            buf.extend_from_slice(data.len().to_string().as_bytes());
            buf.extend_from_slice(b"\r\n");
            buf.extend_from_slice(data);
            buf.extend_from_slice(b"\r\n");
        }
        RespValue::Array(opt) => match opt.as_deref() {
            None => {
                buf.extend_from_slice(b"*-1\r\n");
            }
            Some(elements) => {
                buf.push(b'*');
                buf.extend_from_slice(elements.len().to_string().as_bytes());
                buf.extend_from_slice(b"\r\n");
                for elem in elements {
                    serialize_value(elem, buf);
                }
            }
        },
    }
}