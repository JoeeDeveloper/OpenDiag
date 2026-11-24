use std::time::Duration;

use opendiag_transport::{Frame, TraceEcu, Transport};

const T: Duration = Duration::from_millis(10);

fn can(id: u32, data: &[u8]) -> Frame {
    Frame::Can {
        id,
        extended: false,
        data: data.to_vec(),
    }
}

const TRACE: &str = "# opendiag trace v1
0.000000 TX can 7E0#0210030000000000
0.001000 RX can 7E8#065003003201F4
0.100000 TX can 7E0#0322F19000000000
0.101000 RX can 7E8#0762F19001020304
0.200000 TX can 7E0#0319020000000000
0.210000 EV recv_timeout 1000
";

#[test]
fn answers_recorded_requests_by_content_in_any_order() {
    let mut ecu = TraceEcu::from_text(TRACE).unwrap();
    // Ask for the second request first.
    ecu.send(&can(0x7E0, &[0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0]))
        .unwrap();
    assert_eq!(
        ecu.recv(T).unwrap(),
        can(0x7E8, &[0x07, 0x62, 0xF1, 0x90, 1, 2, 3, 4])
    );
}

#[test]
fn can_be_asked_again_after_the_end_of_the_recording() {
    let mut ecu = TraceEcu::from_text(TRACE).unwrap();
    let req = can(0x7E0, &[0x02, 0x10, 0x03, 0, 0, 0, 0, 0]);
    for _ in 0..3 {
        ecu.send(&req).unwrap();
        assert_eq!(
            ecu.recv(T).unwrap(),
            can(0x7E8, &[0x06, 0x50, 0x03, 0x00, 0x32, 0x01, 0xF4])
        );
    }
    assert!(ecu.rewinds() >= 1);
}

#[test]
fn unrecorded_request_is_an_error_that_names_it() {
    let mut ecu = TraceEcu::from_text(TRACE).unwrap();
    let err = ecu
        .send(&can(0x7E0, &[0x04, 0x31, 0x01, 0xFF, 0x00, 0, 0, 0]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not in recording"), "{err}");
    assert!(err.contains("7E0#0431"), "{err}");
}

#[test]
fn recorded_silence_is_a_timeout_and_nothing_is_invented() {
    let mut ecu = TraceEcu::from_text(TRACE).unwrap();
    ecu.send(&can(0x7E0, &[0x03, 0x19, 0x02, 0, 0, 0, 0, 0]))
        .unwrap();
    assert!(matches!(ecu.recv(T), Err(opendiag_core::Error::Timeout)));
    // And nothing more is ever delivered.
    assert!(matches!(ecu.recv(T), Err(opendiag_core::Error::Timeout)));
}

#[test]
fn k_line_acks_are_matched_forward_first() {
    let text = "# opendiag trace v1
0.000000 EV slow_init 01
0.100000 RX raw 55 01 8A
0.200000 TX raw 75
0.300000 RX raw 03
0.400000 TX raw 75
0.500000 RX raw 09
";
    let mut ecu = TraceEcu::from_text(text).unwrap();
    ecu.slow_init(0x01).unwrap();
    assert_eq!(ecu.recv(T).unwrap(), Frame::Raw(vec![0x55, 0x01, 0x8A]));
    ecu.send(&Frame::Raw(vec![0x75])).unwrap();
    assert_eq!(ecu.recv(T).unwrap(), Frame::Raw(vec![0x03]));
    // The same ack again lands on the *next* recorded one, not the first.
    ecu.send(&Frame::Raw(vec![0x75])).unwrap();
    assert_eq!(ecu.recv(T).unwrap(), Frame::Raw(vec![0x09]));
}
