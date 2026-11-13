//! KWP2000 client against the Simulator carrying raw K-line bytes.

use opendiag_core::{Error, Result, WriteMode};
use opendiag_proto::kwp::{KeyBytes, KwpClient, KwpConfig, KwpDtc, KwpService, Message};
use opendiag_proto::uds::SeedKey;
use opendiag_transport::{Frame, Simulator};

const ECU: u8 = 0x01;
const TESTER: u8 = 0xF1;

/// Bytes the tester puts on the wire for `data`.
fn req(data: &[u8]) -> Frame {
    Frame::Raw(
        Message {
            target: ECU,
            source: TESTER,
            data: data.to_vec(),
        }
        .encode()
        .unwrap(),
    )
}

/// Bytes the ECU answers with.
fn resp(data: &[u8]) -> Frame {
    Frame::Raw(
        Message {
            target: TESTER,
            source: ECU,
            data: data.to_vec(),
        }
        .encode()
        .unwrap(),
    )
}

fn client(sim: Simulator) -> KwpClient<Simulator> {
    KwpClient::new(sim, KwpConfig::new(ECU))
}

fn unlocked(sim: Simulator) -> KwpClient<Simulator> {
    let mut c = client(sim);
    c.set_write_mode(WriteMode::Unlocked);
    c
}

#[test]
fn fast_init_then_start_communication() {
    let sim = Simulator::new().on(req(&[0x81]), vec![resp(&[0xC1, 0xEF, 0x8F])]);
    let mut c = client(sim);
    let kb = c.start_communication().unwrap();
    assert_eq!(
        kb,
        KeyBytes {
            kb1: 0xEF,
            kb2: 0x8F
        }
    );
    assert_eq!(c.transport().fast_inits(), 1);
}

#[test]
fn own_echo_and_foreign_traffic_are_ignored() {
    let echo = req(&[0x21, 0x01]);
    let other_ecu = Frame::Raw(
        Message {
            target: TESTER,
            source: 0x17,
            data: vec![0x61, 0x01, 0xFF],
        }
        .encode()
        .unwrap(),
    );
    let sim = Simulator::new().on(
        req(&[0x21, 0x01]),
        vec![echo, other_ecu, resp(&[0x61, 0x01, 0xAB, 0xCD])],
    );
    assert_eq!(
        client(sim).read_data_by_local_identifier(0x01).unwrap(),
        vec![0xAB, 0xCD]
    );
}

#[test]
fn response_split_across_reads_is_reassembled() {
    let Frame::Raw(bytes) = resp(&[0x5A, 0x9B, b'0', b'3', b'6']) else {
        unreachable!()
    };
    let (a, b) = bytes.split_at(3);
    let sim = Simulator::new().on(
        req(&[0x1A, 0x9B]),
        vec![Frame::Raw(a.to_vec()), Frame::Raw(b.to_vec())],
    );
    assert_eq!(
        client(sim).read_ecu_identification(0x9B).unwrap(),
        b"036".to_vec()
    );
}

#[test]
fn read_dtcs_by_status() {
    let sim = Simulator::new().on(
        req(&[0x18, 0x00, 0xFF, 0x00]),
        vec![resp(&[0x58, 0x02, 0x01, 0x23, 0x20, 0x45, 0x67, 0x2F])],
    );
    let dtcs = client(sim).read_dtcs_by_status(0x00, 0xFF00).unwrap();
    assert_eq!(
        dtcs,
        vec![
            KwpDtc {
                code: 0x0123,
                status: 0x20
            },
            KwpDtc {
                code: 0x4567,
                status: 0x2F
            }
        ]
    );
}

#[test]
fn dtc_count_mismatch_is_an_error() {
    let sim = Simulator::new().on(
        req(&[0x18, 0x00, 0xFF, 0x00]),
        vec![resp(&[0x58, 0x02, 0x01, 0x23, 0x20])],
    );
    assert!(matches!(
        client(sim).read_dtcs_by_status(0x00, 0xFF00),
        Err(Error::Protocol(_))
    ));
}

#[test]
fn negative_response_and_pending() {
    let sim = Simulator::new().on(req(&[0x21, 0x05]), vec![resp(&[0x7F, 0x21, 0x12])]);
    assert!(matches!(
        client(sim).read_data_by_local_identifier(0x05),
        Err(Error::NegativeResponse {
            service: 0x21,
            nrc: 0x12
        })
    ));

    let sim = Simulator::new().on(
        req(&[0x21, 0x05]),
        vec![resp(&[0x7F, 0x21, 0x78]), resp(&[0x61, 0x05, 0x01])],
    );
    assert_eq!(
        client(sim).read_data_by_local_identifier(0x05).unwrap(),
        vec![0x01]
    );
}

#[test]
fn corrupt_checksum_is_reported() {
    let Frame::Raw(mut bytes) = resp(&[0x7E]) else {
        unreachable!()
    };
    *bytes.last_mut().unwrap() ^= 0x55;
    let sim = Simulator::new().on(req(&[0x3E]), vec![Frame::Raw(bytes)]);
    assert!(matches!(
        client(sim).tester_present(),
        Err(Error::Protocol(_))
    ));
}

#[test]
fn silent_ecu_times_out() {
    assert!(matches!(
        client(Simulator::new()).tester_present(),
        Err(Error::Timeout)
    ));
}

#[test]
fn session_and_stop() {
    let sim = Simulator::new()
        .on(req(&[0x10, 0x89]), vec![resp(&[0x50, 0x89])])
        .on(req(&[0x82]), vec![resp(&[0xC2])]);
    let mut c = client(sim);
    c.start_diagnostic_session(0x89).unwrap();
    c.stop_communication().unwrap();
}

struct XorKey(u8);
impl SeedKey for XorKey {
    fn key(&self, _level: u8, seed: &[u8]) -> Result<Vec<u8>> {
        Ok(seed.iter().map(|b| b ^ self.0).collect())
    }
}

#[test]
fn modifying_services_blocked_when_read_only_and_send_nothing() {
    let mut c = client(Simulator::new());
    assert_eq!(c.write_mode(), WriteMode::ReadOnly);
    assert!(matches!(c.clear_dtcs(0xFF00), Err(Error::WritesLocked(_))));
    assert!(matches!(
        c.write_data_by_local_identifier(0x10, &[1]),
        Err(Error::WritesLocked(_))
    ));
    assert!(matches!(
        c.security_access(0x01, &XorKey(0)),
        Err(Error::WritesLocked(_))
    ));
    assert!(c.transport().sent().is_empty());
}

#[test]
fn gated_services_work_when_unlocked() {
    let sim = Simulator::new()
        .on(req(&[0x14, 0xFF, 0x00]), vec![resp(&[0x54, 0xFF, 0x00])])
        .on(req(&[0x3B, 0x10, 0xAA]), vec![resp(&[0x7B, 0x10])])
        .on(req(&[0x27, 0x01]), vec![resp(&[0x67, 0x01, 0xAA, 0xBB])])
        .on(req(&[0x27, 0x02, 0x55, 0x44]), vec![resp(&[0x67, 0x02])]);
    let mut c = unlocked(sim);
    c.clear_dtcs(0xFF00).unwrap();
    c.write_data_by_local_identifier(0x10, &[0xAA]).unwrap();
    c.security_access(0x01, &XorKey(0xFF)).unwrap();
}
