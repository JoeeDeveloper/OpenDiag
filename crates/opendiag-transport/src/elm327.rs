//! ELM327 (and compatible) adapters as a CAN transport.
//!
//! The adapter is put in raw mode (`ATCAF0`): we send up to 8 data bytes for a
//! fixed 11-bit request id, and it prints every CAN frame it receives from the
//! chosen response id. ISO-TP stays in our own stack, so UDS works over it the
//! same way as over SocketCAN.
//!
//! This is a degraded transport, by nature of the adapters:
//! - only 11-bit identifiers on 500 kbit/s CAN (`ATSP6`);
//! - the adapter answers each frame we send, so a multi-frame exchange runs at
//!   the adapter's pace and long replies can overflow its small buffer;
//! - no K-line, no slow init, no fast init (use a K-line cable for those);
//! - many cheap clones implement only part of the AT command set.
//!
//! Only commands built here are ever sent to the adapter. Nothing in this
//! module takes AT text from a caller.
//!
//! **Not verified against any real adapter.** It is tested against the
//! [`Elm327Emulator`] below, which encodes our reading of the ELM327 data
//! sheet, so a shared misunderstanding would not be caught.

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use opendiag_core::{Error, Result};

use crate::serial::SerialIo;
use crate::{Capabilities, Frame, Transport};

/// Longest we wait for the adapter's `>` prompt after a command.
const COMMAND_WAIT: Duration = Duration::from_millis(1500);
/// ATZ resets the chip and can take a while.
const RESET_WAIT: Duration = Duration::from_millis(3000);
/// Longest we wait after sending a frame: the adapter's own response timeout
/// (set by `ATST`) plus slack.
const FRAME_WAIT: Duration = Duration::from_millis(600);
/// Adapter response timeout in units of 4 ms (`ATST`): 0x20 is about 128 ms.
const ADAPTER_TIMEOUT_UNITS: u8 = 0x20;

/// Error lines an adapter prints instead of data.
const ERROR_LINES: [&str; 9] = [
    "?",
    "CAN ERROR",
    "BUS ERROR",
    "BUS BUSY",
    "BUFFER FULL",
    "DATA ERROR",
    "FB ERROR",
    "UNABLE TO CONNECT",
    "STOPPED",
];

/// An ELM327-style adapter over any [`SerialIo`].
pub struct Elm327<P: SerialIo> {
    port: P,
    queue: VecDeque<Frame>,
    header: Option<u32>,
    version: Option<String>,
}

/// An adapter on a real serial port.
pub type Elm327Serial = Elm327<Box<dyn serialport::SerialPort>>;

impl Elm327Serial {
    /// Open `path` (e.g. `COM4`). 38400 baud is the usual default; some clones use 115200 or 9600.
    pub fn open(path: &str, baud: u32) -> Result<Self> {
        let port = serialport::new(path, baud)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| Error::Transport(format!("cannot open {path}: {e}")))?;
        Ok(Self::new(port))
    }
}

impl<P: SerialIo> Elm327<P> {
    pub fn new(port: P) -> Self {
        Self {
            port,
            queue: VecDeque::new(),
            header: None,
            version: None,
        }
    }

    /// The version text the adapter printed at reset (e.g. `ELM327 v1.5`).
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Reset the adapter and set it up for raw 11-bit CAN with the given ids.
    pub fn init(&mut self, tester_id: u32, ecu_id: u32) -> Result<()> {
        if tester_id > 0x7FF || ecu_id > 0x7FF {
            return Err(Error::Transport(
                "ELM327 support is limited to 11-bit CAN identifiers".into(),
            ));
        }
        let _ = self.port.clear_input();
        let lines = self.command("ATZ", RESET_WAIT)?;
        let version = lines
            .iter()
            .find(|l| l.to_ascii_uppercase().contains("ELM"))
            .ok_or_else(|| {
                Error::Transport(format!(
                    "not an ELM327-style adapter (reset answered {lines:?}); check the port and baud rate"
                ))
            })?
            .clone();
        self.version = Some(version);
        for cmd in [
            "ATE0",   // no echo
            "ATL0",   // no line feeds
            "ATS0",   // no spaces
            "ATH1",   // show headers (the CAN id)
            "ATCAF0", // raw frames: no automatic ISO-TP handling
            "ATSP6",  // ISO 15765-4, 11-bit, 500 kbit/s
        ] {
            self.expect_ok(cmd)?;
        }
        self.expect_ok(&format!("ATST{ADAPTER_TIMEOUT_UNITS:02X}"))?;
        self.expect_ok(&format!("ATCRA{ecu_id:03X}"))?;
        self.set_header(tester_id)
    }

