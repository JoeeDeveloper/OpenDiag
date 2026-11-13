//! Sender <-> Receiver loopback under a fake clock, plus timeout and robustness tests.

use std::time::Duration;

use opendiag_core::Error;
use opendiag_proto::clock::{Clock, FakeClock};
use opendiag_proto::isotp::{Config, IsoTpFrame, Poll, Receiver, RxEvent, Sender};

const MS: Duration = Duration::from_millis(1);

/// Deterministic pseudo-random generator (no external deps).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// Run a full transfer; returns the received message and elapsed fake time.
fn transfer(payload: &[u8], tx: &Config, rx: &Config) -> (Vec<u8>, Duration) {
    let clock = FakeClock::new();
    let (mut s, first) = Sender::new(payload, tx, clock.now()).unwrap();
    let mut r = Receiver::new(rx.clone());
    let mut event = r.on_frame(&first, clock.now()).unwrap();
    loop {
        match event {
            RxEvent::Message(m) => return (m, clock.now()),
            RxEvent::SendFlowControl(fc) => {
                s.on_frame(&fc, clock.now()).unwrap();
                event = RxEvent::Pending;
            }
            RxEvent::Pending => match s.poll(clock.now()).unwrap() {
                Poll::Send(cf) => event = r.on_frame(&cf, clock.now()).unwrap(),
                Poll::WaitUntil(t) => clock.advance_to(t),
                Poll::Done => panic!("sender finished but receiver has no message"),
            },
        }
    }
}

#[test]
fn single_frame_needs_no_flow_control() {
    let (msg, t) = transfer(&[0x22, 0xF1, 0x90], &Config::default(), &Config::default());
    assert_eq!(msg, vec![0x22, 0xF1, 0x90]);
    assert_eq!(t, Duration::ZERO);
}

#[test]
fn multi_frame_roundtrip_across_lengths_block_sizes_and_stmin() {
    let mut rng = Lcg(42);
    for len in [8usize, 13, 14, 15, 118, 119, 120, 4095] {
        for bs in [0u8, 1, 3, 8] {
            for st in [0u8, 1, 0x7F, 0xF1, 0xF9] {
                let payload: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
                let rx = Config {
                    block_size: bs,
                    st_min: st,
                    padding: Some(0xAA),
                    ..Config::default()
                };
                let tx = Config {
                    padding: Some(0xAA),
                    ..Config::default()
                };
                let (msg, _) = transfer(&payload, &tx, &rx);
                assert_eq!(msg, payload, "len={len} bs={bs} st={st:#x}");
            }
        }
    }
}

#[test]
fn random_lengths_roundtrip() {
    let mut rng = Lcg(7);
    for _ in 0..300 {
        let len = 1 + (rng.next() as usize % 4095);
        let payload: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let rx = Config {
            block_size: (rng.next() % 10) as u8,
            st_min: [0, 1, 0xF5][(rng.next() % 3) as usize],
            ..Config::default()
        };
        assert_eq!(transfer(&payload, &Config::default(), &rx).0, payload);
    }
}

#[test]
fn st_min_spaces_consecutive_frames() {
    // 27 bytes = FF (6) + 3 CFs (7 each). First CF is immediate, then 2 gaps of 10 ms.
    let rx = Config {
        st_min: 10,
        ..Config::default()
    };
    let (_, elapsed) = transfer(&[0x55; 27], &Config::default(), &rx);
    assert_eq!(elapsed, 20 * MS);
}

#[test]
fn sender_holds_cf_until_st_min_elapses() {
    let tx = Config::default();
    let clock = FakeClock::new();
    let (mut s, _) = Sender::new(&[0; 30], &tx, clock.now()).unwrap();
    let fc = IsoTpFrame::FlowControl {
        status: opendiag_proto::isotp::FlowStatus::ContinueToSend,
        block_size: 0,
        st_min: 5,
    }
    .encode(None);
    s.on_frame(&fc, clock.now()).unwrap();
    assert!(matches!(s.poll(clock.now()).unwrap(), Poll::Send(_)));
    assert_eq!(s.poll(clock.now()).unwrap(), Poll::WaitUntil(5 * MS));
    clock.advance(4 * MS);
    assert_eq!(s.poll(clock.now()).unwrap(), Poll::WaitUntil(5 * MS));
    clock.advance(MS);
    assert!(matches!(s.poll(clock.now()).unwrap(), Poll::Send(_)));
}

