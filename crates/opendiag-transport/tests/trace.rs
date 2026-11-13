use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::Duration;

use opendiag_core::Error;
use opendiag_transport::{
    format_frame, parse_trace, Frame, Simulator, TraceEvent, TraceReplay, Tracing, Transport,
    TRACE_HEADER,
};

const MS: Duration = Duration::from_millis(1);

/// A writer whose contents the test can read after the transport is gone.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn text(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).unwrap()
    }
}

/// Timestamps that advance 1 ms per event.
fn ticking() -> impl FnMut() -> Duration {
    let mut n = 0u64;
    move || {
        n += 1;
        Duration::from_millis(n)
    }
}

#[test]
fn frames_format_like_candump_and_hex_bytes() {
    assert_eq!(
        format_frame(&Frame::Raw(vec![0x81, 0x01, 0xF1])),
        "raw 81 01 F1"
    );
    assert_eq!(format_frame(&Frame::Raw(vec![])), "raw");
    assert_eq!(
        format_frame(&Frame::can(0x7E0, &[2, 0x10, 3])),
        "can 7E0#021003"
    );
    let ext = Frame::Can {
        id: 0x18DA_F110,
        extended: true,
        data: vec![1, 2],
    };
    assert_eq!(format_frame(&ext), "can 18DAF110#0102");
}

#[test]
fn every_event_kind_roundtrips_through_text() {
    let events = vec![
        TraceEvent::Tx(Frame::Raw(vec![0x81])),
        TraceEvent::Rx(Frame::Raw(vec![])),
        TraceEvent::Tx(Frame::can(0x7E0, &[])),
        TraceEvent::Rx(Frame::Can {
            id: 0x18DA_F110,
            extended: true,
            data: vec![9, 8],
        }),
        TraceEvent::SlowInit(0x17),
        TraceEvent::FastInit,
        TraceEvent::SetBaud(10400),
        TraceEvent::RecvTimeout(1000 * MS),
        TraceEvent::Error("send: boom".into()),
    ];
    let text: String = events
        .iter()
        .enumerate()
        .map(|(i, e)| format!("{i}.{:06} {e}\n", i * 1000))
        .collect();
    let parsed = parse_trace(&text).unwrap();
    assert_eq!(
        parsed.iter().map(|e| e.event.clone()).collect::<Vec<_>>(),
        events
    );
    assert_eq!(parsed[3].time, Duration::new(3, 3_000_000));
    assert_eq!(parsed[4].line, 5, "line numbers are 1-based file lines");
}

#[test]
fn parser_skips_comments_and_reports_the_bad_line() {
    let ok = format!("{TRACE_HEADER}\n\n# note\n0.000001 TX raw 01\n");
    assert_eq!(parse_trace(&ok).unwrap().len(), 1);
    for (text, needle) in [
        ("0.000001 TX raw ZZ\n", "not a hex byte"),
        ("0.000001 TX raw 123\n", "not a hex byte"),
        ("0.000001 TX can 7E0\n", "ID#DATA"),
        ("0.000001 TX can 7E0#0\n", "bad CAN data"),
        ("0.000001 TX pigeon 01\n", "unknown frame kind"),
        ("0.000001 EV nonsense\n", "unrecognised"),
        ("0.1 TX raw 01\n", "6 decimals"),
        ("x TX raw 01\n", "bad timestamp"),
        ("0.000001\n", "expected"),
    ] {
        let err = parse_trace(&format!("# fine\n{text}"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("line 2") && err.contains(needle),
            "{text:?}: {err}"
        );
    }
}

#[test]
fn tracing_records_everything_in_order_and_passes_data_through() {
    let sim = Simulator::new().on(Frame::Raw(vec![0x81]), vec![Frame::Raw(vec![0xC1, 0xEF])]);
    let buf = Shared::default();
    let mut t = Tracing::new(sim, buf.clone()).with_time_source(ticking());
    t.set_baud(10400).unwrap();
    t.slow_init(0x01).unwrap();
    t.send(&Frame::Raw(vec![0x81])).unwrap();
    assert_eq!(t.recv(50 * MS).unwrap(), Frame::Raw(vec![0xC1, 0xEF]));
    assert!(matches!(t.recv(50 * MS), Err(Error::Timeout)));
    assert!(t.capabilities().kline, "capabilities pass through");

    let text = buf.text();
    assert!(text.starts_with(TRACE_HEADER));
    let events = parse_trace(&text).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| e.event.to_string())
            .collect::<Vec<_>>(),
        vec![
            "EV baud 10400",
            "EV slow_init 01",
            "TX raw 81",
            "RX raw C1 EF",
            "EV recv_timeout 50",
        ]
    );
    assert_eq!(events[0].time, MS);
    assert_eq!(events[4].time, 5 * MS);
    assert_eq!(t.status().events(), 5);
}

