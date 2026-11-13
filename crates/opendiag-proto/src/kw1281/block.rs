//! KW1281 block layout, titles and payload parsers.

use opendiag_core::{Error, Result};

/// Every block ends with this byte. It is the only byte that is not acknowledged.
pub const END_BYTE: u8 = 0x03;

/// Block titles (service codes) from the public KW1281 documentation.
pub mod title {
    pub const READ_IDENT: u8 = 0x00;
    pub const FAULT_CODES_DELETE: u8 = 0x05;
    pub const END: u8 = 0x06;
    pub const FAULT_CODES_READ: u8 = 0x07;
    pub const ACK: u8 = 0x09;
    pub const NAK: u8 = 0x0A;
    pub const GROUP_READ: u8 = 0x29;
    pub const GROUP_READ_RESPONSE: u8 = 0xE7;
    pub const ASCII_DATA: u8 = 0xF6;
    pub const FAULT_CODES_RESPONSE: u8 = 0xFC;
}

fn bad(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("kw1281: {}", msg.into()))
}

/// One protocol block (without the length and end bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Running counter shared by both directions; each block is previous + 1.
    pub counter: u8,
    pub title: u8,
    pub data: Vec<u8>,
}

impl Block {
    /// Wire bytes: `[len, counter, title, data.., 0x03]`, where `len` counts
    /// everything after itself, including the end byte.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let len = self.data.len() + 3;
        let len = u8::try_from(len).map_err(|_| bad("block data too long"))?;
        let mut out = Vec::with_capacity(usize::from(len) + 1);
        out.extend_from_slice(&[len, self.counter, self.title]);
        out.extend_from_slice(&self.data);
        out.push(END_BYTE);
        Ok(out)
    }

    /// Parse wire bytes produced by [`Block::encode`] (or received from a module).
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let (&len, rest) = bytes.split_first().ok_or_else(|| bad("empty block"))?;
        if usize::from(len) != rest.len() {
            return Err(bad(format!(
                "length byte {len} does not match {} following bytes",
                rest.len()
            )));
        }
        if len < 3 {
            return Err(bad("block shorter than counter + title + end"));
        }
        if rest[rest.len() - 1] != END_BYTE {
            return Err(bad("block does not end with 0x03"));
        }
        Ok(Self {
            counter: rest[0],
            title: rest[1],
            data: rest[2..rest.len() - 1].to_vec(),
        })
    }
}

/// Decode an ASCII data block's text and the "more data via ReadIdent" flag
/// (set when the first data byte has its top bit set).
pub fn parse_ascii(block: &Block) -> (String, bool) {
    let more = block.data.first().is_some_and(|b| *b > 0x7F);
    let text = block
        .data
        .iter()
        .map(|b| char::from(b & 0x7F))
        .collect::<String>();
    (text, more)
}

/// A stored fault: 16-bit code (shown as 5 decimal digits) and a raw status byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    pub code: u16,
    pub status: u8,
}

impl Fault {
    /// VAG-style 5-digit decimal code, e.g. `00525`.
    pub fn text(&self) -> String {
        format!("{:05}", self.code)
    }
}

/// Parse the data of a fault-codes response: 3-byte records (code hi, code lo,
/// status). A record of `FF FF 88` means "no fault" and is skipped.
pub fn parse_faults(data: &[u8]) -> Result<Vec<Fault>> {
    if data.len() % 3 != 0 {
        return Err(bad("fault data is not a multiple of 3 bytes"));
    }
    Ok(data
        .chunks_exact(3)
        .map(|c| Fault {
            code: u16::from_be_bytes([c[0], c[1]]),
            status: c[2],
        })
        .filter(|f| !(f.code == 0xFFFF && f.status == 0x88))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ack_block_wire_format() {
        let b = Block {
            counter: 0x05,
            title: title::ACK,
            data: vec![],
        };
        assert_eq!(b.encode().unwrap(), vec![0x03, 0x05, 0x09, 0x03]);
    }

    #[test]
    fn roundtrip_with_data() {
        let b = Block {
            counter: 0xFF,
            title: title::FAULT_CODES_READ,
            data: vec![1, 2, 3],
        };
        let bytes = b.encode().unwrap();
        assert_eq!(bytes[0], 6, "len = counter + title + 3 data + end");
        assert_eq!(Block::decode(&bytes).unwrap(), b);
    }

    #[test]
    fn rejects_malformed_blocks() {
        assert!(Block::decode(&[]).is_err());
        assert!(Block::decode(&[0x03, 1, 9]).is_err(), "length mismatch");
        assert!(Block::decode(&[0x03, 1, 9, 0x04]).is_err(), "bad end byte");
        assert!(Block::decode(&[0x02, 1, 0x03]).is_err(), "too short");
        let too_long = Block {
            counter: 0,
            title: 0,
            data: vec![0; 253],
        };
        assert!(too_long.encode().is_err());
        let max = Block {
            data: vec![0; 252],
            ..too_long
        };
        assert!(max.encode().is_ok());
    }

    #[test]
    fn ascii_masks_high_bit_and_reports_more_flag() {
        let b = Block {
            counter: 1,
            title: title::ASCII_DATA,
            data: vec![b'1' | 0x80, b'J', b'0'],
        };
        assert_eq!(parse_ascii(&b), ("1J0".to_string(), true));
        let b = Block {
            data: b"VW".to_vec(),
            ..b
        };
        assert_eq!(parse_ascii(&b), ("VW".to_string(), false));
    }

    #[test]
    fn faults_parse_and_skip_the_no_fault_sentinel() {
        let f = parse_faults(&[0x02, 0x0D, 0x20, 0x00, 0x11, 0x01]).unwrap();
        assert_eq!(
            f,
            vec![
                Fault {
                    code: 0x020D,
                    status: 0x20
                },
                Fault {
                    code: 0x0011,
                    status: 0x01
                }
            ]
        );
        assert_eq!(f[0].text(), "00525");
        assert!(parse_faults(&[0xFF, 0xFF, 0x88]).unwrap().is_empty());
        assert!(parse_faults(&[1, 2]).is_err());
    }
}
