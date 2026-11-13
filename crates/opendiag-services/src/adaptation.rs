//! Guarded changes to module settings (KW1281 adaptation channels).
//!
//! The only way to write is [`apply_change`] (or [`apply_restore`], which uses
//! it), and it always does, in this order:
//!
//! 1. check that the confirmation is for exactly this change;
//! 2. re-read the current value and refuse if it is no longer the one shown;
//! 3. write the backup to disk (flushed) — if that fails, **nothing is sent**;
//! 4. send the test, then the save;
//! 5. read the value back and mark the backup verified, or failed.
//!
//! The module-facing `test` and `save` methods demand a [`WriteToken`], which
//! only this crate can create, so they cannot be called around the flow above.
//! A failed verification is reported with the backup id; nothing is rewritten
//! automatically, because putting a value back is itself a write that needs consent.

use opendiag_core::{Error, Result};

use crate::backup::{BackupStatus, BackupStore, NewBackup};

/// Permission to write, issued only inside this crate by the guarded flow.
#[derive(Debug)]
pub struct WriteToken(());

impl WriteToken {
    pub(crate) fn new() -> Self {
        Self(())
    }
}

/// A module that has adaptation channels.
pub trait AdaptationAccess {
    fn protocol(&self) -> &'static str;
    fn module_address(&self) -> u8;
    /// Read a channel. Never writes.
    fn read(&mut self, channel: u8) -> Result<u16>;
    /// Ask the module to evaluate `value` without storing it.
    fn test(&mut self, token: &WriteToken, channel: u8, value: u16) -> Result<u16>;
    /// Store `value`. The workshop code is whatever the module expects (user-supplied).
    fn save(
        &mut self,
        token: &WriteToken,
        channel: u8,
        value: u16,
        workshop_code: u32,
    ) -> Result<()>;
}

/// What would change: one channel, from its current value to a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub channel: u8,
    pub old: u16,
    pub new: u16,
}

/// A person's agreement to one specific change. A front end creates it after
/// showing the diff; [`apply_change`] refuses one that does not match.
#[derive(Debug)]
pub struct WriteConfirmation {
    change: Change,
}

impl WriteConfirmation {
    pub fn for_change(change: &Change) -> Self {
        Self { change: *change }
    }
}

/// Read the current value and describe the change. Writes nothing.
pub fn plan_change(access: &mut dyn AdaptationAccess, channel: u8, new: u16) -> Result<Change> {
    let old = access.read(channel)?;
    Ok(Change { channel, old, new })
}

/// A change that was written and read back correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub backup_id: String,
}

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("adaptation: {}", msg.into()))
}

/// Make a confirmed change with a backup, test, save and read-back verification.
pub fn apply_change(
    access: &mut dyn AdaptationAccess,
    store: &BackupStore,
    change: &Change,
    workshop_code: u32,
    confirmation: &WriteConfirmation,
) -> Result<Applied> {
    apply_inner(access, store, change, workshop_code, confirmation, None)
}

