//! A recorded session played back as a stand-in for the vehicle.
//!
//! [`TraceReplay`](crate::TraceReplay) is strict: the client must repeat the recording exactly, which
//! makes it a regression test. `TraceEcu` is lenient: it behaves like the module that was recorded.
//! Each operation the client performs (`send`, `set_baud`, `slow_init`, `fast_init`) is looked up in
//! the recording by *content*, searching forward from the last match and then from the start, and
//! the recorded replies that followed it are served. That lets `scan`, `dtc read`, `log` and `serve`
//! run, repeatedly and in any order, against a capture with no hardware attached.
//!
//! Limits, stated plainly:
//! - Only what was recorded can be answered. An operation that is not in the recording is an error
//!   naming it; nothing is guessed or invented.
//! - Replies are the recorded bytes. A reply that depends on state (a counter, a changing value)
//!   repeats the recorded value, so this does not simulate a live vehicle.
//! - Matching is by content. On K-line, identical single-byte acknowledgements are ambiguous; the
//!   forward-first search resolves them for sessions replayed in recorded order, but a session
//!   replayed out of order can be matched to the wrong place.
//! - Timing is ignored.

use std::time::Duration;

use opendiag_core::{Error, Result};

use crate::trace::{parse_trace, TraceEntry, TraceEvent};
use crate::{Capabilities, Frame, Transport};

#[derive(Debug)]
pub struct TraceEcu {
    entries: Vec<TraceEntry>,
    /// Next entry to hand out as a reply.
    pos: usize,
    rewinds: u32,
}

impl TraceEcu {
    pub fn from_text(text: &str) -> Result<Self> {
        Ok(Self::from_entries(parse_trace(text)?))
    }

    pub fn from_entries(entries: Vec<TraceEntry>) -> Self {
        Self {
            entries,
            pos: 0,
            rewinds: 0,
        }
    }

    /// How many times an operation was only found by going back to the start of the recording.
    pub fn rewinds(&self) -> u32 {
        self.rewinds
    }

    /// Find `op` by content, forward from the cursor first and then from the start.
    fn find(&mut self, op: &TraceEvent) -> Result<()> {
        let len = self.entries.len();
        let forward = (self.pos..len).find(|&i| &self.entries[i].event == op);
        let found = match forward {
            Some(i) => Some(i),
            None => {
                let back = (0..self.pos.min(len)).find(|&i| &self.entries[i].event == op);
                if back.is_some() {
                    self.rewinds += 1;
                }
                back
            }
        };
        match found {
            Some(i) => {
                self.pos = i + 1;
                Ok(())
            }
            None => Err(Error::Transport(format!(
                "not in recording: the client did `{op}`, which was never recorded"
            ))),
        }
    }
}

impl Transport for TraceEcu {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.find(&TraceEvent::Tx(frame.clone()))
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Frame> {
        match self.entries.get(self.pos) {
            Some(TraceEntry {
                event: TraceEvent::Rx(f),
                ..
            }) => {
                let f = f.clone();
                self.pos += 1;
                Ok(f)
            }
            Some(TraceEntry {
                event: TraceEvent::RecvTimeout(_),
                ..
            }) => {
                self.pos += 1;
                Err(Error::Timeout)
            }
            Some(TraceEntry {
                event: TraceEvent::Error(m),
                ..
            }) => {
                let e = Error::Transport(m.clone());
                self.pos += 1;
                Err(e)
            }
            // The module said nothing more after that request.
            _ => Err(Error::Timeout),
        }
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.find(&TraceEvent::SetBaud(baud))
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        self.find(&TraceEvent::SlowInit(address))
    }

    fn fast_init(&mut self) -> Result<()> {
        self.find(&TraceEvent::FastInit)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can: true,
            kline: true,
            set_baud: true,
            slow_init: true,
            fast_init: true,
        }
    }
}
