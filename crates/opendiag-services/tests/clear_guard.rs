//! The guarded fault-code clear: snapshot first, confirm exactly, re-read, never
//! clear what the person did not see. Failure paths first.

use std::path::PathBuf;

use opendiag_core::{Error, WriteMode};
use opendiag_proto::clock::FakeClock;
use opendiag_proto::isotp::{Config, IsoTpLink};
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_services::demo::{virtual_kw1281_module, ECU_ID, TESTER_ID};
use opendiag_services::{
    apply_clear, plan_clear, plan_restore, BackupStatus, BackupStore, ClearConfirmation, DtcReport,
    Module,
};
use opendiag_transport::{Frame, Simulator};

type Uds = UdsClient<Simulator, FakeClock>;

fn uds(sim: Simulator) -> Uds {
    let link = IsoTpLink::new(sim, FakeClock::new(), TESTER_ID, ECU_ID, Config::default());
    UdsClient::new(link, UdsConfig::default())
}

fn req(payload: &[u8]) -> Frame {
    let mut d = vec![payload.len() as u8];
    d.extend_from_slice(payload);
    Frame::can(TESTER_ID, &d)
}

fn rsp(payload: &[u8]) -> Vec<Frame> {
    let mut d = vec![payload.len() as u8];
    d.extend_from_slice(payload);
    vec![Frame::can(ECU_ID, &d)]
}

const READ: [u8; 3] = [0x19, 0x02, 0xFF];
const CLEAR: [u8; 4] = [0x14, 0xFF, 0xFF, 0xFF];
/// P0171 (confirmed + test failed): the one code these modules report.
const ONE_CODE: [u8; 7] = [0x59, 0x02, 0xFF, 0x01, 0x71, 0x00, 0x09];

/// A module that reports one code and accepts the clear but keeps reporting it,
/// like an active fault that re-registers immediately.
fn active_fault_sim() -> Simulator {
    Simulator::new()
        .on(req(&READ), rsp(&ONE_CODE))
        .on(req(&CLEAR), rsp(&[0x54]))
}

