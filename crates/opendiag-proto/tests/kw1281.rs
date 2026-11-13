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

fn adaptation_client(sim: Kw1281Sim) -> Kw1281Client<Kw1281Sim, FakeClock> {
    let mut c = client(sim);
    c.connect(0x01).unwrap();
    c
}

#[test]
fn adaptation_read_is_read_only_and_returns_the_value() {
    let mut c = adaptation_client(demo_sim().adaptation(4, 0x1234));
    assert_eq!(c.write_mode(), WriteMode::ReadOnly);
    assert_eq!(c.adaptation_read(4).unwrap(), 0x1234);
    let err = c.adaptation_read(5).unwrap_err().to_string();
    assert!(err.contains("NAK"), "unknown channel is refused: {err}");
    assert!(c.transport().violations().is_empty());
}

#[test]
fn adaptation_test_and_save_are_gated_and_send_nothing_when_locked() {
    let mut c = adaptation_client(demo_sim().adaptation(4, 100));
    let before = c.transport().bytes_received();
    assert!(matches!(
        c.adaptation_test(4, 200),
        Err(Error::WritesLocked(_))
    ));
    assert!(matches!(
        c.adaptation_save(4, 200, 0),
        Err(Error::WritesLocked(_))
    ));
    assert_eq!(
        c.transport().bytes_received(),
        before,
        "nothing hit the bus"
    );
    assert_eq!(c.transport().adaptation_value(4), Some(100));
}

#[test]
fn adaptation_test_does_not_store_but_save_does() {
    let mut c = adaptation_client(demo_sim().adaptation(4, 100));
    c.set_write_mode(WriteMode::Unlocked);
    assert_eq!(c.adaptation_test(4, 200).unwrap(), 200);
    assert_eq!(
        c.transport().adaptation_value(4),
        Some(100),
        "test stores nothing"
    );
    c.adaptation_save(4, 200, 0x01_23_45).unwrap();
    assert_eq!(c.transport().adaptation_value(4), Some(200));
    assert_eq!(c.transport().adaptation_saves(), &[(4, 200, 0x01_23_45)]);
    assert_eq!(c.adaptation_read(4).unwrap(), 200);
    assert!(c.transport().violations().is_empty());
}

#[test]
fn adaptation_save_rejects_oversized_workshop_codes_and_refusals() {
    let mut c = adaptation_client(demo_sim().adaptation(4, 100));
    c.set_write_mode(WriteMode::Unlocked);
    let err = c
        .adaptation_save(4, 1, 0x0100_0000)
        .unwrap_err()
        .to_string();
    assert!(err.contains("24 bits"), "{err}");
    assert!(
        c.transport().adaptation_saves().is_empty(),
        "nothing was sent"
    );

    let mut c = adaptation_client(demo_sim().adaptation(4, 100).refuse_adaptation_save());
    c.set_write_mode(WriteMode::Unlocked);
    assert!(c
        .adaptation_save(4, 1, 0)
        .unwrap_err()
        .to_string()
        .contains("NAK"));
}

/// Delivers the module's bytes with chosen faults, to test what the client does about them.
struct Faulty {
    inner: Kw1281Sim,
    /// Replace the Nth received byte (counting from the first byte after init) with this value.
    corrupt_at: Option<(usize, u8)>,
    received: usize,
}

impl opendiag_transport::Transport for Faulty {
    fn send(&mut self, f: &opendiag_transport::Frame) -> opendiag_core::Result<()> {
        self.inner.send(f)
    }
    fn recv(&mut self, t: Duration) -> opendiag_core::Result<opendiag_transport::Frame> {
        let f = self.inner.recv(t)?;
        let opendiag_transport::Frame::Raw(mut bytes) = f else {
            return Ok(f);
        };
        for b in &mut bytes {
            if let Some((n, v)) = self.corrupt_at {
                if self.received == n {
                    *b = v;
                }
            }
            self.received += 1;
        }
        Ok(opendiag_transport::Frame::Raw(bytes))
    }
    fn set_baud(&mut self, b: u32) -> opendiag_core::Result<()> {
        self.inner.set_baud(b)
    }
    fn slow_init(&mut self, a: u8) -> opendiag_core::Result<()> {
        self.inner.slow_init(a)
    }
    fn capabilities(&self) -> opendiag_transport::Capabilities {
        self.inner.capabilities()
    }
}

