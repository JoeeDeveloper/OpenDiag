//! `opendiag bmw ...`: BMW F-series over an ENET cable (HSFZ). Read-only,
//! except `dtc clear`, which goes through the guarded clear flow.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use clap::{Args, Subcommand};
use opendiag_bmw::ecus;
use opendiag_bmw::hsfz::{DEFAULT_GATEWAY, TCP_PORT};
use opendiag_bmw::mock::{MockGateway, VirtualVehicle};
use opendiag_bmw::{
    broadcast_target, discover, log_dids_csv, read_did, read_ident, scan, BmwEcu, DidLogConfig,
    Discovered, HsfzGateway, HsfzLink, TcpByteStream,
};
use opendiag_proto::clock::SystemClock;
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_services::{apply_clear, plan_clear, ClearConfirmation, DtcReport, Module};

use crate::{backup_store, confirm, StoreArg};

#[derive(Subcommand)]
pub enum BmwCmd {
    /// Find BMW gateways on the network (UDP broadcast, port 6811).
    Discover {
        /// Send the request here instead of the broadcast address.
        #[arg(long)]
        to: Option<SocketAddr>,
        /// How long to listen for answers, in milliseconds.
        #[arg(long, default_value_t = 2000)]
        wait_ms: u64,
        /// Use the built-in virtual BMW (no car needed).
        #[arg(long)]
        simulate: bool,
    },
    /// Read VIN, I-level and vehicle order (FA, raw) from the gateway.
    Ident(Conn),
    /// Read one data identifier from an ECU and print it as hex and text.
    Did {
        #[command(flatten)]
        conn: Conn,
        /// Data identifier in hex, e.g. 0xF190.
        #[arg(value_parser = parse_hex_u16)]
        did: u16,
    },
    /// Find which ECUs answer behind the gateway, and how many fault codes each holds.
    Scan {
        #[command(flatten)]
        conn: Conn,
        /// Probe every address 0x00-0xFF instead of just the known ones (slower).
        #[arg(long)]
        all: bool,
        /// How long to wait for each address, in milliseconds.
        #[arg(long, default_value_t = 300)]
        probe_ms: u64,
    },
    /// Read or clear fault codes of one ECU (default: the gateway; pick with --target).
    Dtc {
        #[command(subcommand)]
        action: BmwDtc,
    },
    /// Log data identifiers from one ECU as raw CSV at a steady rate.
    Live {
        #[command(flatten)]
        conn: Conn,
        /// Data identifiers in hex, comma separated, e.g. 0x4002,0xF190.
        #[arg(long, value_delimiter = ',', required = true, value_parser = parse_hex_u16)]
        did: Vec<u16>,
        /// Time between rounds, in milliseconds.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        /// Number of rounds.
        #[arg(long, default_value_t = 10)]
        samples: u32,
        /// Write the CSV here instead of standard output.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum BmwDtc {
    /// List the stored fault codes (read-only).
    Read(Conn),
    /// Clear all stored fault codes. Saves a snapshot of them first, asks you to
    /// confirm exactly what was shown, then checks what the ECU reports. There is no --yes.
    Clear {
        #[command(flatten)]
        conn: Conn,
        #[command(flatten)]
        store: StoreArg,
    },
}

#[derive(Args)]
pub struct Conn {
    /// Gateway IP address (TCP 6801). Omit to discover it on the network.
    #[arg(long)]
    ip: Option<IpAddr>,
    /// Gateway TCP port.
    #[arg(long, default_value_t = TCP_PORT)]
    port: u16,
    /// Use the built-in virtual BMW (no car needed).
    #[arg(long)]
    simulate: bool,
    /// Diagnostic address of the ECU to talk to, in hex (default: the gateway).
    #[arg(long, value_parser = crate::parse_hex_u8)]
    target: Option<u8>,
    /// Tester address, in hex.
    #[arg(long, default_value = "0xF4", value_parser = crate::parse_hex_u8)]
    tester: u8,
}

fn parse_hex_u16(s: &str) -> Result<u16, String> {
    u16::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16)
        .map_err(|e| e.to_string())
}

const NOTICE: &str =
    "BMW support is simulator-tested only: it has not been run against a real car. \
It only reads. Keep the ignition on and a battery charger connected.";

pub fn run(cmd: BmwCmd) -> Result<(), String> {
    match cmd {
        BmwCmd::Discover {
            to,
            wait_ms,
            simulate,
        } => discover_cmd(to, wait_ms, simulate),
        BmwCmd::Ident(c) => ident_cmd(&c),
        BmwCmd::Did { conn, did } => did_cmd(&conn, did),
        BmwCmd::Scan {
            conn,
            all,
            probe_ms,
        } => scan_cmd(&conn, all, probe_ms),
        BmwCmd::Dtc {
            action: BmwDtc::Read(conn),
        } => dtc_read_cmd(&conn),
        BmwCmd::Dtc {
            action: BmwDtc::Clear { conn, store },
        } => dtc_clear_cmd(&conn, &store),
        BmwCmd::Live {
            conn,
            did,
            interval_ms,
            samples,
            out,
        } => live_cmd(&conn, did, interval_ms, samples, out),
    }
}

