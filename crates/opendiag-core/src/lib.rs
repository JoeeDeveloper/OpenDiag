//! Shared types for OpenDiag.

use std::fmt;

/// Errors common to all OpenDiag layers.
#[derive(Debug)]
pub enum Error {
    /// A transport-level failure (adapter missing, I/O, timeout).
    Transport(String),
    /// A protocol-level failure (malformed frame, negative response).
    Protocol(String),
    /// No frame arrived within the allotted time.
    Timeout,
    /// The ECU refused a request (UDS/KWP negative response).
    NegativeResponse { service: u8, nrc: u8 },
    /// A write was attempted while the session is read-only.
    WritesLocked(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(m) => write!(f, "transport error: {m}"),
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
            Error::Timeout => write!(f, "timed out waiting for a frame"),
            Error::NegativeResponse { service, nrc } => {
                write!(f, "negative response {nrc:#04x} to service {service:#04x}")
            }
            Error::WritesLocked(what) => {
                write!(f, "writes are locked (read-only mode): {what}")
            }
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Whether the session may perform writes (coding, adaptation, output tests).
/// Defaults to read-only; see CLAUDE.md ground rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WriteMode {
    #[default]
    ReadOnly,
    Unlocked,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_mode_defaults_to_read_only() {
        assert_eq!(WriteMode::default(), WriteMode::ReadOnly);
    }
}