    fn set_header(&mut self, id: u32) -> Result<()> {
        self.expect_ok(&format!("ATSH{id:03X}"))?;
        self.header = Some(id);
        Ok(())
    }

    fn expect_ok(&mut self, cmd: &str) -> Result<()> {
        let lines = match self.command(cmd, COMMAND_WAIT) {
            Err(Error::Transport(m)) => {
                return Err(Error::Transport(format!(
                    "adapter did not accept {cmd} ({m}); it may not support this command"
                )))
            }
            other => other?,
        };
        if lines.iter().any(|l| l == "OK") {
            Ok(())
        } else {
            Err(Error::Transport(format!(
                "adapter did not accept {cmd} (answered {lines:?}); it may not support this command"
            )))
        }
    }

    /// Send one line and return the reply lines (echo and prompt removed).
    /// Adapter error lines become errors; `NO DATA` yields no lines.
    fn command(&mut self, cmd: &str, wait: Duration) -> Result<Vec<String>> {
        let _ = self.port.clear_input();
        self.port
            .write_all(format!("{cmd}\r").as_bytes())
            .map_err(|e| Error::Transport(e.to_string()))?;
        let text = self.read_until_prompt(wait)?;
        parse_lines(&text, cmd)
    }

    fn read_until_prompt(&mut self, wait: Duration) -> Result<String> {
        let deadline = Instant::now() + wait;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 128];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            match self.port.read(&mut chunk, left) {
                Ok(0) => {}
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.contains(&b'>') {
                        return Ok(String::from_utf8_lossy(&buf).into_owned());
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) => {}
                Err(e) => return Err(Error::Transport(e.to_string())),
            }
        }
    }
}

/// Split adapter output into trimmed, non-empty lines, dropping the echoed
/// command and the prompt, and turning error lines into errors.
fn parse_lines(text: &str, cmd: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for raw in text.split(['\r', '\n']) {
        let line = raw.replace('>', "").trim().to_string();
        if line.is_empty() || line == cmd || line == "SEARCHING..." {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if upper == "NO DATA" {
            continue;
        }
        if let Some(e) = ERROR_LINES.iter().find(|e| upper.contains(*e)) {
            return Err(Error::Transport(format!("adapter reported `{e}`")));
        }
        out.push(line);
    }
    Ok(out)
}

/// `7E8037F2278` or `7E8 03 7F 22 78` -> a CAN frame.
fn parse_frame(line: &str) -> Option<Frame> {
    let hex: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    if hex.len() < 5 || hex.len() % 2 == 0 || hex.len() > 3 + 16 {
        return None;
    }
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let id = u32::from_str_radix(&hex[..3], 16).ok()?;
    let data = (3..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<std::result::Result<Vec<u8>, _>>()
        .ok()?;
    Some(Frame::can(id, &data))
}

impl<P: SerialIo> Transport for Elm327<P> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let Frame::Can { id, extended, data } = frame else {
            return Err(Error::Transport("ELM327 carries CAN frames only".into()));
        };
        if *extended || *id > 0x7FF {
            return Err(Error::Transport(
                "ELM327 support is limited to 11-bit CAN identifiers".into(),
            ));
        }
        if data.is_empty() || data.len() > 8 {
            return Err(Error::Transport(format!(
                "a CAN frame carries 1 to 8 bytes, not {}",
                data.len()
            )));
        }
        if self.header != Some(*id) {
            self.set_header(*id)?;
        }
        let hex: String = data.iter().map(|b| format!("{b:02X}")).collect();
        let lines = self.command(&hex, FRAME_WAIT)?;
        for line in lines {
            match parse_frame(&line) {
                Some(f) => self.queue.push_back(f),
                None => {
                    return Err(Error::Transport(format!(
                        "unexpected adapter output `{line}`"
                    )))
                }
            }
        }
        Ok(())
    }

    /// Frames arrive as the adapter's answer to `send`, so this only drains
    /// what is already queued; `timeout` is not used.
    fn recv(&mut self, _timeout: Duration) -> Result<Frame> {
        self.queue.pop_front().ok_or(Error::Timeout)
    }

    fn set_baud(&mut self, _baud: u32) -> Result<()> {
        Err(Error::Transport(
            "ELM327 adapters cannot change the K-line baud rate".into(),
        ))
    }

    fn slow_init(&mut self, _address: u8) -> Result<()> {
        Err(Error::Transport(
            "ELM327 adapters are not supported for 5-baud init; use a K-line cable".into(),
        ))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can: true,
            ..Capabilities::default()
        }
    }
}

