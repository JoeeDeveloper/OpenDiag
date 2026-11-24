//! HSFZ ("High-Speed-Fahrzeug-Zugang") framing, BMW's diagnostic-over-IP protocol.
//!
//! Wire format (VERIFIED against the public Scapy `bmw/hsfz.py` dissector and
//! the Quantex HSFZ description, see `docs/sources.md`):
//!
//! ```text
//! +----------------+------------------+---------------------------+
//! | length: u32 BE | control: u16 BE  | payload (length bytes)    |
//! +----------------+------------------+---------------------------+
//! ```
//!
//! `length` counts the payload only (not the 6-byte header). For a diagnostic
//! message (`0x0001`) the payload is `source, target, UDS data...`, so
//! `length = 2 + data.len()`. Addresses are one byte.
//!
//! NOT VERIFIED (flagged, see README): the exact layout of the acknowledge
//! (`0x0002`) payload beyond its two address bytes, and the reply an alive
//! check (`0x0012`) expects. This client therefore ignores both.

use opendiag_core::{Error, Result};

/// TCP port of the gateway's diagnostic service.
pub const TCP_PORT: u16 = 6801;
/// UDP port used for vehicle discovery.
pub const UDP_PORT: u16 = 6811;
/// Usual tester address.
pub const DEFAULT_TESTER: u8 = 0xF4;
/// Usual gateway (ZGW) diagnostic address. Reported by discovery; this is only a fallback.
pub const DEFAULT_GATEWAY: u8 = 0x10;

pub const HEADER_LEN: usize = 6;
/// Our own sanity limit, not a protocol value: a longer length field means the
/// stream is out of step (or not HSFZ), so we refuse rather than buffer it.
pub const MAX_PAYLOAD: usize = 0x1_0000;

pub const CTRL_DIAGNOSTIC: u16 = 0x0001;
pub const CTRL_ACK: u16 = 0x0002;
pub const CTRL_TERMINAL15: u16 = 0x0010;
pub const CTRL_VEHICLE_IDENT: u16 = 0x0011;
pub const CTRL_ALIVE_CHECK: u16 = 0x0012;
pub const CTRL_STATUS_INQUIRY: u16 = 0x0013;

/// Name of an error control word (`0x40..=0x45`, `0xFF`), or `None` for other words.
pub fn error_name(control: u16) -> Option<&'static str> {
    Some(match control {
        0x0040 => "incorrect tester address",
        0x0041 => "incorrect control word",
        0x0042 => "incorrect format",
        0x0043 => "incorrect destination address",
        0x0044 => "message too large",
        0x0045 => "diagnostic application not ready",
        0x00FF => "out of memory",
        _ => return None,
    })
}

/// One HSFZ packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HsfzFrame {
    pub control: u16,
    pub payload: Vec<u8>,
}

impl HsfzFrame {
    pub fn new(control: u16, payload: Vec<u8>) -> Self {
        Self { control, payload }
    }

    /// A diagnostic message carrying one UDS request or response.
    pub fn diagnostic(source: u8, target: u8, data: &[u8]) -> Self {
        let mut payload = Vec::with_capacity(2 + data.len());
        payload.extend_from_slice(&[source, target]);
        payload.extend_from_slice(data);
        Self::new(CTRL_DIAGNOSTIC, payload)
    }

    /// Encode header and payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.control.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// `(source, target, data)` if this is a well-formed diagnostic message.
    pub fn as_diagnostic(&self) -> Option<(u8, u8, &[u8])> {
        match (self.control, self.payload.as_slice()) {
            (CTRL_DIAGNOSTIC, [s, t, data @ ..]) => Some((*s, *t, data)),
            _ => None,
        }
    }
}

/// Take one complete frame off the front of `buf`, if there is one.
///
/// Returns `Ok(None)` when more bytes are needed. A length beyond
/// [`MAX_PAYLOAD`] is an error: the stream cannot be trusted any more.
pub fn take_frame(buf: &mut Vec<u8>) -> Result<Option<HsfzFrame>> {
    if buf.len() < HEADER_LEN {
        return Ok(None);
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(Error::Protocol(format!(
            "hsfz: implausible frame length {len}; the stream is out of step"
        )));
    }
    if buf.len() < HEADER_LEN + len {
        return Ok(None);
    }
    let control = u16::from_be_bytes([buf[4], buf[5]]);
    let payload = buf[HEADER_LEN..HEADER_LEN + len].to_vec();
    buf.drain(..HEADER_LEN + len);
    Ok(Some(HsfzFrame { control, payload }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_diagnostic_message() {
        let f = HsfzFrame::diagnostic(0xF4, 0x10, &[0x22, 0xF1, 0x90]);
        assert_eq!(
            f.encode(),
            [0, 0, 0, 5, 0, 1, 0xF4, 0x10, 0x22, 0xF1, 0x90],
            "length counts source+target+data, not the header"
        );
    }

    #[test]
    fn decodes_across_split_reads_and_back_to_back_frames() {
        let a = HsfzFrame::diagnostic(0x10, 0xF4, &[0x62, 0xF1, 0x90, b'W']).encode();
        let b = HsfzFrame::new(CTRL_ACK, vec![0x10, 0xF4]).encode();
        let mut all = a.clone();
        all.extend_from_slice(&b);
        let mut buf = Vec::new();
        for chunk in all.chunks(3) {
            buf.extend_from_slice(chunk);
        }
        let first = take_frame(&mut buf).unwrap().unwrap();
        assert_eq!(first.as_diagnostic().unwrap().2, [0x62, 0xF1, 0x90, b'W']);
        let second = take_frame(&mut buf).unwrap().unwrap();
        assert_eq!(second.control, CTRL_ACK);
        assert!(take_frame(&mut buf).unwrap().is_none());
        assert!(buf.is_empty());
    }

    #[test]
    fn a_partial_frame_waits_for_more_bytes() {
        let mut buf = HsfzFrame::diagnostic(1, 2, &[3, 4, 5]).encode();
        buf.truncate(buf.len() - 1);
        assert!(take_frame(&mut buf).unwrap().is_none());
        assert_eq!(buf.len(), HEADER_LEN + 4, "nothing consumed");
    }

    #[test]
    fn an_absurd_length_is_an_error_not_a_huge_buffer() {
        let mut buf = vec![0xFF, 0xFF, 0xFF, 0xFF, 0, 1];
        assert!(take_frame(&mut buf).is_err());
    }

    #[test]
    fn a_diagnostic_frame_too_short_for_addresses_is_not_diagnostic() {
        let f = HsfzFrame::new(CTRL_DIAGNOSTIC, vec![0xF4]);
        assert!(f.as_diagnostic().is_none());
    }

    #[test]
    fn error_words_have_names() {
        assert_eq!(error_name(0x43), Some("incorrect destination address"));
        assert_eq!(error_name(CTRL_DIAGNOSTIC), None);
    }
}
