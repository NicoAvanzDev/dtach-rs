use std::io::{self, Read, Write};

use crate::args::RedrawMethod;

const MAGIC: &[u8; 4] = b"DTRS";
const HEADER_LEN: usize = 9;
const MAX_PAYLOAD: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Message {
    Hello(String),
    Attach,
    Detach,
    Push(Vec<u8>),
    Resize(TerminalSize),
    Redraw {
        method: RedrawMethod,
        size: TerminalSize,
    },
}

impl TerminalSize {
    pub fn fallback() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

pub fn write_message(mut writer: impl Write, message: &Message) -> io::Result<()> {
    let (kind, payload) = encode_message(message);
    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(MAGIC);
    header[4] = kind;
    header[5..9].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    writer.write_all(&header)?;
    writer.write_all(&payload)?;
    writer.flush()
}

pub fn read_message(mut reader: impl Read) -> io::Result<Message> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header)?;
    if &header[..4] != MAGIC {
        return Err(invalid("invalid protocol magic"));
    }

    let kind = header[4];
    let len = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(invalid("payload too large"));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    decode_message(kind, payload)
}

fn encode_message(message: &Message) -> (u8, Vec<u8>) {
    match message {
        Message::Hello(token) => (1, token.as_bytes().to_vec()),
        Message::Attach => (2, Vec::new()),
        Message::Detach => (3, Vec::new()),
        Message::Push(bytes) => (4, bytes.clone()),
        Message::Resize(size) => (5, encode_size(*size).to_vec()),
        Message::Redraw { method, size } => {
            let mut payload = Vec::with_capacity(5);
            payload.push(method.to_wire());
            payload.extend_from_slice(&encode_size(*size));
            (6, payload)
        }
    }
}

fn decode_message(kind: u8, payload: Vec<u8>) -> io::Result<Message> {
    match kind {
        1 => Ok(Message::Hello(
            String::from_utf8(payload).map_err(|_| invalid("token is not utf-8"))?,
        )),
        2 if payload.is_empty() => Ok(Message::Attach),
        3 if payload.is_empty() => Ok(Message::Detach),
        4 => Ok(Message::Push(payload)),
        5 => Ok(Message::Resize(decode_size(&payload)?)),
        6 => {
            if payload.len() != 5 {
                return Err(invalid("invalid redraw payload"));
            }
            let method =
                RedrawMethod::from_wire(payload[0]).ok_or_else(|| invalid("bad redraw method"))?;
            Ok(Message::Redraw {
                method,
                size: decode_size(&payload[1..])?,
            })
        }
        _ => Err(invalid("unknown protocol message")),
    }
}

fn encode_size(size: TerminalSize) -> [u8; 4] {
    let rows = size.rows.to_be_bytes();
    let cols = size.cols.to_be_bytes();
    [rows[0], rows[1], cols[0], cols[1]]
}

fn decode_size(payload: &[u8]) -> io::Result<TerminalSize> {
    if payload.len() != 4 {
        return Err(invalid("invalid size payload"));
    }
    Ok(TerminalSize {
        rows: u16::from_be_bytes([payload[0], payload[1]]),
        cols: u16::from_be_bytes([payload[2], payload[3]]),
    })
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_redraw() {
        let input = Message::Redraw {
            method: RedrawMethod::Winch,
            size: TerminalSize {
                rows: 33,
                cols: 120,
            },
        };
        let mut bytes = Vec::new();
        write_message(&mut bytes, &input).unwrap();
        assert_eq!(read_message(bytes.as_slice()).unwrap(), input);
    }
}