/// A software ELM327 in front of any [`Transport`], for tests and demos.
///
/// It speaks the subset of the AT command set that [`Elm327`] uses, and prints
/// frames the way the data sheet describes. It is our reading of that sheet,
/// not a captured adapter.
pub struct Elm327Emulator<T: Transport> {
    bus: T,
    line: Vec<u8>,
    out: VecDeque<u8>,
    echo: bool,
    spaces: bool,
    header: u32,
    unsupported: Vec<String>,
    /// Every command line received, for assertions.
    pub log: Vec<String>,
}

impl<T: Transport> Elm327Emulator<T> {
    pub fn new(bus: T) -> Self {
        Self {
            bus,
            line: Vec::new(),
            out: VecDeque::new(),
            echo: true,
            spaces: true,
            header: 0x7DF,
            unsupported: Vec::new(),
            log: Vec::new(),
        }
    }

    /// Make the emulator answer `?` to AT commands starting with `prefix`,
    /// like a clone that lacks them.
    pub fn without_command(mut self, prefix: &str) -> Self {
        self.unsupported.push(prefix.to_ascii_uppercase());
        self
    }

    pub fn bus(&self) -> &T {
        &self.bus
    }

    fn reply(&mut self, text: &str) {
        self.out.extend(text.bytes());
    }

    fn handle(&mut self, cmd: &str) {
        self.log.push(cmd.to_string());
        let upper = cmd.to_ascii_uppercase();
        if self.echo {
            self.reply(&format!("{cmd}\r"));
        }
        if let Some(rest) = upper.strip_prefix("AT") {
            if self.unsupported.iter().any(|p| upper.starts_with(p)) {
                self.reply("?\r\r>");
                return;
            }
            match rest {
                "Z" => {
                    self.echo = true;
                    self.reply("\r\rELM327 v1.5\r\r>");
                    return;
                }
                "E0" => self.echo = false,
                "E1" => self.echo = true,
                "S0" => self.spaces = false,
                "S1" => self.spaces = true,
                "L0" | "L1" | "H0" | "H1" | "CAF0" | "CAF1" | "SP6" => {}
                r if r.starts_with("ST") || r.starts_with("CRA") => {}
                r if r.starts_with("SH") => match u32::from_str_radix(&r[2..], 16) {
                    Ok(id) => self.header = id,
                    Err(_) => {
                        self.reply("?\r\r>");
                        return;
                    }
                },
                _ => {
                    self.reply("?\r\r>");
                    return;
                }
            }
            self.reply("OK\r\r>");
            return;
        }
        // Anything else is hex data for a raw frame.
        let digits: String = cmd.chars().filter(|c| !c.is_whitespace()).collect();
        let data: Option<Vec<u8>> = (digits.len() % 2 == 0 && !digits.is_empty())
            .then(|| {
                (0..digits.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).ok())
                    .collect()
            })
            .flatten();
        let Some(data) = data.filter(|d| d.len() <= 8) else {
            self.reply("?\r\r>");
            return;
        };
        if self.bus.send(&Frame::can(self.header, &data)).is_err() {
            self.reply("CAN ERROR\r\r>");
            return;
        }
        let mut any = false;
        while let Ok(Frame::Can { id, data, .. }) = self.bus.recv(Duration::ZERO) {
            any = true;
            let mut line = format!("{id:03X}");
            for b in data {
                if self.spaces {
                    line.push(' ');
                }
                line.push_str(&format!("{b:02X}"));
            }
            self.reply(&format!("{line}\r"));
        }
        if !any {
            self.reply("NO DATA\r");
        }
        self.reply("\r>");
    }
}

