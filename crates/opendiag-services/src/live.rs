//! Live data: read measuring-block groups at a steady rate and log them as CSV.
//!
//! Values are logged raw as `(formula, a, b)` cells. Converting them to
//! engineering units needs a per-formula table, which belongs in the open data
//! format rather than being guessed here.

use std::io::Write;
use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_proto::clock::Clock;
use opendiag_proto::kw1281::RawMeasurement;

/// A module that can report measuring-block groups.
pub trait LiveData {
    fn read_group(&mut self, group: u8) -> Result<Vec<RawMeasurement>>;
}

/// When to stop logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// After this many sampling rounds.
    Samples(u32),
    /// Once a sampling round would start at or after this much logged time.
    Duration(Duration),
}

#[derive(Debug, Clone)]
pub struct LogConfig {
    /// Groups to read each round, in order.
    pub groups: Vec<u8>,
    /// Time between the start of one round and the next.
    pub interval: Duration,
    pub limit: Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogSummary {
    pub samples: u32,
    pub rows: u64,
}

/// CSV column header. One row per cell, so groups of any size share one layout.
pub const CSV_HEADER: &str = "time_ms,group,cell,formula,a,b";

fn io_err(e: std::io::Error) -> Error {
    Error::Io(e.to_string())
}

/// Run `round` on a fixed time grid until `limit`; `round` gets the logged time in ms and
/// returns how many rows it wrote.
///
/// Rounds start on a grid (`interval` apart) so timestamps do not drift. If a round takes
/// longer than the interval, the missed rounds are skipped rather than fired back to back.
pub(crate) fn run_rounds<C: Clock>(
    clock: &C,
    interval: Duration,
    limit: Limit,
    mut round: impl FnMut(u128) -> Result<u64>,
) -> Result<LogSummary> {
    if interval.is_zero() {
        return Err(Error::Protocol(
            "live data: interval must be above zero".into(),
        ));
    }
    let start = clock.now();
    let mut summary = LogSummary {
        samples: 0,
        rows: 0,
    };
    let mut tick: u32 = 0;
    // A schedule too far out to represent is the end of any real log.
    while let Some(scheduled) = interval.checked_mul(tick) {
        match limit {
            Limit::Samples(n) if summary.samples >= n => break,
            Limit::Duration(d) if scheduled >= d => break,
            _ => {}
        }
        clock.wait_until(start + scheduled);
        let time_ms = (clock.now() - start).as_millis();
        summary.rows += round(time_ms)?;
        summary.samples += 1;

        // Next grid point after "now"; never fire missed rounds back to back.
        let elapsed = clock.now() - start;
        let after_now = u32::try_from(elapsed.as_nanos() / interval.as_nanos())
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        tick = tick.saturating_add(1).max(after_now);
    }
    Ok(summary)
}

/// Log `cfg.groups` from `source` to `out` as CSV at a fixed rate.
///
/// Output is flushed after every round, so a failure part-way through still leaves the
/// earlier rows in the file.
pub fn log_csv<L, C, W>(
    source: &mut L,
    clock: &C,
    cfg: &LogConfig,
    out: &mut W,
) -> Result<LogSummary>
where
    L: LiveData + ?Sized,
    C: Clock,
    W: Write,
{
    if cfg.groups.is_empty() {
        return Err(Error::Protocol("live data: no groups to log".into()));
    }
    if cfg.interval.is_zero() {
        return Err(Error::Protocol(
            "live data: interval must be above zero".into(),
        ));
    }
    writeln!(out, "{CSV_HEADER}").map_err(io_err)?;
    run_rounds(clock, cfg.interval, cfg.limit, |time_ms| {
        let mut rows = 0;
        for &group in &cfg.groups {
            let cells = source.read_group(group)?;
            for (i, c) in cells.iter().enumerate() {
                writeln!(
                    out,
                    "{time_ms},{group},{},{},{},{}",
                    i + 1,
                    c.formula,
                    c.a,
                    c.b
                )
                .map_err(io_err)?;
                rows += 1;
            }
        }
        out.flush().map_err(io_err)?;
        Ok(rows)
    })
}
