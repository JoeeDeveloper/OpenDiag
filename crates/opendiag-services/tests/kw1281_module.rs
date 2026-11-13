use opendiag_core::WriteMode;
use opendiag_services::demo::virtual_kw1281_module;
use opendiag_services::{
    apply_clear, plan_clear, BackupStatus, BackupStore, ClearConfirmation, Module,
};

#[test]
fn identify_uses_ident_lines_with_part_number_first() {
    let mut m = virtual_kw1281_module().unwrap();
    let ident = m.identify().unwrap();
    assert_eq!(ident.part_number.as_deref(), Some("DEMO-KW-0001"));
    assert_eq!(ident.lines, vec!["DEMO-KW-0001", "VIRTUAL ENGINE ECU"]);
    assert_eq!(ident.vin, None);
}

#[test]
fn reads_faults_as_five_digit_codes_with_raw_status() {
    let mut m = virtual_kw1281_module().unwrap();
    let dtcs = m.read_dtcs().unwrap();
    assert_eq!(dtcs.len(), 2);
    assert_eq!(dtcs[0].text, "00525");
    assert_eq!(dtcs[0].status, 0x20);
    assert!(
        dtcs[0].flags.is_empty(),
        "no guessed meaning for status bytes"
    );
}

#[test]
fn guarded_clear_snapshots_clears_verifies_and_relocks() {
    let path = std::env::temp_dir().join(format!("opendiag-km-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let store = BackupStore::new(&path);

    let mut m = virtual_kw1281_module().unwrap();
    let shown = plan_clear(m.faults()).unwrap();
    assert_eq!(shown.len(), 2);
    let outcome = apply_clear(
        m.faults(),
        &store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    )
    .unwrap();

    assert_eq!(outcome.cleared.len(), 2);
    assert!(outcome.remaining.is_empty());
    assert!(
        m.read_dtcs().unwrap().is_empty(),
        "module reports no faults now"
    );
    assert!(m.client().transport().cleared());
    assert_eq!(m.client().write_mode(), WriteMode::ReadOnly);
    assert!(m.client().transport().violations().is_empty());

    let rec = store.get(&outcome.backup_id).unwrap().unwrap();
    assert_eq!(rec.kind, "dtc_clear");
    assert_eq!(rec.status, BackupStatus::Verified);
    assert_eq!(rec.codes, vec!["00525 status 0x20", "00017 status 0x01"]);
    assert_eq!(rec.module.as_deref(), Some("KW1281 module 0x01"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dropping_the_module_ends_the_session_cleanly() {
    let m = virtual_kw1281_module().unwrap();
    drop(m); // must not panic or hang
}
