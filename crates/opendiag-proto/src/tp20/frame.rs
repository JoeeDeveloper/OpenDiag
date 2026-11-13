//! TP2.0 message encoding and decoding.

use std::time::Duration;

use opendiag_core::{Error, Result};

fn bad(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("tp20: {}", msg.into()))
}

/// CAN id that channel-setup requests are sent on. The reply comes back on
/// `0x200 + module address`.
pub const SETUP_REQUEST_ID: u32 = 0x200;
/// Application type observed for KWP2000 channels.
pub const APP_KWP: u8 = 0x01;

const OP_SETUP_REQUEST: u8 = 0xC0;
const OP_SETUP_OK: u8 = 0xD0;
const OP_SETUP_REFUSED: std::ops::RangeInclusive<u8> = 0xD6..=0xD8;

/// Encode an 11-bit id as the two setup bytes: low byte, then a validity nibble
/// (0 valid, 1 invalid) over the top 3 id bits.
fn encode_id(id: Option<u16>) -> [u8; 2] {
    match id {
        Some(id) => [id as u8, ((id >> 8) & 0x7) as u8],
        None => [0x00, 0x10],
    }
}

fn decode_id(low: u8, high: u8) -> Result<Option<u16>> {
    if high >> 4 != 0 {
        return Ok(None);
    }
    let pref = u16::from(high & 0x0F);
    if pref > 7 {
        return Err(bad("CAN id prefix above 0x7"));
    }
    Ok(Some((pref << 8) | u16::from(low)))
}

/// Channel setup request, sent by the tester. The ids are from the *module's*
/// point of view: `rx` is where it should listen (usually left invalid) and
/// `tx` is where it should transmit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupRequest {
    pub dest: u8,
    pub rx: Option<u16>,
    pub tx: Option<u16>,
    pub app: u8,
}

impl SetupRequest {
    pub fn encode(&self) -> [u8; 7] {
        let rx = encode_id(self.rx);
        let tx = encode_id(self.tx);
        [
            self.dest,
            OP_SETUP_REQUEST,
            rx[0],
            rx[1],
            tx[0],
            tx[1],
            self.app,
        ]
    }
}

/// Positive setup reply. Here the ids are from the *tester's* point of view:
/// `rx` is the id the tester receives on, `tx` the id it must transmit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetupResponse {
    pub dest: u8,
    pub rx: Option<u16>,
    pub tx: Option<u16>,
    pub app: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupReply {
    Positive(SetupResponse),
    Refused { opcode: u8 },
}

fn check_len(bytes: &[u8], want: usize, what: &str) -> Result<()> {
    if bytes.len() == want {
        Ok(())
    } else {
        Err(bad(format!(
            "{what} must be {want} bytes, got {}",
            bytes.len()
        )))
    }
}

impl SetupReply {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        check_len(bytes, 7, "setup reply")?;
        match bytes[1] {
            OP_SETUP_OK => Ok(Self::Positive(SetupResponse {
                dest: bytes[0],
                rx: decode_id(bytes[2], bytes[3])?,
                tx: decode_id(bytes[4], bytes[5])?,
                app: bytes[6],
            })),
            op if OP_SETUP_REFUSED.contains(&op) => Ok(Self::Refused { opcode: op }),
            op => Err(bad(format!("unexpected setup opcode {op:#04X}"))),
        }
    }
}

/// Encode a positive setup reply (used by the virtual module).
pub fn encode_setup_reply(r: &SetupResponse) -> [u8; 7] {
    let rx = encode_id(r.rx);
    let tx = encode_id(r.tx);
    [r.dest, OP_SETUP_OK, rx[0], rx[1], tx[0], tx[1], r.app]
}

/// Decode a setup request (used by the virtual module).
pub fn decode_setup_request(bytes: &[u8]) -> Result<SetupRequest> {
    check_len(bytes, 7, "setup request")?;
    if bytes[1] != OP_SETUP_REQUEST {
        return Err(bad(format!(
            "not a setup request: opcode {:#04X}",
            bytes[1]
        )));
    }
    Ok(SetupRequest {
        dest: bytes[0],
        rx: decode_id(bytes[2], bytes[3])?,
        tx: decode_id(bytes[4], bytes[5])?,
        app: bytes[6],
    })
}

