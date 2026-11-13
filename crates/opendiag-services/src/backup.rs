//! Durable record of every change made to a module, written *before* the change.
//!
//! An append-only JSON-lines file: each line is one record, and a later line with
//! the same `id` supersedes earlier ones (that is how a status moves from
//! `planned` to `verified`). Every append is flushed to disk with `sync_all`
//! before the caller is allowed to continue, so the old value is on disk before
//! anything is sent to the module.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use opendiag_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupStatus {
    /// Backup written; the change has been attempted or is about to be.
    Planned,
    /// The new value was read back from the module and matches.
    Verified,
    /// The module accepted the change but reads back a different value.
    VerificationFailed,
    /// The module refused or the exchange failed.
    SaveFailed,
    /// A later change put the old value back.
    Restored,
}

/// One change to one setting of one module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupRecord {
    /// Short id like `b0007`, unique within the file.
    pub id: String,
    pub created_unix: u64,
    /// Protocol the module was reached over (`kw1281`).
    pub protocol: String,
    pub module_address: u8,
    /// What kind of setting (`adaptation`).
    pub kind: String,
    pub channel: u8,
    /// The value before the change: what a restore puts back.
    pub old_value: u16,
    pub new_value: u16,
    pub status: BackupStatus,
    pub note: Option<String>,
    /// Set when this change was itself a restore of another record.
    pub restores: Option<String>,
    /// For `dtc_clear` records: the fault codes (with status) stored before they
    /// were cleared. Absent in older records.
    #[serde(default)]
    pub codes: Vec<String>,
}

fn io(e: impl std::fmt::Display) -> Error {
    Error::Io(e.to_string())
}

/// The fields a caller supplies; the store adds id, time and status.
/// (Normally created by the guarded change flow, not by hand.)
#[derive(Debug, Clone)]
pub struct NewBackup {
    pub protocol: String,
    pub module_address: u8,
    pub kind: String,
    pub channel: u8,
    pub old_value: u16,
    pub new_value: u16,
    pub restores: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BackupStore {
    path: PathBuf,
}

impl BackupStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Latest state of every record, in the order first created. A missing file
    /// is an empty store; a corrupt line is an error (never silently skipped,
    /// because this file is what makes a bad change reversible).
    pub fn records(&self) -> Result<Vec<BackupRecord>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(format!("cannot read {}: {e}", self.path.display()))),
        };
        let mut out: Vec<BackupRecord> = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let rec: BackupRecord = serde_json::from_str(line).map_err(|e| {
                io(format!(
                    "{} line {}: not a valid backup record: {e}",
                    self.path.display(),
                    i + 1
                ))
            })?;
            match out.iter_mut().find(|r| r.id == rec.id) {
                Some(existing) => *existing = rec,
                None => out.push(rec),
            }
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Option<BackupRecord>> {
        Ok(self.records()?.into_iter().find(|r| r.id == id))
    }

    fn append(&self, rec: &BackupRecord) -> Result<()> {
        let line = serde_json::to_string(rec).map_err(io)?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| {
                io(format!(
                    "cannot open backup file {}: {e}",
                    self.path.display()
                ))
            })?;
        writeln!(f, "{line}").map_err(io)?;
        // Durable before we return: the caller sends nothing to the module until then.
        f.sync_all().map_err(io)
    }

    fn next_id(&self) -> Result<String> {
        let next = self
            .records()?
            .iter()
            .filter_map(|r| r.id.strip_prefix('b').and_then(|n| n.parse::<u32>().ok()))
            .max()
            .unwrap_or(0)
            + 1;
        Ok(format!("b{next:04}"))
    }

    fn now_unix() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    /// Record the old value and the intended change, durably, status `planned`.
    pub fn plan(&self, new: NewBackup) -> Result<BackupRecord> {
        let rec = BackupRecord {
            id: self.next_id()?,
            created_unix: Self::now_unix(),
            protocol: new.protocol,
            module_address: new.module_address,
            kind: new.kind,
            channel: new.channel,
            old_value: new.old_value,
            new_value: new.new_value,
            status: BackupStatus::Planned,
            note: None,
            restores: new.restores,
            codes: Vec::new(),
        };
        self.append(&rec)?;
        Ok(rec)
    }

    /// Record the fault codes stored in a module, durably, just before they are
    /// cleared. Clearing cannot be undone, so this is a snapshot, not a restore point.
    /// `module_address` is 0 for protocols without one (UDS); `note` says which module.
    pub fn plan_clear(
        &self,
        protocol: &str,
        module_address: u8,
        note: String,
        codes: Vec<String>,
    ) -> Result<BackupRecord> {
        let rec = BackupRecord {
            id: self.next_id()?,
            created_unix: Self::now_unix(),
            protocol: protocol.to_string(),
            module_address,
            kind: "dtc_clear".to_string(),
            channel: 0,
            old_value: 0,
            new_value: 0,
            status: BackupStatus::Planned,
            note: Some(note),
            restores: None,
            codes,
        };
        self.append(&rec)?;
        Ok(rec)
    }

    /// Move a record to a new status (appends; history stays in the file).
    pub fn set_status(
        &self,
        id: &str,
        status: BackupStatus,
        note: Option<String>,
    ) -> Result<BackupRecord> {
        let mut rec = self.get(id)?.ok_or_else(|| {
            io(format!(
                "no backup record `{id}` in {}",
                self.path.display()
            ))
        })?;
        rec.status = status;
        if note.is_some() {
            rec.note = note;
        }
        self.append(&rec)?;
        Ok(rec)
    }
}
