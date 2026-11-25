//! Record a session against the mock gateway, then replay it with no network at all.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use opendiag_bmw::mock::{MockGateway, VirtualVehicle};
use opendiag_bmw::{
    read_ident, ByteStream, HsfzGateway, RecordingStream, ReplayStream, TcpByteStream,
};
use opendiag_core::Error;
use opendiag_proto::clock::SystemClock;
use opendiag_proto::uds::{UdsClient, UdsConfig};

/// A `Write` the test can read back after the recorder owns it.
#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn record_ident() -> String {
    let mock = MockGateway::start(VirtualVehicle::demo()).unwrap();
    let tcp = TcpByteStream::connect(mock.tcp_addr()).unwrap();
    let out = Shared::default();
    let rec = RecordingStream::new(tcp, out.clone()).unwrap();
    let gw = HsfzGateway::new(rec, SystemClock::new(), 0xF4);
    let mut uds = UdsClient::new(gw.link(0x10), UdsConfig::default());
    read_ident(&mut uds).unwrap();
    let text = String::from_utf8(out.0.borrow().clone()).unwrap();
    text
}

fn replay_gateway(text: &str) -> HsfzGateway<Box<dyn ByteStream>, SystemClock> {
    let s: Box<dyn ByteStream> = Box::new(ReplayStream::from_text(text).unwrap());
    HsfzGateway::new(s, SystemClock::new(), 0xF4)
}

#[test]
fn a_recorded_session_replays_without_a_network() {
    let text = record_ident();
    assert!(text.starts_with("# opendiag trace v1"));
    assert!(
        text.contains(" TX raw ") && text.contains(" RX raw "),
        "{text}"
    );
    let gw = replay_gateway(&text);
    let mut uds = UdsClient::new(gw.link(0x10), UdsConfig::default());
    let id = read_ident(&mut uds).unwrap();
    assert_eq!(id.vin.as_deref(), Some("WBASIMULATED00001"));
    assert_eq!(id.i_level_text().as_deref(), Some("F020-00-00-000"));
    // Asking again is fine: the recording is searched again from the start.
    let again = read_ident(&mut uds).unwrap();
    assert_eq!(again.vin, id.vin);
}

#[test]
fn a_request_that_was_never_recorded_is_named_not_invented() {
    let text = record_ident();
    let gw = replay_gateway(&text);
    let mut uds = UdsClient::new(gw.link(0x10), UdsConfig::default());
    let err = uds.read_data_by_identifier(0x1234).unwrap_err();
    assert!(
        matches!(&err, Error::Transport(m) if m.contains("not in recording")),
        "{err:?}"
    );
}

#[test]
fn an_empty_recording_answers_nothing() {
    let gw = replay_gateway("# opendiag trace v1\n");
    let mut uds = UdsClient::new(gw.link(0x10), UdsConfig::default());
    let err = read_ident(&mut uds).unwrap_err();
    assert!(
        matches!(&err, Error::Transport(m) if m.contains("not in recording")),
        "{err:?}"
    );
}
