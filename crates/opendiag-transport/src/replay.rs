//! Replay: feeds frames from a recorded candump log back through `recv`.

use std::collections::VecDeque;
use std::time::Duration;

use opendiag_core::{Error, Result};

use crate::{Capabilities, Frame, Transport};

/// Plays back a capture. `recv` yields the logged frames in order, then
/// times out. Frames sent by the code under test are recorded in `sent`.
#[derive(Debug)]
pub struct Replay {
    frames: VecDeque<Frame>,
    sent: Vec<Frame>,
}

impl Replay {
    pub fn from_frames(frames: Vec<Frame>) -> Self {
        Self {
            frames: frames.into(),
            sent: Vec::new(),
        }
    }

    /// Parse a `candump -L` log: `(timestamp) iface ID#DATA`, one frame per line.
    /// Blank lines and lines starting with `#` are skipped.
    pub fn from_candump(text: &str) -> Result<Self> {
        let mut frames = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            frames.push(
                parse_candump_line(line)
                    .map_err(|e| Error::Transport(format!("candump line {}: {e}", n + 1)))?,
            );
        }
        Ok(Self::from_frames(frames))
    }

    pub fn sent(&self) -> &[Frame] {
        &self.sent
    }

    pub fn remaining(&self) -> usize {
        self.frames.len()
    }
}

fn parse_candump_line(line: &str) -> std::result::Result<Frame, String> {
    let mut parts = line.split_whitespace();
    let (_ts, _iface, frame) = (parts.next(), parts.next(), parts.next());
    let frame = frame.ok_or("expected `(ts) iface ID#DATA`")?;
    let (id_str, data_str) = frame.split_once('#').ok_or("missing `#` separator")?;
    let id = u32::from_str_radix(id_str, 16).map_err(|_| format!("bad id `{id_str}`"))?;
    // candump writes 8 hex digits for 29-bit identifiers, 3 for 11-bit.
    let extended = id_str.len() > 3;
    if data_str.len() % 2 != 0 {
        return Err(format!("odd-length data `{data_str}`"));
    }
    let data = (0..data_str.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&data_str[i..i + 2], 16))
        .collect::<std::result::Result<Vec<u8>, _>>()
        .map_err(|_| format!("bad data `{data_str}`"))?;
    Ok(Frame::Can { id, extended, data })
}

impl Transport for Replay {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.sent.push(frame.clone());
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Frame> {
        self.frames.pop_front().ok_or(Error::Timeout)
    }

    fn set_baud(&mut self, _baud: u32) -> Result<()> {
        Ok(())
    }

    fn slow_init(&mut self, _address: u8) -> Result<()> {
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can: true,
            ..Capabilities::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Duration = Duration::from_millis(100);

    #[test]
    fn parses_standard_and_extended_frames() {
        let log = "# comment\n(1.0) can0 7E8#025003\n\n(2.0) can0 18DAF110#0102\n";
        let mut r = Replay::from_candump(log).unwrap();
        assert_eq!(r.recv(T).unwrap(), Frame::can(0x7E8, &[0x02, 0x50, 0x03]));
        assert_eq!(
            r.recv(T).unwrap(),
            Frame::Can {
                id: 0x18DA_F110,
                extended: true,
                data: vec![1, 2]
            }
        );
        assert!(matches!(r.recv(T), Err(Error::Timeout)));
    }

    #[test]
    fn rejects_malformed_lines_with_line_number() {
        let err = Replay::from_candump("(1.0) can0 7E8#0\n").unwrap_err();
        assert!(err.to_string().contains("line 1"), "{err}");
        assert!(Replay::from_candump("(1.0) can0 ZZZ#00\n").is_err());
        assert!(Replay::from_candump("(1.0) can0 7E8\n").is_err());
    }

    #[test]
    fn records_sent_frames() {
        let mut r = Replay::from_frames(vec![]);
        r.send(&Frame::can(1, &[2])).unwrap();
        assert_eq!(r.sent(), &[Frame::can(1, &[2])]);
    }
}
