//! The property that matters most for anything that writes:
//!
//!   A modifying request never reaches a module unless a durable backup of the
//!   change already exists on disk, whatever the traffic looks like.
//!
//! The virtual modules call a hook the instant a write arrives; the hook looks at
//! the backup file right then. The guarded flows are run many times over a
//! transport that corrupts, truncates, duplicates and invents frames, so the
//! check also covers confused and partial sessions. Everything is seeded.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use opendiag_core::Result;
use opendiag_proto::clock::FakeClock;
use opendiag_proto::isotp::{Config, IsoTpLink};
use opendiag_proto::kw1281::sim::Kw1281Sim;
use opendiag_proto::kw1281::{Kw1281Client, Kw1281Config};
use opendiag_proto::kwp::sim::KwpSim;
use opendiag_proto::kwp::{KwpClient, KwpConfig};
use opendiag_proto::tp20::sim::Tp20Sim;
use opendiag_proto::tp20::{Tp20Config, Tp20KwpClient, Tp20Link};
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_services::{
    apply_change, apply_clear, plan_change, plan_clear, BackupStore, ClearConfirmation,
    Kw1281Module, KwpModule, Module, WriteConfirmation,
};
use opendiag_transport::{Capabilities, Frame, Simulator, Transport};

const SEEDS: u64 = 250;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Damages what a transport receives (see `robustness.rs` in the proto crate).
struct Mutating<T: Transport> {
    inner: T,
    rng: Rng,
    rate: u64,
    queue: VecDeque<Frame>,
    budget: u32,
}

impl<T: Transport> Mutating<T> {
    fn new(inner: T, seed: u64, rate: u64) -> Self {
        Self {
            inner,
            rng: Rng(seed),
            rate,
            queue: VecDeque::new(),
            budget: 6000,
        }
    }

    fn damage(&mut self, mut f: Frame) -> Frame {
        let duplicate = f.clone();
        let data = match &mut f {
            Frame::Raw(d) => d,
            Frame::Can { data, .. } => data,
        };
        match self.rng.below(6) {
            0 if !data.is_empty() => {
                let i = self.rng.below(data.len() as u64) as usize;
                data[i] ^= 1 << self.rng.below(8);
            }
            1 => data.truncate(self.rng.below(data.len() as u64 + 1) as usize),
            2 => data.push(self.rng.next() as u8),
            3 => self.queue.push_back(duplicate),
            4 => {
                *data = (0..self.rng.below(9))
                    .map(|_| self.rng.next() as u8)
                    .collect()
            }
            _ => data.iter_mut().for_each(|b| *b = 0xFF),
        }
        f
    }
}