/// A timing parameter byte: top 2 bits pick the unit (0.1 ms, 1 ms, 10 ms,
/// 100 ms), the low 6 bits scale it. `0xFF` means "not used".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing(pub u8);

impl Timing {
    pub const UNUSED: Timing = Timing(0xFF);

    pub fn duration(self) -> Option<Duration> {
        if self == Self::UNUSED {
            return None;
        }
        let scale = u64::from(self.0 & 0x3F);
        Some(match self.0 >> 6 {
            0 => Duration::from_micros(scale * 100),
            1 => Duration::from_millis(scale),
            2 => Duration::from_millis(scale * 10),
            _ => Duration::from_millis(scale * 100),
        })
    }

    /// The finest-grained encoding of `d` that fits (rounding to the nearest step).
    pub fn from_duration(d: Duration) -> Self {
        for (units, step_us) in [(0u8, 100u128), (1, 1_000), (2, 10_000), (3, 100_000)] {
            let scale = (d.as_micros() + step_us / 2) / step_us;
            // Units 3 with scale 63 would collide with the "unused" byte.
            if scale <= 63 && !(units == 3 && scale == 63) {
                return Timing((units << 6) | scale as u8);
            }
        }
        Timing(0xFE)
    }
}

/// The six parameter bytes after the 0xA0/0xA1 opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelParams {
    /// Packets sent before an ACK is expected.
    pub block_size: u8,
    /// Time to wait for an ACK.
    pub t1: Timing,
    pub t2: Timing,
    /// Interval between packets.
    pub t3: Timing,
    pub t4: Timing,
}

/// Data packet kind (the high nibble of the first byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataOp {
    /// Waiting for an ACK; more packets follow (block size reached).
    WaitAckMore = 0,
    /// Waiting for an ACK; this is the last packet.
    WaitAckLast = 1,
    /// No ACK expected; more packets follow.
    NoAckMore = 2,
    /// No ACK expected; this is the last packet.
    NoAckLast = 3,
}

impl DataOp {
    pub fn wants_ack(self) -> bool {
        matches!(self, Self::WaitAckMore | Self::WaitAckLast)
    }
}

/// A message on an open channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelMsg {
    ParamsRequest(ChannelParams),
    ParamsResponse(ChannelParams),
    /// Channel test / keep-alive (answered with a parameters response).
    Test,
    Break,
    Disconnect,
    Data {
        op: DataOp,
        seq: u8,
        payload: Vec<u8>,
    },
    /// Acknowledgement carrying the next sequence number expected.
    Ack {
        ready: bool,
        seq: u8,
    },
}

fn params(bytes: &[u8]) -> Result<ChannelParams> {
    check_len(bytes, 6, "parameters message")?;
    Ok(ChannelParams {
        block_size: bytes[1],
        t1: Timing(bytes[2]),
        t2: Timing(bytes[3]),
        t3: Timing(bytes[4]),
        t4: Timing(bytes[5]),
    })
}

impl ChannelMsg {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let &first = bytes.first().ok_or_else(|| bad("empty message"))?;
        match first {
            0xA0 => Ok(Self::ParamsRequest(params(bytes)?)),
            0xA1 => Ok(Self::ParamsResponse(params(bytes)?)),
            0xA3 => check_len(bytes, 1, "channel test").map(|()| Self::Test),
            0xA4 => check_len(bytes, 1, "break").map(|()| Self::Break),
            0xA8 => check_len(bytes, 1, "disconnect").map(|()| Self::Disconnect),
            0xA2 | 0xA5..=0xA7 | 0xA9..=0xAF => {
                Err(bad(format!("unknown control opcode {first:#04X}")))
            }
            _ => {
                let seq = first & 0x0F;
                match first >> 4 {
                    0x0 => data(DataOp::WaitAckMore, seq, bytes),
                    0x1 => data(DataOp::WaitAckLast, seq, bytes),
                    0x2 => data(DataOp::NoAckMore, seq, bytes),
                    0x3 => data(DataOp::NoAckLast, seq, bytes),
                    0x9 => check_len(bytes, 1, "ack").map(|()| Self::Ack { ready: false, seq }),
                    0xB => check_len(bytes, 1, "ack").map(|()| Self::Ack { ready: true, seq }),
                    n => Err(bad(format!("unknown message type {n:#X}"))),
                }
            }
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let params =
            |op: u8, p: &ChannelParams| vec![op, p.block_size, p.t1.0, p.t2.0, p.t3.0, p.t4.0];
        match self {
            Self::ParamsRequest(p) => params(0xA0, p),
            Self::ParamsResponse(p) => params(0xA1, p),
            Self::Test => vec![0xA3],
            Self::Break => vec![0xA4],
            Self::Disconnect => vec![0xA8],
            Self::Data { op, seq, payload } => {
                let mut v = vec![((*op as u8) << 4) | (seq & 0x0F)];
                v.extend_from_slice(payload);
                v
            }
            Self::Ack { ready, seq } => {
                vec![(if *ready { 0xB0 } else { 0x90 }) | (seq & 0x0F)]
            }
        }
    }
}

