//! ISO 14230-2 message framing: `Fmt [Tgt Src] [Len] Data... Checksum`.
//!
//! Supported: physical (0b10) and functional (0b11) addressing with target and
//! source bytes, length in the format byte (1-63) or in an extra length byte
//! (up to 255). The "no address" and CARB header forms are rejected.

use opendiag_core::{Error, Result};

fn bad(msg: &str) -> Error {
    Error::Protocol(format!("kwp: {msg}"))
}

/// Modulo-256 sum of all bytes.
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b))
}

/// One decoded message: addressing plus the service data (SID + parameters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub target: u8,
    pub source: u8,
    pub data: Vec<u8>,
}

impl Message {
    /// Serialise with physical addressing.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let len = self.data.len();
        if len == 0 || len > 255 {
            return Err(bad("data length must be 1..=255"));
        }
        let mut out = Vec::with_capacity(len + 5);
        if len <= 63 {
            out.extend_from_slice(&[0x80 | len as u8, self.target, self.source]);
        } else {
            out.extend_from_slice(&[0x80, self.target, self.source, len as u8]);
        }
        out.extend_from_slice(&self.data);
        out.push(checksum(&out));
        Ok(out)
    }
}

/// Incremental stream parser. Push whatever bytes arrive, in any chunking,
/// then drain complete messages.
#[derive(Debug, Default)]
pub struct Parser {
    buf: Vec<u8>,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Drop any buffered partial data.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Next complete message, `None` if more bytes are needed. On a framing or
    /// checksum error the buffer is discarded so the stream can resynchronise.
    pub fn next_message(&mut self) -> Result<Option<Message>> {
        let Some(&fmt) = self.buf.first() else {
            return Ok(None);
        };
        if fmt >> 6 < 2 {
            self.buf.clear();
            return Err(bad("unsupported header form (no address / CARB)"));
        }
        if self.buf.len() < 3 {
            return Ok(None);
        }
        let short_len = usize::from(fmt & 0x3F);
        let header = if short_len == 0 { 4 } else { 3 };
        if self.buf.len() < header {
            return Ok(None);
        }
        let data_len = if short_len == 0 {
            usize::from(self.buf[3])
        } else {
            short_len
        };
        if data_len == 0 {
            self.buf.clear();
            return Err(bad("zero-length message"));
        }
        let total = header + data_len + 1;
        if self.buf.len() < total {
            return Ok(None);
        }
        let expected = checksum(&self.buf[..total - 1]);
        let got = self.buf[total - 1];
        if expected != got {
            self.buf.clear();
            return Err(bad(&format!(
                "checksum mismatch (expected {expected:#04x}, got {got:#04x})"
            )));
        }
        let msg = Message {
            target: self.buf[1],
            source: self.buf[2],
            data: self.buf[header..total - 1].to_vec(),
        };
        self.buf.drain(..total);
        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(data: &[u8]) -> Message {
        Message {
            target: 0x01,
            source: 0xF1,
            data: data.to_vec(),
        }
    }

    #[test]
    fn encodes_short_message_with_checksum() {
        // 0x82 01 F1 | 3E 01 | cs: sum = 0x1B3, low byte 0xB3
        let b = msg(&[0x3E, 0x01]).encode().unwrap();
        assert_eq!(b, vec![0x82, 0x01, 0xF1, 0x3E, 0x01, 0xB3]);
    }

    #[test]
    fn long_message_uses_length_byte() {
        let data = vec![0x7A; 100];
        let b = msg(&data).encode().unwrap();
        assert_eq!(&b[..4], &[0x80, 0x01, 0xF1, 100]);
        let mut p = Parser::new();
        p.push(&b);
        assert_eq!(p.next_message().unwrap().unwrap().data, data);
    }

    #[test]
    fn rejects_bad_lengths_on_encode() {
        assert!(msg(&[]).encode().is_err());
        assert!(msg(&[0; 256]).encode().is_err());
        assert!(msg(&[0; 255]).encode().is_ok());
    }

    #[test]
    fn parses_byte_by_byte_and_back_to_back() {
        let a = msg(&[0x81]).encode().unwrap();
        let b = msg(&[0x21, 0x01]).encode().unwrap();
        let mut p = Parser::new();
        let mut got = Vec::new();
        for byte in a.iter().chain(b.iter()) {
            p.push(&[*byte]);
            while let Some(m) = p.next_message().unwrap() {
                got.push(m);
            }
        }
        assert_eq!(got, vec![msg(&[0x81]), msg(&[0x21, 0x01])]);
    }

    #[test]
    fn checksum_error_discards_buffer() {
        let mut b = msg(&[0x81]).encode().unwrap();
        *b.last_mut().unwrap() ^= 0xFF;
        let mut p = Parser::new();
        p.push(&b);
        assert!(p.next_message().is_err());
        assert_eq!(p.next_message().unwrap(), None);
    }

    #[test]
    fn rejects_unsupported_header_forms() {
        for first in [0x00u8, 0x03, 0x43] {
            let mut p = Parser::new();
            p.push(&[first, 1, 2, 3]);
            assert!(p.next_message().is_err(), "{first:#x}");
        }
    }

    #[test]
    fn partial_input_waits() {
        let b = msg(&[0x21, 0x01]).encode().unwrap();
        let mut p = Parser::new();
        p.push(&b[..b.len() - 1]);
        assert_eq!(p.next_message().unwrap(), None);
        p.push(&b[b.len() - 1..]);
        assert!(p.next_message().unwrap().is_some());
    }
}
