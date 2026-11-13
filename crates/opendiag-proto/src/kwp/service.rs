//! KWP2000 services shared by every way of carrying them (K-line and TP2.0).
//!
//! A carrier only has to say how to send a request and get the positive
//! response; the services themselves are written once, here.

use opendiag_core::{Error, Result, WriteMode};

use super::{
    KwpDtc, SID_CLEAR_DTC, SID_READ_DATA_BY_LOCAL_ID, SID_READ_DTC_BY_STATUS,
    SID_READ_ECU_IDENTIFICATION, SID_START_DIAGNOSTIC_SESSION, SID_TESTER_PRESENT,
};

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("kwp: {}", msg.into()))
}

/// KWP2000 (ISO 14230-3) services over some carrier.
///
/// Services that modify the module (clearing codes) refuse to run unless the
/// carrier is in [`WriteMode::Unlocked`]; the guarded flow in `opendiag-services`
/// is the only caller that unlocks it.
pub trait KwpService {
    /// Send a service request (SID + parameters); returns the positive response
    /// (SID + 0x40 first). Negative responses are errors.
    fn request(&mut self, data: &[u8]) -> Result<Vec<u8>>;

    fn write_mode(&self) -> WriteMode;
    fn set_write_mode(&mut self, mode: WriteMode);

    /// Politely end the session if the carrier has a way to (K-line sends
    /// StopCommunication). Best effort; the default does nothing.
    fn close(&mut self) {}

    fn start_diagnostic_session(&mut self, mode: u8) -> Result<()> {
        let r = self.request(&[SID_START_DIAGNOSTIC_SESSION, mode])?;
        if r.get(1) != Some(&mode) {
            return Err(protocol(format!("bad session response {r:02X?}")));
        }
        Ok(())
    }

    fn tester_present(&mut self) -> Result<()> {
        self.request(&[SID_TESTER_PRESENT])?;
        Ok(())
    }

    /// ReadECUIdentification (0x1A): the data after the option echo.
    fn read_ecu_identification(&mut self, option: u8) -> Result<Vec<u8>> {
        let r = self.request(&[SID_READ_ECU_IDENTIFICATION, option])?;
        if r.get(1) != Some(&option) {
            return Err(protocol(format!(
                "identification option echo mismatch {r:02X?}"
            )));
        }
        Ok(r[2..].to_vec())
    }

    /// ReadDataByLocalIdentifier (0x21): the data after the id echo.
    fn read_data_by_local_identifier(&mut self, id: u8) -> Result<Vec<u8>> {
        let r = self.request(&[SID_READ_DATA_BY_LOCAL_ID, id])?;
        if r.get(1) != Some(&id) {
            return Err(protocol(format!("local id echo mismatch {r:02X?}")));
        }
        Ok(r[2..].to_vec())
    }

    /// ReadDiagnosticTroubleCodesByStatus (0x18). `group` 0xFF00 means all groups.
    fn read_dtcs_by_status(&mut self, status: u8, group: u16) -> Result<Vec<KwpDtc>> {
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

    /// ClearDiagnosticInformation (0x14). `group` 0xFF00 means all. Requires unlocked writes.
    fn clear_dtcs(&mut self, group: u16) -> Result<()> {
        if self.write_mode() != WriteMode::Unlocked {
            return Err(Error::WritesLocked("ClearDiagnosticInformation".into()));
        }
        let [gh, gl] = group.to_be_bytes();
        self.request(&[SID_CLEAR_DTC, gh, gl])?;
        Ok(())
    }
}