fn data(op: DataOp, seq: u8, bytes: &[u8]) -> Result<ChannelMsg> {
    if bytes.len() > 8 {
        return Err(bad("data packet longer than 8 bytes"));
    }
    Ok(ChannelMsg::Data {
        op,
        seq,
        payload: bytes[1..].to_vec(),
    })
}

/// Split a message into packet bodies (the bytes after the op/seq byte). The
/// first body starts with the 2-byte big-endian message length, then 5 data
/// bytes; the rest carry 7 data bytes each.
pub fn segment(payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    if payload.is_empty() {
        return Err(bad("empty message"));
    }
    let len = u16::try_from(payload.len()).map_err(|_| bad("message longer than 65535 bytes"))?;
    let split = payload.len().min(5);
    let mut first = len.to_be_bytes().to_vec();
    first.extend_from_slice(&payload[..split]);
    let mut out = vec![first];
    out.extend(payload[split..].chunks(7).map(<[u8]>::to_vec));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_request_matches_the_documented_capture() {
        // 200: 01 C0 00 10 00 03 01 - ask module 0x01 to transmit on 0x300.
        let req = SetupRequest {
            dest: 0x01,
            rx: None,
            tx: Some(0x300),
            app: APP_KWP,
        };
        assert_eq!(req.encode(), [0x01, 0xC0, 0x00, 0x10, 0x00, 0x03, 0x01]);
        assert_eq!(decode_setup_request(&req.encode()).unwrap(), req);
    }

    #[test]
    fn setup_reply_matches_the_documented_capture() {
        // 201: 00 D0 00 03 40 07 01 - tester listens on 0x300, transmits on 0x740.
        let reply = SetupReply::decode(&[0x00, 0xD0, 0x00, 0x03, 0x40, 0x07, 0x01]).unwrap();
        assert_eq!(
            reply,
            SetupReply::Positive(SetupResponse {
                dest: 0,
                rx: Some(0x300),
                tx: Some(0x740),
                app: 1
            })
        );
        let SetupReply::Positive(r) = reply else {
            unreachable!()
        };
        assert_eq!(
            encode_setup_reply(&r),
            [0x00, 0xD0, 0x00, 0x03, 0x40, 0x07, 0x01]
        );
    }

    #[test]
    fn setup_refusals_and_garbage() {
        for op in [0xD6, 0xD7, 0xD8] {
            let r = SetupReply::decode(&[0, op, 0, 0x10, 0, 0x10, 1]).unwrap();
            assert_eq!(r, SetupReply::Refused { opcode: op });
        }
        assert!(SetupReply::decode(&[0, 0xD1, 0, 0, 0, 0, 0]).is_err());
        assert!(SetupReply::decode(&[0, 0xD0]).is_err(), "short");
        assert!(
            SetupReply::decode(&[0, 0xD0, 0, 0x0F, 0, 0, 1]).is_err(),
            "prefix > 7"
        );
    }

    #[test]
    fn timing_bytes_decode_like_the_capture() {
        assert_eq!(Timing(0x8A).duration(), Some(Duration::from_millis(100)));
        assert_eq!(Timing(0x4A).duration(), Some(Duration::from_millis(10)));
        assert_eq!(Timing(0x32).duration(), Some(Duration::from_millis(5)));
        assert_eq!(Timing(0xFF).duration(), None);
    }

    #[test]
    fn timing_encoding_roundtrips_and_picks_fine_units() {
        assert_eq!(
            Timing::from_duration(Duration::from_millis(5)),
            Timing(0x32)
        );
        assert_eq!(
            Timing::from_duration(Duration::from_millis(100)),
            Timing(0x8A)
        );
        assert_eq!(
            Timing::from_duration(Duration::from_millis(10)),
            Timing(0x4A)
        );
        for ms in [1u64, 5, 10, 63, 64, 100, 500, 1000, 6000] {
            let d = Duration::from_millis(ms);
            let back = Timing::from_duration(d).duration().unwrap();
            let err = back.abs_diff(d);
            assert!(
                err <= d / 10 + Duration::from_micros(500),
                "{ms} ms -> {back:?}"
            );
        }
        assert_ne!(
            Timing::from_duration(Duration::from_secs(100)),
            Timing::UNUSED
        );
    }

    #[test]
    fn parameter_messages_match_the_capture() {
        let p = ChannelParams {
            block_size: 0x0F,
            t1: Timing(0x8A),
            t2: Timing::UNUSED,
            t3: Timing(0x32),
            t4: Timing::UNUSED,
        };
        let bytes = ChannelMsg::ParamsRequest(p).encode();
        assert_eq!(bytes, [0xA0, 0x0F, 0x8A, 0xFF, 0x32, 0xFF]);
        assert_eq!(
            ChannelMsg::decode(&bytes).unwrap(),
            ChannelMsg::ParamsRequest(p)
        );
        let resp = ChannelMsg::decode(&[0xA1, 0x0F, 0x8A, 0xFF, 0x4A, 0xFF]).unwrap();
        assert!(matches!(resp, ChannelMsg::ParamsResponse(r) if r.t3 == Timing(0x4A)));
    }

    #[test]
    fn data_and_ack_messages_match_the_capture() {
        // 740: 10 00 02 10 89 - last packet, wants ACK, seq 0, length 2.
        let m = ChannelMsg::decode(&[0x10, 0x00, 0x02, 0x10, 0x89]).unwrap();
        assert_eq!(
            m,
            ChannelMsg::Data {
                op: DataOp::WaitAckLast,
                seq: 0,
                payload: vec![0x00, 0x02, 0x10, 0x89]
            }
        );
        assert_eq!(m.encode(), [0x10, 0x00, 0x02, 0x10, 0x89]);
        assert_eq!(
            ChannelMsg::decode(&[0xB1]).unwrap(),
            ChannelMsg::Ack {
                ready: true,
                seq: 1
            }
        );
        assert_eq!(
            ChannelMsg::Ack {
                ready: false,
                seq: 3
            }
            .encode(),
            [0x93]
        );
        assert!(matches!(
            ChannelMsg::decode(&[0x21, 0, 0x1A, 0x61, 1, 1, 0, 0]).unwrap(),
            ChannelMsg::Data {
                op: DataOp::NoAckMore,
                seq: 1,
                ..
            }
        ));
    }

    #[test]
    fn control_messages_roundtrip_and_garbage_is_rejected() {
        for m in [ChannelMsg::Test, ChannelMsg::Break, ChannelMsg::Disconnect] {
            assert_eq!(ChannelMsg::decode(&m.encode()).unwrap(), m);
        }
        for bad in [
            &[][..],
            &[0xA2],
            &[0xAF],
            &[0xA3, 0],
            &[0xA0, 1, 2],
            &[0xB1, 0],
            &[0x40],
            &[0xC0],
        ] {
            assert!(ChannelMsg::decode(bad).is_err(), "{bad:02X?}");
        }
    }

    #[test]
    fn segmentation_matches_the_26_byte_capture() {
        let payload: Vec<u8> = (0..26).collect();
        let parts = segment(&payload).unwrap();
        // 5 bytes in the first packet after the 2-byte length, then 7 each.
        assert_eq!(
            parts.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![7, 7, 7, 7]
        );
        assert_eq!(&parts[0][..2], &[0x00, 26]);
        assert_eq!(
            parts
                .iter()
                .flat_map(|p| p.iter().copied())
                .skip(2)
                .collect::<Vec<_>>(),
            payload
        );
        assert_eq!(segment(&[1, 2, 3]).unwrap(), vec![vec![0, 3, 1, 2, 3]]);
        assert!(segment(&[]).is_err());
        assert!(segment(&vec![0; 65536]).is_err());
        assert!(segment(&vec![0; 65535]).is_ok());
    }
}