fn apply_inner(
    access: &mut dyn AdaptationAccess,
    store: &BackupStore,
    change: &Change,
    workshop_code: u32,
    confirmation: &WriteConfirmation,
    restores: Option<String>,
) -> Result<Applied> {
    // 1. The confirmation must be for exactly this change.
    if confirmation.change != *change {
        return Err(protocol(
            "the confirmation is for a different change than the one being applied",
        ));
    }
    if change.old == change.new {
        return Err(protocol(format!(
            "channel {} already has value {}; nothing to change",
            change.channel, change.new
        )));
    }
    // 2. The value shown to the person must still be the current one.
    let current = access.read(change.channel)?;
    if current != change.old {
        return Err(protocol(format!(
            "channel {} now reads {current}, not {} as planned; nothing was written",
            change.channel, change.old
        )));
    }
    // 3. Durable backup first. If this fails, we have sent nothing.
    let record = store.plan(NewBackup {
        protocol: access.protocol().into(),
        module_address: access.module_address(),
        kind: "adaptation".into(),
        channel: change.channel,
        old_value: change.old,
        new_value: change.new,
        restores,
    })?;
    let id = record.id.clone();
    let fail = |status: BackupStatus, note: String| -> Result<()> {
        store.set_status(&id, status, Some(note)).map(|_| ())
    };

    // 4. Test, then save.
    let token = WriteToken::new();
    if let Err(e) = access.test(&token, change.channel, change.new) {
        fail(BackupStatus::SaveFailed, format!("test failed: {e}"))?;
        return Err(protocol(format!(
            "the module rejected the test; nothing was stored (backup {id})"
        )));
    }
    if let Err(e) = access.save(&token, change.channel, change.new, workshop_code) {
        fail(BackupStatus::SaveFailed, format!("save failed: {e}"))?;
        return Err(protocol(format!(
            "the save failed: {e}. The value may be unchanged; check it with `adaptation read` (backup {id})"
        )));
    }

    // 5. Read it back. The save reply is not trusted on its own.
    match access.read(change.channel) {
        Ok(v) if v == change.new => {
            store.set_status(&id, BackupStatus::Verified, None)?;
            Ok(Applied { backup_id: id })
        }
        Ok(v) => {
            fail(
                BackupStatus::VerificationFailed,
                format!("reads back {v}, expected {}", change.new),
            )?;
            Err(protocol(format!(
                "the module accepted the save but channel {} reads {v}, not {}. \
                 Backup {id} holds the old value {}; restore it with `opendiag backup restore {id}`",
                change.channel, change.new, change.old
            )))
        }
        Err(e) => {
            fail(
                BackupStatus::VerificationFailed,
                format!("could not read back: {e}"),
            )?;
            Err(protocol(format!(
                "saved, but the value could not be read back to verify it ({e}). \
                 Backup {id} holds the old value {}",
                change.old
            )))
        }
    }
}

/// Work out what restoring backup `id` would do right now. Writes nothing.
pub fn plan_restore(
    access: &mut dyn AdaptationAccess,
    store: &BackupStore,
    id: &str,
) -> Result<(crate::backup::BackupRecord, Change)> {
    let record = store.get(id)?.ok_or_else(|| {
        protocol(format!(
            "no backup record `{id}` in {}",
            store.path().display()
        ))
    })?;
    if record.kind == "dtc_clear" {
        return Err(protocol(format!(
            "backup {id} is a snapshot of cleared fault codes; those cannot be written back"
        )));
    }
    if record.kind != "adaptation" || record.protocol != access.protocol() {
        return Err(protocol(format!(
            "backup {id} is a {} {} change, which this connection cannot restore",
            record.protocol, record.kind
        )));
    }
    if record.module_address != access.module_address() {
        return Err(protocol(format!(
            "backup {id} was taken from module {:#04X}, but this connection is to {:#04X}",
            record.module_address,
            access.module_address()
        )));
    }
    let current = access.read(record.channel)?;
    let change = Change {
        channel: record.channel,
        old: current,
        new: record.old_value,
    };
    Ok((record, change))
}

/// Put a backed-up value back. This is an ordinary guarded change (it takes its
/// own backup first), and on success the original record is marked restored.
pub fn apply_restore(
    access: &mut dyn AdaptationAccess,
    store: &BackupStore,
    record: &crate::backup::BackupRecord,
    change: &Change,
    workshop_code: u32,
    confirmation: &WriteConfirmation,
) -> Result<Applied> {
    if change.new != record.old_value || change.channel != record.channel {
        return Err(protocol("the change does not restore this backup"));
    }
    let applied = apply_inner(
        access,
        store,
        change,
        workshop_code,
        confirmation,
        Some(record.id.clone()),
    )?;
    store.set_status(&record.id, BackupStatus::Restored, None)?;
    Ok(applied)
}
