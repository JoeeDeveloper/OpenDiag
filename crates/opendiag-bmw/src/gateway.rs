//! A TCP session to a BMW gateway, and per-ECU message links on top of it.
//!
//! One TCP connection serves every ECU behind the gateway: requests carry the
//! target address. [`HsfzGateway`] owns the connection; [`HsfzLink`] is a view
//! of it addressed to one ECU, and implements [`MessageLink`] so the shared
//! UDS client runs over it unchanged (there is no ISO-TP here: the gateway
//! carries whole UDS messages).
//!
//! The session is single-threaded request/response, like the rest of the
//! workspace. Keep-alive is therefore driven by the caller between requests
//! (`UdsClient::tester_present`, as `serve` does), not by a background thread:
//! a second thread could read another request's reply off the shared stream.

use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::rc::Rc;
use std::time::Duration;

use opendiag_core::{Error, Result, WriteMode};
use opendiag_proto::clock::Clock;
use opendiag_proto::link::MessageLink;

use crate::hsfz::{self, take_frame, HsfzFrame};

/// A byte stream with timed reads. `TcpByteStream` for real, scripted fakes in tests.
pub trait ByteStream {
    fn write_all(&mut self, data: &[u8]) -> Result<()>;
    /// Whatever bytes arrive within `timeout`; `Error::Timeout` if none.
    fn read_some(&mut self, timeout: Duration) -> Result<Vec<u8>>;
    /// Open the connection again after it dropped. Unsupported by default.
    fn reconnect(&mut self) -> Result<()> {
        Err(Error::Transport("this stream cannot reconnect".into()))
    }
}

/// [`ByteStream`] over TCP.
pub struct TcpByteStream {
    addr: SocketAddr,
    stream: TcpStream,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

fn io_err(what: &str, e: std::io::Error) -> Error {
    Error::Transport(format!("{what}: {e}"))
}

impl TcpByteStream {
    pub fn connect(addr: SocketAddr) -> Result<Self> {
        Ok(Self {
            addr,
            stream: Self::open(addr)?,
        })
    }

    fn open(addr: SocketAddr) -> Result<TcpStream> {
        let s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
            .map_err(|e| io_err(&format!("connecting to {addr}"), e))?;
        s.set_nodelay(true).map_err(|e| io_err("set_nodelay", e))?;
        Ok(s)
    }
}

impl ByteStream for TcpByteStream {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.stream
            .write_all(data)
            .map_err(|e| io_err("writing to the gateway", e))
    }

    fn read_some(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        // A zero timeout means "block forever" to the OS; never pass one.
        let t = timeout.max(Duration::from_millis(1));
        self.stream
            .set_read_timeout(Some(t))
            .map_err(|e| io_err("set_read_timeout", e))?;
        let mut buf = [0u8; 4096];
        match self.stream.read(&mut buf) {
            Ok(0) => Err(Error::Transport("the gateway closed the connection".into())),
            Ok(n) => Ok(buf[..n].to_vec()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Err(Error::Timeout)
            }
            Err(e) => Err(io_err("reading from the gateway", e)),
        }
    }

    fn reconnect(&mut self) -> Result<()> {
        self.stream = Self::open(self.addr)?;
        Ok(())
    }
}

struct Inner<S: ByteStream, C: Clock> {
    stream: S,
    clock: C,
    buf: Vec<u8>,
    tester: u8,
}

impl<S: ByteStream, C: Clock> Inner<S, C> {
    /// Next complete frame within `timeout`.
    fn next_frame(&mut self, timeout: Duration) -> Result<HsfzFrame> {
        let deadline = self.clock.now() + timeout;
        loop {
            if let Some(f) = take_frame(&mut self.buf)? {
                return Ok(f);
            }
            let now = self.clock.now();
            if now >= deadline {
                return Err(Error::Timeout);
            }
            let more = self.stream.read_some(deadline - now)?;
            self.buf.extend_from_slice(&more);
        }
    }
}

/// The connection to a gateway. Clone-free: make links with [`HsfzGateway::link`].
pub struct HsfzGateway<S: ByteStream, C: Clock> {
    inner: Rc<RefCell<Inner<S, C>>>,
}

impl<S: ByteStream, C: Clock> HsfzGateway<S, C> {
    pub fn new(stream: S, clock: C, tester: u8) -> Self {
        Self {
            inner: Rc::new(RefCell::new(Inner {
                stream,
                clock,
                buf: Vec::new(),
                tester,
            })),
        }
    }

    /// A message link to the ECU with diagnostic address `target`.
    pub fn link(&self, target: u8) -> HsfzLink<S, C> {
        HsfzLink {
            inner: Rc::clone(&self.inner),
            target,
        }
    }

    /// Drop buffered bytes and reopen the connection. Any diagnostic session
    /// state in the ECUs is the caller's to re-establish.
    pub fn reconnect(&self) -> Result<()> {
        let mut i = self.inner.borrow_mut();
        i.buf.clear();
        i.stream.reconnect()
    }
}

/// Services that start flashing or reprogramming. They are never sent, by
/// anything, at any write mode (CLAUDE.md ground rule 3).
fn forbidden(req: &[u8]) -> Option<&'static str> {
    match req {
        [0x10, sub, ..] if sub & 0x7F == 0x02 => Some("programming session (0x10 02)"),
        [0x34..=0x38, ..] => Some("download/upload/transfer (flash) service"),
        _ => None,
    }
}

/// Message link to one ECU behind the gateway.
pub struct HsfzLink<S: ByteStream, C: Clock> {
    inner: Rc<RefCell<Inner<S, C>>>,
    target: u8,
}

