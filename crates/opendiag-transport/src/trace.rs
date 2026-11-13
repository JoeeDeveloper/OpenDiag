//! Session tracing: record everything a transport sends and receives, and
//! replay a recording strictly (for regression fixtures).
//!
//! The format is plain text, one event per line, easy to read and diff:
//!
//! ```text
//! # opendiag trace v1
//! 0.000120 EV slow_init 01
//! 0.002300 RX raw 55 01 8A
//! 0.004100 TX raw 75
//! 0.010000 TX can 7E0#0210030000000000
//! 0.012000 RX can 7E8#065003003201F4
//! 0.500000 EV recv_timeout 1000
//! ```
//!
//! CAN frames use the `candump -L` notation (3 hex digits for 11-bit ids,
//! 8 for 29-bit). Lines starting with `#` are comments.
//!
//! Traces contain whatever the vehicle reported, including VINs and part
//! numbers. Review one before sharing it.

use std::fmt;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opendiag_core::{Error, Result};

use crate::{Capabilities, Frame, Transport};

pub const TRACE_HEADER: &str = "# opendiag trace v1";

/// One thing that happened on the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceEvent {
    Tx(Frame),
    Rx(Frame),
    SlowInit(u8),
    FastInit,
    SetBaud(u32),
    /// A receive that ended without data, after waiting this long.
    RecvTimeout(Duration),
    /// A transport operation failed.
    Error(String),
}

fn hex_spaced(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn hex_compact(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Text form of a frame as used in trace lines (`raw 81 01` / `can 7E0#0210`).
pub fn format_frame(f: &Frame) -> String {
    match f {
        Frame::Raw(b) if b.is_empty() => "raw".to_string(),
        Frame::Raw(b) => format!("raw {}", hex_spaced(b)),
        Frame::Can {
            id,
            extended: true,
            data,
        } => format!("can {id:08X}#{}", hex_compact(data)),
        Frame::Can { id, data, .. } => format!("can {id:03X}#{}", hex_compact(data)),
    }
}

impl fmt::Display for TraceEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tx(fr) => write!(f, "TX {}", format_frame(fr)),
            Self::Rx(fr) => write!(f, "RX {}", format_frame(fr)),
            Self::SlowInit(a) => write!(f, "EV slow_init {a:02X}"),
            Self::FastInit => write!(f, "EV fast_init"),
            Self::SetBaud(b) => write!(f, "EV baud {b}"),
            Self::RecvTimeout(d) => write!(f, "EV recv_timeout {}", d.as_millis()),
            // Errors are one line; flatten any newlines.
            Self::Error(m) => write!(f, "EV error {}", m.replace(['\n', '\r'], " ")),
        }
    }
}

/// An event with its place in the recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    /// Time since the trace started.
    pub time: Duration,
    /// 1-based line number in the trace file.
    pub line: usize,
    pub event: TraceEvent,
}

fn bad_line(line: usize, msg: impl fmt::Display) -> Error {
    Error::Transport(format!("trace line {line}: {msg}"))
}

fn parse_byte(tok: &str, line: usize) -> Result<u8> {
    if tok.len() > 2 {
        return Err(bad_line(line, format!("`{tok}` is not a hex byte")));
    }
    u8::from_str_radix(tok, 16).map_err(|_| bad_line(line, format!("`{tok}` is not a hex byte")))
}

fn parse_can(tok: &str, line: usize) -> Result<Frame> {
    let (id_str, data_str) = tok
        .split_once('#')
        .ok_or_else(|| bad_line(line, "CAN frame needs `ID#DATA`"))?;
    let id = u32::from_str_radix(id_str, 16)
        .map_err(|_| bad_line(line, format!("bad id `{id_str}`")))?;
    if data_str.len() % 2 != 0 || !data_str.is_ascii() {
        return Err(bad_line(line, format!("bad CAN data `{data_str}`")));
    }
    let data = (0..data_str.len())
        .step_by(2)
        .map(|i| parse_byte(&data_str[i..i + 2], line))
        .collect::<Result<Vec<u8>>>()?;
    Ok(Frame::Can {
        id,
        extended: id_str.len() > 3,
        data,
    })
}

fn parse_frame(kind: &str, rest: &[&str], line: usize) -> Result<Frame> {
    match kind {
        "raw" => Ok(Frame::Raw(
            rest.iter()
                .map(|t| parse_byte(t, line))
                .collect::<Result<Vec<u8>>>()?,
        )),
        "can" => match rest {
            [one] => parse_can(one, line),
            _ => Err(bad_line(line, "CAN frame needs exactly `ID#DATA`")),
        },
        other => Err(bad_line(line, format!("unknown frame kind `{other}`"))),
    }
}

