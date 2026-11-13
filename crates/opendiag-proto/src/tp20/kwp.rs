//! KWP2000 services carried over a TP2.0 channel.
//!
//! The payloads are plain ISO 14230-3 service messages (no K-line header or
//! checksum; TP2.0 provides the framing). The services themselves are the shared
//! [`KwpService`] ones; this type only knows how to send a request over the
//! channel. Services that modify a module refuse to run unless the client is in
//! [`WriteMode::Unlocked`].

use std::time::Duration;

use opendiag_core::{Error, Result, WriteMode};
use opendiag_transport::Transport;

use super::link::Tp20Link;
use crate::clock::Clock;
use crate::kwp::KwpService;

const NEGATIVE_RESPONSE: u8 = 0x7F;
const NRC_RESPONSE_PENDING: u8 = 0x78;
const POSITIVE_OFFSET: u8 = 0x40;

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("kwp/tp20: {}", msg.into()))
}

pub struct Tp20KwpClient<T: Transport, C: Clock> {
    link: Tp20Link<T, C>,
    p2: Duration,
    p2_star: Duration,
    max_pending: u8,
    write_mode: WriteMode,
}

impl<T: Transport, C: Clock> Tp20KwpClient<T, C> {
    /// Wrap an open channel. Read-only by default.
    pub fn new(link: Tp20Link<T, C>) -> Self {
        Self {
            link,
            p2: Duration::from_millis(1000),
            p2_star: Duration::from_millis(5000),
            max_pending: 20,
            write_mode: WriteMode::ReadOnly,
        }
    }

    pub fn link(&self) -> &Tp20Link<T, C> {
        &self.link
    }

    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    pub fn set_write_mode(&mut self, mode: WriteMode) {
        self.write_mode = mode;
    }

    /// Send a service request and return the positive response (SID + 0x40 first).
    pub fn request(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let sid = *data.first().ok_or_else(|| protocol("empty request"))?;
        self.link.send(data)?;
        let mut wait = self.p2;
        let mut pending = 0u8;
        loop {
            let resp = self.link.recv(wait)?;
            match resp.as_slice() {
                [NEGATIVE_RESPONSE, s, nrc, ..] if *s == sid => {
                    if *nrc == NRC_RESPONSE_PENDING {
                        pending += 1;
                        if pending > self.max_pending {
                            return Err(protocol("too many ResponsePending replies"));
                        }
                        wait = self.p2_star;
                        continue;
                    }
                    return Err(Error::NegativeResponse {
                        service: sid,
                        nrc: *nrc,
                    });
                }
                [r, ..] if *r == sid.wrapping_add(POSITIVE_OFFSET) => return Ok(resp),
                [NEGATIVE_RESPONSE, ..] => continue, // stale reply to another service
                _ => return Err(protocol(format!("unexpected response {resp:02X?}"))),
            }
        }
    }
}

impl<T: Transport, C: Clock> KwpService for Tp20KwpClient<T, C> {
    fn request(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        Tp20KwpClient::request(self, data)
    }

    fn write_mode(&self) -> WriteMode {
        Tp20KwpClient::write_mode(self)
    }

    fn set_write_mode(&mut self, mode: WriteMode) {
        Tp20KwpClient::set_write_mode(self, mode);
    }
    // The channel is closed when the link is dropped, so `close` keeps its no-op default.
}
