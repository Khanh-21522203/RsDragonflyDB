use std::io;
use common::resp::RespValue;
use std::io::{BufRead, Cursor, Read};
use common::constants::MAX_VALUE_SIZE;

#[derive(Debug)]
pub enum ParseError {
    Incomplete,
    InvalidType(u8),
    InvalidInteger,
    InvalidLength,
    InvalidFormat,
}

pub struct RespParser {
    buffer: Vec<u8>,
    cursor: usize,
}

impl RespParser {
    pub fn new() -> Self {
        RespParser {
            buffer: Vec::with_capacity(4096),
            cursor: 0,
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    pub fn parse(&mut self) -> Result<Option<RespValue>, ParseError> {
        if self.cursor >= self.buffer.len() {
            return Ok(None);
        }

        let start = self.cursor;
        let mut reader = Cursor::new(&self.buffer[start..]);

        match Self::parse_value(&mut reader) {
            Ok(value) => {
                self.cursor = start + reader.position() as usize;
                Ok(Some(value))
            }
            Err(ParseError::Incomplete) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn parse_value(reader: &mut Cursor<&[u8]>) -> Result<RespValue, ParseError> {
        let mut type_byte = [0u8; 1];
        read_exact_or_incomplete(reader, &mut type_byte)?;

        match type_byte[0] {
            b'+' => Self::parse_simple_string(reader),
            b'-' => Self::parse_error(reader),
            b':' => Self::parse_integer(reader),
            b'$' => Self::parse_bulk_string(reader),
            b'*' => Self::parse_array(reader),
            other => Err(ParseError::InvalidType(other)),
        }
    }

    fn parse_simple_string(reader: &mut Cursor<&[u8]>) -> Result<RespValue, ParseError> {
        let line = read_crlf_line(reader)?;
        Ok(RespValue::SimpleString(String::from_utf8_lossy(&line).into_owned()))
    }

    fn parse_error(reader: &mut Cursor<&[u8]>) -> Result<RespValue, ParseError> {
        let line = read_crlf_line(reader)?;
        Ok(RespValue::Error(String::from_utf8_lossy(&line).into_owned()))
    }

    fn parse_integer(reader: &mut Cursor<&[u8]>) -> Result<RespValue, ParseError> {
        let line = read_crlf_line(reader)?;
        let s = std::str::from_utf8(&line).map_err(|_| ParseError::InvalidInteger)?;
        let n = s.parse::<i64>().map_err(|_| ParseError::InvalidInteger)?;
        Ok(RespValue::Integer(n))
    }

    fn parse_bulk_string(reader: &mut Cursor<&[u8]>) -> Result<RespValue, ParseError> {
        let line = read_crlf_line(reader)?;
        let s = std::str::from_utf8(&line).map_err(|_| ParseError::InvalidInteger)?;
        let len = s.parse::<i64>().map_err(|_| ParseError::InvalidInteger)?;

        if len == -1 {
            return Ok(RespValue::BulkString(None));
        }
        if len < -1 {
            return Err(ParseError::InvalidLength);
        }

        let len = len as usize;
        if len > MAX_VALUE_SIZE {
            return Err(ParseError::InvalidLength);
        }

        let mut data = vec![0u8; len];
        read_exact_or_incomplete(reader, &mut data)?;

        // trailing CRLF
        let mut crlf = [0u8; 2];
        read_exact_or_incomplete(reader, &mut crlf)?;
        if crlf != *b"\r\n" {
            return Err(ParseError::InvalidFormat);
        }

        Ok(RespValue::BulkString(Some(data)))
    }

    fn parse_array(reader: &mut Cursor<&[u8]>) -> Result<RespValue, ParseError> {
        let line = read_crlf_line(reader)?;
        let s = std::str::from_utf8(&line).map_err(|_| ParseError::InvalidInteger)?;
        let count = s.parse::<i64>().map_err(|_| ParseError::InvalidInteger)?;

        if count == -1 {
            return Ok(RespValue::Array(None));
        }
        if count < -1 {
            return Err(ParseError::InvalidLength);
        }

        let count = count as usize;
        // TODO: if count > SOME_LIMIT { return Err(ParseError::InvalidLength); }

        let mut elements = Vec::with_capacity(count);
        for _ in 0..count {
            elements.push(Self::parse_value(reader)?);
        }
        Ok(RespValue::Array(Some(elements)))
    }

    pub fn compact(&mut self) {
        if self.cursor > 0 {
            self.buffer.drain(0..self.cursor);
            self.cursor = 0;
        }
    }
}

fn read_exact_or_incomplete<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<(), ParseError> {
    match r.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Err(ParseError::Incomplete),
        Err(_) => Err(ParseError::InvalidFormat),
    }
}

/// Read a CRLF-terminated line (excluding CRLF).
fn read_crlf_line(reader: &mut Cursor<&[u8]>) -> Result<Vec<u8>, ParseError> {
    let mut line = Vec::new();

    // Cursor<&[u8]> implements BufRead, so read_until works.
    match reader.read_until(b'\n', &mut line) {
        Ok(0) => return Err(ParseError::Incomplete), // no more bytes
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(ParseError::Incomplete),
        Err(_) => return Err(ParseError::InvalidFormat),
    }

    // Must end with \r\n
    if line.len() < 2 || &line[line.len() - 2..] != b"\r\n" {
        // If it ends with '\n' but not enough bytes for '\r\n', consider it invalid,
        // because RESP mandates CRLF.
        return Err(ParseError::InvalidFormat);
    }

    line.truncate(line.len() - 2);
    Ok(line)
}