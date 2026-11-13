//! Guarded clearing of fault codes.
//!
//! Clearing erases the codes and their status, and cannot be undone, so it
//! follows the same discipline as adaptation changes ([`crate::adaptation`]),
//! with a *snapshot* in place of a restore point:
//!
//! 1. the confirmation must be for exactly the codes that were shown;
//! 2. the codes are re-read, and if they differ from what was shown (a new
//!    fault, a changed status) nothing is cleared, because that evidence would be lost;
//! 3. a snapshot of the codes is written to disk (flushed) — if that fails,
//!    **nothing is sent**;
//! 4. the clear is sent, using a [`WriteToken`] only this crate can create;
//! 5. the codes are read again. Active faults re-register at once, so codes that
//!    are still reported are returned and recorded, not treated as an error.

use opendiag_core::{Error, Result};

use crate::adaptation::WriteToken;
use crate::backup::{BackupStatus, BackupStore};
use crate::DtcReport;

/// A module whose fault codes can be read and cleared.
pub trait FaultAccess {
    fn protocol(&self) -> &'static str;
    /// Diagnostic address, or 0 for protocols that have none (UDS).
    fn module_address(&self) -> u8;
    /// Human-readable name for snapshots, e.g. `KW1281 module 0x01`.
    fn describe(&self) -> String;
    fn read(&mut self) -> Result<Vec<DtcReport>>;
    /// Erase all fault codes. Needs the token the guarded flow issues.
    fn clear(&mut self, token: &WriteToken) -> Result<()>;
}

/// A person's agreement to clear one specific set of codes. A front end creates
/// it after showing them; [`apply_clear`] refuses one that does not match.
#[derive(Debug)]
pub struct ClearConfirmation {
    codes: Vec<DtcReport>,
}

impl ClearConfirmation {
    pub fn for_codes(shown: &[DtcReport]) -> Self {
        Self {
            codes: shown.to_vec(),
        }
    }
}

/// Read what would be cleared. Writes nothing.
pub fn plan_clear(access: &mut dyn FaultAccess) -> Result<Vec<DtcReport>> {
    access.read()
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearOutcome {
    /// Id of the snapshot in the backup file.
    pub backup_id: String,
    /// The codes that were stored before clearing.
    pub cleared: Vec<DtcReport>,
    /// Codes still reported afterwards (active faults come straight back).
    pub remaining: Vec<DtcReport>,
}

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("clear: {}", msg.into()))
}

fn line(d: &DtcReport) -> String {
    format!("{} status {:#04X}", d.text, d.status)
}

/// Clear fault codes with a confirmation, a durable snapshot and a re-read.
pub fn apply_clear(
    access: &mut dyn FaultAccess,
    store: &BackupStore,
    shown: &[DtcReport],
    confirmation: &ClearConfirmation,
) -> Result<ClearOutcome> {
    // 1. The confirmation must be for exactly what was shown.
    if confirmation.codes != shown {
        return Err(protocol(
            "the confirmation is for different codes than the ones being cleared",
        ));
    }
    if shown.is_empty() {
        return Err(protocol("no trouble codes are stored; nothing to clear"));
    }
    // 2. What is stored must still be what the person saw.
    let current = access.read()?;
    if current != shown {
        return Err(protocol(
            "the stored codes changed since they were shown; nothing was cleared",
        ));
    }
    // 3. Durable snapshot first. If this fails, nothing has been sent.
    let snapshot = store.plan_clear(
        access.protocol(),
        access.module_address(),
        access.describe(),
        shown.iter().map(line).collect(),
    )?;
    let id = snapshot.id;

    // 4. Clear.
    let token = WriteToken::new();
    if let Err(e) = access.clear(&token) {
        store.set_status(
            &id,
            BackupStatus::SaveFailed,
            Some(format!("clear failed: {e}")),
        )?;
        return Err(protocol(format!(
            "the module did not clear its codes: {e} (snapshot {id})"
        )));
    }

    // 5. Look again.
    match access.read() {
        Ok(remaining) if remaining.is_empty() => {
            store.set_status(&id, BackupStatus::Verified, None)?;
            Ok(ClearOutcome {
                backup_id: id,
                cleared: shown.to_vec(),
                remaining,
            })
        }
        Ok(remaining) => {
            store.set_status(
                &id,
                BackupStatus::VerificationFailed,
                Some(format!(
                    "{} code(s) still reported after clearing: {}",
                    remaining.len(),
                    remaining
                        .iter()
                        .map(|d| d.text.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            )?;
            Ok(ClearOutcome {
                backup_id: id,
                cleared: shown.to_vec(),
                remaining,
            })
        }
        Err(e) => {
            store.set_status(
                &id,
                BackupStatus::VerificationFailed,
                Some(format!("could not re-read after clearing: {e}")),
            )?;
            Err(protocol(format!(
                "cleared, but the codes could not be read back to verify it ({e}) (snapshot {id})"
            )))
        }
    }
}
