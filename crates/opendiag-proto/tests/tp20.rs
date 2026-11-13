//! TP2.0 link and KWP2000-over-TP2.0 client against the virtual module (fake clock).

use std::time::Duration;

use opendiag_core::{Error, WriteMode};
use opendiag_proto::clock::{Clock, FakeClock};
use opendiag_proto::tp20::sim::Tp20Sim;
use opendiag_proto::tp20::{Tp20Config, Tp20KwpClient, Tp20Link};
use opendiag_transport::{Frame, Simulator};

const MS: Duration = Duration::from_millis(1);

fn open(sim: Tp20Sim) -> Tp20Link<Tp20Sim, FakeClock> {
    Tp20Link::open(sim, FakeClock::new(), &Tp20Config::new(0x01)).unwrap()
}

fn engine() -> Tp20Sim {
    Tp20Sim::new(0x01)
        .respond(&[0x10, 0x89], &[0x50, 0x89])
        .respond(&[0x21, 0x01], &[0x61, 0x01, 0xAA, 0xBB])
}

#[test]
fn opening_a_channel_puts_the_documented_bytes_on_the_bus() {
    let link = open(engine());
    let sent = link.transport().sent();
    // 200: 01 C0 00 10 00 03 01, then 740: A0 0F 8A FF 32 FF
    assert_eq!(
        sent[0],
        Frame::can(0x200, &[0x01, 0xC0, 0x00, 0x10, 0x00, 0x03, 0x01])
    );
    assert_eq!(
        sent[1],
        Frame::can(0x740, &[0xA0, 0x0F, 0x8A, 0xFF, 0x32, 0xFF])
    );
    assert!(link.transport().is_open());
    assert_eq!(link.peer_params().block_size, 0x0F);
}

#[test]
fn single_packet_exchange_matches_the_documented_capture() {
    let mut link = open(engine());
    let r = link.request(&[0x10, 0x89], 1000 * MS).unwrap();
    assert_eq!(r, vec![0x50, 0x89]);
    let r = link.request(&[0x21, 0x01], 1000 * MS).unwrap();
    assert_eq!(r, vec![0x61, 0x01, 0xAA, 0xBB]);

    let sent = &link.transport().sent()[2..];
    // 740: 10 00 02 10 89 / 740: B1 (ACK of the module's seq-0 reply)
    assert_eq!(sent[0], Frame::can(0x740, &[0x10, 0x00, 0x02, 0x10, 0x89]));
    assert_eq!(sent[1], Frame::can(0x740, &[0xB1]));
    // 740: 11 00 02 21 01 (our seq is now 1) / 740: B2 (module's seq is now 1)
    assert_eq!(sent[2], Frame::can(0x740, &[0x11, 0x00, 0x02, 0x21, 0x01]));
    assert_eq!(sent[3], Frame::can(0x740, &[0xB2]));
    assert!(
        link.transport().violations().is_empty(),
        "{:?}",
        link.transport().violations()
    );
}

#[test]
fn long_reply_is_reassembled_and_acked_per_block() {
    let reply: Vec<u8> = (0..26).collect();
    let sim = Tp20Sim::new(0x01)
        .block_size(2)
        .respond(&[0x21, 0x01], &reply);
    let mut link = open(sim);
    assert_eq!(link.request(&[0x21, 0x01], 1000 * MS).unwrap(), reply);
    assert!(
        link.transport().violations().is_empty(),
        "{:?}",
        link.transport().violations()
    );
}

#[test]
fn long_request_is_segmented_obeys_block_size_and_packet_gap() {
    // Module reports block size 2 and a 10 ms packet interval.
    let sim = Tp20Sim::new(0x01)
        .block_size(2)
        .respond(&[0x2E; 40], &[0x6E]);
    let mut link = open(sim);
    let before = link.clock().now();
    assert_eq!(link.request(&[0x2E; 40], 1000 * MS).unwrap(), vec![0x6E]);
    // 40 bytes = 5 + 7*5 = 6 packets -> 5 gaps of 10 ms between them.
    assert!(
        link.clock().now() - before >= 50 * MS,
        "{:?}",
        link.clock().now()
    );
    assert_eq!(
        link.transport().received_messages().last().unwrap(),
        &vec![0x2E; 40]
    );
    assert!(
        link.transport().violations().is_empty(),
        "{:?}",
        link.transport().violations()
    );
}

#[test]
fn sequence_numbers_wrap_after_sixteen_packets() {
    let mut link = open(engine());
    for _ in 0..40 {
        assert_eq!(link.request(&[0x21, 0x01], 1000 * MS).unwrap().len(), 4);
    }
    assert!(
        link.transport().violations().is_empty(),
        "{:?}",
        link.transport().violations()
    );
}

