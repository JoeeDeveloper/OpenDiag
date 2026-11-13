//! UDS (ISO 14229) client over ISO-TP.
//!
//! Safety: every service that can modify an ECU (SecurityAccess, WriteDataByIdentifier,
//! RoutineControl, ClearDiagnosticInformation) refuses to run unless the client is in
//! [`WriteMode::Unlocked`]. Backups and confirmation prompts belong to the services
//! layer above; this layer only enforces the gate.

use std::time::Duration;

use opendiag_core::{Error, Result, WriteMode};
use opendiag_transport::Transport;

use crate::clock::Clock;
use crate::isotp::IsoTpLink;

// Service identifiers (ISO 14229-1).
pub const SID_SESSION_CONTROL: u8 = 0x10;
pub const SID_CLEAR_DTC: u8 = 0x14;
pub const SID_READ_DTC: u8 = 0x19;
pub const SID_READ_DID: u8 = 0x22;
pub const SID_SECURITY_ACCESS: u8 = 0x27;
pub const SID_WRITE_DID: u8 = 0x2E;
pub const SID_ROUTINE_CONTROL: u8 = 0x31;
pub const SID_TESTER_PRESENT: u8 = 0x3E;
const NEGATIVE_RESPONSE: u8 = 0x7F;
const NRC_RESPONSE_PENDING: u8 = 0x78;
const POSITIVE_OFFSET: u8 = 0x40;

/// Standard diagnostic sessions.
pub const SESSION_DEFAULT: u8 = 0x01;
pub const SESSION_PROGRAMMING: u8 = 0x02;
pub const SESSION_EXTENDED: u8 = 0x03;

/// Human-readable name for a negative response code, where standardised.
pub fn nrc_name(nrc: u8) -> &'static str {
    match nrc {
        0x10 => "generalReject",
        0x11 => "serviceNotSupported",
        0x12 => "subFunctionNotSupported",
        0x13 => "incorrectMessageLengthOrInvalidFormat",
        0x14 => "responseTooLong",
        0x21 => "busyRepeatRequest",
        0x22 => "conditionsNotCorrect",
        0x24 => "requestSequenceError",
        0x31 => "requestOutOfRange",
        0x33 => "securityAccessDenied",
        0x35 => "invalidKey",
        0x36 => "exceededNumberOfAttempts",
        0x37 => "requiredTimeDelayNotExpired",
        0x70 => "uploadDownloadNotAccepted",
        0x72 => "generalProgrammingFailure",
        0x78 => "requestCorrectlyReceivedResponsePending",
        0x7E => "subFunctionNotSupportedInActiveSession",
        0x7F => "serviceNotSupportedInActiveSession",
        _ => "unknown",
    }
}

/// Pluggable seed -> key algorithm for SecurityAccess. Implement it only for
/// algorithms that are publicly documented; none are bundled.
pub trait SeedKey {
    fn key(&self, level: u8, seed: &[u8]) -> Result<Vec<u8>>;
}

#[derive(Debug, Clone)]
pub struct UdsConfig {
    /// Wait for a first response (P2Client).
    pub p2: Duration,
    /// Wait after a ResponsePending (0x78) (P2*Client).
    pub p2_star: Duration,
    /// Give up after this many ResponsePending replies to one request.
    pub max_pending: u8,
}

impl Default for UdsConfig {
    fn default() -> Self {
        Self {
            p2: Duration::from_millis(1000),
            p2_star: Duration::from_millis(5000),
            max_pending: 20,
        }
    }
}

/// Timing parameters an ECU reports when a session starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTimings {
    pub p2: Duration,
    pub p2_star: Duration,
}

/// One stored DTC: 3-byte code plus status byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dtc {
    pub code: u32,
    pub status: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutineAction {
    Start = 0x01,
    Stop = 0x02,
    RequestResults = 0x03,
}

pub struct UdsClient<T: Transport, C: Clock> {
    link: IsoTpLink<T, C>,
    cfg: UdsConfig,
    write_mode: WriteMode,
}

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("uds: {}", msg.into()))
}

impl<T: Transport, C: Clock> UdsClient<T, C> {
    /// New client, read-only by default.
    pub fn new(link: IsoTpLink<T, C>, cfg: UdsConfig) -> Self {
        Self {
            link,
            cfg,
            write_mode: WriteMode::ReadOnly,
        }
    }

    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    pub fn set_write_mode(&mut self, mode: WriteMode) {
        self.write_mode = mode;
    }

    pub fn link(&self) -> &IsoTpLink<T, C> {
        &self.link
    }

    fn require_unlocked(&self, what: &str) -> Result<()> {
        match self.write_mode {
            WriteMode::Unlocked => Ok(()),
            WriteMode::ReadOnly => Err(Error::WritesLocked(what.into())),
        }
    }

