//! `opendiag bmw ...`: BMW F-series over an ENET cable (HSFZ). Read-only,
//! except `dtc clear`, which goes through the guarded clear flow.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use clap::{Args, Subcommand};
use opendiag_bmw::ecus;
use opendiag_bmw::hsfz::{DEFAULT_GATEWAY, TCP_PORT};
use opendiag_bmw::mock::{MockGateway, VirtualVehicle};
use opendiag_bmw::BmwVehicle;
use opendiag_bmw::{
    broadcast_target, discover, log_dids_csv, read_did, scan, BmwEcu, DidLogConfig, Discovered,
    HsfzGateway, HsfzLink, TcpByteStream,
};
use opendiag_bmw_coding::{decode, encode_change, CodingSource, SyntheticSource};
use opendiag_proto::clock::SystemClock;
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_services::{
    apply_block_change, apply_clear, plan_block_change, plan_block_restore, plan_clear, unhex,
    BlockAccess, BlockChange, BlockConfirmation, ClearConfirmation, DtcReport, EcuEntry, Module,
    Vehicle, VehicleModule,
};

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
    /// Read, write or restore a raw data block (coding data) of one ECU. Writes are
    /// guarded: backup first, typed confirmation, read-back check. No --yes.
    Block {
        #[command(subcommand)]
        action: BmwBlock,
    },
    /// Show or change a block by field name, using a definition file in OpenDiag's own
    /// format (docs/examples/bmw-coding-synthetic.json). Changes go through `block write`'s
    /// guarded flow. BMW's own files are not read.
    Coding {
        #[command(subcommand)]
        action: BmwCoding,
    },
    /// Serve a read-only web page for one ECU on 127.0.0.1 (this computer only).
    Serve {
        #[command(flatten)]
        conn: Conn,
        /// Port to listen on (default: any free port).
        #[arg(long, default_value_t = 0)]
        listen_port: u16,
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

#[derive(Args)]
pub struct BlockConn {
    #[command(flatten)]
    conn: Conn,
    /// Open the extended diagnostic session (0x10 03) first. Whether an F-series
    /// ECU needs it is unverified, so it is off by default.
    #[arg(long)]
    extended_session: bool,
}

#[derive(Args)]
pub struct CodingArgs {
    #[command(flatten)]
    bc: BlockConn,
    /// Data identifier of the block, in hex.
    #[arg(long, value_parser = parse_hex_u16)]
    did: u16,
    /// Definition file in the `opendiag-bmw-coding/0` format.
    #[arg(long)]
    coding_file: std::path::PathBuf,
    /// Software level of the ECU, if the definitions name one. Must match exactly.
    #[arg(long)]
    i_level: Option<String>,
}

#[derive(Subcommand)]
pub enum BmwCoding {
    /// Print every field of the block by name (read-only).
    Show(CodingArgs),
    /// Change one field by name. Prints the named change and the byte diff, then asks for YES.
    Set {
        #[command(flatten)]
        args: CodingArgs,
        /// Field name.
        #[arg(long)]
        field: String,
        /// A label from the definitions, or a number (decimal or 0x hex).
        #[arg(long)]
        value: String,
        #[command(flatten)]
        store: StoreArg,
    },
}

#[derive(Subcommand)]
pub enum BmwBlock {
    /// Print one block as hex (read-only).
    Read {
        #[command(flatten)]
        bc: BlockConn,
        /// Data identifier of the block, in hex.
        #[arg(long, value_parser = parse_hex_u16)]
        did: u16,
    },
    /// Replace one block with new hex bytes of the same length.
    Write {
        #[command(flatten)]
        bc: BlockConn,
        #[arg(long, value_parser = parse_hex_u16)]
        did: u16,
        /// The complete new block as hex, e.g. 010203AA05060708.
        #[arg(long)]
        hex: String,
        #[command(flatten)]
        store: StoreArg,
    },
    /// Put a block back from a backup id (see `opendiag backup list`).
    Restore {
        #[command(flatten)]
        bc: BlockConn,
        /// Backup id, e.g. b0003.
        id: String,
        #[command(flatten)]
        store: StoreArg,
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

impl Conn {
    /// Default options, used when `--brand bmw` routes another command here.
    pub fn routed(simulate: bool) -> Self {
        Self {
            ip: None,
            port: TCP_PORT,
            simulate,
            target: None,
            tester: opendiag_bmw::hsfz::DEFAULT_TESTER,
        }
    }
}

/// How many gateways answer on the local network right now (UDP discovery, about
/// 1.5 s). Used by `--brand auto`; sends only the discovery broadcast.
pub fn count_gateways() -> usize {
    discover(broadcast_target(), Duration::from_millis(1500)).map_or(0, |v| v.len())
}

fn serve_cmd(c: &Conn, listen_port: u16) -> Result<(), String> {
    let s = connect(c)?;
    let simulated = c.simulate;
    let connection = if simulated {
        "Simulated: built-in virtual BMW (HSFZ)".to_string()
    } else {
        format!(
            "BMW ENET (HSFZ), ECU {:#04X}. Experimental: simulator-tested only",
            s.target
        )
    };
    // `_mock` stays alive until the server stops (it owns the simulator, when there is one).
    let (v, _mock) = vehicle_of(s);
    let id = v.default_ecu();
    let module = VehicleModule::new(Box::new(v), id).map_err(|e| e.to_string())?;
    crate::serve_module(Box::new(module), connection, simulated, listen_port)
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
        BmwCmd::Block { action } => block_cmd(action),
        BmwCmd::Coding { action } => coding_cmd(action),
        BmwCmd::Serve { conn, listen_port } => serve_cmd(&conn, listen_port),
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

fn vehicle_of(s: Session) -> (BmwVehicle<TcpByteStream, SystemClock>, Option<MockGateway>) {
    let Session {
        gateway,
        target,
        _mock,
    } = s;
    (
        BmwVehicle::new(
            gateway,
            target,
            format!("BMW ENET (HSFZ), ECU {target:#04X}"),
        ),
        _mock,
    )
}

fn ident_cmd(c: &Conn) -> Result<(), String> {
    let (mut v, _mock) = vehicle_of(connect(c)?);
    let id = v.identify().map_err(|e| e.to_string())?;
    println!("ECU {:#04X}", v.default_ecu());
    println!(
        "  VIN:      {}",
        id.vin.as_deref().unwrap_or("(not available)")
    );
    for l in &id.lines {
        let note = if l.verified { "" } else { "  [unverified]" };
        println!("  {}: {}{note}", l.label, l.value);
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
    if !all {
        // The brand-neutral path: ask the vehicle which ECUs it has.
        let (mut v, _mock) = vehicle_of(s);
        let found = v.scan().map_err(|e| e.to_string())?;
        return print_vehicle_scan(&found);
    }
    let addresses: Vec<u8> = (0..=0xFFu8).filter(|a| *a != c.tester).collect();
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

fn print_vehicle_scan(found: &[EcuEntry]) -> Result<(), String> {
    if found.is_empty() {
        println!("No ECU answered. Check the ENET cable and that the ignition is on.");
        return Ok(());
    }
    println!("{} ECU(s) answered:", found.len());
    for e in found {
        let desc = ecus::description(e.id).unwrap_or("not in the address table");
        let codes = match e.dtc_count {
            Some(0) => "no fault codes".to_string(),
            Some(n) => format!("{n} fault code(s)"),
            None => "fault codes unreadable".to_string(),
        };
        println!(
            "  {:#04X}  {:<6} {desc}: {codes}",
            e.id,
            e.name.as_deref().unwrap_or("?")
        );
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

fn block_ecu(bc: &BlockConn) -> Result<(CliEcu, Session), String> {
    let s = connect(&bc.conn)?;
    let mut ecu = ecu_of(&s);
    if bc.extended_session {
        ecu = ecu.with_extended_session();
    }
    Ok((ecu, s))
}

fn hex_str(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

fn block_cmd(action: BmwBlock) -> Result<(), String> {
    match action {
        BmwBlock::Read { bc, did } => {
            let (mut ecu, s) = block_ecu(&bc)?;
            let b = ecu.read_block(did).map_err(|e| e.to_string())?;
            println!(
                "ECU {:#04X} block {did:#06X} ({} bytes): {}",
                s.target,
                b.len(),
                hex_str(&b)
            );
            Ok(())
        }
        BmwBlock::Write {
            bc,
            did,
            hex,
            store,
        } => {
            let new = unhex(hex.trim()).ok_or("--hex must be an even number of hex digits")?;
            let (mut ecu, s) = block_ecu(&bc)?;
            let change = plan_block_change(&mut ecu, did, new).map_err(|e| e.to_string())?;
            block_apply(
                &mut ecu,
                &s,
                &backup_store(&store),
                &change,
                bc.conn.simulate,
            )
        }
        BmwBlock::Restore { bc, id, store } => {
            let store = backup_store(&store);
            let (mut ecu, s) = block_ecu(&bc)?;
            let change = plan_block_restore(&mut ecu, &store, &id).map_err(|e| e.to_string())?;
            block_apply(&mut ecu, &s, &store, &change, bc.conn.simulate)
        }
    }
}

fn block_apply(
    ecu: &mut CliEcu,
    s: &Session,
    store: &opendiag_services::BackupStore,
    change: &BlockChange,
    simulate: bool,
) -> Result<(), String> {
    println!(
        "ECU {:#04X} block {:#06X} ({} bytes)",
        s.target,
        change.id,
        change.old.len()
    );
    println!("  now: {}", hex_str(&change.old));
    println!("  new: {}", hex_str(&change.new));
    let diff = change.differing_offsets();
    if change.old.len() == change.new.len() {
        println!("  {} byte(s) differ, at offset(s) {:?}", diff.len(), diff);
    }
    if change.old == change.new {
        println!("The new block equals the current one; nothing to do.");
        return Ok(());
    }
    println!(
        "The whole current block is saved to {} first, and the ECU is read back afterwards.",
        store.path().display()
    );
    println!(
        "Coding errors can make an ECU misbehave. This has only been tested against a simulator."
    );
    if !simulate {
        println!("Keep the ignition on and a battery charger connected until this finishes.");
    }
    if !confirm()? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    let done = apply_block_change(ecu, store, change, &BlockConfirmation::for_change(change))
        .map_err(|e| e.to_string())?;
    println!(
        "Written and verified by reading it back. Backup {} is in {}.",
        done.backup_id,
        store.path().display()
    );
    println!(
        "To undo: opendiag bmw block restore {} (same connection options).",
        done.backup_id
    );
    Ok(())
}

fn show_fields(fields: &[opendiag_bmw_coding::DecodedField]) {
    for f in fields {
        println!(
            "  {:<16} = {} ({})",
            f.name,
            f.label.as_deref().unwrap_or("no label"),
            f.raw
        );
    }
}

/// Load definitions and read the block twice: if two reads differ the data is not
/// stable and nothing is decoded.
fn coding_context(
    a: &CodingArgs,
) -> Result<(CliEcu, Session, opendiag_bmw_coding::BlockLayout, Vec<u8>), String> {
    let text = std::fs::read_to_string(&a.coding_file)
        .map_err(|e| format!("cannot read {}: {e}", a.coding_file.display()))?;
    let source = SyntheticSource::from_json(&text).map_err(|e| e.to_string())?;
    eprintln!("Definitions: {}", source.provenance);
    let (mut ecu, s) = block_ecu(&a.bc)?;
    let layout = source
        .layout(s.target, a.i_level.as_deref(), a.did)
        .map_err(|e| e.to_string())?
        .clone();
    let first = ecu.read_block(a.did).map_err(|e| e.to_string())?;
    let second = ecu.read_block(a.did).map_err(|e| e.to_string())?;
    if first != second {
        return Err("two consecutive reads of the block differ; not decoding unstable data".into());
    }
    Ok((ecu, s, layout, first))
}

fn coding_cmd(action: BmwCoding) -> Result<(), String> {
    match action {
        BmwCoding::Show(a) => {
            let (_, s, layout, block) = coding_context(&a)?;
            println!(
                "ECU {:#04X} block {:#06X}: {}",
                s.target,
                a.did,
                hex_str(&block)
            );
            show_fields(&decode(&layout, &block).map_err(|e| e.to_string())?);
            Ok(())
        }
        BmwCoding::Set {
            args,
            field,
            value,
            store,
        } => {
            let (mut ecu, s, layout, block) = coding_context(&args)?;
            let new = encode_change(&layout, &block, &field, &value).map_err(|e| e.to_string())?;
            let before = decode(&layout, &block).map_err(|e| e.to_string())?;
            let after = decode(&layout, &new).map_err(|e| e.to_string())?;
            for (b, a) in before.iter().zip(&after) {
                if b.raw != a.raw {
                    println!(
                        "Field {}: {} -> {}",
                        b.name,
                        b.label.as_deref().map_or(b.raw.to_string(), str::to_string),
                        a.label.as_deref().map_or(a.raw.to_string(), str::to_string)
                    );
                }
            }
            let change = plan_block_change(&mut ecu, args.did, new).map_err(|e| e.to_string())?;
            block_apply(
                &mut ecu,
                &s,
                &backup_store(&store),
                &change,
                args.bc.conn.simulate,
            )
        }
    }
}
