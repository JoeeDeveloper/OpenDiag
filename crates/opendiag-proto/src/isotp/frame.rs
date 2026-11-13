//! ISO-TP frame (N_PDU) encoding and decoding.

use opendiag_core::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStatus {
    ContinueToSend,
    Wait,
    Overflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsoTpFrame {
    Single {
        data: Vec<u8>,
    },
    First {
        total_len: usize,
        data: Vec<u8>,
    },
    Consecutive {
        sn: u8,
        data: Vec<u8>,
    },
    FlowControl {
        status: FlowStatus,
        block_size: u8,
        st_min: u8,
    },
}

fn bad(msg: &str) -> Error {
    Error::Protocol(format!("isotp: {msg}"))
}

impl IsoTpFrame {
    /// Parse a CAN payload. Trailing padding is ignored where the length is explicit.
    pub fn decode(b: &[u8]) -> Result<Self> {
        let first = *b.first().ok_or_else(|| bad("empty frame"))?;
        match first >> 4 {
            0x0 => {
                let len = usize::from(first & 0x0F);
                if len == 0 || len > 7 {
                    return Err(bad("invalid single frame length"));
                }
                let data = b
                    .get(1..1 + len)
                    .ok_or_else(|| bad("single frame truncated"))?;
                Ok(Self::Single {
                    data: data.to_vec(),
                })
            }
            0x1 => {
                if b.len() < 8 {
                    return Err(bad("first frame shorter than 8 bytes"));
                }
                let total_len = (usize::from(first & 0x0F) << 8) | usize::from(b[1]);
                if total_len < 8 {
                    return Err(bad("first frame length must be at least 8"));
                }
                Ok(Self::First {
                    total_len,
                    data: b[2..8].to_vec(),
                })
            }
            0x2 => Ok(Self::Consecutive {
                sn: first & 0x0F,
                data: b[1..].to_vec(),
            }),
            0x3 => {
                if b.len() < 3 {
                    return Err(bad("flow control truncated"));
                }
                let status = match first & 0x0F {
                    0 => FlowStatus::ContinueToSend,
                    1 => FlowStatus::Wait,
                    2 => FlowStatus::Overflow,
                    _ => return Err(bad("invalid flow status")),
                };
                Ok(Self::FlowControl {
                    status,
                    block_size: b[1],
                    st_min: b[2],
                })
            }
            _ => Err(bad("unknown frame type")),
        }
    }

    /// Serialise to a CAN payload, padded to 8 bytes with `padding` if given.
    pub fn encode(&self, padding: Option<u8>) -> Vec<u8> {
        let mut out = match self {
            Self::Single { data } => {
                let mut v = vec![data.len() as u8];
                v.extend_from_slice(data);
                v
            }
            Self::First { total_len, data } => {
                let mut v = vec![0x10 | (*total_len >> 8) as u8, *total_len as u8];
                v.extend_from_slice(data);
                v
            }
            Self::Consecutive { sn, data } => {
                let mut v = vec![0x20 | (sn & 0x0F)];
                v.extend_from_slice(data);
                v
            }
            Self::FlowControl {
                status,
                block_size,
                st_min,
            } => {
                let fs = match status {
                    FlowStatus::ContinueToSend => 0,
                    FlowStatus::Wait => 1,
                    FlowStatus::Overflow => 2,
                };
                vec![0x30 | fs, *block_size, *st_min]
            }
        };
        if let Some(p) = padding {
            if out.len() < 8 {
                out.resize(8, p);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_frame_roundtrip_and_padding() {
        let f = IsoTpFrame::Single {
            data: vec![0x22, 0xF1, 0x90],
        };
        assert_eq!(f.encode(None), vec![0x03, 0x22, 0xF1, 0x90]);
        let padded = f.encode(Some(0xAA));
        assert_eq!(padded, vec![0x03, 0x22, 0xF1, 0x90, 0xAA, 0xAA, 0xAA, 0xAA]);
        assert_eq!(IsoTpFrame::decode(&padded).unwrap(), f);
    }

    #[test]
    fn first_frame_encodes_12_bit_length() {
        let f = IsoTpFrame::First {
            total_len: 0x123,
            data: vec![1, 2, 3, 4, 5, 6],
        };
        let b = f.encode(None);
        assert_eq!(&b[..2], &[0x11, 0x23]);
        assert_eq!(IsoTpFrame::decode(&b).unwrap(), f);
    }

    #[test]
    fn consecutive_and_flow_control_roundtrip() {
        let cf = IsoTpFrame::Consecutive {
            sn: 5,
            data: vec![1, 2, 3, 4, 5, 6, 7],
        };
        assert_eq!(IsoTpFrame::decode(&cf.encode(None)).unwrap(), cf);
        let fc = IsoTpFrame::FlowControl {
            status: FlowStatus::Wait,
            block_size: 8,
            st_min: 0xF3,
        };
        assert_eq!(fc.encode(None), vec![0x31, 8, 0xF3]);
        assert_eq!(IsoTpFrame::decode(&fc.encode(Some(0))).unwrap(), fc);
    }

    #[test]
    fn rejects_invalid_frames() {
        for bad in [
            &[][..],
            &[0x00, 0, 0, 0, 0, 0, 0, 0],    // SF length 0
            &[0x08, 0, 0, 0, 0, 0, 0, 0],    // SF length 8
            &[0x05, 1, 2],                   // SF truncated
            &[0x10, 0x05, 1, 2, 3, 4, 5, 6], // FF length < 8
            &[0x10, 0x20, 1, 2],             // FF short
            &[0x33, 0, 0],                   // bad flow status
            &[0x30, 0],                      // FC truncated
            &[0x40, 0, 0, 0],                // unknown type
        ] {
            assert!(IsoTpFrame::decode(bad).is_err(), "{bad:?}");
        }
    }
}