    /// Send a request and return the positive response (starting with SID + 0x40).
    /// Handles ResponsePending (0x78) and surfaces other negative responses as errors.
    pub fn request(&mut self, req: &[u8]) -> Result<Vec<u8>> {
        let sid = *req.first().ok_or_else(|| protocol("empty request"))?;
        self.link.send(req)?;
        let mut wait = self.cfg.p2;
        let mut pending = 0u8;
        loop {
            let resp = self.link.recv(wait)?;
            match resp.as_slice() {
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
                [r, ..] if *r == sid.wrapping_add(POSITIVE_OFFSET) => return Ok(resp),
                // A reply to some other service (stale): ignore and keep waiting.
                [NEGATIVE_RESPONSE, ..] => continue,
                _ => return Err(protocol(format!("unexpected response {resp:02X?}"))),
            }
        }
    }

    /// DiagnosticSessionControl (0x10).
    pub fn session_control(&mut self, session: u8) -> Result<SessionTimings> {
        let r = self.request(&[SID_SESSION_CONTROL, session])?;
        if r.len() < 6 || r[1] != session {
            return Err(protocol(format!("bad session response {r:02X?}")));
        }
        let p2 = u64::from(u16::from_be_bytes([r[2], r[3]]));
        let p2_star = u64::from(u16::from_be_bytes([r[4], r[5]])) * 10;
        Ok(SessionTimings {
            p2: Duration::from_millis(p2),
            p2_star: Duration::from_millis(p2_star),
        })
    }

    /// TesterPresent (0x3E 00) keep-alive.
    pub fn tester_present(&mut self) -> Result<()> {
        let r = self.request(&[SID_TESTER_PRESENT, 0x00])?;
        if r.get(1) != Some(&0x00) {
            return Err(protocol(format!("bad tester present response {r:02X?}")));
        }
        Ok(())
    }

    /// ReadDataByIdentifier (0x22) for one DID; returns the data after the DID echo.
    pub fn read_data_by_identifier(&mut self, did: u16) -> Result<Vec<u8>> {
        let [hi, lo] = did.to_be_bytes();
        let r = self.request(&[SID_READ_DID, hi, lo])?;
        if r.len() < 3 || r[1] != hi || r[2] != lo {
            return Err(protocol(format!("DID echo mismatch in {r:02X?}")));
        }
        Ok(r[3..].to_vec())
    }

    /// ReadDTCInformation (0x19 02): DTCs matching `status_mask`.
    pub fn read_dtcs_by_status_mask(&mut self, status_mask: u8) -> Result<Vec<Dtc>> {
        let r = self.request(&[SID_READ_DTC, 0x02, status_mask])?;
        if r.len() < 3 || r[1] != 0x02 {
            return Err(protocol(format!("bad ReadDTC response {r:02X?}")));
        }
        let records = &r[3..];
        if records.len() % 4 != 0 {
            return Err(protocol("DTC records not a multiple of 4 bytes"));
        }
        Ok(records
            .chunks_exact(4)
            .map(|c| Dtc {
                code: u32::from_be_bytes([0, c[0], c[1], c[2]]),
                status: c[3],
            })
            .collect())
    }

    /// ClearDiagnosticInformation (0x14). `group` is a 3-byte group (0xFFFFFF = all DTCs).
    /// Requires unlocked writes.
    pub fn clear_dtcs(&mut self, group: u32) -> Result<()> {
        self.require_unlocked("ClearDiagnosticInformation")?;
        let g = group.to_be_bytes();
        self.request(&[SID_CLEAR_DTC, g[1], g[2], g[3]])?;
        Ok(())
    }

    /// WriteDataByIdentifier (0x2E). Requires unlocked writes. The caller is
    /// responsible for backing up the previous value first.
    pub fn write_data_by_identifier(&mut self, did: u16, data: &[u8]) -> Result<()> {
        self.require_unlocked("WriteDataByIdentifier")?;
        let [hi, lo] = did.to_be_bytes();
        let mut req = vec![SID_WRITE_DID, hi, lo];
        req.extend_from_slice(data);
        let r = self.request(&req)?;
        if r.len() < 3 || r[1] != hi || r[2] != lo {
            return Err(protocol(format!("DID echo mismatch in {r:02X?}")));
        }
        Ok(())
    }

    /// RoutineControl (0x31). Requires unlocked writes. Returns routine status bytes.
    pub fn routine_control(
        &mut self,
        action: RoutineAction,
        routine_id: u16,
        params: &[u8],
    ) -> Result<Vec<u8>> {
        self.require_unlocked("RoutineControl")?;
        let [hi, lo] = routine_id.to_be_bytes();
        let mut req = vec![SID_ROUTINE_CONTROL, action as u8, hi, lo];
        req.extend_from_slice(params);
        let r = self.request(&req)?;
        if r.len() < 4 || r[1] != action as u8 || r[2] != hi || r[3] != lo {
            return Err(protocol(format!("bad routine response {r:02X?}")));
        }
        Ok(r[4..].to_vec())
    }

    /// SecurityAccess (0x27): request a seed at odd `level`, answer with the key from
    /// `algo`. Requires unlocked writes. An all-zero seed means already unlocked.
    pub fn security_access(&mut self, level: u8, algo: &dyn SeedKey) -> Result<()> {
        self.require_unlocked("SecurityAccess")?;
        if level % 2 == 0 {
            return Err(protocol("security level must be odd (seed request)"));
        }
        let r = self.request(&[SID_SECURITY_ACCESS, level])?;
        if r.len() < 2 || r[1] != level {
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
