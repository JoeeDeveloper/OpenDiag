//! ISO-TP (ISO 15765-2) for classic CAN, normal addressing.
//!
//! Sans-IO state machines: they take the current time as an argument and
//! return the bytes to put on the bus, so they are fully deterministic.
//! Limits: 8-byte frames, messages up to 4095 bytes (12-bit length).

mod frame;
mod receiver;
mod sender;

use std::time::Duration;

pub use frame::{FlowStatus, IsoTpFrame};
pub use receiver::{Receiver, RxEvent};
pub use sender::{Poll, Sender};

/// Maximum payload representable with the 12-bit First Frame length.
pub const MAX_MESSAGE_LEN: usize = 4095;

/// ISO-TP parameters. Timeouts follow N_Bs / N_Cr from the standard (default 1 s).
#[derive(Debug, Clone)]
pub struct Config {
    /// Block size we request when receiving (0 = send everything without more flow control).
    pub block_size: u8,
    /// Raw STmin byte we request when receiving (see [`st_min_duration`]).
    pub st_min: u8,
    /// Sender: max wait for a Flow Control frame (N_Bs).
    pub n_bs: Duration,
    /// Receiver: max wait for the next Consecutive Frame (N_Cr).
    pub n_cr: Duration,
    /// Receiver: largest message we accept; larger gets a Flow Control overflow.
    pub max_len: usize,
    /// Sender: max consecutive FC "wait" frames tolerated (N_WFTmax).
    pub max_wait_frames: u8,
    /// Pad every frame to 8 bytes with this value; `None` sends short frames.
    pub padding: Option<u8>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            block_size: 0,
            st_min: 0,
            n_bs: Duration::from_millis(1000),
            n_cr: Duration::from_millis(1000),
            max_len: MAX_MESSAGE_LEN,
            max_wait_frames: 10,
            padding: None,
        }
    }
}

/// Decode a raw STmin byte. 0x00-0x7F are milliseconds, 0xF1-0xF9 are
/// 100-900 microseconds, and reserved values are treated as the 127 ms maximum.
pub fn st_min_duration(raw: u8) -> Duration {
    match raw {
        0x00..=0x7F => Duration::from_millis(u64::from(raw)),
        0xF1..=0xF9 => Duration::from_micros(u64::from(raw - 0xF0) * 100),
        _ => Duration::from_millis(127),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn st_min_decoding() {
        assert_eq!(st_min_duration(0), Duration::ZERO);
        assert_eq!(st_min_duration(0x7F), Duration::from_millis(127));
        assert_eq!(st_min_duration(0xF1), Duration::from_micros(100));
        assert_eq!(st_min_duration(0xF9), Duration::from_micros(900));
        assert_eq!(st_min_duration(0x80), Duration::from_millis(127));
        assert_eq!(st_min_duration(0xFA), Duration::from_millis(127));
    }
}