#[test]
fn failures_are_recorded_and_still_returned() {
    struct Failing;
    impl Transport for Failing {
        fn send(&mut self, _: &Frame) -> opendiag_core::Result<()> {
            Err(Error::Transport("unplugged\nreally".into()))
        }
        fn recv(&mut self, _: Duration) -> opendiag_core::Result<Frame> {
            Err(Error::Transport("gone".into()))
        }
        fn set_baud(&mut self, _: u32) -> opendiag_core::Result<()> {
            Ok(())
        }
        fn slow_init(&mut self, _: u8) -> opendiag_core::Result<()> {
            Ok(())
        }
        fn capabilities(&self) -> opendiag_transport::Capabilities {
            Default::default()
        }
    }
    let buf = Shared::default();
    let mut t = Tracing::new(Failing, buf.clone()).with_time_source(ticking());
    assert!(t.send(&Frame::Raw(vec![1])).is_err());
    assert!(t.recv(MS).is_err());
    assert!(t.fast_init().is_err(), "default fast_init is unsupported");
    let events: Vec<String> = parse_trace(&buf.text())
        .unwrap()
        .iter()
        .map(|e| e.event.to_string())
        .collect();
    assert_eq!(events[0], "TX raw 01");
    assert!(
        events[1].starts_with("EV error send:") && !events[1].contains('\n'),
        "{events:?}"
    );
    assert!(events[2].starts_with("EV error recv:"));
    assert_eq!(events[3], "EV fast_init");
}

#[test]
fn passive_capture_can_skip_timeout_lines() {
    let buf = Shared::default();
    let mut t = Tracing::new(Simulator::new(), buf.clone()).log_timeouts(false);
    assert!(matches!(t.recv(MS), Err(Error::Timeout)));
    assert!(parse_trace(&buf.text()).unwrap().is_empty());
}

#[test]
fn a_failing_trace_file_never_disturbs_the_session() {
    struct Full;
    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let sim = Simulator::new().on(Frame::Raw(vec![1]), vec![Frame::Raw(vec![2])]);
    let mut t = Tracing::new(sim, Full);
    t.send(&Frame::Raw(vec![1])).unwrap();
    assert_eq!(
        t.recv(MS).unwrap(),
        Frame::Raw(vec![2]),
        "session still works"
    );
    let status = t.status();
    assert_eq!(status.events(), 0);
    assert!(status.error().unwrap().contains("disk full"));
}

const SESSION: &str = "\
# opendiag trace v1
0.000001 EV slow_init 01
0.200000 RX raw 55 01 8A
0.200500 TX raw 75
0.201000 EV recv_timeout 1000
0.900000 RX raw 03
";

#[test]
fn replay_hands_back_receives_and_checks_everything_sent() {
    let mut r = TraceReplay::from_text(SESSION).unwrap();
    assert_eq!(r.remaining(), 5);
    r.slow_init(0x01).unwrap();
    assert_eq!(r.recv(MS).unwrap(), Frame::Raw(vec![0x55, 1, 0x8A]));
    r.send(&Frame::Raw(vec![0x75])).unwrap();
    assert!(
        matches!(r.recv(MS), Err(Error::Timeout)),
        "recorded timeout replays"
    );
    assert_eq!(r.recv(MS).unwrap(), Frame::Raw(vec![3]));
    assert!(r.is_finished());
    assert!(
        matches!(r.recv(MS), Err(Error::Timeout)),
        "past the end is a timeout"
    );
}

