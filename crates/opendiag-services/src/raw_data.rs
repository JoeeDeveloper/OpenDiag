//! Raw data by identifier: read what a module returns for an identifier and log it as hex,
//! without interpreting it.
//!
//! Over KWP2000 this is ReadDataByLocalIdentifier (0x21) and over UDS ReadDataByIdentifier
//! (0x22). The layout of the returned bytes is module-specific and, for VAG measuring blocks
//! over KWP2000, not documented in a public source we could cite. So nothing is decoded here:
//! the bytes are logged as they arrive, and a decoder belongs in the open data format once a
//! source or a real capture exists.

use std::io::Write;

use opendiag_core::{Error, Result};
use opendiag_proto::clock::Clock;

use crate::live::{run_rounds, Limit, LogSummary};

/// A module that returns raw bytes for an identifier.
pub trait RawData {
    /// What an identifier is called on this protocol, for messages.
    fn id_kind(&self) -> &'static str;
    /// Read one identifier. Identifiers the protocol cannot express are an error.
    fn read_raw(&mut self, id: u16) -> Result<Vec<u8>>;
}

#[derive(Debug, Clone)]
pub struct RawLogConfig {
    pub ids: Vec<u16>,
    pub interval: std::time::Duration,
    pub limit: Limit,
}

/// One row per identifier read: the bytes have no known structure to split into cells.
pub const RAW_CSV_HEADER: &str = "time_ms,id,length,hex";

fn io_err(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Log `cfg.ids` from `source` to `out` as CSV at a fixed rate (same schedule as
/// [`log_csv`](crate::log_csv): a fixed grid, missed rounds skipped, flushed every round).
pub fn log_raw_csv<R, C, W>(
    source: &mut R,
    clock: &C,
    cfg: &RawLogConfig,
    out: &mut W,
) -> Result<LogSummary>
where
    R: RawData + ?Sized,
    C: Clock,
    W: Write,
{
    if cfg.ids.is_empty() {
        return Err(Error::Protocol("raw data: no identifiers to log".into()));
    }
    writeln!(out, "{RAW_CSV_HEADER}").map_err(io_err)?;
    run_rounds(clock, cfg.interval, cfg.limit, |time_ms| {
        let mut rows = 0;
        for &id in &cfg.ids {
            let data = source.read_raw(id)?;
            writeln!(out, "{time_ms},{id:#06X},{},{}", data.len(), hex(&data)).map_err(io_err)?;
            rows += 1;
        }
        out.flush().map_err(io_err)?;
        Ok(rows)
    })
}
