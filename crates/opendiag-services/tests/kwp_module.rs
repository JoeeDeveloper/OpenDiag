//! KWP2000 modules (K-line fast init and TP2.0 on CAN) through the `Module` interface,
//! including the guarded fault-code clear.

use std::cell::RefCell;
use std::io::{self, Write};
use std::path::PathBuf;
use std::rc::Rc;

use opendiag_core::WriteMode;
use opendiag_proto::clock::FakeClock;
use opendiag_proto::kwp::sim::KwpSim;
use opendiag_proto::kwp::{KwpClient, KwpConfig, Parser};
use opendiag_proto::tp20::sim::Tp20Sim;
use opendiag_proto::tp20::{Tp20Config, Tp20KwpClient, Tp20Link};
use opendiag_services::demo::{virtual_kwp_bus, virtual_kwp_module, virtual_kwp_module_over};
use opendiag_services::{
    apply_clear, plan_clear, BackupStatus, BackupStore, ClearConfirmation, KwpModule, Module,
};
use opendiag_transport::{parse_trace, Frame, TraceEvent, Tracing};

fn temp_store(name: &str) -> BackupStore {
    let path: PathBuf =
        std::env::temp_dir().join(format!("opendiag-kwp-{}-{name}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    BackupStore::new(path)
}

#[test]
fn connecting_runs_the_session_handshake_in_order_without_mistakes() {
    let m = virtual_kwp_module().unwrap();
    let sim = m.service().transport();
    assert_eq!(sim.fast_inits(), 1);
    assert_eq!(sim.requests(), &[vec![0x81], vec![0x10, 0x89]]);
    assert!(sim.is_started());
    assert!(sim.violations().is_empty(), "{:?}", sim.violations());
}

#[test]
fn identification_is_shown_raw_and_no_part_number_is_claimed() {
    let mut m = virtual_kwp_module().unwrap();
    let ident = m.identify().unwrap();
    assert_eq!(ident.lines, vec!["VIRTUAL KWP2000 ECU"]);
    assert_eq!(
        ident.part_number, None,
        "the field layout is unknown, so nothing is parsed"
    );

    // Non-printable bytes are masked, and a module without identification gives no lines.
    let client = KwpClient::new(
        KwpSim::new(1).ident(&[0x00, b'1', b'J', 0x03, b'0', 0xFF]),
        KwpConfig::new(1),
    );
    let mut raw = start_over(client);
    assert_eq!(raw.identify().unwrap().lines, vec!["1J.0"]);
    let mut none = start_over(KwpClient::new(KwpSim::new(1), KwpConfig::new(1)));
    assert!(none.identify().unwrap().lines.is_empty());
}

fn start_over(mut kwp: KwpClient<KwpSim>) -> KwpModule<KwpClient<KwpSim>> {
    kwp.start_communication().unwrap();
    KwpModule::start(kwp, 1, "kwp2000").unwrap()
}

#[test]
fn fault_codes_are_five_digit_decimals_with_the_raw_status_byte() {
    let mut m = virtual_kwp_module().unwrap();
    let dtcs = m.read_dtcs().unwrap();
    assert_eq!(
        dtcs.iter().map(|d| d.text.as_str()).collect::<Vec<_>>(),
        ["12345", "54321"]
    );
    assert_eq!(dtcs[0].status, 0x20);
    assert!(
        dtcs[0].flags.is_empty(),
        "no guessed meaning for status bytes"
    );
}

#[test]
fn the_guarded_clear_snapshots_clears_verifies_and_relocks_over_k_line() {
    let store = temp_store("kline");
    let mut m = virtual_kwp_module().unwrap();
    let shown = plan_clear(m.faults()).unwrap();
    assert_eq!(shown.len(), 2);
    let outcome = apply_clear(
        m.faults(),
        &store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    )
    .unwrap();

    assert!(outcome.remaining.is_empty());
    assert!(m.service().transport().cleared());
    assert_eq!(m.service().write_mode(), WriteMode::ReadOnly, "relocked");
    assert!(m.service().transport().violations().is_empty());
    assert!(m.read_dtcs().unwrap().is_empty());

    let rec = store.get(&outcome.backup_id).unwrap().unwrap();
    assert_eq!((rec.protocol.as_str(), rec.module_address), ("kwp2000", 1));
    assert_eq!(rec.module.as_deref(), Some("KWP2000 module 0x01"));
    assert_eq!(rec.codes, vec!["12345 status 0x20", "54321 status 0x01"]);
    assert_eq!(rec.status, BackupStatus::Verified);
    let _ = std::fs::remove_file(store.path());
}

#[derive(Clone, Default)]
struct Shared(Rc<RefCell<Vec<u8>>>);
impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn dropping_the_module_sends_stop_communication() {
    let buf = Shared::default();
    let m = virtual_kwp_module_over(Tracing::new(virtual_kwp_bus(), buf.clone())).unwrap();
    drop(m);
    let text = String::from_utf8(buf.0.borrow().clone()).unwrap();
    let sent: Vec<Vec<u8>> = parse_trace(&text)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.event {
            TraceEvent::Tx(Frame::Raw(b)) => Some(b),
            _ => None,
        })
        .map(|bytes| {
            let mut p = Parser::new();
            p.push(&bytes);
            p.next_message().unwrap().unwrap().data
        })
        .collect();
    assert_eq!(sent.last(), Some(&vec![0x82]), "{sent:02X?}");
}

fn tp20_module(sim: Tp20Sim) -> KwpModule<Tp20KwpClient<Tp20Sim, FakeClock>> {
    let link = Tp20Link::open(sim, FakeClock::new(), &Tp20Config::new(1)).unwrap();
    KwpModule::start(Tp20KwpClient::new(link), 1, "kwp2000-tp20").unwrap()
}

#[test]
fn kwp_over_tp20_identifies_reads_codes_and_reports_active_faults_after_a_clear() {
    let store = temp_store("tp20");
    let sim = Tp20Sim::new(1)
        .respond(&[0x10, 0x89], &[0x50, 0x89])
        .respond(&[0x1A, 0x9B], &[0x5A, 0x9B, b'A', b'B'])
        .respond(&[0x18, 0x00, 0xFF, 0x00], &[0x58, 0x01, 0x30, 0x39, 0x20])
        .respond(&[0x14, 0xFF, 0x00], &[0x54, 0xFF, 0x00]);
    let mut m = tp20_module(sim);
    assert_eq!(m.identify().unwrap().lines, vec!["AB"]);
    let shown = plan_clear(m.faults()).unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].text, "12345", "0x3039");

    // This scripted gateway keeps reporting the code, like an active fault.
    let outcome = apply_clear(
        m.faults(),
        &store,
        &shown,
        &ClearConfirmation::for_codes(&shown),
    )
    .unwrap();
    assert_eq!(outcome.remaining.len(), 1);
    let rec = store.get(&outcome.backup_id).unwrap().unwrap();
    assert_eq!(rec.protocol, "kwp2000-tp20");
    assert_eq!(rec.module.as_deref(), Some("KWP2000/TP2.0 module 0x01"));
    assert_eq!(rec.status, BackupStatus::VerificationFailed);
    assert!(
        rec.note.as_deref().unwrap().contains("still reported"),
        "the failure text must not erase which module this was"
    );
    assert_eq!(m.service().write_mode(), WriteMode::ReadOnly);
    assert!(m.service().link().transport().violations().is_empty());
    let _ = std::fs::remove_file(store.path());
}

#[test]
fn a_module_that_refuses_the_session_or_the_identification_is_still_usable() {
    // The scripted gateway answers unknown services with serviceNotSupported (0x11).
    let sim = Tp20Sim::new(1).respond(&[0x18, 0x00, 0xFF, 0x00], &[0x58, 0x00]);
    let mut m = tp20_module(sim);
    assert!(m.identify().unwrap().lines.is_empty());
    assert!(m.read_dtcs().unwrap().is_empty());
}
