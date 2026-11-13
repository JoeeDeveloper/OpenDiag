//! UDS client against the Simulator (ISO-TP underneath, fake clock).

use std::time::Duration;

use opendiag_core::{Error, Result, WriteMode};
use opendiag_proto::clock::FakeClock;
use opendiag_proto::isotp::{Config, IsoTpFrame, IsoTpLink};
use opendiag_proto::uds::{Dtc, RoutineAction, SeedKey, UdsClient, UdsConfig};
use opendiag_transport::{Frame, Simulator};

const TESTER: u32 = 0x7E0;
const ECU: u32 = 0x7E8;

/// Single-frame request as the tester will put it on the bus.
fn req(payload: &[u8]) -> Frame {
    let mut d = vec![payload.len() as u8];
    d.extend_from_slice(payload);
    Frame::can(TESTER, &d)
}

/// ECU reply of any length, segmented into ISO-TP frames.
fn reply(payload: &[u8]) -> Vec<Frame> {
    if payload.len() <= 7 {
        let mut d = vec![payload.len() as u8];
        d.extend_from_slice(payload);
        return vec![Frame::can(ECU, &d)];
    }
    let mut out = vec![Frame::can(
        ECU,
        &IsoTpFrame::First {
            total_len: payload.len(),
            data: payload[..6].to_vec(),
        }
        .encode(None),
    )];
    for (i, chunk) in payload[6..].chunks(7).enumerate() {
        let f = IsoTpFrame::Consecutive {
            sn: ((i + 1) & 0xF) as u8,
            data: chunk.to_vec(),
        };
        out.push(Frame::can(ECU, &f.encode(Some(0))));
    }
    out
}

fn client(sim: Simulator) -> UdsClient<Simulator, FakeClock> {
    let link = IsoTpLink::new(sim, FakeClock::new(), TESTER, ECU, Config::default());
    UdsClient::new(link, UdsConfig::default())
}

fn unlocked(sim: Simulator) -> UdsClient<Simulator, FakeClock> {
    let mut c = client(sim);
    c.set_write_mode(WriteMode::Unlocked);
    c
}

#[test]
fn session_control_parses_timings() {
    let sim = Simulator::new().on(
        req(&[0x10, 0x03]),
        reply(&[0x50, 0x03, 0x00, 0x32, 0x01, 0xF4]),
    );
    let t = client(sim).session_control(0x03).unwrap();
    assert_eq!(t.p2, Duration::from_millis(50));
    assert_eq!(t.p2_star, Duration::from_millis(5000));
}

#[test]
fn tester_present() {
    let sim = Simulator::new().on(req(&[0x3E, 0x00]), reply(&[0x7E, 0x00]));
    client(sim).tester_present().unwrap();
}

#[test]
fn read_vin_over_multi_frame_response() {
    let vin = b"WVWZZZ9NZ7Y123456";
    let mut payload = vec![0x62, 0xF1, 0x90];
    payload.extend_from_slice(vin);
    let sim = Simulator::new().on(req(&[0x22, 0xF1, 0x90]), reply(&payload));
    let got = client(sim).read_data_by_identifier(0xF190).unwrap();
    assert_eq!(got, vin);
}

#[test]
fn did_echo_mismatch_is_a_protocol_error() {
    let sim = Simulator::new().on(req(&[0x22, 0xF1, 0x90]), reply(&[0x62, 0xF1, 0x91, 0x00]));
    let err = client(sim).read_data_by_identifier(0xF190).unwrap_err();
    assert!(matches!(err, Error::Protocol(_)), "{err}");
}

#[test]
fn negative_response_becomes_error_with_code() {
    let sim = Simulator::new().on(req(&[0x22, 0xF1, 0x90]), reply(&[0x7F, 0x22, 0x31]));
    let err = client(sim).read_data_by_identifier(0xF190).unwrap_err();
    assert!(matches!(
        err,
        Error::NegativeResponse {
            service: 0x22,
            nrc: 0x31
        }
    ));
    assert_eq!(opendiag_proto::uds::nrc_name(0x31), "requestOutOfRange");
}

#[test]
fn response_pending_is_waited_out() {
    let mut frames = reply(&[0x7F, 0x22, 0x78]);
    frames.extend(reply(&[0x62, 0xF1, 0x90, 0x42]));
    let sim = Simulator::new().on(req(&[0x22, 0xF1, 0x90]), frames);
    assert_eq!(
        client(sim).read_data_by_identifier(0xF190).unwrap(),
        vec![0x42]
    );
}

#[test]
fn endless_response_pending_gives_up() {
    let frames: Vec<Frame> = (0..30).flat_map(|_| reply(&[0x7F, 0x22, 0x78])).collect();
    let sim = Simulator::new().on(req(&[0x22, 0xF1, 0x90]), frames);
    let err = client(sim).read_data_by_identifier(0xF190).unwrap_err();
    assert!(matches!(err, Error::Protocol(_)), "{err}");
}

