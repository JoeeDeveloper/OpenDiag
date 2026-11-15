//! Vehicle discovery over UDP port 6811.
//!
//! VERIFIED: discovery is a UDP broadcast to port 6811 and the gateway answers
//! with an HSFZ frame of control word `0x0011` ("vehicle ident data") that
//! carries VIN, diagnostic address, IP and MAC (Quantex HSFZ page, Scapy).
//! Scapy's dissector reads a `0x0011` frame with length 0 as the *request*.
//!
//! NOT VERIFIED (flagged): the exact ASCII layout of the identification
//! string. [`parse_ident_string`] expects the form
//! `DIAGADR<hex>BMWMAC<12 hex>BMWVIN<17 chars>` as remembered from BMW
//! captures, and degrades to "raw text only" when it does not match, so a
//! different layout shows up as an unparsed string rather than wrong data.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use opendiag_core::{Error, Result};

use crate::hsfz::{take_frame, HsfzFrame, CTRL_VEHICLE_IDENT, UDP_PORT};

/// A gateway that answered discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub ip: IpAddr,
    /// The identification text exactly as received.
    pub raw: String,
    pub vin: Option<String>,
    pub diag_address: Option<u8>,
    pub mac: Option<String>,
}

/// Fields found in an identification string. Best effort, see the module docs.
pub fn parse_ident_string(s: &str) -> (Option<u8>, Option<String>, Option<String>) {
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    let between = |from: &str, to: Option<&str>| -> Option<String> {
        let start = s.find(from)? + from.len();
        let rest = &s[start..];
        let end = match to {
            Some(t) => rest.find(t)?,
            None => rest.len(),
        };
        Some(rest[..end].trim().to_string()).filter(|v| !v.is_empty())
    };
    let addr = between("DIAGADR", Some("BMWMAC"))
        .and_then(|h| u8::from_str_radix(h.trim_start_matches("0x"), 16).ok());
    let mac = between("BMWMAC", Some("BMWVIN")).map(|m| m.replace([':', '-'], ""));
    let vin = between("BMWVIN", None);
    (addr, mac, vin)
}

/// Broadcast target for real use: every host on the local network, port 6811.
pub fn broadcast_target() -> SocketAddr {
    SocketAddr::from(([255, 255, 255, 255], UDP_PORT))
}

/// Send a discovery request to `target` and collect every answer for `wait`.
pub fn discover(target: SocketAddr, wait: Duration) -> Result<Vec<Discovered>> {
    let sock = UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| Error::Transport(format!("binding a UDP socket: {e}")))?;
    sock.set_broadcast(true)
        .map_err(|e| Error::Transport(format!("enabling broadcast: {e}")))?;
    let request = HsfzFrame::new(CTRL_VEHICLE_IDENT, Vec::new()).encode();
    sock.send_to(&request, target)
        .map_err(|e| Error::Transport(format!("sending discovery to {target}: {e}")))?;

    let deadline = Instant::now() + wait;
    let mut found: Vec<Discovered> = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        sock.set_read_timeout(Some(left.max(Duration::from_millis(1))))
            .map_err(|e| Error::Transport(format!("set_read_timeout: {e}")))?;
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break
            }
            Err(e) => return Err(Error::Transport(format!("receiving discovery: {e}"))),
        };
        let mut datagram = buf[..n].to_vec();
        let Ok(Some(frame)) = take_frame(&mut datagram) else {
            continue; // not HSFZ
        };
        if frame.control != CTRL_VEHICLE_IDENT || frame.payload.is_empty() {
            continue; // our own request echoed back, or something else
        }
        let raw = String::from_utf8_lossy(&frame.payload).into_owned();
        let (diag_address, mac, vin) = parse_ident_string(&raw);
        let d = Discovered {
            ip: from.ip(),
            raw,
            vin,
            diag_address,
            mac,
        };
        if !found.contains(&d) {
            found.push(d);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_expected_layout() {
        let (a, m, v) = parse_ident_string("DIAGADR10BMWMAC001A37B44210BMWVINWBASIMULATED00001");
        assert_eq!(a, Some(0x10));
        assert_eq!(m.as_deref(), Some("001A37B44210"));
        assert_eq!(v.as_deref(), Some("WBASIMULATED00001"));
    }

    #[test]
    fn an_unexpected_layout_yields_nothing_rather_than_guesses() {
        assert_eq!(parse_ident_string("hello world"), (None, None, None));
    }

    #[test]
    fn tolerates_padding_and_mac_separators() {
        let (a, m, v) =
            parse_ident_string("DIAGADR0x1ABMWMAC00:1A:37:B4:42:10BMWVIN WBASIMULATED00001\0\0");
        assert_eq!(a, Some(0x1A));
        assert_eq!(m.as_deref(), Some("001A37B44210"));
        assert_eq!(v.as_deref(), Some("WBASIMULATED00001"));
    }
}