fn temp_store(name: &str) -> BackupStore {
    let path: PathBuf =
        std::env::temp_dir().join(format!("opendiag-cg-{}-{name}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    BackupStore::new(path)
}

fn clear_was_sent(u: &Uds) -> bool {
    u.link().transport().sent().contains(&req(&CLEAR))
}

fn confirm(shown: &[DtcReport]) -> ClearConfirmation {
    ClearConfirmation::for_codes(shown)
}

#[test]
fn codes_that_come_straight_back_are_reported_and_recorded_not_an_error() {
    let store = temp_store("remain");
    let mut u = uds(active_fault_sim());
    let shown = plan_clear(u.faults()).unwrap();
    assert_eq!(shown.len(), 1);

    let outcome = apply_clear(u.faults(), &store, &shown, &confirm(&shown)).unwrap();
    assert_eq!(outcome.cleared, shown);
    assert_eq!(outcome.remaining, shown, "the module still reports it");

    let rec = store.get(&outcome.backup_id).unwrap().unwrap();
    assert_eq!(rec.status, BackupStatus::VerificationFailed);
    assert!(rec.note.unwrap().contains("still reported"));
    assert_eq!(rec.codes, vec!["P0171-00 status 0x09"]);
    assert_eq!((rec.protocol.as_str(), rec.module_address), ("uds", 0));
    assert_eq!(rec.kind, "dtc_clear");
    assert_eq!(u.write_mode(), WriteMode::ReadOnly, "relocked");
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn a_confirmation_for_different_codes_clears_nothing_and_writes_no_snapshot() {
    let store = temp_store("mismatch");
    let mut u = uds(active_fault_sim());
    let shown = plan_clear(u.faults()).unwrap();
    let other = DtcReport {
        code: 0x03_00_00,
        text: "P0300-00".into(),
        status: 0x24,
        flags: vec![],
    };
    let err = apply_clear(u.faults(), &store, &shown, &confirm(&[other])).unwrap_err();
    assert!(err.to_string().contains("different codes"), "{err}");
    assert!(!clear_was_sent(&u));
    assert!(!store.path().exists());
}

#[test]
fn codes_that_changed_since_they_were_shown_are_not_cleared() {
    let store = temp_store("stale");
    let mut u = uds(active_fault_sim());
    let mut shown = plan_clear(u.faults()).unwrap();
    shown[0].status = 0x00; // the person saw a different status than the module now holds
    let err = apply_clear(u.faults(), &store, &shown, &confirm(&shown)).unwrap_err();
    assert!(
        err.to_string().contains("changed since they were shown"),
        "{err}"
    );
    assert!(!clear_was_sent(&u), "evidence must not be destroyed");
    assert!(!store.path().exists());
}

#[test]
fn nothing_stored_means_nothing_to_clear() {
    let store = temp_store("empty");
    let sim = Simulator::new()
        .on(req(&READ), rsp(&[0x59, 0x02, 0xFF]))
        .on(req(&CLEAR), rsp(&[0x54]));
    let mut u = uds(sim);
    let shown = plan_clear(u.faults()).unwrap();
    assert!(shown.is_empty());
    let err = apply_clear(u.faults(), &store, &shown, &confirm(&shown)).unwrap_err();
    assert!(err.to_string().contains("nothing to clear"), "{err}");
    assert!(!clear_was_sent(&u));
}

#[test]
fn if_the_snapshot_cannot_be_written_nothing_is_sent_to_the_module() {
    let store = BackupStore::new("no-such-directory-xyz/backups.jsonl");
    let mut u = uds(active_fault_sim());
    let shown = plan_clear(u.faults()).unwrap();
    let err = apply_clear(u.faults(), &store, &shown, &confirm(&shown)).unwrap_err();
    assert!(matches!(err, Error::Io(_)), "{err}");
    assert!(!clear_was_sent(&u), "the clear request never left");
}

#[test]
fn a_module_that_refuses_is_recorded_and_writes_are_relocked() {
    let store = temp_store("refused");
    let sim = Simulator::new()
        .on(req(&READ), rsp(&ONE_CODE))
        .on(req(&CLEAR), rsp(&[0x7F, 0x14, 0x22]));
    let mut u = uds(sim);
    let shown = plan_clear(u.faults()).unwrap();
    let err = apply_clear(u.faults(), &store, &shown, &confirm(&shown)).unwrap_err();
    assert!(err.to_string().contains("did not clear"), "{err}");

    let rec = store.get("b0001").unwrap().unwrap();
    assert_eq!(rec.status, BackupStatus::SaveFailed);
    assert!(rec.note.unwrap().contains("clear failed"));
    assert_eq!(
        rec.codes,
        vec!["P0171-00 status 0x09"],
        "the snapshot is kept"
    );
    assert_eq!(u.write_mode(), WriteMode::ReadOnly);
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn a_fault_code_snapshot_cannot_be_restored() {
    let store = temp_store("norestore");
    let mut m = virtual_kw1281_module().unwrap();
    let shown = plan_clear(m.faults()).unwrap();
    let outcome = apply_clear(m.faults(), &store, &shown, &confirm(&shown)).unwrap();
    let err = plan_restore(m.adaptation().unwrap(), &store, &outcome.backup_id)
        .err()
        .unwrap();
    assert!(err.to_string().contains("cannot be written back"), "{err}");
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn records_written_before_snapshots_existed_still_load() {
    let store = temp_store("compat");
    // An adaptation record in the old format: no `codes` field.
    let old = r#"{"id":"b0001","created_unix":1,"protocol":"kw1281","module_address":1,"kind":"adaptation","channel":4,"old_value":1,"new_value":2,"status":"verified","note":null,"restores":null}"#;
    std::fs::write(store.path(), format!("{old}\n")).unwrap();
    let recs = store.records().unwrap();
    assert_eq!(recs.len(), 1);
    assert!(recs[0].codes.is_empty());
    // And a new snapshot gets the next id.
    let mut u = uds(active_fault_sim());
    let shown = plan_clear(u.faults()).unwrap();
    assert_eq!(
        apply_clear(u.faults(), &store, &shown, &confirm(&shown))
            .unwrap()
            .backup_id,
        "b0002"
    );
    let _ = std::fs::remove_file(store.path());
}
