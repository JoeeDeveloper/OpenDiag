//! ISO-TP over the Simulator transport with a fake clock.

use std::time::Duration;

use opendiag_core::Error;
use opendiag_proto::clock::{Clock, FakeClock};
use opendiag_proto::isotp::{Config, IsoTpLink};
use opendiag_transport::{Frame, Simulator};

const TESTER: u32 = 0x7E0;
const ECU: u32 = 0x7E8;
const MS: Duration = Duration::from_millis(1);

fn link(sim: Simulator, cfg: Config) -> IsoTpLink<Simulator, FakeClock> {
    IsoTpLink::new(sim, FakeClock::new(), TESTER, ECU, cfg)
}

#[test]
fn single_frame_request_and_reply() {
    let sim = Simulator::new().on(
        Frame::can(TESTER, &[0x03, 0x22, 0xF1, 0x90]),
        vec![Frame::can(ECU, &[0x04, 0x62, 0xF1, 0x90, 0x01])],
    );
    let mut l = link(sim, Config::default());
    let reply = l.request(&[0x22, 0xF1, 0x90], 100 * MS).unwrap();
    assert_eq!(reply, vec![0x62, 0xF1, 0x90, 0x01]);
}

#[test]
fn multi_frame_send_obeys_flow_control_and_stmin() {
    let payload: Vec<u8> = (0..20).collect();
    let first = Frame::can(TESTER, &[0x10, 0x14, 0, 1, 2, 3, 4, 5]);
    // Receiver asks for BS=0 and STmin=10 ms.
    let sim = Simulator::new().on(first.clone(), vec![Frame::can(ECU, &[0x30, 0x00, 0x0A])]);
    let mut l = link(sim, Config::default());
    l.send(&payload).unwrap();

    let sent = l.transport().sent();
    assert_eq!(sent.len(), 1 + 2, "FF + 2 CFs (14 bytes remaining)");
    assert_eq!(sent[0], first);
    assert_eq!(sent[1], Frame::can(TESTER, &[0x21, 6, 7, 8, 9, 10, 11, 12]));
    assert_eq!(
        sent[2],
        Frame::can(TESTER, &[0x22, 13, 14, 15, 16, 17, 18, 19])
    );
    assert_eq!(
        l.clock().now(),
        10 * MS,
        "one STmin gap between the two CFs"
    );
}

#[test]
fn multi_frame_receive_sends_flow_control() {
    let req = Frame::can(TESTER, &[0x02, 0x19, 0x02]);
    let sim = Simulator::new().on(
        req,
        vec![
            Frame::can(ECU, &[0x10, 0x0A, 0x59, 0x02, 0xFF, 1, 2, 3]),
            Frame::can(ECU, &[0x21, 4, 5, 6, 7, 0, 0, 0]),
        ],
    );
    let mut l = link(sim, Config::default());
    let reply = l.request(&[0x19, 0x02], 100 * MS).unwrap();
    assert_eq!(reply, vec![0x59, 0x02, 0xFF, 1, 2, 3, 4, 5, 6, 7]);
    // Request, then our flow control frame.
    assert_eq!(
        l.transport().sent().last().unwrap(),
        &Frame::can(TESTER, &[0x30, 0, 0])
    );
}

#[test]
fn send_times_out_when_no_flow_control_arrives() {
    let mut l = link(Simulator::new(), Config::default());
    let err = l.send(&[0; 20]).unwrap_err();
    assert!(matches!(err, Error::Timeout), "{err}");
    assert_eq!(l.clock().now(), 1000 * MS, "waited the full N_Bs");
}

#[test]
fn recv_times_out_on_silent_bus() {
    let mut l = link(Simulator::new(), Config::default());
    assert!(matches!(l.recv(50 * MS), Err(Error::Timeout)));
}

#[test]
fn frames_from_other_ids_are_ignored() {
    let req = Frame::can(TESTER, &[0x01, 0x3E]);
    let sim = Simulator::new().on(
        req,
        vec![
            Frame::can(0x123, &[0x02, 0xDE, 0xAD]),
            Frame::can(ECU, &[0x01, 0x7E]),
        ],
    );
    let mut l = link(sim, Config::default());
    assert_eq!(l.request(&[0x3E], 100 * MS).unwrap(), vec![0x7E]);
}
