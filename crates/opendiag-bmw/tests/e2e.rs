//! End-to-end over real localhost sockets against the mock gateway.
//! No car and no network beyond loopback.

use std::time::Duration;

use opendiag_bmw::ecus::known_addresses;
use opendiag_bmw::mock::{MockGateway, VirtualVehicle};
use opendiag_bmw::{
    discover, log_dids_csv, read_did, read_ident, scan, BmwEcu, DidLogConfig, HsfzGateway,
    HsfzLink, TcpByteStream,
};
use opendiag_core::{Error, WriteMode};
use opendiag_proto::clock::SystemClock;
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_services::{
    apply_block_change, apply_clear, plan_block_change, plan_block_restore, plan_clear,
    BackupStatus, BackupStore, BlockAccess, BlockConfirmation, ClearConfirmation, Module,
};

type Link = HsfzLink<TcpByteStream, SystemClock>;

fn setup() -> (MockGateway, HsfzGateway<TcpByteStream, SystemClock>) {
    let mock = MockGateway::start(VirtualVehicle::demo()).unwrap();
    let stream = TcpByteStream::connect(mock.tcp_addr()).unwrap();
    let gw = HsfzGateway::new(stream, SystemClock::new(), 0xF4);
    (mock, gw)
}

fn uds(gw: &HsfzGateway<TcpByteStream, SystemClock>, ecu: u8) -> UdsClient<Link> {
    UdsClient::new(gw.link(ecu), UdsConfig::default())
}

#[test]
fn discovery_finds_the_vehicle_and_parses_it() {
    let mock = MockGateway::start(VirtualVehicle::demo()).unwrap();
    let found = discover(mock.udp_addr(), Duration::from_millis(400)).unwrap();
    assert_eq!(found.len(), 1);
    let d = &found[0];
    assert_eq!(d.vin.as_deref(), Some("WBASIMULATED00001"));
    assert_eq!(d.diag_address, Some(0x10));
    assert_eq!(d.mac.as_deref(), Some("001A37B44210"));
    assert!(d.ip.is_loopback());
}

#[test]
fn discovery_with_no_answer_returns_nothing() {
    // A bound-but-silent UDP socket stands in for an empty network.
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let found = discover(silent.local_addr().unwrap(), Duration::from_millis(150)).unwrap();
    assert!(found.is_empty());
}

#[test]
fn reads_vin_i_level_and_fa_from_the_gateway() {
    let (_mock, gw) = setup();
    let id = read_ident(&mut uds(&gw, 0x10)).unwrap();
    assert_eq!(id.vin.as_deref(), Some("WBASIMULATED00001"));
    assert_eq!(id.i_level_text().as_deref(), Some("F020-00-00-000"));
    assert_eq!(id.fa_raw, Some(vec![1, 2, 3, 4]));
}

#[test]
fn a_missing_did_is_reported_as_absent_not_an_error() {
    let (_mock, gw) = setup();
    // ECU 0x12 has a VIN but no I-level / FA DIDs.
    let id = read_ident(&mut uds(&gw, 0x12)).unwrap();
    assert!(id.vin.is_some());
    assert_eq!(id.i_level_raw, None);
    assert_eq!(id.fa_raw, None);
}

#[test]
fn response_pending_is_waited_out() {
    let (mock, gw) = setup();
    let mut u = uds(&gw, 0x12);
    assert_eq!(read_did(&mut u, 0x4001).unwrap(), Some(vec![0x12, 0x34]));
    assert_eq!(mock.requests().len(), 1, "one request, two replies");
}

#[test]
fn two_ecus_share_one_connection() {
    let (_mock, gw) = setup();
    let mut a = uds(&gw, 0x10);
    let mut b = uds(&gw, 0x12);
    assert!(read_did(&mut a, 0xF190).unwrap().is_some());
    assert!(read_did(&mut b, 0xF190).unwrap().is_some());
    assert!(read_did(&mut a, 0x100B).unwrap().is_some());
}

#[test]
fn reads_dtcs_with_status() {
    let (_mock, gw) = setup();
    let dtcs = uds(&gw, 0x12).read_dtcs_by_status_mask(0xFF).unwrap();
    assert_eq!(dtcs.len(), 2);
    assert_eq!(dtcs[0].code, 0x480A12);
    assert_eq!(dtcs[0].status, 0x09);
}

#[test]
fn an_unknown_ecu_is_a_clear_gateway_error() {
    let (_mock, gw) = setup();
    let err = read_did(&mut uds(&gw, 0x99), 0xF190).unwrap_err();
    assert!(
        err.to_string().contains("incorrect destination address"),
        "{err}"
    );
}

