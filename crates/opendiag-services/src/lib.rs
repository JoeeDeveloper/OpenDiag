//! High-level diagnostic services on top of the protocol layer.
//!
//! Currently UDS only: identification, DTC read, and DTC clear with a mandatory
//! confirmation token and a record of what was cleared.

pub mod adaptation;
pub mod backup;
pub mod demo;
mod kw1281_module;
pub mod live;

pub use adaptation::{
    apply_change, apply_restore, plan_change, plan_restore, AdaptationAccess, Applied, Change,
    WriteConfirmation, WriteToken,
};
pub use backup::{BackupRecord, BackupStatus, BackupStore};
pub use kw1281_module::Kw1281Module;
pub use live::{log_csv, Limit, LiveData, LogConfig, LogSummary, CSV_HEADER};

use opendiag_core::{Error, Result, WriteMode};
use opendiag_proto::uds::UdsClient;
use opendiag_proto::{clock::Clock, uds::Dtc};
use opendiag_transport::Transport;

/// Standard UDS identification DIDs (ISO 14229 Annex C).
const DID_PART_NUMBER: u16 = 0xF187;
const DID_SOFTWARE_VERSION: u16 = 0xF189;
const DID_VIN: u16 = 0xF190;

/// What a module says about itself. `None` means the ECU does not support that item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ident {
    pub part_number: Option<String>,
    pub software_version: Option<String>,
    pub vin: Option<String>,
    /// Protocol-specific extra text lines (KW1281 modules report free-form ident lines).
    pub lines: Vec<String>,
}

/// A trouble code ready for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtcReport {
    pub code: u32,
    /// SAE J2012-style text, e.g. `P0123-45` (generic format, not a manufacturer code).
    pub text: String,
    pub status: u8,
    pub flags: Vec<&'static str>,
}

impl From<Dtc> for DtcReport {
    fn from(d: Dtc) -> Self {
        Self {
            code: d.code,
            text: format_dtc(d.code),
            status: d.status,
            flags: status_flags(d.status),
        }
    }
}

/// Format a 3-byte UDS DTC as `<letter><digits>-<failure type>`.
pub fn format_dtc(code: u32) -> String {
    let [_, hi, lo, ftb] = code.to_be_bytes();
    let letter = ['P', 'C', 'B', 'U'][usize::from(hi >> 6)];
    format!(
        "{letter}{}{:X}{:X}{:X}-{ftb:02X}",
        (hi >> 4) & 0x3,
        hi & 0xF,
        lo >> 4,
        lo & 0xF
    )
}

/// Decode the UDS DTC status byte (ISO 14229-1 Annex D).
pub fn status_flags(status: u8) -> Vec<&'static str> {
    const FLAGS: [(u8, &str); 8] = [
        (0x01, "testFailed"),
        (0x02, "testFailedThisOperationCycle"),
        (0x04, "pending"),
        (0x08, "confirmed"),
        (0x10, "testNotCompletedSinceLastClear"),
        (0x20, "testFailedSinceLastClear"),
        (0x40, "testNotCompletedThisOperationCycle"),
        (0x80, "warningIndicatorRequested"),
    ];
    FLAGS
        .iter()
        .filter(|(bit, _)| status & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}

fn text_did<T: Transport, C: Clock>(uds: &mut UdsClient<T, C>, did: u16) -> Result<Option<String>> {
    match uds.read_data_by_identifier(did) {
        Ok(data) => {
            let s = String::from_utf8_lossy(&data);
            Ok(Some(
                s.trim_matches(|c: char| c == '\0' || c.is_whitespace())
                    .to_string(),
            ))
        }
        // requestOutOfRange: the ECU simply doesn't have this identifier.
        Err(Error::NegativeResponse { nrc: 0x31, .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Read the standard identification items from a module.
pub fn identify<T: Transport, C: Clock>(uds: &mut UdsClient<T, C>) -> Result<Ident> {
    Ok(Ident {
        part_number: text_did(uds, DID_PART_NUMBER)?,
        software_version: text_did(uds, DID_SOFTWARE_VERSION)?,
        vin: text_did(uds, DID_VIN)?,
        lines: Vec::new(),
    })
}

/// A connected diagnostic module, whatever protocol it speaks. Front ends
/// (CLI, later the GUI) work against this so they need no protocol knowledge.
pub trait Module {
    fn identify(&mut self) -> Result<Ident>;
    fn read_dtcs(&mut self) -> Result<Vec<DtcReport>>;
    /// Clear all DTCs. Needs a [`ClearConfirmation`]; returns the codes stored
    /// beforehand so the caller can keep a record.
    fn clear_dtcs(&mut self, confirmation: &ClearConfirmation) -> Result<Vec<DtcReport>>;

    /// Live measuring data, if this kind of module supports it (KW1281 only so far).
    fn live(&mut self) -> Option<&mut dyn LiveData> {
        None
    }

    /// Adaptation channels, if this kind of module has them (KW1281 only so far).
    fn adaptation(&mut self) -> Option<&mut dyn AdaptationAccess> {
        None
    }
}

impl<T: Transport, C: Clock> Module for UdsClient<T, C> {
    fn identify(&mut self) -> Result<Ident> {
        identify(self)
    }

    fn read_dtcs(&mut self) -> Result<Vec<DtcReport>> {
        read_dtcs(self)
    }

    fn clear_dtcs(&mut self, confirmation: &ClearConfirmation) -> Result<Vec<DtcReport>> {
        clear_dtcs(self, confirmation)
    }
}

/// Read every stored DTC (status mask 0xFF).
pub fn read_dtcs<T: Transport, C: Clock>(uds: &mut UdsClient<T, C>) -> Result<Vec<DtcReport>> {
    Ok(uds
        .read_dtcs_by_status_mask(0xFF)?
        .into_iter()
        .map(DtcReport::from)
        .collect())
}

/// Proof that a person agreed to clear codes. Only front ends should create one,
/// after showing the user what will be cleared.
#[derive(Debug)]
pub struct ClearConfirmation(());

impl ClearConfirmation {
    pub fn confirmed_by_user() -> Self {
        Self(())
    }
}

/// Clear all DTCs. Unlocks writes only for the duration of the call, always
/// restores the previous mode, and returns the codes that were stored beforehand
/// so the caller can keep a record.
pub fn clear_dtcs<T: Transport, C: Clock>(
    uds: &mut UdsClient<T, C>,
    _confirmation: &ClearConfirmation,
) -> Result<Vec<DtcReport>> {
    let before = read_dtcs(uds)?;
    let previous = uds.write_mode();
    uds.set_write_mode(WriteMode::Unlocked);
    let result = uds.clear_dtcs(0xFF_FFFF);
    uds.set_write_mode(previous);
    result?;
    Ok(before)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dtc_like_sae_j2012() {
        assert_eq!(format_dtc(0x01_23_45), "P0123-45");
        assert_eq!(format_dtc(0x41_00_00), "C0100-00");
        assert_eq!(format_dtc(0x81_23_01), "B0123-01");
        assert_eq!(format_dtc(0xC1_23_FF), "U0123-FF");
        assert_eq!(format_dtc(0x13_AB_CD), "P13AB-CD");
    }

    #[test]
    fn decodes_status_bits() {
        assert_eq!(status_flags(0x00), Vec::<&str>::new());
        assert_eq!(status_flags(0x09), vec!["testFailed", "confirmed"]);
        assert!(status_flags(0xFF).len() == 8);
    }
}