#[test]
fn silent_ecu_times_out() {
    let err = client(Simulator::new()).tester_present().unwrap_err();
    assert!(matches!(err, Error::Timeout), "{err}");
}

#[test]
fn read_dtcs_by_status_mask() {
    let payload = [
        0x59, 0x02, 0xFF, 0x01, 0x23, 0x45, 0x2F, 0xAB, 0xCD, 0xEF, 0x08,
    ];
    let sim = Simulator::new().on(req(&[0x19, 0x02, 0xFF]), reply(&payload));
    let dtcs = client(sim).read_dtcs_by_status_mask(0xFF).unwrap();
    assert_eq!(
        dtcs,
        vec![
            Dtc {
                code: 0x012345,
                status: 0x2F
            },
            Dtc {
                code: 0xABCDEF,
                status: 0x08
            }
        ]
    );
}

#[test]
fn modifying_services_are_blocked_in_read_only_mode_and_send_nothing() {
    struct NoKey;
    impl SeedKey for NoKey {
        fn key(&self, _: u8, _: &[u8]) -> Result<Vec<u8>> {
            panic!("must not be called");
        }
    }
    let mut c = client(Simulator::new());
    assert_eq!(c.write_mode(), WriteMode::ReadOnly);
    assert!(matches!(
        c.clear_dtcs(0xFFFFFF),
        Err(Error::WritesLocked(_))
    ));
    assert!(matches!(
        c.write_data_by_identifier(0xF190, &[1]),
        Err(Error::WritesLocked(_))
    ));
    assert!(matches!(
        c.routine_control(RoutineAction::Start, 0xFF00, &[]),
        Err(Error::WritesLocked(_))
    ));
    assert!(matches!(
        c.security_access(0x01, &NoKey),
        Err(Error::WritesLocked(_))
    ));
    assert!(
        c.link().transport().sent().is_empty(),
        "nothing hit the bus"
    );
}

#[test]
fn clear_dtcs_when_unlocked() {
    let sim = Simulator::new().on(req(&[0x14, 0xFF, 0xFF, 0xFF]), reply(&[0x54]));
    unlocked(sim).clear_dtcs(0xFFFFFF).unwrap();
}

#[test]
fn write_data_by_identifier_when_unlocked() {
    let sim = Simulator::new().on(
        req(&[0x2E, 0x01, 0x00, 0xAA, 0xBB]),
        reply(&[0x6E, 0x01, 0x00]),
    );
    unlocked(sim)
        .write_data_by_identifier(0x0100, &[0xAA, 0xBB])
        .unwrap();
}

#[test]
fn routine_control_returns_status_bytes() {
    let sim = Simulator::new().on(
        req(&[0x31, 0x01, 0xFF, 0x00, 0x05]),
        reply(&[0x71, 0x01, 0xFF, 0x00, 0x99]),
    );
    let out = unlocked(sim)
        .routine_control(RoutineAction::Start, 0xFF00, &[0x05])
        .unwrap();
    assert_eq!(out, vec![0x99]);
}

struct XorKey(u8);
impl SeedKey for XorKey {
    fn key(&self, _level: u8, seed: &[u8]) -> Result<Vec<u8>> {
        Ok(seed.iter().map(|b| b ^ self.0).collect())
    }
}

#[test]
fn security_access_seed_then_key() {
    let sim = Simulator::new()
        .on(req(&[0x27, 0x01]), reply(&[0x67, 0x01, 0xAA, 0xBB]))
        .on(req(&[0x27, 0x02, 0x55, 0x44]), reply(&[0x67, 0x02]));
    unlocked(sim).security_access(0x01, &XorKey(0xFF)).unwrap();
}

#[test]
fn security_access_zero_seed_means_already_unlocked() {
    let sim = Simulator::new().on(req(&[0x27, 0x01]), reply(&[0x67, 0x01, 0x00, 0x00]));
    let mut c = unlocked(sim);
    c.security_access(0x01, &XorKey(0xFF)).unwrap();
    assert_eq!(c.link().transport().sent().len(), 1, "no key sent");
}

#[test]
fn security_access_invalid_key_is_reported() {
    let sim = Simulator::new()
        .on(req(&[0x27, 0x01]), reply(&[0x67, 0x01, 0xAA, 0xBB]))
        .on(req(&[0x27, 0x02, 0x55, 0x44]), reply(&[0x7F, 0x27, 0x35]));
    let err = unlocked(sim)
        .security_access(0x01, &XorKey(0xFF))
        .unwrap_err();
    assert!(matches!(
        err,
        Error::NegativeResponse {
            service: 0x27,
            nrc: 0x35
        }
    ));
}

#[test]
fn security_access_rejects_even_level() {
    let err = unlocked(Simulator::new())
        .security_access(0x02, &XorKey(0))
        .unwrap_err();
    assert!(matches!(err, Error::Protocol(_)));
}