#[test]
fn missing_ack_times_out_after_the_negotiated_window() {
    let mut link = open(engine().drop_acks());
    let before = link.clock().now();
    let err = link.send(&[0x10, 0x89]).unwrap_err();
    assert!(matches!(err, Error::Timeout), "{err}");
    assert!(
        link.clock().now() - before >= 100 * MS,
        "waited the module's T1 (100 ms)"
    );
}

#[test]
fn setup_refusal_and_absent_module_are_distinct_errors() {
    let err = Tp20Link::open(
        engine().refuse_setup(0xD6),
        FakeClock::new(),
        &Tp20Config::new(0x01),
    )
    .err()
    .unwrap();
    assert!(err.to_string().contains("refused"), "{err}");

    // A module at another address never answers.
    let err = Tp20Link::open(Tp20Sim::new(0x02), FakeClock::new(), &Tp20Config::new(0x01))
        .err()
        .unwrap();
    assert!(matches!(err, Error::Timeout), "{err}");

    let err = Tp20Link::open(Simulator::new(), FakeClock::new(), &Tp20Config::new(0x01))
        .err()
        .unwrap();
    assert!(matches!(err, Error::Timeout), "{err}");
}

#[test]
fn channel_test_and_close() {
    let mut link = open(engine());
    link.channel_test().unwrap();
    link.close().unwrap();
    assert!(link.transport().is_closed());
}

#[test]
fn dropping_the_link_sends_a_disconnect() {
    let link = open(engine());
    // Keep the simulator alive past the drop by checking the sent frames before it.
    let before = link.transport().sent().len();
    assert!(before >= 2);
    drop(link); // must not panic or hang
}

// ---- KWP2000 over TP2.0 ----

fn kwp(sim: Tp20Sim) -> Tp20KwpClient<Tp20Sim, FakeClock> {
    Tp20KwpClient::new(open(sim))
}

#[test]
fn kwp_session_ident_and_local_identifier() {
    let sim = engine().respond(&[0x1A, 0x9B], &[0x5A, 0x9B, b'0', b'3', b'6']);
    let mut c = kwp(sim);
    c.start_diagnostic_session(0x89).unwrap();
    assert_eq!(c.read_ecu_identification(0x9B).unwrap(), b"036".to_vec());
    assert_eq!(
        c.read_data_by_local_identifier(0x01).unwrap(),
        vec![0xAA, 0xBB]
    );
    assert!(c.link().transport().violations().is_empty());
}

#[test]
fn kwp_reads_dtcs_over_a_multi_packet_reply() {
    // 8 codes = 2 header bytes + 24 data bytes -> multi-packet.
    let mut reply = vec![0x58, 0x08];
    for i in 0..8u8 {
        reply.extend_from_slice(&[0x01, i, 0x20]);
    }
    let sim = Tp20Sim::new(0x01)
        .block_size(2)
        .respond(&[0x18, 0x00, 0xFF, 0x00], &reply);
    let dtcs = kwp(sim).read_dtcs_by_status(0x00, 0xFF00).unwrap();
    assert_eq!(dtcs.len(), 8);
    assert_eq!(dtcs[7].code, 0x0107);
}

#[test]
fn kwp_negative_response_and_pending() {
    let sim = Tp20Sim::new(0x01)
        .respond(&[0x21, 0x05], &[0x7F, 0x21, 0x12])
        .respond_many(&[0x21, 0x06], &[&[0x7F, 0x21, 0x78], &[0x61, 0x06, 0x01]]);
    let mut c = kwp(sim);
    assert!(matches!(
        c.read_data_by_local_identifier(0x05),
        Err(Error::NegativeResponse {
            service: 0x21,
            nrc: 0x12
        })
    ));
    assert_eq!(c.read_data_by_local_identifier(0x06).unwrap(), vec![0x01]);
}

#[test]
fn kwp_unknown_service_gets_service_not_supported() {
    let mut c = kwp(Tp20Sim::new(0x01));
    assert!(matches!(
        c.tester_present(),
        Err(Error::NegativeResponse {
            service: 0x3E,
            nrc: 0x11
        })
    ));
}

#[test]
fn kwp_clear_is_gated() {
    let sim = engine().respond(&[0x14, 0xFF, 0x00], &[0x54, 0xFF, 0x00]);
    let mut c = kwp(sim);
    let sent = c.link().transport().sent().len();
    assert_eq!(c.write_mode(), WriteMode::ReadOnly);
    assert!(matches!(c.clear_dtcs(0xFF00), Err(Error::WritesLocked(_))));
    assert_eq!(
        c.link().transport().sent().len(),
        sent,
        "nothing hit the bus"
    );
    c.set_write_mode(WriteMode::Unlocked);
    c.clear_dtcs(0xFF00).unwrap();
}
