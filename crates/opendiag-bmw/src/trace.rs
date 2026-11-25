//! Record and replay of the HSFZ byte stream, so BMW commands can run with no car attached.
//!
//! [`RecordingStream`] wraps any [`ByteStream`] and writes everything that crosses it to a trace in
//! the same text format as the K-line and CAN traces (`TX raw ..` for bytes sent to the gateway,
//! `RX raw ..` for bytes read from it). It records raw HSFZ frames, uninterpreted.
//!
//! [`ReplayStream`] plays such a trace back as the gateway. Like `TraceEcu`, it is lenient: each
//! write is looked up in the recording by *content* (forward from the last match, then from the
//! start), and the recorded bytes that followed it are served to the next reads. A write that was
//! never recorded is an error naming it; nothing is invented. Replies are the recorded bytes, so
//! values that changed over time repeat. Timing is ignored.
//!
//! A replayed session can only ever do what was recorded, which is why `bmw` write commands refuse
//! to run against one (see the CLI).

use std::io::Write;
use std::time::{Duration, Instant};

use opendiag_core::{Error, Result};
use opendiag_transport::{format_frame, parse_trace, Frame, TraceEntry, TraceEvent, TRACE_HEADER};

use crate::gateway::ByteStream;

/// A boxed stream is a stream, so one gateway type serves live, recorded and replayed sessions.
impl ByteStream for Box<dyn ByteStream> {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        (**self).write_all(data)
    }
    fn read_some(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        (**self).read_some(timeout)
    }
    fn reconnect(&mut self) -> Result<()> {
        (**self).reconnect()
    }
}

/// Records everything that crosses `inner` to `out`. Silent timeouts are not recorded.
pub struct RecordingStream<S: ByteStream, W: Write> {
    inner: S,
    out: W,
    started: Instant,
}

impl<S: ByteStream, W: Write> RecordingStream<S, W> {
    pub fn new(inner: S, mut out: W) -> Result<Self> {
        writeln!(out, "{TRACE_HEADER}")
            .and_then(|()| writeln!(out, "# hsfz byte stream: TX/RX are raw HSFZ frames"))
            .and_then(|()| out.flush())
            .map_err(|e| Error::Io(format!("cannot write the trace: {e}")))?;
        Ok(Self {
            inner,
            out,
            started: Instant::now(),
        })
    }

    fn log(&mut self, event: &TraceEvent) -> Result<()> {
        let t = self.started.elapsed();
        writeln!(self.out, "{}.{:06} {event}", t.as_secs(), t.subsec_micros())
            .and_then(|()| self.out.flush())
            .map_err(|e| Error::Io(format!("cannot write the trace: {e}")))
    }
}

impl<S: ByteStream, W: Write> ByteStream for RecordingStream<S, W> {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.log(&TraceEvent::Tx(Frame::Raw(data.to_vec())))?;
        self.inner.write_all(data)
    }

    fn read_some(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let bytes = self.inner.read_some(timeout)?;
        self.log(&TraceEvent::Rx(Frame::Raw(bytes.clone())))?;
        Ok(bytes)
    }

    fn reconnect(&mut self) -> Result<()> {
        self.inner.reconnect()
    }
}

/// A recorded HSFZ session played back as the gateway.
#[derive(Debug)]
pub struct ReplayStream {
    entries: Vec<TraceEntry>,
    pos: usize,
}

impl ReplayStream {
    pub fn from_text(text: &str) -> Result<Self> {
        Ok(Self {
            entries: parse_trace(text)?,
            pos: 0,
        })
    }
}

impl ByteStream for ReplayStream {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        let want = TraceEvent::Tx(Frame::Raw(data.to_vec()));
        let hit = (self.pos..self.entries.len())
            .chain(0..self.pos.min(self.entries.len()))
            .find(|&i| self.entries[i].event == want);
        match hit {
            Some(i) => {
                self.pos = i + 1;
                Ok(())
            }
            None => Err(Error::Transport(format!(
                "not in recording: the client sent `{}`, which was never recorded ({})",
                format_frame(&Frame::Raw(data.to_vec())),
                "replay answers only what was captured"
            ))),
        }
    }

    fn read_some(&mut self, _timeout: Duration) -> Result<Vec<u8>> {
        match self.entries.get(self.pos) {
            Some(TraceEntry {
                event: TraceEvent::Rx(Frame::Raw(b)),
                ..
            }) => {
                let b = b.clone();
                self.pos += 1;
                Ok(b)
            }
            // The gateway said nothing more after that request.
            _ => Err(Error::Timeout),
        }
    }

    fn reconnect(&mut self) -> Result<()> {
        Ok(())
    }
}
