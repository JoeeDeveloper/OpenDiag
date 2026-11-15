//! ECU scan through the gateway: ask each candidate address whether an ECU
//! answers, and count its stored fault codes. Read-only (TesterPresent and
//! ReadDTCInformation only).
//!
//! How a missing ECU shows up is partly NOT VERIFIED: we treat both silence
//! and an HSFZ error frame from the gateway (our mock sends one) as "absent".
//! A real gateway may behave differently; the first scan of a real car will tell.

use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_proto::clock::Clock;
use opendiag_proto::uds::{UdsClient, UdsConfig};

use crate::gateway::{ByteStream, HsfzGateway};

/// What a scan found at one address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    pub address: u8,
    pub present: bool,
    /// Why it counts as present or absent, for display.
    pub detail: String,
    /// Stored fault codes, when the ECU answered the read.
    pub dtc_count: Option<usize>,
}

/// Probe each address. Fatal errors (the connection itself failing) abort the scan.
pub fn scan<S: ByteStream, C: Clock>(
    gateway: &HsfzGateway<S, C>,
    addresses: &[u8],
    probe_timeout: Duration,
) -> Result<Vec<ScanEntry>> {
    let cfg = UdsConfig {
        p2: probe_timeout,
        ..UdsConfig::default()
    };
    let mut out = Vec::with_capacity(addresses.len());
    for &address in addresses {
        let mut uds = UdsClient::new(gateway.link(address), cfg.clone());
        let (present, detail) = match uds.tester_present() {
            Ok(()) => (true, "answered".to_string()),
            // A negative response is still an answer: the ECU is there.
            Err(Error::NegativeResponse { nrc, .. }) => {
                (true, format!("answered (negative response {nrc:#04X})"))
            }
            Err(Error::Timeout) => (false, "no answer".to_string()),
            Err(Error::Protocol(m)) if m.starts_with("hsfz: gateway reported") => {
                (false, m.trim_start_matches("hsfz: ").to_string())
            }
            Err(e) => return Err(e),
        };
        let dtc_count = if present {
            uds.read_dtcs_by_status_mask(0xFF).ok().map(|v| v.len())
        } else {
            None
        };
        out.push(ScanEntry {
            address,
            present,
            detail,
            dtc_count,
        });
    }
    Ok(out)
}