impl<T: Transport> Transport for Mutating<T> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.inner.send(frame)
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame> {
        if self.budget == 0 {
            return Err(opendiag_core::Error::Timeout);
        }
        self.budget -= 1;
        if let Some(f) = self.queue.pop_front() {
            return Ok(f);
        }
        let f = self.inner.recv(timeout)?;
        if self.rng.below(100) < self.rate {
            Ok(self.damage(f))
        } else {
            Ok(f)
        }
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.inner.set_baud(baud)
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        self.inner.slow_init(address)
    }

    fn fast_init(&mut self) -> Result<()> {
        self.inner.fast_init()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

/// Everything the hook observed, shared with the test.
#[derive(Clone, Default)]
struct Watch {
    writes: Rc<Cell<u32>>,
    violations: Rc<RefCell<Vec<String>>>,
}

impl Watch {
    /// A hook that, on every write, requires a record with `needle` in the backup file.
    fn hook(&self, path: &Path, needle: &'static str) -> impl FnMut(&str) + 'static {
        let (writes, violations, path) = (
            Rc::clone(&self.writes),
            Rc::clone(&self.violations),
            path.to_path_buf(),
        );
        move |what: &str| {
            writes.set(writes.get() + 1);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let ok = text
                .lines()
                .any(|l| l.contains("\"status\":\"planned\"") && l.contains(needle));
            if !ok {
                violations.borrow_mut().push(format!(
                    "`{what}` reached the module with no matching backup on disk"
                ));
            }
        }
    }

    fn assert_clean(&self, what: &str) {
        let v = self.violations.borrow();
        assert!(
            v.is_empty(),
            "{what}: {} violation(s), e.g. {:?}",
            v.len(),
            v.first()
        );
    }
}

fn temp_store(name: &str, seed: u64) -> BackupStore {
    let path: PathBuf = std::env::temp_dir().join(format!(
        "opendiag-ws-{}-{name}-{seed}.jsonl",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    BackupStore::new(path)
}

/// A store that cannot be written (its directory does not exist).
fn unwritable_store() -> BackupStore {
    BackupStore::new("no-such-directory-xyz/backups.jsonl")
}

fn rate(seed: u64) -> u64 {
    // From clean traffic up to heavily damaged traffic.
    [0, 3, 8, 15, 30][(seed % 5) as usize]
}

fn run_kw1281_clear(store: &BackupStore, seed: u64, watch: &Watch) {
    let sim = Kw1281Sim::new()
        .ident_line("X")
        .fault(525, 0x20)
        .fault(17, 0x01)
        .on_write(watch.hook(store.path(), "\"kind\":\"dtc_clear\""));
    let t = Mutating::new(sim, seed, rate(seed));
    let client = Kw1281Client::new(t, FakeClock::new(), Kw1281Config::default());
    let Ok(mut m) = Kw1281Module::connect(client, 1) else {
        return;
    };
    let Ok(shown) = plan_clear(m.faults()) else {
        return;
    };
    let _ = apply_clear(
        m.faults(),
        store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    );
}

fn run_kwp_clear(store: &BackupStore, seed: u64, watch: &Watch) {
    let sim = KwpSim::new(1)
        .ident(b"ID")
        .fault(12345, 0x20)
        .on_write(watch.hook(store.path(), "\"kind\":\"dtc_clear\""));
    let t = Mutating::new(sim, seed, rate(seed));
    let mut kwp = KwpClient::new(t, KwpConfig::new(1));
    if kwp.start_communication().is_err() {
        return;
    }
    let Ok(mut m) = KwpModule::start(kwp, 1, "kwp2000") else {
        return;
    };
    let Ok(shown) = plan_clear(m.faults()) else {
        return;
    };
    let _ = apply_clear(
        m.faults(),
        store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    );
}

fn run_kw1281_adaptation(store: &BackupStore, seed: u64, watch: &Watch) {
    let sim = Kw1281Sim::new()
        .ident_line("X")
        .adaptation(4, 100)
        .on_write(watch.hook(
            store.path(),
            "\"kind\":\"adaptation\",\"channel\":4,\"old_value\":100,\"new_value\":200",
        ));
    let t = Mutating::new(sim, seed, rate(seed));
    let client = Kw1281Client::new(t, FakeClock::new(), Kw1281Config::default());
    let Ok(mut m) = Kw1281Module::connect(client, 1) else {
        return;
    };
    let Ok(change) = plan_change(m.adaptation().unwrap(), 4, 200) else {
        return;
    };
    let confirmation = WriteConfirmation::for_change(&change);
    let _ = apply_change(
        m.adaptation().unwrap(),
        store,
        &change,
        12345,
        &confirmation,
    );
}

/// A scripted `Simulator` that calls a hook when a ClearDiagnosticInformation request
/// (SID 0x14, in a single ISO-TP frame) is sent to it.
struct HookedSim {
    inner: Simulator,
    hook: Box<dyn FnMut(&str)>,
}

impl Transport for HookedSim {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        if let Frame::Can { data, .. } = frame {
            if data.get(1) == Some(&0x14) {
                (self.hook)("clear");
            }
        }
        self.inner.send(frame)
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame> {
        self.inner.recv(timeout)
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.inner.set_baud(baud)
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        self.inner.slow_init(address)
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

fn run_uds_clear(store: &BackupStore, seed: u64, watch: &Watch) {
    // One stored code (P0171), a positive reply to the clear; scripted, so the code
    // "comes back" like an active fault (that outcome is irrelevant to this property).
    let req = |p: &[u8]| {
        let mut d = vec![p.len() as u8];
        d.extend_from_slice(p);
        Frame::can(0x7E0, &d)
    };
    let rsp = |p: &[u8]| {
        let mut d = vec![p.len() as u8];
        d.extend_from_slice(p);
        vec![Frame::can(0x7E8, &d)]
    };
    let sim = Simulator::new()
        .on(
            req(&[0x19, 0x02, 0xFF]),
            rsp(&[0x59, 0x02, 0xFF, 0x01, 0x71, 0x00, 0x09]),
        )
        .on(req(&[0x14, 0xFF, 0xFF, 0xFF]), rsp(&[0x54]));
    let sim = HookedSim {
        inner: sim,
        hook: Box::new(watch.hook(store.path(), "\"kind\":\"dtc_clear\"")),
    };
    let t = Mutating::new(sim, seed, rate(seed));
    let link = IsoTpLink::new(t, FakeClock::new(), 0x7E0, 0x7E8, Config::default());
    let mut uds = UdsClient::new(link, UdsConfig::default());
    let Ok(shown) = plan_clear(uds.faults()) else {
        return;
    };
    let _ = apply_clear(
        uds.faults(),
        store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    );
}

fn run_tp20_clear(store: &BackupStore, seed: u64, watch: &Watch) {
    let sim = Tp20Sim::new(1)
        .block_size(2)
        .respond(&[0x10, 0x89], &[0x50, 0x89])
        .respond(&[0x18, 0x00, 0xFF, 0x00], &[0x58, 0x01, 0x30, 0x39, 0x20])
        .respond(&[0x14, 0xFF, 0x00], &[0x54, 0xFF, 0x00])
        .on_write(watch.hook(store.path(), "\"kind\":\"dtc_clear\""));
    let t = Mutating::new(sim, seed, rate(seed));
    let Ok(link) = Tp20Link::open(t, FakeClock::new(), &Tp20Config::new(1)) else {
        return;
    };
    let Ok(mut m) = KwpModule::start(Tp20KwpClient::new(link), 1, "kwp2000-tp20") else {
        return;
    };
    let Ok(shown) = plan_clear(m.faults()) else {
        return;
    };
    let _ = apply_clear(
        m.faults(),
        store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    );
}

type Run = fn(&BackupStore, u64, &Watch);

const FLOWS: [(&str, Run); 5] = [
    ("clear over KW1281", run_kw1281_clear),
    ("clear over KWP2000 (K-line)", run_kwp_clear),
    ("adaptation over KW1281", run_kw1281_adaptation),
    ("clear over UDS (ISO-TP)", run_uds_clear),
    ("clear over KWP2000 over TP2.0", run_tp20_clear),
];

#[test]
fn no_write_reaches_a_module_without_a_durable_backup_even_under_corrupted_traffic() {
    for (name, run) in FLOWS {
        let watch = Watch::default();
        for seed in 0..SEEDS {
            let store = temp_store("good", seed);
            run(&store, seed, &watch);
            let _ = std::fs::remove_file(store.path());
        }
        watch.assert_clean(name);
        // Guard against a vacuous pass: the flows really did get as far as writing.
        assert!(
            watch.writes.get() >= 40,
            "{name}: only {} write(s) reached the module in {SEEDS} runs; the test is not exercising writes",
            watch.writes.get()
        );
    }
}

#[test]
fn if_the_backup_cannot_be_written_no_write_ever_reaches_the_module() {
    for (name, run) in FLOWS {
        let watch = Watch::default();
        for seed in 0..SEEDS {
            run(&unwritable_store(), seed, &watch);
        }
        assert_eq!(
            watch.writes.get(),
            0,
            "{name}: a write was sent although nothing could be backed up"
        );
    }
}

#[test]
fn the_hook_really_fires_and_catches_a_missing_backup() {
    // Sanity check of the checker itself: with no guarded flow in between, a direct
    // write DOES trip the hook. (Otherwise the tests above could never fail.)
    let watch = Watch::default();
    let store = temp_store("checker", 0);
    let sim = KwpSim::new(1)
        .fault(1, 1)
        .on_write(watch.hook(store.path(), "\"kind\":\"dtc_clear\""));
    let mut kwp = KwpClient::new(sim, KwpConfig::new(1));
    kwp.start_communication().unwrap();
    use opendiag_proto::kwp::KwpService;
    kwp.set_write_mode(opendiag_core::WriteMode::Unlocked);
    kwp.clear_dtcs(0xFF00).unwrap(); // bypasses the guarded flow on purpose
    assert_eq!(watch.writes.get(), 1);
    assert_eq!(
        watch.violations.borrow().len(),
        1,
        "an unguarded write must be flagged"
    );
}
