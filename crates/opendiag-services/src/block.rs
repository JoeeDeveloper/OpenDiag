//! Guarded writes of a whole data block (raw coding data), over any protocol
//! that can read and write a block by identifier.
//!
//! The flow, in order. Any failure before step 5 means **nothing was sent**:
//!
//! 1. the confirmation must be for exactly the change that was shown;
//! 2. refuse blocks that must never be written, and changes that are not
//!    changes (identical) or that alter the block's length;
//! 3. the block is re-read; if it no longer equals what was shown, abort;
//! 4. the complete old block is written to the backup file and flushed;
//! 5. the write is sent, using a [`WriteToken`] only this crate can create;
//! 6. the block is read back and must equal what was written. A mismatch is
//!    recorded as `verification_failed` and reported with the backup id.
//!
//! Nothing is rewritten automatically: putting the old block back is itself a
//! write, so it is a separate guarded change ([`plan_block_restore`]).
//!
//! This layer is raw bytes only. It knows nothing about what the bytes mean.

use opendiag_core::{Error, Result};

use crate::adaptation::WriteToken;
use crate::backup::{with_record_failure, BackupStatus, BackupStore};

/// A module with readable and writable data blocks.
pub trait BlockAccess {
    fn protocol(&self) -> &'static str;
    fn module_address(&self) -> u8;
    fn describe(&self) -> String;
    /// Read a block. Never writes.
    fn read_block(&mut self, id: u16) -> Result<Vec<u8>>;
    /// Whether this block may be written at all (identity data never may).
    fn writable(&self, id: u16) -> std::result::Result<(), String>;
    /// Write a block. Needs the token the guarded flow issues.
    fn write_block(&mut self, token: &WriteToken, id: u16, data: &[u8]) -> Result<()>;
}

/// What would change: one block, from its current bytes to new ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockChange {
    pub id: u16,
    pub old: Vec<u8>,
    pub new: Vec<u8>,
    /// Backup id this change puts back, when it is a restore.
    pub restores: Option<String>,
}

impl BlockChange {
    /// Byte offsets where old and new differ.
    pub fn differing_offsets(&self) -> Vec<usize> {
        self.old
            .iter()
            .zip(&self.new)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect()
    }
}

/// A person's agreement to one specific change. A front end creates it after
/// showing the diff; [`apply_block_change`] refuses one that does not match.
#[derive(Debug)]
pub struct BlockConfirmation {
    change: BlockChange,
}

impl BlockConfirmation {
    pub fn for_change(change: &BlockChange) -> Self {
        Self {
            change: change.clone(),
        }
    }
}

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("block write: {}", msg.into()))
}

/// Read the current block and describe the change. Writes nothing.
pub fn plan_block_change(
    access: &mut dyn BlockAccess,
    id: u16,
    new: Vec<u8>,
) -> Result<BlockChange> {
    access.writable(id).map_err(protocol)?;
    let old = access.read_block(id)?;
    Ok(BlockChange {
        id,
        old,
        new,
        restores: None,
    })
}

/// Describe putting a recorded block back. Writes nothing.
pub fn plan_block_restore(
    access: &mut dyn BlockAccess,
    store: &BackupStore,
    backup_id: &str,
) -> Result<BlockChange> {
    let rec = store
        .get(backup_id)?
        .ok_or_else(|| protocol(format!("no backup record `{backup_id}`")))?;
    if rec.kind != "block_write" {
        return Err(protocol(format!("`{backup_id}` is not a block backup")));
    }
    if rec.protocol != access.protocol() || rec.module_address != access.module_address() {
        return Err(protocol(format!(
            "`{backup_id}` was taken from a different module ({})",
            rec.module.as_deref().unwrap_or("unknown")
        )));
    }
    let (Some(id), Some(old_hex)) = (rec.block, rec.old_hex.as_deref()) else {
        return Err(protocol(format!("`{backup_id}` holds no block data")));
    };
    let new = unhex(old_hex).ok_or_else(|| protocol("the recorded block data is not valid hex"))?;
    access.writable(id).map_err(protocol)?;
    let old = access.read_block(id)?;
    Ok(BlockChange {
        id,
        old,
        new,
        restores: Some(backup_id.to_string()),
    })
}

/// Parse a hex string (no separators) into bytes.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// A change that was written and read back correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockApplied {
    pub backup_id: String,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// Write a block with a confirmation, a durable backup and a read-back check.
pub fn apply_block_change(
    access: &mut dyn BlockAccess,
    store: &BackupStore,
    change: &BlockChange,
    confirmation: &BlockConfirmation,
) -> Result<BlockApplied> {
    // 1. Exactly what was shown.
    if &confirmation.change != change {
        return Err(protocol(
            "the confirmation is for a different change than the one being applied",
        ));
    }
    // 2. Allowed, a real change, same size.
    access.writable(change.id).map_err(protocol)?;
    if change.old == change.new {
        return Err(protocol(
            "the new block equals the current one; nothing to do",
        ));
    }
    if change.old.len() != change.new.len() {
        return Err(protocol(format!(
            "the new block has {} bytes but the current one has {}; the length must not change",
            change.new.len(),
            change.old.len()
        )));
    }
    // 3. The block must still be what the person saw.
    let current = access.read_block(change.id)?;
    if current != change.old {
        return Err(protocol(
            "the block changed since it was shown; nothing was written",
        ));
    }
    // 4. Durable backup first. If this fails, nothing has been sent.
    let rec = store.plan_block(
        access.protocol(),
        access.module_address(),
        access.describe(),
        change.id,
        &change.old,
        &change.new,
        change.restores.clone(),
    )?;
    let id = rec.id;

    // 5. Write. From here on the module may have changed.
    let token = WriteToken::new();
    if let Err(e) = access.write_block(&token, change.id, &change.new) {
        let update = store.set_status(
            &id,
            BackupStatus::SaveFailed,
            Some(format!("write failed: {e}")),
        );
        return Err(protocol(with_record_failure(
            format!("the module did not accept the block: {e} (backup {id})"),
            &id,
            update,
        )));
    }

    // 6. Read it back.
    let (status, note, problem) = match access.read_block(change.id) {
        Ok(b) if b == change.new => (BackupStatus::Verified, None, None),
        Ok(b) => (
            BackupStatus::VerificationFailed,
            Some(format!(
                "read back {} instead of {}",
                hex(&b),
                hex(&change.new)
            )),
            Some(format!(
                "the block reads back differently from what was written (backup {id} holds the \
                 original; put it back with a restore)"
            )),
        ),
        Err(e) => (
            BackupStatus::VerificationFailed,
            Some(format!("could not read back: {e}")),
            Some(format!(
                "written, but the block could not be read back to verify it ({e}) (backup {id})"
            )),
        ),
    };
    let update = store.set_status(&id, status, note);
    if let Some(msg) = problem {
        return Err(protocol(with_record_failure(msg, &id, update)));
    }
    if let Err(e) = update {
        return Err(protocol(format!(
            "the block WAS written and verified, but backup {id} could not be updated: {e}. \
             It still says `planned`"
        )));
    }
    if let Some(orig) = &change.restores {
        // The restore itself succeeded and is recorded under its own id.
        let _ = store.set_status(
            orig,
            BackupStatus::Restored,
            Some(format!("restored by {id}")),
        );
    }
    Ok(BlockApplied { backup_id: id })
}
