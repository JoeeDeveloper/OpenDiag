//! Regression test against a recorded session: protocol code must keep producing
//! exactly the bytes in the recording.
//!
//! The fixture is synthetic (recorded from the virtual KW1281 module), but the
//! mechanism is the point: a trace captured from a real vehicle drops in the same way.

use std::path::Path;

use opendiag_core::Error;
use opendiag_proto::clock::FakeClock;
use opendiag_proto::kw1281::{Kw1281Client, Kw1281Config, RawMeasurement};
use opendiag_transport::TraceReplay;

fn fixture() -> String {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/kw1281_sim_session.trace");
    std::fs::read_to_string(path).expect("fixture exists")
}

fn client(cfg: Kw1281Config) -> Kw1281Client<TraceReplay, FakeClock> {
    Kw1281Client::new(
        TraceReplay::from_text(&fixture()).unwrap(),
        FakeClock::new(),
        cfg,
    )
}

#[test]
fn the_client_reproduces_the_recorded_session_byte_for_byte() {
    let mut c = client(Kw1281Config::default());
    let info = c.connect(0x01).unwrap();
    assert_eq!(info.ident, vec!["DEMO-KW-0001", "VIRTUAL ENGINE ECU"]);

    // The recording holds two rounds of groups 1 and 2.
    for round in 0..2u8 {
        let g1 = c.read_group(1).unwrap();
        assert_eq!(
            g1,
            vec![
                RawMeasurement {
                    formula: 1,
                    a: 0x10,
                    b: 0x20 + 8 * round
                },
                RawMeasurement {
                    formula: 7,
                    a: 0,
                    b: 100 + round
                },
            ]
        );
        let g2 = c.read_group(2).unwrap();
        assert_eq!(
            g2,
            vec![RawMeasurement {
                formula: 5,
                a: 0,
                b: 0x80
            }]
        );
    }
    c.end().unwrap();

    assert!(
        c.transport().is_finished(),
        "{} recorded events were never exercised",
        c.transport().remaining()
    );
}

#[test]
fn a_client_that_behaves_differently_fails_against_the_recording() {
    // Same session, but the client now expects no cable echo. The recording has
    // echoes, so the bytes no longer line up.
    let mut c = client(Kw1281Config {
        echo: false,
        ..Kw1281Config::default()
    });
    assert!(c.connect(0x01).is_err());
}

#[test]
fn asking_for_something_that_was_not_recorded_is_a_clear_error() {
    let mut c = client(Kw1281Config::default());
    c.connect(0x01).unwrap();
    // The recording's next request is group 1; asking for group 2 first sends other bytes.
    let err = c.read_group(2).unwrap_err();
    assert!(
        matches!(&err, Error::Transport(m) if m.contains("trace mismatch")),
        "{err}"
    );
}