fn print_found(found: &[Discovered]) {
    for d in found {
        println!("gateway {}", d.ip);
        println!(
            "  VIN:        {}",
            d.vin.as_deref().unwrap_or("(not parsed)")
        );
        match d.diag_address {
            Some(a) => println!("  diag addr:  {a:#04X}"),
            None => println!("  diag addr:  (not parsed)"),
        }
        println!(
            "  MAC:        {}",
            d.mac.as_deref().unwrap_or("(not parsed)")
        );
        if d.vin.is_none() && d.diag_address.is_none() && d.mac.is_none() {
            println!("  raw reply:  {:?}", d.raw);
        }
    }
}

fn discover_cmd(to: Option<SocketAddr>, wait_ms: u64, simulate: bool) -> Result<(), String> {
    let _mock;
    let target = if simulate {
        eprintln!("[simulated] virtual BMW gateway");
        let m = MockGateway::start(VirtualVehicle::demo()).map_err(|e| e.to_string())?;
        let t = m.udp_addr();
        _mock = Some(m);
        t
    } else {
        _mock = None;
        to.unwrap_or_else(broadcast_target)
    };
    let found = discover(target, Duration::from_millis(wait_ms)).map_err(|e| e.to_string())?;
    if found.is_empty() {
        println!("No gateway answered. Check the ENET cable, that the ignition is on, and that");
        println!("your network adapter is on the same link-local network (169.254.x.x).");
    } else {
        print_found(&found);
    }
    Ok(())
}

struct Session {
    gateway: HsfzGateway<TcpByteStream, SystemClock>,
    target: u8,
    _mock: Option<MockGateway>,
}

