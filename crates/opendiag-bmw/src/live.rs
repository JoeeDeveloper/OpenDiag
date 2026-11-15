//! Live data by data identifier: read a list of DIDs at a steady rate and log
//! them as CSV. Values are logged raw (hex). Turning them into engineering
//! units needs per-DID definitions we do not have verified, so none are guessed.
//!
//! Read-only: only ReadDataByIdentifier is sent.

use std::io::Write;
use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_proto::clock::Clock;
use opendiag_proto::link::MessageLink;
use opendiag_proto::uds::UdsClient;

pub const CSV_HEADER: &str = "time_ms,did,status,hex";

#[derive(Debug, Clone)]
pub struct DidLogConfig {
    pub dids: Vec<u16>,
    /// Time between the start of one round and the next.
    pub interval: Duration,
    pub samples: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DidLogSummary {
    pub samples: u32,
    pub rows: u64,
}

fn io_err(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// Log `cfg.dids` to `out`. Rounds start on a fixed grid so timestamps do not
/// drift; output is flushed after every round, so a failure part-way through
/// keeps the earlier rows. An ECU that says a DID does not exist, or refuses
/// it, gets a row saying so; a lost connection or timeout stops the log.
pub fn log_dids_csv<L: MessageLink, C: Clock, W: Write>(
    uds: &mut UdsClient<L>,
    clock: &C,
    cfg: &DidLogConfig,
    out: &mut W,
) -> Result<DidLogSummary> {
    writeln!(out, "{CSV_HEADER}").map_err(io_err)?;
    let start = clock.now();
    let mut rows = 0u64;
    let mut samples = 0u32;
    for round in 0..cfg.samples {
        clock.wait_until(start + cfg.interval * round);
        let ms = clock.now().saturating_sub(start).as_millis();
        for &did in &cfg.dids {
            match uds.read_data_by_identifier(did) {
                Ok(d) => writeln!(out, "{ms},{did:#06X},ok,{}", hex(&d)),
                Err(Error::NegativeResponse { nrc: 0x31, .. }) => {
                    writeln!(out, "{ms},{did:#06X},absent,")
                }
                Err(Error::NegativeResponse { nrc, .. }) => {
                    writeln!(out, "{ms},{did:#06X},nrc_{nrc:#04X},")
                }
                Err(e) => {
                    let _ = out.flush();
                    return Err(e);
                }
            }
            .map_err(io_err)?;
            rows += 1;
        }
        out.flush().map_err(io_err)?;
        samples += 1;
    }
    Ok(DidLogSummary { samples, rows })
}
