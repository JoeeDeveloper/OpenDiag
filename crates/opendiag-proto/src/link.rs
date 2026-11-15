//! A message-level channel to one ECU.
//!
//! UDS needs to send a request and receive a whole reply message. How that
//! message is carried is not UDS's business: ISO-TP segments it over CAN
//! frames, while BMW's HSFZ carries it whole inside a TCP packet. This trait
//! is the seam between the two.

use std::time::Duration;

use opendiag_core::Result;

/// Sends and receives whole diagnostic messages to a single ECU.
pub trait MessageLink {
    /// Send one message (starting with the service id).
    fn send(&mut self, payload: &[u8]) -> Result<()>;
    /// Receive one message, or `Error::Timeout` if none arrives in `timeout`.
    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>>;
    /// Short human-readable description of the peer, for backup records.
    fn describe(&self) -> String;
}