/// What the client reads after `slow_init`: sync, kb1, kb2 (3 bytes), the echo of its own
/// keyword complement (1 byte), then the first block, which starts with its length byte.
const FIRST_BLOCK_LEN_AT: usize = 4;

#[test]
fn an_invalid_block_length_is_not_acknowledged() {
    // Corrupt the first block's length to 1, which can never be valid.
    let t = Faulty {
        inner: demo_sim(),
        corrupt_at: Some((FIRST_BLOCK_LEN_AT, 0x01)),
        received: 0,
    };
    let mut c = Kw1281Client::new(t, FakeClock::new(), Kw1281Config::default());
    let sent_before = c.transport().inner.bytes_received();
    let err = c.connect(0x01).unwrap_err().to_string();
    assert!(
        err.contains("block length") && err.contains("not acknowledging"),
        "{err}"
    );
    // The only byte the client sent was the keyword complement: no ack for the bad length.
    assert_eq!(c.transport().inner.bytes_received() - sent_before, 1);
    assert!(c.is_out_of_sync());
}

#[test]
fn an_out_of_sequence_block_counter_is_not_acknowledged() {
    // The first block's counter is unpredictable, but each later one must follow the
    // previous. Where the second block's counter sits in the byte stream:
    //   first block   = len 14 (counter, title, 11 text bytes, end): 14 acknowledged bytes,
    //                   each followed by the echo of our complement, plus the end byte
    //                   = 2*14 + 1 = 29 bytes, starting at FIRST_BLOCK_LEN_AT
    //   our ack block = 3 acknowledged bytes (echo + module's complement) + end echo = 7 bytes
    //   second block  = len (+echo), then its counter
    let second_counter_at = FIRST_BLOCK_LEN_AT + 29 + 7 + 2;
    let t = Faulty {
        inner: demo_sim(),
        corrupt_at: Some((second_counter_at, 0x7F)),
        received: 0,
    };
    let mut c = Kw1281Client::new(t, FakeClock::new(), Kw1281Config::default());
    let err = c.connect(0x01).unwrap_err().to_string();
    assert!(
        err.contains("block counter 0x7F") && err.contains("not acknowledging"),
        "{err}"
    );
    assert!(c.is_out_of_sync());
    // The module never saw an acknowledgement of the bad counter, so it saw no violation.
    assert!(
        c.transport().inner.violations().is_empty(),
        "{:?}",
        c.transport().inner.violations()
    );
}

#[test]
fn after_an_error_nothing_more_is_sent_into_the_session() {
    let t = Faulty {
        inner: demo_sim(),
        corrupt_at: Some((FIRST_BLOCK_LEN_AT, 0x01)),
        received: 0,
    };
    let mut c = Kw1281Client::new(t, FakeClock::new(), Kw1281Config::default());
    assert!(c.connect(0x01).is_err());
    assert!(c.is_out_of_sync());

    let before = c.transport().inner.bytes_received();
    c.set_write_mode(WriteMode::Unlocked);
    for result in [
        c.read_fault_codes().map(|_| ()),
        c.clear_fault_codes().map(|_| ()),
        c.adaptation_save(4, 1, 0),
        c.keep_alive(),
    ] {
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("out of sync") || err.contains("reconnect"),
            "{err}"
        );
    }
    c.end().unwrap(); // does not send either
    assert_eq!(
        c.transport().inner.bytes_received(),
        before,
        "not a single byte was sent"
    );
}

#[test]
fn reconnecting_clears_the_out_of_sync_state() {
    let t = Faulty {
        inner: demo_sim(),
        corrupt_at: Some((FIRST_BLOCK_LEN_AT, 0x01)),
        received: 0,
    };
    let mut c = Kw1281Client::new(t, FakeClock::new(), Kw1281Config::default());
    assert!(c.connect(0x01).is_err());
    // The fault was one-off; a fresh init works again.
    c.connect(0x01).unwrap();
    assert!(!c.is_out_of_sync());
    assert_eq!(c.read_fault_codes().unwrap().len(), 2);
}

#[test]
fn a_module_nak_does_not_desync_the_session() {
    let mut c = adaptation_client(demo_sim().adaptation(4, 1));
    assert!(c
        .adaptation_read(99)
        .unwrap_err()
        .to_string()
        .contains("NAK"));
    assert!(!c.is_out_of_sync(), "a NAK is a legitimate, in-sync reply");
    assert_eq!(c.adaptation_read(4).unwrap(), 1);
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
