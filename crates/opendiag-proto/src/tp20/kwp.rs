//! KWP2000 services carried over a TP2.0 channel.
//!
//! The payloads are plain ISO 14230-3 service messages (no K-line header or
//! checksum; TP2.0 provides the framing). Services that modify a module refuse
//! to run unless the client is in [`WriteMode::Unlocked`].

use std::time::Duration;

use opendiag_core::{Error, Result, WriteMode};
use opendiag_transport::Transport;

use super::link::Tp20Link;
use crate::clock::Clock;
use crate::kwp::{
    KwpDtc, SID_CLEAR_DTC, SID_READ_DATA_BY_LOCAL_ID, SID_READ_DTC_BY_STATUS,
    SID_READ_ECU_IDENTIFICATION, SID_START_DIAGNOSTIC_SESSION, SID_TESTER_PRESENT,
};

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

    fn require_unlocked(&self, what: &str) -> Result<()> {
        match self.write_mode {
            WriteMode::Unlocked => Ok(()),
            WriteMode::ReadOnly => Err(Error::WritesLocked(what.into())),
        }
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

    pub fn start_diagnostic_session(&mut self, mode: u8) -> Result<()> {
        let r = self.request(&[SID_START_DIAGNOSTIC_SESSION, mode])?;
        if r.get(1) != Some(&mode) {
            return Err(protocol(format!("bad session response {r:02X?}")));
        }
        Ok(())
    }

    pub fn tester_present(&mut self) -> Result<()> {
        self.request(&[SID_TESTER_PRESENT])?;
        Ok(())
    }

    /// ReadECUIdentification (0x1A): the data after the option echo.
    pub fn read_ecu_identification(&mut self, option: u8) -> Result<Vec<u8>> {
        let r = self.request(&[SID_READ_ECU_IDENTIFICATION, option])?;
        if r.get(1) != Some(&option) {
            return Err(protocol(format!("option echo mismatch {r:02X?}")));
        }
        Ok(r[2..].to_vec())
    }

    /// ReadDataByLocalIdentifier (0x21): the data after the id echo.
    pub fn read_data_by_local_identifier(&mut self, id: u8) -> Result<Vec<u8>> {
        let r = self.request(&[SID_READ_DATA_BY_LOCAL_ID, id])?;
        if r.get(1) != Some(&id) {
            return Err(protocol(format!("local id echo mismatch {r:02X?}")));
        }
        Ok(r[2..].to_vec())
    }

    /// ReadDiagnosticTroubleCodesByStatus (0x18). `group` 0xFF00 means all groups.
    pub fn read_dtcs_by_status(&mut self, status: u8, group: u16) -> Result<Vec<KwpDtc>> {
        let [gh, gl] = group.to_be_bytes();
        let r = self.request(&[SID_READ_DTC_BY_STATUS, status, gh, gl])?;
        let count = usize::from(*r.get(1).ok_or_else(|| protocol("missing DTC count"))?);
        let records = &r[2..];
        if records.len() != count * 3 {
            return Err(protocol(format!(
                "DTC count {count} does not match {} data bytes",
                records.len()
            )));
        }
        Ok(records
            .chunks_exact(3)
            .map(|c| KwpDtc {
                code: u16::from_be_bytes([c[0], c[1]]),
                status: c[2],
            })
            .collect())
    }

    /// ClearDiagnosticInformation (0x14). Requires unlocked writes.
    pub fn clear_dtcs(&mut self, group: u16) -> Result<()> {
        self.require_unlocked("ClearDiagnosticInformation")?;
        let [gh, gl] = group.to_be_bytes();
        self.request(&[SID_CLEAR_DTC, gh, gl])?;
        Ok(())
    }
}
