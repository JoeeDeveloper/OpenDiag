//! Shared types for OpenDiag.

use std::fmt;

/// Errors common to all OpenDiag layers.
#[derive(Debug)]
pub enum Error {
    /// A transport-level failure (adapter missing, I/O, timeout).
    Transport(String),
    /// A protocol-level failure (malformed frame, negative response).
    Protocol(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(m) => write!(f, "transport error: {m}"),
            Error::Protocol(m) => write!(f, "protocol error: {m}"),
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
