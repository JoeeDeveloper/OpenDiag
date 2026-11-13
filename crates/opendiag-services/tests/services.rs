use opendiag_core::Error;
use opendiag_proto::clock::FakeClock;
use opendiag_proto::isotp::{Config, IsoTpLink};
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_services::demo::{virtual_uds_client, ECU_ID, TESTER_ID};
use opendiag_services::{identify, read_dtcs};
use opendiag_transport::{Frame, Simulator};

fn client(sim: Simulator) -> UdsClient<Simulator, FakeClock> {
    let link = IsoTpLink::new(sim, FakeClock::new(), TESTER_ID, ECU_ID, Config::default());
    UdsClient::new(link, UdsConfig::default())
}

/// Single-frame request / reply helpers for hand-scripted exchanges.
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

#[test]
fn identify_reads_all_items() {
    let ident = identify(&mut virtual_uds_client()).unwrap();
    assert_eq!(ident.part_number.as_deref(), Some("DEMO-0001-A"));
    assert_eq!(ident.software_version.as_deref(), Some("SW 1.0.3"));
    assert_eq!(ident.vin.as_deref(), Some("DEMO00000000VIN01"));
}

#[test]
fn identifiers_the_ecu_does_not_have_become_none() {
    // requestOutOfRange (0x31) for two DIDs, a real value for the third.
    let sim = Simulator::new()
        .on(req(&[0x22, 0xF1, 0x87]), rsp(&[0x7F, 0x22, 0x31]))
        .on(req(&[0x22, 0xF1, 0x89]), rsp(&[0x7F, 0x22, 0x31]))
        .on(
            req(&[0x22, 0xF1, 0x90]),
            rsp(&[0x62, 0xF1, 0x90, b'A', b'B']),
        );
    let ident = identify(&mut client(sim)).unwrap();
    assert_eq!(ident.part_number, None);
    assert_eq!(ident.software_version, None);
    assert_eq!(ident.vin.as_deref(), Some("AB"));
}

#[test]
fn a_silent_ecu_is_an_error_not_unsupported() {
    assert!(matches!(
        identify(&mut client(Simulator::new())),
        Err(Error::Timeout)
    ));
}

#[test]
fn reads_and_formats_dtcs() {
    let dtcs = read_dtcs(&mut virtual_uds_client()).unwrap();
    assert_eq!(dtcs.len(), 2);
    assert_eq!(dtcs[0].text, "P0171-00");
    assert_eq!(dtcs[0].flags, vec!["testFailed", "confirmed"]);
    assert_eq!(dtcs[1].text, "P0300-00");
}
