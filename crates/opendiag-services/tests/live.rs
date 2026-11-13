use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_proto::clock::{Clock, FakeClock};
use opendiag_proto::kw1281::RawMeasurement;
use opendiag_services::demo::virtual_kw1281_module;
use opendiag_services::{log_csv, Limit, LiveData, LogConfig, Module, CSV_HEADER};

const MS: Duration = Duration::from_millis(1);

fn cfg(groups: &[u8], interval_ms: u64, limit: Limit) -> LogConfig {
    LogConfig {
        groups: groups.to_vec(),
        interval: Duration::from_millis(interval_ms),
        limit,
    }
}

/// A source whose reads take a fixed amount of fake time and return a counter.
struct Counter<'a> {
    clock: &'a FakeClock,
    cost: Duration,
    reads: u8,
}

impl LiveData for Counter<'_> {
    fn read_group(&mut self, group: u8) -> Result<Vec<RawMeasurement>> {
        self.clock.advance(self.cost);
        self.reads += 1;
        if group == 99 {
            return Err(Error::Protocol("no such group".into()));
        }
        Ok(vec![RawMeasurement {
            formula: group,
            a: self.reads,
            b: 0,
        }])
    }
}

fn run(cost_ms: u64, cfg: &LogConfig) -> (Result<opendiag_services::LogSummary>, Vec<String>) {
    let clock = FakeClock::new();
    let mut src = Counter {
        clock: &clock,
        cost: Duration::from_millis(cost_ms),
        reads: 0,
    };
    let mut out = Vec::new();
    let r = log_csv(&mut src, &clock, cfg, &mut out);
    let lines = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    (r, lines)
}

#[test]
fn writes_header_and_one_row_per_cell_on_a_fixed_grid() {
    let (r, lines) = run(1, &cfg(&[1], 100, Limit::Samples(3)));
    let s = r.unwrap();
    assert_eq!((s.samples, s.rows), (3, 3));
    assert_eq!(lines[0], CSV_HEADER);
    // Round n starts at n*100 ms regardless of the 1 ms read cost: no drift.
    assert_eq!(lines[1], "0,1,1,1,1,0");
    assert_eq!(lines[2], "100,1,1,1,2,0");
    assert_eq!(lines[3], "200,1,1,1,3,0");
}

#[test]
fn several_groups_share_a_timestamp_per_round() {
    let (r, lines) = run(0, &cfg(&[1, 2], 50, Limit::Samples(2)));
    assert_eq!(r.unwrap().rows, 4);
    assert_eq!(
        &lines[1..],
        ["0,1,1,1,1,0", "0,2,1,2,2,0", "50,1,1,1,3,0", "50,2,1,2,4,0"]
    );
}

#[test]
fn slow_reads_skip_missed_rounds_instead_of_bursting() {
    // Each read costs 25 ms against a 10 ms interval.
    let (r, lines) = run(25, &cfg(&[1], 10, Limit::Samples(4)));
    assert_eq!(r.unwrap().samples, 4);
    let times: Vec<u128> = lines[1..]
        .iter()
        .map(|l| l.split(',').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(times, vec![0, 30, 60, 90], "rounds stay on the 10 ms grid");
}

#[test]
fn duration_limit_stops_on_schedule() {
    let (r, lines) = run(0, &cfg(&[1], 100, Limit::Duration(350 * MS)));
    // Rounds at 0, 100, 200, 300; the one at 400 would be past the limit.
    assert_eq!(r.unwrap().samples, 4);
    assert_eq!(lines.len(), 5);
}

#[test]
fn a_read_error_keeps_the_rows_already_written() {
    let (r, lines) = run(0, &cfg(&[1, 99], 100, Limit::Samples(5)));
    assert!(matches!(r, Err(Error::Protocol(_))));
    assert_eq!(lines, vec![CSV_HEADER, "0,1,1,1,1,0"]);
}

#[test]
fn rejects_bad_configurations() {
    assert!(run(0, &cfg(&[], 100, Limit::Samples(1))).0.is_err());
    assert!(run(0, &cfg(&[1], 0, Limit::Samples(1))).0.is_err());
}

#[test]
fn a_failing_writer_reports_an_io_error() {
    struct Broken;
    impl std::io::Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let clock = FakeClock::new();
    let mut src = Counter {
        clock: &clock,
        cost: MS,
        reads: 0,
    };
    let err = log_csv(
        &mut src,
        &clock,
        &cfg(&[1], 10, Limit::Samples(1)),
        &mut Broken,
    )
    .unwrap_err();
    assert!(matches!(err, Error::Io(m) if m.contains("disk full")));
}

#[test]
fn the_virtual_kw1281_module_logs_changing_values_end_to_end() {
    let mut m = virtual_kw1281_module().unwrap();
    let clock = FakeClock::new();
    let mut out = Vec::new();
    let live = m.live().expect("KW1281 supports live data");
    let s = log_csv(
        live,
        &clock,
        &cfg(&[1, 2], 200, Limit::Samples(3)),
        &mut out,
    )
    .unwrap();
    assert_eq!(s.samples, 3);
    let text = String::from_utf8(out).unwrap();
    // Group 1 has 2 cells, group 2 has 1: 3 rows per round.
    assert_eq!(text.lines().count(), 1 + 3 * 3);
    assert!(text.contains(",1,1,1,16,32\n"), "{text}"); // 0x10, 0x20
    assert!(text.contains(",1,1,1,16,40\n"), "{text}"); // 0x10, 0x28: value changed
    assert!(m.client().transport().violations().is_empty());
    let _ = clock.now();
}
