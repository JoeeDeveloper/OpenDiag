//! KWP2000 (ISO 14230) on K-line: framing (-2) and diagnostic services (-3).
//!
//! The client works on a byte stream carried in `Frame::Raw`. On a single-wire
//! K-line the interface echoes everything the tester transmits; the echo parses
//! as a message whose source is the tester, so the client ignores it.
//! Inter-byte and inter-message timing (P1..P4) is the transport driver's job.

mod client;
mod frame;
mod service;
pub mod sim;

pub use client::{KeyBytes, KwpClient, KwpConfig, KwpDtc};
pub use frame::{checksum, Message, Parser};
pub use service::KwpService;

// Service identifiers (ISO 14230-3).
pub const SID_START_DIAGNOSTIC_SESSION: u8 = 0x10;
pub const SID_CLEAR_DTC: u8 = 0x14;
pub const SID_READ_DTC_BY_STATUS: u8 = 0x18;
pub const SID_READ_ECU_IDENTIFICATION: u8 = 0x1A;
pub const SID_READ_DATA_BY_LOCAL_ID: u8 = 0x21;
pub const SID_SECURITY_ACCESS: u8 = 0x27;
pub const SID_WRITE_DATA_BY_LOCAL_ID: u8 = 0x3B;
pub const SID_TESTER_PRESENT: u8 = 0x3E;
pub const SID_START_COMMUNICATION: u8 = 0x81;
pub const SID_STOP_COMMUNICATION: u8 = 0x82;