#[test]
fn replay_flags_any_deviation_with_the_recorded_line() {
    let mut r = TraceReplay::from_text(SESSION).unwrap();
    // Wrong address for the init.
    let err = r.slow_init(0x02).unwrap_err().to_string();
    assert!(
        err.contains("line 2") && err.contains("slow_init 01") && err.contains("slow_init 02"),
        "{err}"
    );

    let mut r = TraceReplay::from_text(SESSION).unwrap();
    r.slow_init(1).unwrap();
    r.recv(MS).unwrap();
    // Wrong byte sent.
    let err = r.send(&Frame::Raw(vec![0x76])).unwrap_err().to_string();
    assert!(
        err.contains("line 4") && err.contains("TX raw 75") && err.contains("TX raw 76"),
        "{err}"
    );

    // Receiving when the recording says the client should have sent.
    let mut r = TraceReplay::from_text(SESSION).unwrap();
    r.slow_init(1).unwrap();
    r.recv(MS).unwrap();
    let err = r.recv(MS).unwrap_err().to_string();
    assert!(
        err.contains("is receiving") && err.contains("TX raw 75"),
        "{err}"
    );

    // Doing more than was recorded.
    let mut r = TraceReplay::from_text("0.000001 TX raw 01\n").unwrap();
    r.send(&Frame::Raw(vec![1])).unwrap();
    assert!(r
        .send(&Frame::Raw(vec![2]))
        .unwrap_err()
        .to_string()
        .contains("trace ended"));
    assert!(r.set_baud(9600).is_err());
    assert!(r.fast_init().is_err());
}

#[test]
fn replay_reproduces_a_recorded_failure_as_the_next_outcome() {
    let mut r =
        TraceReplay::from_text("0.000001 EV error send: unplugged\n0.000002 TX raw 01\n").unwrap();
    let err = r.send(&Frame::Raw(vec![1])).unwrap_err();
    assert!(matches!(err, Error::Transport(m) if m.contains("unplugged")));
    r.send(&Frame::Raw(vec![1])).unwrap();
}

#[test]
fn a_trace_written_by_tracing_replays_through_traceplay() {
    // Record a session...
    let sim = Simulator::new().on(
        Frame::can(0x7E0, &[2, 0x3E, 0]),
        vec![Frame::can(0x7E8, &[2, 0x7E, 0])],
    );
    let buf = Shared::default();
    let mut rec = Tracing::new(sim, buf.clone());
    rec.send(&Frame::can(0x7E0, &[2, 0x3E, 0])).unwrap();
    rec.recv(MS).unwrap();
    let _ = rec.recv(MS);
    // ...and replay it with no simulator at all.
    let mut r = TraceReplay::from_text(&buf.text()).unwrap();
    r.send(&Frame::can(0x7E0, &[2, 0x3E, 0])).unwrap();
    assert_eq!(r.recv(MS).unwrap(), Frame::can(0x7E8, &[2, 0x7E, 0]));
    assert!(matches!(r.recv(MS), Err(Error::Timeout)));
    assert!(r.is_finished());
}

#[test]
fn boxed_transports_delegate_everything() {
    let sim = Simulator::new().on(Frame::Raw(vec![1]), vec![Frame::Raw(vec![2])]);
    let mut b: Box<dyn Transport> = Box::new(sim);
    b.send(&Frame::Raw(vec![1])).unwrap();
    assert_eq!(b.recv(MS).unwrap(), Frame::Raw(vec![2]));
    b.fast_init().unwrap(); // the Simulator supports it; the default would error
    assert!(b.capabilities().fast_init);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn can_interfaces_report_a_clear_error_off_linux() {
    let err = opendiag_transport::open_can("can0")
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("only available on Linux"), "{err}");
}
