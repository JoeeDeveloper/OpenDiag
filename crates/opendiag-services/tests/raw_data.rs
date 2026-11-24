use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_proto::clock::{Clock, FakeClock};
use opendiag_services::demo::{virtual_kwp_module, virtual_uds_client};
use opendiag_services::{log_raw_csv, Limit, Module, RawData, RawLogConfig, RAW_CSV_HEADER};

fn cfg(ids: &[u16], interval_ms: u64, limit: Limit) -> RawLogConfig {
    RawLogConfig {
        ids: ids.to_vec(),
        interval: Duration::from_millis(interval_ms),
        limit,
    }
}

fn log(
    src: &mut dyn RawData,
    cfg: &RawLogConfig,
) -> (Result<opendiag_services::LogSummary>, String) {
    let clock = FakeClock::new();
    let mut out = Vec::new();
    let r = log_raw_csv(src, &clock, cfg, &mut out);
    (r, String::from_utf8(out).unwrap())
}

#[test]
fn kwp_local_identifiers_are_logged_as_uninterpreted_hex_on_a_steady_grid() {
    let mut m = virtual_kwp_module().unwrap();
    let raw = m.raw_data().expect("KWP2000 offers raw data");
    assert_eq!(raw.id_kind(), "local identifier");
    let (r, csv) = log(raw, &cfg(&[1, 2], 100, Limit::Samples(2)));
    let s = r.unwrap();
    assert_eq!((s.samples, s.rows), (2, 4));
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], RAW_CSV_HEADER);
    // time, id, length, hex: the module's bytes, nothing decoded.
    assert_eq!(lines[1], "0,0x0001,4,10203000");
    assert_eq!(lines[2], "0,0x0002,2,0065");
    assert!(lines[3].starts_with("100,0x0001,4,"), "{}", lines[3]);
    assert_ne!(
        lines[1][8..],
        lines[3][10..],
        "the demo values move between reads"
    );
}

#[test]
fn uds_data_identifiers_are_logged_too() {
    let mut m = virtual_uds_client();
    let raw = m.raw_data().expect("UDS offers raw data");
    let (r, csv) = log(raw, &cfg(&[0xF190], 50, Limit::Samples(1)));
    r.unwrap();
    assert!(
        csv.contains("0,0xF190,17,44454D4F303030303030303056494E3031"),
        "{csv}"
    );
}

#[test]
fn an_identifier_the_protocol_cannot_express_is_refused_before_anything_is_sent() {
    let mut m = virtual_kwp_module().unwrap();
    let raw = m.raw_data().unwrap();
    let (r, _) = log(raw, &cfg(&[0x1FF], 100, Limit::Samples(1)));
    assert!(matches!(r, Err(Error::Protocol(m)) if m.contains("local identifier")));
}

#[test]
fn an_unknown_identifier_is_the_modules_negative_response_not_a_guess() {
    let mut m = virtual_kwp_module().unwrap();
    let raw = m.raw_data().unwrap();
    let (r, csv) = log(raw, &cfg(&[1, 0x99], 100, Limit::Samples(1)));
    assert!(matches!(r, Err(Error::NegativeResponse { .. })), "{r:?}");
    // The row read before the failure is already in the output.
    assert!(csv.contains("0,0x0001,4,"), "{csv}");
}

#[test]
fn no_identifiers_or_a_zero_interval_is_refused() {
    let mut m = virtual_kwp_module().unwrap();
    let raw = m.raw_data().unwrap();
    assert!(log(raw, &cfg(&[], 100, Limit::Samples(1))).0.is_err());
    assert!(log(raw, &cfg(&[1], 0, Limit::Samples(1))).0.is_err());
}

#[test]
fn a_duration_limit_stops_on_the_grid() {
    let clock = FakeClock::new();
    let mut m = virtual_kwp_module().unwrap();
    let raw = m.raw_data().unwrap();
    let mut out = Vec::new();
    let s = log_raw_csv(
        raw,
        &clock,
        &cfg(&[1], 100, Limit::Duration(Duration::from_millis(300))),
        &mut out,
    )
    .unwrap();
    assert_eq!(s.samples, 3);
    assert_eq!(clock.now(), Duration::from_millis(200));
}

#[test]
fn kw1281_has_no_raw_identifiers() {
    let mut m = opendiag_services::demo::virtual_kw1281_module().unwrap();
    assert!(m.raw_data().is_none());
}