#[test]
fn a_negative_response_is_decoded() {
    let (_mock, gw) = setup();
    let mut u = uds(&gw, 0x12);
    let err = u.request(&[0x27, 0x01]).unwrap_err();
    assert!(matches!(
        err,
        Error::NegativeResponse {
            service: 0x27,
            nrc: 0x11
        }
    ));
}

#[test]
fn read_only_clients_cannot_clear_and_nothing_reaches_the_gateway() {
    let (mock, gw) = setup();
    let mut u = uds(&gw, 0x12);
    assert_eq!(u.write_mode(), WriteMode::ReadOnly);
    assert!(matches!(
        u.clear_dtcs(0xFF_FFFF),
        Err(Error::WritesLocked(_))
    ));
    assert!(mock.requests().is_empty());
}

#[test]
fn programming_session_is_refused_even_when_writes_are_unlocked() {
    let (mock, gw) = setup();
    let mut u = uds(&gw, 0x12);
    u.set_write_mode(WriteMode::Unlocked);
    assert!(matches!(
        u.session_control(0x02),
        Err(Error::WritesLocked(_))
    ));
    assert!(mock.requests().is_empty());
    // The extended session is allowed and answered.
    u.session_control(0x03).unwrap();
}

#[test]
fn reconnecting_gives_a_working_session_again() {
    let (_mock, gw) = setup();
    let mut u = uds(&gw, 0x10);
    assert!(read_did(&mut u, 0xF190).unwrap().is_some());
    gw.reconnect().unwrap();
    assert!(read_did(&mut u, 0xF190).unwrap().is_some());
}

#[test]
fn tester_present_keeps_the_session_alive() {
    let (_mock, gw) = setup();
    uds(&gw, 0x12).tester_present().unwrap();
}

fn ecu(gw: &HsfzGateway<TcpByteStream, SystemClock>, address: u8) -> BmwEcu<Link> {
    BmwEcu::new(gw.link(address), address, UdsConfig::default())
}