impl<S: ByteStream, C: Clock> HsfzLink<S, C> {
    pub fn target(&self) -> u8 {
        self.target
    }
}

impl<S: ByteStream, C: Clock> MessageLink for HsfzLink<S, C> {
    fn send(&mut self, payload: &[u8]) -> Result<()> {
        if let Some(what) = forbidden(payload) {
            return Err(Error::WritesLocked(format!(
                "{what} is never sent: OpenDiag does not flash or reprogram ECUs"
            )));
        }
        let mut i = self.inner.borrow_mut();
        let frame = HsfzFrame::diagnostic(i.tester, self.target, payload);
        i.stream.write_all(&frame.encode())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let mut i = self.inner.borrow_mut();
        let deadline = i.clock.now() + timeout;
        loop {
            let left = deadline.saturating_sub(i.clock.now());
            let frame = i.next_frame(left)?;
            if let Some((src, dst, data)) = frame.as_diagnostic() {
                if src == self.target && dst == i.tester {
                    return Ok(data.to_vec());
                }
                continue; // another ECU's traffic
            }
            if let Some(name) = hsfz::error_name(frame.control) {
                return Err(Error::Protocol(format!(
                    "hsfz: gateway reported `{name}` (control {:#06X}, payload {:02X?})",
                    frame.control, frame.payload
                )));
            }
            // Acknowledge, alive check, terminal 15, status, ident: not a reply to us.
        }
    }

    fn describe(&self) -> String {
        format!("BMW ECU {:#04X} via HSFZ", self.target)
    }
}

/// Convenience: a read-only default, spelled out so callers need not import `WriteMode`.
pub const DEFAULT_WRITE_MODE: WriteMode = WriteMode::ReadOnly;

#[cfg(test)]
mod tests {
    use super::*;
    use opendiag_proto::clock::FakeClock;
    use std::collections::VecDeque;

    /// Feeds scripted chunks and records what was written.
    struct Scripted {
        incoming: VecDeque<Vec<u8>>,
        written: Vec<u8>,
    }

    impl Scripted {
        fn new(chunks: Vec<Vec<u8>>) -> Self {
            Self {
                incoming: chunks.into(),
                written: Vec::new(),
            }
        }
    }

    impl ByteStream for Scripted {
        fn write_all(&mut self, data: &[u8]) -> Result<()> {
            self.written.extend_from_slice(data);
            Ok(())
        }
        fn read_some(&mut self, _t: Duration) -> Result<Vec<u8>> {
            self.incoming.pop_front().ok_or(Error::Timeout)
        }
    }

    fn gw(chunks: Vec<Vec<u8>>) -> HsfzGateway<Scripted, FakeClock> {
        HsfzGateway::new(Scripted::new(chunks), FakeClock::new(), 0xF4)
    }

    #[test]
    fn skips_acks_and_other_ecus_and_returns_the_reply() {
        let ack = HsfzFrame::new(hsfz::CTRL_ACK, vec![0x12, 0xF4]).encode();
        let other = HsfzFrame::diagnostic(0x40, 0xF4, &[0x62, 1, 1, 1]).encode();
        let mine = HsfzFrame::diagnostic(0x12, 0xF4, &[0x62, 0xF1, 0x90, b'X']).encode();
        let g = gw(vec![[ack, other, mine].concat()]);
        let mut link = g.link(0x12);
        link.send(&[0x22, 0xF1, 0x90]).unwrap();
        assert_eq!(
            link.recv(Duration::from_secs(1)).unwrap(),
            [0x62, 0xF1, 0x90, b'X']
        );
    }

    #[test]
    fn the_request_is_addressed_from_the_tester_to_the_ecu() {
        let g = gw(vec![]);
        let mut link = g.link(0x12);
        link.send(&[0x3E, 0x00]).unwrap();
        let i = g.inner.borrow();
        assert_eq!(i.stream.written, [0, 0, 0, 4, 0, 1, 0xF4, 0x12, 0x3E, 0x00]);
    }

    #[test]
    fn silence_is_a_timeout() {
        let g = gw(vec![]);
        let mut link = g.link(0x10);
        assert!(matches!(
            link.recv(Duration::from_millis(100)),
            Err(Error::Timeout)
        ));
    }

    #[test]
    fn a_gateway_error_word_becomes_a_protocol_error() {
        let e = HsfzFrame::new(0x43, vec![0x99]).encode();
        let g = gw(vec![e]);
        let mut link = g.link(0x99);
        let err = link.recv(Duration::from_secs(1)).unwrap_err().to_string();
        assert!(err.contains("incorrect destination address"), "{err}");
    }

    #[test]
    fn flash_and_programming_requests_are_never_written() {
        let g = gw(vec![]);
        let mut link = g.link(0x12);
        for req in [
            &[0x10u8, 0x02][..],
            &[0x10, 0x82],
            &[0x34, 0, 0],
            &[0x36, 1],
            &[0x37],
        ] {
            assert!(
                matches!(link.send(req), Err(Error::WritesLocked(_))),
                "{req:02X?}"
            );
        }
        assert!(g.inner.borrow().stream.written.is_empty());
        // The extended session is fine.
        link.send(&[0x10, 0x03]).unwrap();
    }

    #[test]
    fn a_frame_split_across_reads_is_reassembled() {
        let f = HsfzFrame::diagnostic(0x12, 0xF4, &[0x62, 0xF1, 0x90, b'A', b'B']).encode();
        let (a, b) = f.split_at(4);
        let g = gw(vec![a.to_vec(), b.to_vec()]);
        let mut link = g.link(0x12);
        assert_eq!(link.recv(Duration::from_secs(1)).unwrap().len(), 5);
    }
}
