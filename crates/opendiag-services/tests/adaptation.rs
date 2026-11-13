//! The guarded write flow: backup first, confirm exactly, verify by reading back,
//! restore on request. Failure paths matter most, so most of these are failures.

use std::path::PathBuf;

use opendiag_core::{Error, WriteMode};
use opendiag_proto::clock::FakeClock;
use opendiag_proto::kw1281::sim::Kw1281Sim;
use opendiag_proto::kw1281::{Kw1281Client, Kw1281Config};
use opendiag_services::backup::NewBackup;
use opendiag_services::{
    apply_change, apply_restore, plan_change, plan_restore, Applied, BackupStatus, BackupStore,
    Change, Kw1281Module, Module, WriteConfirmation,
};

type Mod = Kw1281Module<Kw1281Sim, FakeClock>;

fn module(sim: Kw1281Sim) -> Mod {
    let client = Kw1281Client::new(sim, FakeClock::new(), Kw1281Config::default());
    Kw1281Module::connect(client, 0x01).unwrap()
}

fn sim() -> Kw1281Sim {
    Kw1281Sim::new()
        .ident_line("X")
        .adaptation(4, 100)
        .adaptation(7, 5)
}

fn temp_store(name: &str) -> BackupStore {
    let path: PathBuf =
        std::env::temp_dir().join(format!("opendiag-bk-{}-{name}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    BackupStore::new(path)
}

fn cleanup(store: &BackupStore) {
    let _ = std::fs::remove_file(store.path());
}

fn apply(
    m: &mut Mod,
    store: &BackupStore,
    change: Change,
    wc: u32,
) -> opendiag_core::Result<Applied> {
    let confirmation = WriteConfirmation::for_change(&change);
    apply_change(m.adaptation().unwrap(), store, &change, wc, &confirmation)
}

fn sent(m: &Mod) -> usize {
    m.client().transport().bytes_received()
}

#[test]
fn reading_and_planning_never_write() {
    let mut m = module(sim());
    let before = m.client().transport().adaptation_saves().len();
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    assert_eq!(
        change,
        Change {
            channel: 4,
            old: 100,
            new: 200
        }
    );
    assert_eq!(m.client().transport().adaptation_saves().len(), before);
    assert!(m.client().transport().adaptation_tests().is_empty());
    assert_eq!(m.client().write_mode(), WriteMode::ReadOnly);
}

#[test]
fn a_confirmed_change_is_backed_up_tested_saved_verified_and_relocked() {
    let store = temp_store("happy");
    let mut m = module(sim());
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    let applied = apply(&mut m, &store, change, 0x01_23_45).unwrap();

    assert_eq!(applied.backup_id, "b0001");
    let t = m.client().transport();
    assert_eq!(t.adaptation_value(4), Some(200));
    assert_eq!(t.adaptation_tests(), &[(4, 200)]);
    assert_eq!(
        t.adaptation_saves(),
        &[(4, 200, 0x01_23_45)],
        "workshop code is sent"
    );
    assert!(t.violations().is_empty(), "{:?}", t.violations());
    assert_eq!(m.client().write_mode(), WriteMode::ReadOnly, "relocked");

    let rec = store.get("b0001").unwrap().unwrap();
    assert_eq!((rec.old_value, rec.new_value, rec.channel), (100, 200, 4));
    assert_eq!(rec.status, BackupStatus::Verified);
    assert_eq!(
        (rec.protocol.as_str(), rec.module_address),
        ("kw1281", 0x01)
    );
    cleanup(&store);
}

#[test]
fn a_confirmation_for_a_different_change_sends_nothing_and_writes_no_backup() {
    let store = temp_store("mismatch");
    let mut m = module(sim());
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    let wrong = WriteConfirmation::for_change(&Change { new: 999, ..change });
    let before = sent(&m);
    let err = apply_change(m.adaptation().unwrap(), &store, &change, 0, &wrong).unwrap_err();
    assert!(err.to_string().contains("different change"), "{err}");
    assert_eq!(sent(&m), before, "nothing reached the module");
    assert!(store.records().unwrap().is_empty());
    assert!(!store.path().exists());
}

#[test]
fn if_the_backup_cannot_be_written_nothing_is_sent_to_the_module() {
    let store = BackupStore::new("no-such-directory-xyz/backups.jsonl");
    let mut m = module(sim());
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    let err = apply(&mut m, &store, change, 0).unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err}");
    let t = m.client().transport();
    assert!(t.adaptation_tests().is_empty() && t.adaptation_saves().is_empty());
    assert_eq!(t.adaptation_value(4), Some(100), "module untouched");
}

#[test]
fn a_value_that_changed_since_it_was_shown_is_refused() {
    let store = temp_store("stale");
    let mut m = module(sim());
    // The person was shown 99, but the module actually holds 100.
    let stale = Change {
        channel: 4,
        old: 99,
        new: 200,
    };
    let err = apply(&mut m, &store, stale, 0).unwrap_err();
    assert!(err.to_string().contains("now reads 100"), "{err}");
    assert!(m.client().transport().adaptation_saves().is_empty());
    assert!(
        store.records().unwrap().is_empty(),
        "no backup for a change never attempted"
    );
}

#[test]
fn changing_to_the_same_value_is_refused() {
    let store = temp_store("same");
    let mut m = module(sim());
    let before = sent(&m);
    let err = apply(
        &mut m,
        &store,
        Change {
            channel: 4,
            old: 100,
            new: 100,
        },
        0,
    )
    .unwrap_err();
    assert!(err.to_string().contains("nothing to change"), "{err}");
    assert_eq!(sent(&m), before);
}

#[test]
fn a_refused_save_is_recorded_leaves_the_value_and_relocks() {
    let store = temp_store("refused");
    let mut m = module(sim().refuse_adaptation_save());
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    let err = apply(&mut m, &store, change, 0).unwrap_err();
    assert!(err.to_string().contains("save failed"), "{err}");

    assert_eq!(m.client().transport().adaptation_value(4), Some(100));
    assert_eq!(m.client().write_mode(), WriteMode::ReadOnly);
    let rec = store.get("b0001").unwrap().unwrap();
    assert_eq!(rec.status, BackupStatus::SaveFailed);
    assert!(rec.note.unwrap().contains("save failed"));
    cleanup(&store);
}

#[test]
fn a_module_that_acks_but_does_not_store_is_caught_by_the_readback() {
    let store = temp_store("lying");
    let mut m = module(sim().ignore_adaptation_save());
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    let err = apply(&mut m, &store, change, 0).unwrap_err().to_string();
    assert!(err.contains("reads 100, not 200"), "{err}");
    assert!(
        err.contains("b0001") && err.contains("backup restore"),
        "points at the restore: {err}"
    );
    assert_eq!(
        store.get("b0001").unwrap().unwrap().status,
        BackupStatus::VerificationFailed
    );
    assert_eq!(m.client().write_mode(), WriteMode::ReadOnly);
    cleanup(&store);
}

#[test]
fn an_invalid_workshop_code_fails_safely_after_the_backup_is_recorded() {
    let store = temp_store("wc");
    let mut m = module(sim());
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    let err = apply(&mut m, &store, change, 0x0100_0000).unwrap_err();
    assert!(err.to_string().contains("24 bits"), "{err}");
    assert_eq!(m.client().transport().adaptation_value(4), Some(100));
    assert_eq!(
        store.get("b0001").unwrap().unwrap().status,
        BackupStatus::SaveFailed
    );
    cleanup(&store);
}

#[test]
fn restoring_puts_the_old_value_back_with_its_own_backup() {
    let store = temp_store("restore");
    let mut m = module(sim());
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    apply(&mut m, &store, change, 0).unwrap();
    assert_eq!(m.client().transport().adaptation_value(4), Some(200));

    let (record, undo) = plan_restore(m.adaptation().unwrap(), &store, "b0001").unwrap();
    assert_eq!(
        undo,
        Change {
            channel: 4,
            old: 200,
            new: 100
        }
    );
    let confirmation = WriteConfirmation::for_change(&undo);
    let applied = apply_restore(
        m.adaptation().unwrap(),
        &store,
        &record,
        &undo,
        0,
        &confirmation,
    )
    .unwrap();

    assert_eq!(
        m.client().transport().adaptation_value(4),
        Some(100),
        "original value is back"
    );
    assert_eq!(applied.backup_id, "b0002");
    assert_eq!(
        store.get("b0001").unwrap().unwrap().status,
        BackupStatus::Restored
    );
    let second = store.get("b0002").unwrap().unwrap();
    assert_eq!(
        (second.old_value, second.new_value),
        (200, 100),
        "the restore can itself be undone"
    );
    assert_eq!(second.restores.as_deref(), Some("b0001"));
    assert_eq!(second.status, BackupStatus::Verified);
    assert!(m.client().transport().violations().is_empty());
    cleanup(&store);
}

#[test]
fn restore_refuses_the_wrong_module_unknown_ids_and_mismatched_changes() {
    let store = temp_store("restore-bad");
    store
        .plan(NewBackup {
            protocol: "kw1281".into(),
            module_address: 0x02,
            kind: "adaptation".into(),
            channel: 4,
            old_value: 1,
            new_value: 2,
            restores: None,
        })
        .unwrap();
    let mut m = module(sim()); // connected to module 0x01
    let err = plan_restore(m.adaptation().unwrap(), &store, "b0001")
        .err()
        .unwrap();
    assert!(
        err.to_string().contains("0x02") && err.to_string().contains("0x01"),
        "{err}"
    );
    let err = plan_restore(m.adaptation().unwrap(), &store, "b9999")
        .err()
        .unwrap();
    assert!(err.to_string().contains("no backup record"), "{err}");

    // A change that does not restore the record is refused.
    let rec = store.get("b0001").unwrap().unwrap();
    let not_a_restore = Change {
        channel: 4,
        old: 100,
        new: 77,
    };
    let c = WriteConfirmation::for_change(&not_a_restore);
    let err =
        apply_restore(m.adaptation().unwrap(), &store, &rec, &not_a_restore, 0, &c).unwrap_err();
    assert!(err.to_string().contains("does not restore"), "{err}");
    cleanup(&store);
}

#[test]
fn the_store_numbers_records_supersedes_by_id_and_survives_reopening() {
    let store = temp_store("store");
    assert!(
        store.records().unwrap().is_empty(),
        "missing file is an empty store"
    );
    let new = |ch| NewBackup {
        protocol: "kw1281".into(),
        module_address: 1,
        kind: "adaptation".into(),
        channel: ch,
        old_value: 1,
        new_value: 2,
        restores: None,
    };
    assert_eq!(store.plan(new(1)).unwrap().id, "b0001");
    assert_eq!(store.plan(new(2)).unwrap().id, "b0002");
    store
        .set_status("b0001", BackupStatus::Verified, Some("ok".into()))
        .unwrap();

    let reopened = BackupStore::new(store.path());
    let records = reopened.records().unwrap();
    assert_eq!(records.len(), 2, "one entry per id, latest wins");
    assert_eq!(records[0].status, BackupStatus::Verified);
    assert_eq!(records[0].note.as_deref(), Some("ok"));
    assert_eq!(reopened.plan(new(3)).unwrap().id, "b0003");
    assert!(reopened
        .set_status("nope", BackupStatus::Restored, None)
        .is_err());
    cleanup(&store);
}

#[test]
fn a_corrupt_backup_file_is_an_error_not_silently_skipped() {
    let store = temp_store("corrupt");
    store
        .plan(NewBackup {
            protocol: "kw1281".into(),
            module_address: 1,
            kind: "adaptation".into(),
            channel: 1,
            old_value: 1,
            new_value: 2,
            restores: None,
        })
        .unwrap();
    let mut text = std::fs::read_to_string(store.path()).unwrap();
    text.push_str("this is not json\n");
    std::fs::write(store.path(), text).unwrap();
    let err = store.records().unwrap_err().to_string();
    assert!(
        err.contains("line 2") && err.contains("not a valid backup record"),
        "{err}"
    );
    // And a change refuses to proceed rather than write without a readable history.
    let mut m = module(sim());
    let change = plan_change(m.adaptation().unwrap(), 4, 200).unwrap();
    assert!(apply(&mut m, &store, change, 0).is_err());
    assert!(m.client().transport().adaptation_saves().is_empty());
    cleanup(&store);
}
