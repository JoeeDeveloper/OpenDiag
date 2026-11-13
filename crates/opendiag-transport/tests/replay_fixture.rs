use std::path::PathBuf;
use std::time::Duration;

use opendiag_core::Error;
use opendiag_transport::{Frame, Replay, Transport};

#[test]
fn sample_log_reproduces_expected_frames() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample.candump");
    let text = std::fs::read_to_string(path).unwrap();
    let mut r = Replay::from_candump(&text).unwrap();
    let t = Duration::from_millis(10);

    assert_eq!(
        r.recv(t).unwrap(),
        Frame::can(0x7E0, &[0x02, 0x10, 0x03, 0, 0, 0, 0, 0])
    );
    assert_eq!(
        r.recv(t).unwrap(),
        Frame::can(0x7E8, &[0x06, 0x50, 0x03, 0x00, 0x32, 0x01, 0xF4])
    );
    assert!(matches!(r.recv(t), Err(Error::Timeout)));
}
