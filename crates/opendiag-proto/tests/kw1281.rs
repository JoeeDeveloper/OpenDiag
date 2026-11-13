//! KW1281 client against the byte-level virtual module (fake clock, no hardware).

use std::time::Duration;

use opendiag_core::{Error, WriteMode};
use opendiag_proto::clock::{Clock, FakeClock};
use opendiag_proto::kw1281::sim::Kw1281Sim;
use opendiag_proto::kw1281::{Fault, Kw1281Client, Kw1281Config, RawMeasurement};
use opendiag_transport::Simulator;

fn demo_sim() -> Kw1281Sim {
    Kw1281Sim::new()
        .ident_line("1J0906018AB")
        .ident_line("1.8L R4/5VT  ")
        .fault(525, 0x20)
        .fault(17, 0x01)
        .group(1, &[(0x01, 0x10, 0x20), (0x07, 0x00, 0x64)])
}

fn client(sim: Kw1281Sim) -> Kw1281Client<Kw1281Sim, FakeClock> {
    Kw1281Client::new(sim, FakeClock::new(), Kw1281Config::default())
}

#[test]
fn connect_runs_slow_init_and_reads_identification() {
    let mut c = client(demo_sim());
    let info = c.connect(0x01).unwrap();
    assert_eq!(info.key_bytes.kb1, 0x01);
    assert_eq!(info.key_bytes.kb2, 0x8A);
    assert_eq!(info.ident, vec!["1J0906018AB", "1.8L R4/5VT  "]);
    assert_eq!(c.transport().slow_inits(), &[0x01]);
    assert!(
        c.transport().violations().is_empty(),
        "{:?}",
        c.transport().violations()
    );
}

#[test]
fn works_on_cables_with_and_without_echo() {
    for echo in [true, false] {
        let cfg = Kw1281Config {
            echo,
            ..Kw1281Config::default()
        };
        let mut c = Kw1281Client::new(demo_sim().echo(echo), FakeClock::new(), cfg);
        c.connect(0x01).unwrap();
        assert_eq!(c.read_fault_codes().unwrap().len(), 2, "echo={echo}");
        assert!(c.transport().violations().is_empty(), "echo={echo}");
    }
}

#[test]
fn echo_setting_mismatch_is_reported_not_hung() {
    // Client expects an echo the cable never sends: the next module byte is
    // misread as the echo, which must surface as an error.
    let mut c = Kw1281Client::new(
        demo_sim().echo(false),
        FakeClock::new(),
        Kw1281Config::default(),
    );
    assert!(c.connect(0x01).is_err());
}

#[test]
fn non_kw1281_keyword_is_rejected_with_a_hint() {
    let mut c = client(demo_sim().keywords(0xE9, 0x8F));
    let err = c.connect(0x01).unwrap_err().to_string();
    assert!(
        err.contains("not KW1281") && err.contains("KWP2000"),
        "{err}"
    );
}

#[test]
fn silent_module_times_out_on_init() {
    let mut c = Kw1281Client::new(Simulator::new(), FakeClock::new(), Kw1281Config::default());
    assert!(matches!(c.connect(0x01), Err(Error::Timeout)));
}

#[test]
fn reads_fault_codes_and_formats_them() {
    let mut c = client(demo_sim());
    c.connect(0x01).unwrap();
    let faults = c.read_fault_codes().unwrap();
    assert_eq!(
        faults,
        vec![
            Fault {
                code: 525,
                status: 0x20
            },
            Fault {
                code: 17,
                status: 0x01
            }
        ]
    );
    assert_eq!(faults[0].text(), "00525");
    assert!(c.transport().violations().is_empty());
}

#[test]
fn no_faults_gives_an_empty_list() {
    let mut c = client(Kw1281Sim::new().ident_line("X"));
    c.connect(0x01).unwrap();
    assert!(c.read_fault_codes().unwrap().is_empty());
}

#[test]
fn clear_is_blocked_when_read_only_and_sends_nothing() {
    let mut c = client(demo_sim());
    c.connect(0x01).unwrap();
    let sent_before = c.transport().bytes_received();
    assert_eq!(c.write_mode(), WriteMode::ReadOnly);
    assert!(matches!(c.clear_fault_codes(), Err(Error::WritesLocked(_))));
    assert_eq!(c.transport().bytes_received(), sent_before);
    assert!(!c.transport().cleared());
}

#[test]
fn clear_works_when_unlocked() {
    let mut c = client(demo_sim());
    c.connect(0x01).unwrap();
    c.set_write_mode(WriteMode::Unlocked);
    assert!(c.clear_fault_codes().unwrap().is_empty());
    assert!(c.transport().cleared());
    assert!(c.read_fault_codes().unwrap().is_empty());
}

#[test]
fn reads_a_raw_measuring_group_and_nak_is_an_error() {
    let mut c = client(demo_sim());
    c.connect(0x01).unwrap();
    assert_eq!(
        c.read_group(1).unwrap(),
        vec![
            RawMeasurement {
                formula: 0x01,
                a: 0x10,
                b: 0x20
            },
            RawMeasurement {
                formula: 0x07,
                a: 0x00,
                b: 0x64
            }
        ]
    );
    let err = c.read_group(99).unwrap_err().to_string();
    assert!(err.contains("NAK"), "{err}");
}

#[test]
fn block_counters_stay_in_step_across_wraparound() {
    let mut c = client(demo_sim());
    c.connect(0x01).unwrap();
    // Two blocks per keep-alive; 300 rounds wraps the 8-bit counter several times.
    for _ in 0..300 {
        c.keep_alive().unwrap();
    }
    assert!(
        c.transport().violations().is_empty(),
        "{:?}",
        c.transport().violations()
    );
}

#[test]
fn end_closes_the_session() {
    let mut c = client(demo_sim());
    c.connect(0x01).unwrap();
    c.end().unwrap();
    assert!(c.transport().violations().is_empty());
}

#[test]
fn timing_pauses_use_the_injected_clock() {
    let mut c = client(demo_sim());
    c.connect(0x01).unwrap();
    // Each block we send waits block_gap (25 ms) first; connect sends several ACKs.
    assert!(c.clock().now() >= Duration::from_millis(25 * 2));
}