fn parse_time(tok: &str, line: usize) -> Result<Duration> {
    let (s, us) = tok
        .split_once('.')
        .ok_or_else(|| bad_line(line, format!("bad timestamp `{tok}`")))?;
    let secs: u64 = s
        .parse()
        .map_err(|_| bad_line(line, format!("bad timestamp `{tok}`")))?;
    let micros: u32 = us
        .parse()
        .map_err(|_| bad_line(line, format!("bad timestamp `{tok}`")))?;
    if us.len() != 6 {
        return Err(bad_line(
            line,
            format!("timestamp `{tok}` needs 6 decimals"),
        ));
    }
    Ok(Duration::new(secs, micros * 1000))
}

/// Parse a whole trace. Blank lines and `#` comments are skipped.
pub fn parse_trace(text: &str) -> Result<Vec<TraceEntry>> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let raw = raw.trim().trim_start_matches('\u{feff}');
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = raw.split_whitespace().collect();
        let [time, dir, rest @ ..] = toks.as_slice() else {
            return Err(bad_line(line, "expected `<time> <TX|RX|EV> ...`"));
        };
        let time = parse_time(time, line)?;
        let event = match (*dir, rest) {
            ("TX" | "RX", [kind, data @ ..]) => {
                let f = parse_frame(kind, data, line)?;
                if *dir == "TX" {
                    TraceEvent::Tx(f)
                } else {
                    TraceEvent::Rx(f)
                }
            }
            ("EV", ["slow_init", a]) => TraceEvent::SlowInit(parse_byte(a, line)?),
            ("EV", ["fast_init"]) => TraceEvent::FastInit,
            ("EV", ["baud", b]) => TraceEvent::SetBaud(
                b.parse()
                    .map_err(|_| bad_line(line, format!("bad baud `{b}`")))?,
            ),
            ("EV", ["recv_timeout", ms]) => TraceEvent::RecvTimeout(Duration::from_millis(
                ms.parse()
                    .map_err(|_| bad_line(line, format!("bad timeout `{ms}`")))?,
            )),
            ("EV", ["error", msg @ ..]) => TraceEvent::Error(msg.join(" ")),
            _ => {
                return Err(bad_line(
                    line,
                    format!("unrecognised event `{dir} {}`", rest.join(" ")),
                ))
            }
        };
        out.push(TraceEntry { time, line, event });
    }
    Ok(out)
}

/// Shared view of how a trace is going, readable after the transport has
/// been boxed up inside a client.
#[derive(Clone, Default)]
pub struct TraceStatus(Arc<Mutex<StatusInner>>);

#[derive(Default)]
struct StatusInner {
    events: u64,
    error: Option<String>,
}

impl TraceStatus {
    fn lock(&self) -> std::sync::MutexGuard<'_, StatusInner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Events written so far.
    pub fn events(&self) -> u64 {
        self.lock().events
    }

    /// The first write failure, if any. After one, tracing stops (the session goes on).
    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }
}

/// Wraps a transport and records everything that passes through it.
///
/// A failure to write the trace never disturbs the diagnostic session: tracing
/// stops and the failure is reported through [`TraceStatus`].
pub struct Tracing<T: Transport, W: Write> {
    inner: T,
    out: W,
    now: Box<dyn FnMut() -> Duration>,
    status: TraceStatus,
    log_timeouts: bool,
    failed: bool,
}

impl<T: Transport, W: Write> Tracing<T, W> {
    /// Start tracing to `out`, timestamping from now.
    pub fn new(inner: T, out: W) -> Self {
        let start = Instant::now();
        let mut t = Self {
            inner,
            out,
            now: Box::new(move || start.elapsed()),
            status: TraceStatus::default(),
            log_timeouts: true,
            failed: false,
        };
        t.write_raw_line(TRACE_HEADER);
        t
    }

    /// Replace the clock (tests use this for deterministic timestamps).
    pub fn with_time_source(mut self, now: impl FnMut() -> Duration + 'static) -> Self {
        self.now = Box::new(now);
        self
    }

    /// Whether empty receives are recorded (default yes; passive capture turns it off).
    pub fn log_timeouts(mut self, on: bool) -> Self {
        self.log_timeouts = on;
        self
    }

    pub fn status(&self) -> TraceStatus {
        self.status.clone()
    }