impl<T: Transport> SerialIo for Elm327Emulator<T> {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        for &b in data {
            if b == b'\r' {
                let cmd = String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned();
                self.handle(cmd.trim());
            } else {
                self.line.push(b);
            }
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8], _timeout: Duration) -> io::Result<usize> {
        if self.out.is_empty() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let n = buf.len().min(self.out.len());
        for slot in buf.iter_mut().take(n) {
            *slot = self.out.pop_front().unwrap_or(0);
        }
        Ok(n)
    }

    fn set_baud(&mut self, _baud: u32) -> io::Result<()> {
        Ok(())
    }

    fn set_break(&mut self, _on: bool) -> io::Result<()> {
        Ok(())
    }

    fn clear_input(&mut self) -> io::Result<()> {
        self.out.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Simulator;

    #[test]
    fn parses_frames_with_and_without_spaces() {
        let want = Frame::can(0x7E8, &[0x03, 0x7F, 0x22, 0x78]);
        assert_eq!(parse_frame("7E8037F2278"), Some(want.clone()));
        assert_eq!(parse_frame("7E8 03 7F 22 78"), Some(want));
        assert_eq!(parse_frame("7E8"), None);
        assert_eq!(parse_frame("7E80"), None); // half a byte
        assert_eq!(parse_frame("7E8ZZ"), None);
        assert_eq!(parse_frame("7E8 00 01 02 03 04 05 06 07 08"), None); // nine data bytes
    }

    #[test]
    fn error_lines_become_errors_and_no_data_is_empty() {
        for bad in [
            "CAN ERROR",
            "BUFFER FULL",
            "?",
            "UNABLE TO CONNECT",
            "<DATA ERROR",
        ] {
            assert!(parse_lines(&format!("{bad}\r\r>"), "X").is_err(), "{bad}");
        }
        assert_eq!(
            parse_lines("NO DATA\r\r>", "X").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            parse_lines("0100\rSEARCHING...\r7E8 03 41\r\r>", "0100").unwrap(),
            vec!["7E8 03 41".to_string()]
        );
    }

    fn adapter(sim: Simulator) -> Elm327<Elm327Emulator<Simulator>> {
        Elm327::new(Elm327Emulator::new(sim))
    }

    #[test]
    fn init_configures_the_adapter_in_order() {
        let mut a = adapter(Simulator::new());
        a.init(0x7E0, 0x7E8).unwrap();
        assert_eq!(a.version(), Some("ELM327 v1.5"));
        assert_eq!(
            a.port.log,
            [
                "ATZ", "ATE0", "ATL0", "ATS0", "ATH1", "ATCAF0", "ATSP6", "ATST20", "ATCRA7E8",
                "ATSH7E0"
            ]
        );
    }

    #[test]
    fn rejects_extended_ids_and_bad_lengths_without_touching_the_adapter() {
        let mut a = adapter(Simulator::new());
        a.init(0x7E0, 0x7E8).unwrap();
        let before = a.port.log.len();
        let ext = Frame::Can {
            id: 0x18DA00F1,
            extended: true,
            data: vec![1],
        };
        assert!(a.send(&ext).is_err());
        assert!(a.send(&Frame::can(0x7E0, &[])).is_err());
        assert!(a.send(&Frame::can(0x7E0, &[0; 9])).is_err());
        assert!(a.send(&Frame::Raw(vec![1])).is_err());
        assert!(a.init(0x1234, 0x7E8).is_err());
        assert_eq!(a.port.log.len(), before);
    }

    #[test]
    fn a_non_elm_device_is_refused() {
        // An adapter that answers reset with something else entirely.
        struct Silent;
        impl SerialIo for Silent {
            fn write_all(&mut self, _: &[u8]) -> io::Result<()> {
                Ok(())
            }
            fn read(&mut self, _: &mut [u8], _: Duration) -> io::Result<usize> {
                Err(io::ErrorKind::TimedOut.into())
            }
            fn set_baud(&mut self, _: u32) -> io::Result<()> {
                Ok(())
            }
            fn set_break(&mut self, _: bool) -> io::Result<()> {
                Ok(())
            }
            fn clear_input(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let started = Instant::now();
        let err = Elm327::new(Silent).init(0x7E0, 0x7E8).unwrap_err();
        assert!(matches!(err, Error::Timeout), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(6));
    }

    #[test]
    fn a_clone_lacking_a_command_gives_a_clear_error() {
        let mut a = Elm327::new(Elm327Emulator::new(Simulator::new()).without_command("ATCAF"));
        let e = a.init(0x7E0, 0x7E8).unwrap_err().to_string();
        assert!(e.contains("ATCAF0") && e.contains("may not support"), "{e}");
    }

    #[test]
    fn frames_round_trip_through_the_adapter() {
        let sim = Simulator::new().on(
            Frame::can(0x7E0, &[0x02, 0x3E, 0x00]),
            vec![Frame::can(0x7E8, &[0x02, 0x7E, 0x00])],
        );
        let mut a = adapter(sim);
        a.init(0x7E0, 0x7E8).unwrap();
        a.send(&Frame::can(0x7E0, &[0x02, 0x3E, 0x00])).unwrap();
        assert_eq!(
            a.recv(Duration::ZERO).unwrap(),
            Frame::can(0x7E8, &[0x02, 0x7E, 0x00])
        );
        // Nothing else is queued, and unanswered requests time out.
        assert!(matches!(a.recv(Duration::ZERO), Err(Error::Timeout)));
        a.send(&Frame::can(0x7E0, &[0x02, 0x10, 0x03])).unwrap();
        assert!(matches!(a.recv(Duration::ZERO), Err(Error::Timeout)));
    }

    #[test]
    fn k_line_operations_are_refused() {
        let mut a = adapter(Simulator::new());
        assert!(a.set_baud(10400).is_err());
        assert!(a.slow_init(1).is_err());
        let c = a.capabilities();
        assert!(c.can && !c.kline && !c.slow_init && !c.fast_init);
    }
}
