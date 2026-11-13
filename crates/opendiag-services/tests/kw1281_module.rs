use opendiag_core::WriteMode;
use opendiag_services::demo::virtual_kw1281_module;
use opendiag_services::{ClearConfirmation, Module};

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
fn clear_records_previous_codes_and_relocks() {
    let mut m = virtual_kw1281_module().unwrap();
    let before = m
        .clear_dtcs(&ClearConfirmation::confirmed_by_user())
        .unwrap();
    assert_eq!(before.len(), 2);
    assert!(
        m.read_dtcs().unwrap().is_empty(),
        "module reports no faults now"
    );
    assert!(m.client().transport().cleared());
    assert_eq!(m.client().write_mode(), WriteMode::ReadOnly);
    assert!(m.client().transport().violations().is_empty());
}

#[test]
fn dropping_the_module_ends_the_session_cleanly() {
    let m = virtual_kw1281_module().unwrap();
    drop(m); // must not panic or hang
}