    fn write_raw_line(&mut self, line: &str) {
        if self.failed {
            return;
        }
        let result = writeln!(self.out, "{line}").and_then(|()| self.out.flush());
        if let Err(e) = result {
            self.failed = true;
            self.status.lock().error = Some(e.to_string());
        }
    }

    fn log(&mut self, event: &TraceEvent) {
        if self.failed {
            return;
        }
        let t = (self.now)();
        let line = format!("{}.{:06} {event}", t.as_secs(), t.subsec_micros());
        self.write_raw_line(&line);
        if !self.failed {
            self.status.lock().events += 1;
        }
    }

    fn log_error(&mut self, what: &str, e: &Error) {
        self.log(&TraceEvent::Error(format!("{what}: {e}")));
    }
}

impl<T: Transport, W: Write> Transport for Tracing<T, W> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.log(&TraceEvent::Tx(frame.clone()));
        let r = self.inner.send(frame);
        if let Err(e) = &r {
            self.log_error("send", e);
        }
        r
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame> {
        match self.inner.recv(timeout) {
            Ok(f) => {
                self.log(&TraceEvent::Rx(f.clone()));
                Ok(f)
            }
            Err(Error::Timeout) => {
                if self.log_timeouts {
                    self.log(&TraceEvent::RecvTimeout(timeout));
                }
                Err(Error::Timeout)
            }
            Err(e) => {
                self.log_error("recv", &e);
                Err(e)
            }
        }
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.log(&TraceEvent::SetBaud(baud));
        let r = self.inner.set_baud(baud);
        if let Err(e) = &r {
            self.log_error("set_baud", e);
        }
        r
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        self.log(&TraceEvent::SlowInit(address));
        let r = self.inner.slow_init(address);
        if let Err(e) = &r {
            self.log_error("slow_init", e);
        }
        r
    }

    fn fast_init(&mut self) -> Result<()> {
        self.log(&TraceEvent::FastInit);
        let r = self.inner.fast_init();
        if let Err(e) = &r {
            self.log_error("fast_init", e);
        }
        r
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

/// Strict replay of a recorded session.
///
/// The client under test must do exactly what the recording did, in order:
/// each `send`, `set_baud`, `slow_init` and `fast_init` is compared with the next
/// recorded event and any difference is an error, while `recv` hands back what was
/// received (or the recorded timeout). Timing is ignored. This turns a captured
/// session into a regression test for protocol code.
#[derive(Debug)]
pub struct TraceReplay {
    entries: Vec<TraceEntry>,
    pos: usize,
}

impl TraceReplay {
    pub fn from_text(text: &str) -> Result<Self> {
        Ok(Self::from_entries(parse_trace(text)?))
    }

    pub fn from_entries(entries: Vec<TraceEntry>) -> Self {
        Self { entries, pos: 0 }
    }

    /// Recorded events not yet consumed.
    pub fn remaining(&self) -> usize {
        self.entries.len() - self.pos
    }

    /// True once the client has done everything the recording did.
    pub fn is_finished(&self) -> bool {
        self.pos >= self.entries.len()
    }

    /// A recorded failure is the outcome of the next operation, whatever it is.
    fn pending_error(&mut self) -> Result<()> {
        if let Some(TraceEntry {
            event: TraceEvent::Error(m),
            ..
        }) = self.entries.get(self.pos)
        {
            let err = Error::Transport(m.clone());
            self.pos += 1;
            return Err(err);
        }
        Ok(())
    }

    fn expect(&mut self, actual: TraceEvent) -> Result<()> {
        self.pending_error()?;
        match self.entries.get(self.pos) {
            Some(e) if e.event == actual => {
                self.pos += 1;
                Ok(())
            }
            Some(e) => Err(Error::Transport(format!(
                "trace mismatch at line {}: recording has `{}` but the client did `{actual}`",
                e.line, e.event
            ))),
            None => Err(Error::Transport(format!(
                "trace ended, but the client did `{actual}`"
            ))),
        }
    }
}

impl Transport for TraceReplay {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.expect(TraceEvent::Tx(frame.clone()))
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Frame> {
        self.pending_error()?;
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
            Some(e) => Err(Error::Transport(format!(
                "trace mismatch at line {}: the client is receiving but the recording continues with `{}`",
                e.line, e.event
            ))),
            None => Err(Error::Timeout),
        }
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.expect(TraceEvent::SetBaud(baud))
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        self.expect(TraceEvent::SlowInit(address))
    }

    fn fast_init(&mut self) -> Result<()> {
        self.expect(TraceEvent::FastInit)
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
