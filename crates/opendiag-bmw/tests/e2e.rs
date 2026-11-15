//! End-to-end over real localhost sockets against the mock gateway.
//! No car and no network beyond loopback.

use std::time::Duration;

use opendiag_bmw::mock::{MockGateway, VirtualVehicle};
use opendiag_bmw::{discover, read_did, read_ident, HsfzGateway, HsfzLink, TcpByteStream};
use opendiag_core::{Error, WriteMode};
use opendiag_proto::clock::SystemClock;
use opendiag_proto::uds::{UdsClient, UdsConfig};

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