fn temp_store(name: &str) -> BackupStore {
    let path =
        std::env::temp_dir().join(format!("opendiag-bmw-{}-{name}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    BackupStore::new(path)
}

#[test]
fn scan_of_the_known_addresses_finds_the_ecus_and_counts_their_codes() {
    let (_mock, gw) = setup();
    let found = scan(&gw, &known_addresses(), Duration::from_millis(300)).unwrap();
    let get = |a: u8| found.iter().find(|e| e.address == a).unwrap();
    assert!(get(0x10).present);
    assert_eq!(get(0x10).dtc_count, Some(0));
    assert_eq!(get(0x12).dtc_count, Some(2));
    assert_eq!(get(0x60).dtc_count, Some(1));
    // In the table but not in this car: the gateway answers with an error frame.
    assert!(!get(0x29).present);
    assert_eq!(get(0x29).dtc_count, None);
    assert!(!get(0x40).present);
}

#[test]
fn a_full_address_sweep_finds_exactly_the_cars_ecus() {
    let (_mock, gw) = setup();
    let all: Vec<u8> = (0..=0xFFu8).collect();
    let found = scan(&gw, &all, Duration::from_millis(300)).unwrap();
    let present: Vec<u8> = found
        .iter()
        .filter(|e| e.present)
        .map(|e| e.address)
        .collect();
    assert_eq!(present, vec![0x10, 0x12, 0x18, 0x60]);
}

#[test]
fn scanning_never_sends_anything_but_tester_present_and_dtc_reads() {
    let (mock, gw) = setup();
    scan(&gw, &known_addresses(), Duration::from_millis(300)).unwrap();
    for (_, req) in mock.requests() {
        assert!(
            matches!(req[0], 0x3E | 0x19),
            "unexpected request {req:02X?}"
        );
    }
}

#[test]
fn bmw_dtcs_are_shown_as_raw_hex_with_status_flags() {
    let (_mock, gw) = setup();
    let dtcs = ecu(&gw, 0x12).read_dtcs().unwrap();
    assert_eq!(dtcs[0].text, "480A12");
    assert_eq!(dtcs[0].flags, vec!["testFailed", "confirmed"]);
}

#[test]
fn clearing_goes_through_the_guarded_flow_with_a_snapshot() {
    let (mock, gw) = setup();
    let store = temp_store("clear");
    let mut e = ecu(&gw, 0x12);
    let shown = plan_clear(e.faults()).unwrap();
    assert_eq!(shown.len(), 2);
    assert!(
        !mock.requests().iter().any(|(_, r)| r[0] == 0x14),
        "planning must not clear anything"
    );
    let out = apply_clear(
        e.faults(),
        &store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    )
    .unwrap();
    assert_eq!(out.cleared.len(), 2);
    assert!(out.remaining.is_empty());
    let text = std::fs::read_to_string(store.path()).unwrap();
    assert!(text.contains("\"kind\":\"dtc_clear\""), "{text}");
    assert!(text.contains("hsfz-uds"), "{text}");
    assert!(
        text.contains("480A12"),
        "snapshot must hold the codes: {text}"
    );
    assert!(text.contains("\"status\":\"verified\""), "{text}");
    assert!(plan_clear(e.faults()).unwrap().is_empty());
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn a_confirmation_for_other_codes_clears_nothing() {
    let (mock, gw) = setup();
    let store = temp_store("mismatch");
    let mut e = ecu(&gw, 0x12);
    let shown = plan_clear(e.faults()).unwrap();
    let other = ClearConfirmation::for_codes(&shown[..1]);
    assert!(apply_clear(e.faults(), &store, &shown, &other).is_err());
    assert!(!mock.requests().iter().any(|(_, r)| r[0] == 0x14));
    assert!(!store.path().exists());
}

#[test]
fn clearing_is_impossible_if_the_snapshot_cannot_be_written() {
    let (mock, gw) = setup();
    let store = BackupStore::new("no-such-directory-xyz/backups.jsonl");
    let mut e = ecu(&gw, 0x12);
    let shown = plan_clear(e.faults()).unwrap();
    assert!(apply_clear(
        e.faults(),
        &store,
        &shown,
        &ClearConfirmation::for_codes(&shown)
    )
    .is_err());
    assert!(!mock.requests().iter().any(|(_, r)| r[0] == 0x14));
    assert_eq!(
        plan_clear(e.faults()).unwrap().len(),
        2,
        "codes must still be there"
    );
}

#[test]
fn live_data_logs_raw_values_on_a_steady_grid() {
    let (_mock, gw) = setup();
    let mut u = uds(&gw, 0x12);
    let cfg = DidLogConfig {
        dids: vec![0x4002, 0xF190, 0xBEEF],
        interval: Duration::from_millis(10),
        samples: 3,
    };
    let mut out = Vec::new();
    let sum = log_dids_csv(&mut u, &SystemClock::new(), &cfg, &mut out).unwrap();
    assert_eq!((sum.samples, sum.rows), (3, 9));
    let text = String::from_utf8(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "time_ms,did,status,hex");
    assert_eq!(lines.len(), 10);
    assert!(lines[1].ends_with("0x4002,ok,0BB8"), "{}", lines[1]);
    assert!(lines[3].ends_with("0xBEEF,absent,"), "{}", lines[3]);
}

fn writes_seen(mock: &MockGateway) -> usize {
    mock.requests().iter().filter(|(_, r)| r[0] == 0x2E).count()
}

fn changed(block: &[u8], at: usize, to: u8) -> Vec<u8> {
    let mut v = block.to_vec();
    v[at] = to;
    v
}

#[test]
fn a_block_write_is_backed_up_written_and_read_back() {
    let (mock, gw) = setup();
    let store = temp_store("block-ok");
    let mut e = ecu(&gw, 0x12);
    let old = e.read_block(0x2000).unwrap();
    let change = plan_block_change(&mut e, 0x2000, changed(&old, 3, 0xAA)).unwrap();
    assert_eq!(change.differing_offsets(), vec![3]);
    assert_eq!(writes_seen(&mock), 0, "planning must not write");
    let done = apply_block_change(
        &mut e,
        &store,
        &change,
        &BlockConfirmation::for_change(&change),
    )
    .unwrap();
    assert_eq!(e.read_block(0x2000).unwrap(), changed(&old, 3, 0xAA));
    let rec = store.get(&done.backup_id).unwrap().unwrap();
    assert_eq!(rec.kind, "block_write");
    assert_eq!(rec.old_hex.as_deref(), Some("0102030405060708"));
    assert_eq!(rec.new_hex.as_deref(), Some("010203AA05060708"));
    assert_eq!(rec.status, BackupStatus::Verified);
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn a_block_write_can_be_restored_byte_for_byte() {
    let (_mock, gw) = setup();
    let store = temp_store("block-restore");
    let mut e = ecu(&gw, 0x12);
    let old = e.read_block(0x2000).unwrap();
    let change = plan_block_change(&mut e, 0x2000, changed(&old, 0, 0xFF)).unwrap();
    let done = apply_block_change(
        &mut e,
        &store,
        &change,
        &BlockConfirmation::for_change(&change),
    )
    .unwrap();
    let back = plan_block_restore(&mut e, &store, &done.backup_id).unwrap();
    assert_eq!(back.new, old);
    let again =
        apply_block_change(&mut e, &store, &back, &BlockConfirmation::for_change(&back)).unwrap();
    assert_eq!(e.read_block(0x2000).unwrap(), old);
    assert_eq!(
        store.get(&done.backup_id).unwrap().unwrap().status,
        BackupStatus::Restored
    );
    assert_eq!(
        store
            .get(&again.backup_id)
            .unwrap()
            .unwrap()
            .restores
            .as_deref(),
        Some(done.backup_id.as_str())
    );
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn a_write_the_ecu_does_not_keep_is_reported_and_recorded() {
    let (_mock, gw) = setup();
    let store = temp_store("block-lie");
    let mut e = ecu(&gw, 0x12);
    let change = plan_block_change(&mut e, 0x2001, vec![9; 8]).unwrap();
    let err = apply_block_change(
        &mut e,
        &store,
        &change,
        &BlockConfirmation::for_change(&change),
    )
    .unwrap_err();
    assert!(err.to_string().contains("reads back differently"), "{err}");
    let rec = store.records().unwrap().pop().unwrap();
    assert_eq!(rec.status, BackupStatus::VerificationFailed);
    assert_eq!(rec.old_hex.as_deref(), Some("0000000000000000"));
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn refused_block_writes_send_nothing() {
    let (mock, gw) = setup();
    let store = temp_store("block-refused");
    let mut e = ecu(&gw, 0x12);
    let old = e.read_block(0x2000).unwrap();
    let change = plan_block_change(&mut e, 0x2000, changed(&old, 1, 0x77)).unwrap();

    // Confirmation for a different change.
    let other = plan_block_change(&mut e, 0x2000, changed(&old, 2, 0x77)).unwrap();
    assert!(apply_block_change(
        &mut e,
        &store,
        &change,
        &BlockConfirmation::for_change(&other)
    )
    .is_err());
    // Not a change.
    let same = plan_block_change(&mut e, 0x2000, old.clone()).unwrap();
    assert!(
        apply_block_change(&mut e, &store, &same, &BlockConfirmation::for_change(&same)).is_err()
    );
    // Length change.
    let short = plan_block_change(&mut e, 0x2000, old[..4].to_vec()).unwrap();
    assert!(apply_block_change(
        &mut e,
        &store,
        &short,
        &BlockConfirmation::for_change(&short)
    )
    .is_err());
    // Identity data (the VIN) is never writable.
    assert!(plan_block_change(&mut e, 0xF190, vec![0; 17]).is_err());
    // Snapshot cannot be written.
    let bad = BackupStore::new("no-such-directory-xyz/backups.jsonl");
    assert!(apply_block_change(
        &mut e,
        &bad,
        &change,
        &BlockConfirmation::for_change(&change)
    )
    .is_err());

    assert_eq!(writes_seen(&mock), 0, "no write may reach the ECU");
    assert!(!store.path().exists());
    assert_eq!(e.read_block(0x2000).unwrap(), old);
}

#[test]
fn restoring_from_another_modules_backup_is_refused() {
    let (_mock, gw) = setup();
    let store = temp_store("block-other");
    let mut a = ecu(&gw, 0x12);
    let old = a.read_block(0x2000).unwrap();
    let change = plan_block_change(&mut a, 0x2000, changed(&old, 0, 0x42)).unwrap();
    let done = apply_block_change(
        &mut a,
        &store,
        &change,
        &BlockConfirmation::for_change(&change),
    )
    .unwrap();
    let mut other = ecu(&gw, 0x60);
    assert!(plan_block_restore(&mut other, &store, &done.backup_id).is_err());
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn the_extended_session_is_opt_in_and_the_programming_session_is_still_refused() {
    let (mock, gw) = setup();
    let mut plain = ecu(&gw, 0x12);
    plain.read_block(0x2000).unwrap();
    assert!(!mock.requests().iter().any(|(_, r)| r[0] == 0x10));
    let mut ext = BmwEcu::new(gw.link(0x12), 0x12, UdsConfig::default()).with_extended_session();
    ext.read_block(0x2000).unwrap();
    assert!(mock.requests().iter().any(|(_, r)| r == &[0x10, 0x03]));
    assert!(!mock.requests().iter().any(|(_, r)| r[..2] == [0x10, 0x02]));
}