#[test]
fn sender_times_out_without_flow_control() {
    let cfg = Config::default();
    let clock = FakeClock::new();
    let (mut s, _) = Sender::new(&[0; 20], &cfg, clock.now()).unwrap();
    assert_eq!(s.poll(clock.now()).unwrap(), Poll::WaitUntil(1000 * MS));
    clock.advance(1000 * MS);
    assert!(matches!(s.poll(clock.now()), Err(Error::Timeout)));
}

#[test]
fn receiver_times_out_when_consecutive_frames_stop() {
    let clock = FakeClock::new();
    let (_, first) = Sender::new(&[1; 20], &Config::default(), clock.now()).unwrap();
    let mut r = Receiver::new(Config::default());
    r.on_frame(&first, clock.now()).unwrap();
    clock.advance(999 * MS);
    r.poll(clock.now()).unwrap();
    clock.advance(MS);
    assert!(matches!(r.poll(clock.now()), Err(Error::Timeout)));
    r.poll(clock.now()).unwrap(); // back to idle, no repeated error
}

#[test]
fn receiver_rejects_out_of_sequence_frame() {
    let mut r = Receiver::new(Config::default());
    let (_, first) = Sender::new(&[1; 20], &Config::default(), Duration::ZERO).unwrap();
    r.on_frame(&first, Duration::ZERO).unwrap();
    let bad_cf = [0x23, 1, 2, 3, 4, 5, 6, 7]; // should be SN 1
    assert!(matches!(
        r.on_frame(&bad_cf, Duration::ZERO),
        Err(Error::Protocol(_))
    ));
}

#[test]
fn oversize_message_gets_overflow_flow_control() {
    let mut r = Receiver::new(Config {
        max_len: 50,
        ..Config::default()
    });
    let (_, first) = Sender::new(&[0; 100], &Config::default(), Duration::ZERO).unwrap();
    let RxEvent::SendFlowControl(fc) = r.on_frame(&first, Duration::ZERO).unwrap() else {
        panic!("expected flow control");
    };
    assert_eq!(fc[0], 0x32);

    // And the sender turns that into an error.
    let (mut s, _) = Sender::new(&[0; 100], &Config::default(), Duration::ZERO).unwrap();
    assert!(s.on_frame(&fc, Duration::ZERO).is_err());
}

#[test]
fn wait_frames_extend_the_deadline_until_the_limit() {
    let cfg = Config {
        max_wait_frames: 2,
        ..Config::default()
    };
    let wait = IsoTpFrame::FlowControl {
        status: opendiag_proto::isotp::FlowStatus::Wait,
        block_size: 0,
        st_min: 0,
    }
    .encode(None);
    let (mut s, _) = Sender::new(&[0; 20], &cfg, Duration::ZERO).unwrap();
    s.on_frame(&wait, 500 * MS).unwrap();
    assert_eq!(s.poll(600 * MS).unwrap(), Poll::WaitUntil(1500 * MS));
    s.on_frame(&wait, 600 * MS).unwrap();
    assert!(s.on_frame(&wait, 700 * MS).is_err());
}

#[test]
fn sender_rejects_empty_and_oversized_messages() {
    let cfg = Config::default();
    assert!(Sender::new(&[], &cfg, Duration::ZERO).is_err());
    assert!(Sender::new(&vec![0; 4096], &cfg, Duration::ZERO).is_err());
    assert!(Sender::new(&vec![0; 4095], &cfg, Duration::ZERO).is_ok());
}

#[test]
fn parsers_never_panic_on_arbitrary_bytes() {
    let mut rng = Lcg(1234);
    let mut r = Receiver::new(Config {
        block_size: 2,
        ..Config::default()
    });
    let (mut s, _) = Sender::new(&[0; 64], &Config::default(), Duration::ZERO).unwrap();
    for i in 0..50_000u64 {
        let n = (rng.next() % 13) as usize;
        let bytes: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
        let now = Duration::from_micros(i * 100);
        let _ = IsoTpFrame::decode(&bytes);
        let _ = r.on_frame(&bytes, now);
        let _ = r.poll(now);
        let _ = s.on_frame(&bytes, now);
        let _ = s.poll(now);
    }
}