fn connect(c: &Conn) -> Result<Session, String> {
    let mut discovered_addr = None;
    let (addr, mock) = if c.simulate {
        eprintln!("[simulated] virtual BMW gateway");
        let m = MockGateway::start(VirtualVehicle::demo()).map_err(|e| e.to_string())?;
        (m.tcp_addr(), Some(m))
    } else {
        eprintln!("{NOTICE}");
        let ip = match c.ip {
            Some(ip) => ip,
            None => {
                eprintln!("Looking for a gateway on the network...");
                let found = discover(broadcast_target(), Duration::from_secs(2))
                    .map_err(|e| e.to_string())?;
                match found.as_slice() {
                    [] => return Err("no gateway answered; pass --ip if you know it".into()),
                    [one] => {
                        discovered_addr = one.diag_address;
                        one.ip
                    }
                    many => {
                        print_found(many);
                        return Err("several gateways answered; pick one with --ip".into());
                    }
                }
            }
        };
        (SocketAddr::new(ip, c.port), None)
    };
    let stream = TcpByteStream::connect(addr).map_err(|e| e.to_string())?;
    Ok(Session {
        gateway: HsfzGateway::new(stream, SystemClock::new(), c.tester),
        target: c.target.or(discovered_addr).unwrap_or(DEFAULT_GATEWAY),
        _mock: mock,
    })
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn ident_cmd(c: &Conn) -> Result<(), String> {
    let s = connect(c)?;
    let mut uds = UdsClient::new(s.gateway.link(s.target), UdsConfig::default());
    let id = read_ident(&mut uds).map_err(|e| e.to_string())?;
    println!("ECU {:#04X}", s.target);
    println!(
        "  VIN:      {}",
        id.vin.as_deref().unwrap_or("(not available)")
    );
    match (&id.i_level_raw, id.i_level_text()) {
        (_, Some(t)) => println!("  I-level:  {t}"),
        (Some(raw), None) => println!("  I-level:  raw {}", hex(raw)),
        (None, _) => println!("  I-level:  (not available)"),
    }
    match &id.fa_raw {
        Some(raw) => println!("  FA (raw): {} (not decoded)", hex(raw)),
        None => println!("  FA:       (not available)"),
    }
    println!("The I-level and FA data identifiers are unverified guesses (docs/bmw.md).");
    Ok(())
}

fn did_cmd(c: &Conn, did: u16) -> Result<(), String> {
    let s = connect(c)?;
    let mut uds = UdsClient::new(s.gateway.link(s.target), UdsConfig::default());
    match read_did(&mut uds, did).map_err(|e| e.to_string())? {
        None => println!("ECU {:#04X} does not have DID {did:#06X}", s.target),
        Some(d) => {
            println!("ECU {:#04X} DID {did:#06X}: {}", s.target, hex(&d));
            let text: String = d
                .iter()
                .map(|b| {
                    if (0x20..0x7F).contains(b) {
                        *b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            println!("text: {text}");
        }
    }
    Ok(())
}

fn scan_cmd(c: &Conn, all: bool, probe_ms: u64) -> Result<(), String> {
    let s = connect(c)?;
    let addresses: Vec<u8> = if all {
        (0..=0xFFu8).filter(|a| *a != c.tester).collect()
    } else {
        ecus::known_addresses()
    };
    let found =
        scan(&s.gateway, &addresses, Duration::from_millis(probe_ms)).map_err(|e| e.to_string())?;
    let present: Vec<_> = found.iter().filter(|e| e.present).collect();
    if present.is_empty() {
        println!("No ECU answered. Check the ENET cable and that the ignition is on.");
        return Ok(());
    }
    println!("{} ECU(s) answered:", present.len());
    for e in present {
        let name = ecus::name(e.address).unwrap_or("?");
        let desc = ecus::description(e.address).unwrap_or("not in the address table");
        let codes = match e.dtc_count {
            Some(0) => "no fault codes".to_string(),
            Some(n) => format!("{n} fault code(s)"),
            None => "fault codes unreadable".to_string(),
        };
        println!("  {:#04X}  {name:<6} {desc}: {codes}", e.address);
    }
    println!("Names come from a short unverified table (docs/bmw.md); they are labels only.");
    Ok(())
}

type CliEcu = BmwEcu<HsfzLink<TcpByteStream, SystemClock>>;

fn ecu_of(s: &Session) -> CliEcu {
    BmwEcu::new(s.gateway.link(s.target), s.target, UdsConfig::default())
}

fn print_bmw_dtcs(dtcs: &[DtcReport]) {
    if dtcs.is_empty() {
        println!("No fault codes stored.");
        return;
    }
    for d in dtcs {
        println!(
            "  {}  status {:#04X}  {}",
            d.text,
            d.status,
            d.flags.join(", ")
        );
    }
}

fn dtc_read_cmd(c: &Conn) -> Result<(), String> {
    let s = connect(c)?;
    let mut ecu = ecu_of(&s);
    println!("ECU {:#04X}", s.target);
    let dtcs = ecu.read_dtcs().map_err(|e| e.to_string())?;
    print_bmw_dtcs(&dtcs);
    println!("Codes are shown as raw hex; no descriptions are bundled (docs/bmw.md).");
    Ok(())
}

fn dtc_clear_cmd(c: &Conn, store: &StoreArg) -> Result<(), String> {
    let store = backup_store(store);
    let s = connect(c)?;
    let mut ecu = ecu_of(&s);
    let shown = plan_clear(ecu.faults()).map_err(|e| e.to_string())?;
    if shown.is_empty() {
        println!(
            "ECU {:#04X} has no fault codes stored; nothing to clear.",
            s.target
        );
        return Ok(());
    }
    println!(
        "About to clear {} fault code(s) in ECU {:#04X}:",
        shown.len(),
        s.target
    );
    print_bmw_dtcs(&shown);
    println!(
        "Clearing erases these codes and their status for good and cannot be undone. A snapshot of them is saved to {} first.",
        store.path().display()
    );
    if !c.simulate {
        println!("Keep the ignition on and a battery charger connected until this finishes.");
    }
    if !confirm()? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    let confirmation = ClearConfirmation::for_codes(&shown);
    let outcome =
        apply_clear(ecu.faults(), &store, &shown, &confirmation).map_err(|e| e.to_string())?;
    println!(
        "Cleared {} fault code(s). Snapshot {} is in {}.",
        outcome.cleared.len(),
        outcome.backup_id,
        store.path().display()
    );
    if outcome.remaining.is_empty() {
        println!("The ECU now reports no fault codes.");
    } else {
        println!(
            "Note: {} code(s) are still reported (active faults come straight back):",
            outcome.remaining.len()
        );
        print_bmw_dtcs(&outcome.remaining);
    }
    Ok(())
}

fn live_cmd(
    c: &Conn,
    dids: Vec<u16>,
    interval_ms: u64,
    samples: u32,
    out: Option<std::path::PathBuf>,
) -> Result<(), String> {
    let s = connect(c)?;
    let mut uds = UdsClient::new(s.gateway.link(s.target), UdsConfig::default());
    let cfg = DidLogConfig {
        dids,
        interval: Duration::from_millis(interval_ms),
        samples,
    };
    let clock = SystemClock::new();
    let summary = match out {
        Some(path) => {
            let mut f = std::fs::File::create(&path)
                .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
            log_dids_csv(&mut uds, &clock, &cfg, &mut f)
        }
        None => log_dids_csv(&mut uds, &clock, &cfg, &mut std::io::stdout().lock()),
    }
    .map_err(|e| e.to_string())?;
    eprintln!(
        "Logged {} round(s), {} row(s). Values are raw hex, not converted to units.",
        summary.samples, summary.rows
    );
    Ok(())
}
