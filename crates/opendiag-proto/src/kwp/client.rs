//! KWP2000 client over a K-line transport.
//!
//! Safety: services that modify an ECU (ClearDiagnosticInformation,
//! WriteDataByLocalIdentifier, SecurityAccess) refuse to run unless the client
//! is in [`WriteMode::Unlocked`]. Backup and confirmation live in the services layer.

use std::time::Duration;

use opendiag_core::{Error, Result, WriteMode};
use opendiag_transport::{Frame, Transport};

use super::frame::{Message, Parser};
use super::*;
use crate::uds::SeedKey;

const NEGATIVE_RESPONSE: u8 = 0x7F;
const NRC_RESPONSE_PENDING: u8 = 0x78;
const POSITIVE_OFFSET: u8 = 0x40;

#[derive(Debug, Clone)]
pub struct KwpConfig {
    /// Our address (conventionally 0xF1).
    pub tester: u8,
    /// The ECU's address.
    pub ecu: u8,
    /// Wait for a first response.
    pub p2: Duration,
    /// Wait after a ResponsePending (0x78).
    pub p2_star: Duration,
    pub max_pending: u8,
}

impl KwpConfig {
    pub fn new(ecu: u8) -> Self {
        Self {
            tester: 0xF1,
            ecu,
            p2: Duration::from_millis(1000),
            p2_star: Duration::from_millis(5000),
            max_pending: 20,
        }
    }
}

/// Key bytes returned by StartCommunication (they describe the timing/format options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBytes {
    pub kb1: u8,
    pub kb2: u8,
}

/// A KWP2000 trouble code: 2-byte code plus status byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KwpDtc {
    pub code: u16,
    pub status: u8,
}

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("kwp: {}", msg.into()))
}

pub struct KwpClient<T: Transport> {
    transport: T,
    parser: Parser,
    cfg: KwpConfig,
    write_mode: WriteMode,
}

impl<T: Transport> KwpClient<T> {
    /// New client, read-only by default.
    pub fn new(transport: T, cfg: KwpConfig) -> Self {
        Self {
            transport,
            parser: Parser::new(),
            cfg,
            write_mode: WriteMode::ReadOnly,
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    pub fn set_write_mode(&mut self, mode: WriteMode) {
        self.write_mode = mode;
    }

    fn require_unlocked(&self, what: &str) -> Result<()> {
        match self.write_mode {
            WriteMode::Unlocked => Ok(()),
            WriteMode::ReadOnly => Err(Error::WritesLocked(what.into())),
        }
    }

    fn read_message(&mut self, wait: Duration) -> Result<Message> {
        loop {
            if let Some(m) = self.parser.next_message()? {
                return Ok(m);
            }
            if let Frame::Raw(bytes) = self.transport.recv(wait)? {
                self.parser.push(&bytes);
            }
        }
    }

    /// Send a request (SID + parameters) and return the positive response data
    /// (starting with SID + 0x40). Skips our own echo and other modules' traffic.
    pub fn request(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let sid = *data.first().ok_or_else(|| protocol("empty request"))?;
        let bytes = Message {
            target: self.cfg.ecu,
            source: self.cfg.tester,
            data: data.to_vec(),
        }
        .encode()?;
        self.parser.clear();
        self.transport.send(&Frame::Raw(bytes))?;

        let mut wait = self.cfg.p2;
        let mut pending = 0u8;
        loop {
            let m = self.read_message(wait)?;
            if m.source != self.cfg.ecu || m.target != self.cfg.tester {
                continue; // our echo, or someone else
            }
            match m.data.as_slice() {
                [NEGATIVE_RESPONSE, s, nrc, ..] if *s == sid => {
                    if *nrc == NRC_RESPONSE_PENDING {
                        pending += 1;
                        if pending > self.cfg.max_pending {
                            return Err(protocol("too many ResponsePending replies"));
                        }
                        wait = self.cfg.p2_star;
                        continue;
                    }
                    return Err(Error::NegativeResponse {
                        service: sid,
                        nrc: *nrc,
                    });
                }
                [r, ..] if *r == sid.wrapping_add(POSITIVE_OFFSET) => return Ok(m.data),
                [NEGATIVE_RESPONSE, ..] => continue, // stale reply to another service
                _ => return Err(protocol(format!("unexpected response {:02X?}", m.data))),
            }
        }
    }

    /// Fast init: wake-up pattern then StartCommunication (0x81).
    pub fn start_communication(&mut self) -> Result<KeyBytes> {
        self.transport.fast_init()?;
        let r = self.request(&[SID_START_COMMUNICATION])?;
        match r.as_slice() {
            [_, kb1, kb2, ..] => Ok(KeyBytes {
                kb1: *kb1,
                kb2: *kb2,
            }),
            _ => Err(protocol(format!(
                "short StartCommunication response {r:02X?}"
            ))),
        }
    }

    pub fn stop_communication(&mut self) -> Result<()> {
        self.request(&[SID_STOP_COMMUNICATION])?;
        Ok(())
    }
}

impl<T: Transport> KwpService for KwpClient<T> {
    fn request(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        KwpClient::request(self, data)
    }

    fn write_mode(&self) -> WriteMode {
        KwpClient::write_mode(self)
    }

    fn set_write_mode(&mut self, mode: WriteMode) {
        KwpClient::set_write_mode(self, mode);
    }

    fn close(&mut self) {
        let _ = self.stop_communication();
    }
}

impl<T: Transport> KwpClient<T> {
    /// WriteDataByLocalIdentifier (0x3B). Requires unlocked writes; back up first.
    pub fn write_data_by_local_identifier(&mut self, id: u8, data: &[u8]) -> Result<()> {
        self.require_unlocked("WriteDataByLocalIdentifier")?;
        let mut req = vec![SID_WRITE_DATA_BY_LOCAL_ID, id];
        req.extend_from_slice(data);
        let r = self.request(&req)?;
        if r.get(1) != Some(&id) {
            return Err(protocol(format!("local id echo mismatch {r:02X?}")));
        }
        Ok(())
    }

    /// SecurityAccess (0x27) at odd `level`. Requires unlocked writes. An all-zero
    /// seed means already unlocked. No seed-key algorithms are bundled.
    pub fn security_access(&mut self, level: u8, algo: &dyn SeedKey) -> Result<()> {
        self.require_unlocked("SecurityAccess")?;
        if level % 2 == 0 {
            return Err(protocol("security level must be odd (seed request)"));
        }
        let r = self.request(&[SID_SECURITY_ACCESS, level])?;
        if r.get(1) != Some(&level) {
            return Err(protocol(format!("bad seed response {r:02X?}")));
        }
        let seed = &r[2..];
        if seed.iter().all(|&b| b == 0) {
            return Ok(());
        }
        let mut req = vec![SID_SECURITY_ACCESS, level + 1];
        req.extend_from_slice(&algo.key(level, seed)?);
        let r = self.request(&req)?;
        if r.get(1) != Some(&(level + 1)) {
            return Err(protocol(format!("bad key response {r:02X?}")));
        }
        Ok(())
    }
}
