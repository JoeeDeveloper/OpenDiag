use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use opendiag_core::Error;
use opendiag_data::{json_schema_pretty, load_dir, DataSet};
use opendiag_proto::clock::SystemClock;
use opendiag_proto::kw1281::{Kw1281Client, Kw1281Config};
use opendiag_proto::kwp::{KwpClient, KwpConfig, KwpService};
use opendiag_proto::tp20::{Tp20Config, Tp20KwpClient, Tp20Link};
use opendiag_services::demo::{
    virtual_ecu_bus, virtual_kw1281_bus, virtual_kw1281_module_over, virtual_kwp_bus,
    virtual_kwp_module_over, virtual_uds_client_over, TESTER_ID,
};
use opendiag_services::{
    apply_change, apply_clear, apply_restore, format_unix_utc, log_csv, plan_change, plan_clear,
    plan_restore, render_html, BackupStore, ClearConfirmation, DtcReport, Kw1281Module, KwpModule,
    Limit, LogConfig, Module, Report, ReportFault, WriteConfirmation,
};
use opendiag_transport::{
    list_serial_ports, open_can, KLineSerial, TraceStatus, Tracing, Transport,
};

/// Open source VAG group diagnostic tool.
#[derive(Parser)]
#[command(name = "opendiag", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Identify the module (part number, software version, VIN).
    Scan(Target),
    /// Read or clear diagnostic trouble codes.
    Dtc {
        #[command(subcommand)]
        action: DtcCmd,
    },
    /// Read or change adaptation channels (KW1281 modules). Changes are backed up first.
    Adaptation {
        #[command(subcommand)]
        action: AdaptationCmd,
    },
    /// List or restore backups of changes made to modules.
    Backup {
        #[command(subcommand)]
        action: BackupCmd,
    },
    /// Work with the open data files (module names, DTC descriptions, labels).
    Data {
        #[command(subcommand)]
        action: DataCmd,
    },
    /// Log live measuring-block data to CSV (KW1281 modules). Values are raw cells.
    Log {
        #[command(flatten)]
        target: Target,
        /// Measuring-block groups to read each round, e.g. --groups 1,2.
        #[arg(long, value_delimiter = ',', required = true)]
        groups: Vec<u8>,
        /// Stop after this many rounds (default 20 if no limit is given).
        #[arg(long)]
        samples: Option<u32>,
        /// Stop after this many seconds instead.
        #[arg(long, conflicts_with = "samples")]
        duration_secs: Option<u64>,
        /// Time between rounds.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        /// Write CSV here instead of standard output.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Write a standalone HTML report (identification and fault codes). Read-only.
    /// Open it in a browser, or print it to PDF from there.
    Report {
        #[command(flatten)]
        target: Target,
        /// Write the HTML here instead of standard output.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Serve a read-only web page for the connected module on 127.0.0.1 (this computer only).
    /// Prints a URL with a one-time token; open that URL. Stop with Ctrl-C.
    Serve {
        #[command(flatten)]
        target: Target,
        /// Port to listen on (default: any free port).
        #[arg(long, default_value_t = 0)]
        listen_port: u16,
    },
    /// List serial ports (to find your K-line cable).
    Ports,
    /// Read-only K-line check: fast init + StartCommunication + TesterPresent.
    Probe {
        /// Serial port, e.g. COM3 or /dev/ttyUSB0 (see `opendiag ports`).
        #[arg(long)]
        port: String,
        /// Module address in hex, e.g. 0x01 for the engine ECU.
        #[arg(long, default_value = "0x01", value_parser = parse_hex_u8)]
        address: u8,
        /// Serial baud rate.
        #[arg(long, default_value_t = 10400)]
        baud: u32,
        /// Use 5-baud slow init and the KW1281 protocol (older modules, ~2 s to start).
        /// Reads identification and fault codes only; nothing is written or cleared.
        #[arg(long)]
        slow_init: bool,
        /// With --slow-init: the cable does not echo transmitted bytes.
        #[arg(long)]
        no_echo: bool,
        /// Record every byte sent and received to this file (see docs/capture.md).
        #[arg(long)]
        trace: Option<PathBuf>,
    },
    /// Passively record bus traffic to a trace file (K-line serial port or Linux CAN interface).
    /// Sends nothing. Stop with Ctrl-C or --duration-secs; the file is valid at any point.
    #[command(group = clap::ArgGroup::new("source").required(true).args(["port", "can"]))]
    Capture {
        /// Serial port to listen on, e.g. COM3 (receive only).
        #[arg(long)]
        port: Option<String>,
        /// CAN interface to listen on, e.g. can0 (Linux only).
        #[arg(long)]
        can: Option<String>,
        /// Serial baud rate (with --port).
        #[arg(long, default_value_t = 10400)]
        baud: u32,
        /// Stop after this many seconds (default: run until Ctrl-C).
        #[arg(long)]
        duration_secs: Option<u64>,
        /// File to write the trace to.
        #[arg(long)]
        out: PathBuf,
    },
}

fn parse_hex_u8(s: &str) -> Result<u8, String> {
    let digits = s.trim_start_matches("0x").trim_start_matches("0X");
    u8::from_str_radix(digits, 16).map_err(|_| format!("`{s}` is not a hex byte (e.g. 0x01)"))
}

/// The 24-bit workshop code a module expects with a write (module-specific; you supply it).
fn workshop_code_parser() -> clap::builder::RangedI64ValueParser<u32> {
    clap::value_parser!(u32).range(0..=16_777_215)
}

#[derive(Args)]
struct StoreArg {
    /// Backup file. Default: $OPENDIAG_BACKUPS, else ./opendiag-backups.jsonl.
    #[arg(long)]
    backup_file: Option<PathBuf>,
}

#[derive(Subcommand)]
enum AdaptationCmd {
    /// Read the current value of a channel (read-only).
    Read {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        channel: u8,
    },
    /// Change a channel. WRITES to the module: saves the old value first, asks you to
    /// confirm, then reads the new value back to verify it. There is no --yes.
    Set {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        channel: u8,
        #[arg(long)]
        value: u16,
        /// 24-bit workshop code the module expects (0 to 16777215).
        #[arg(long, value_parser = workshop_code_parser())]
        workshop_code: u32,
        #[command(flatten)]
        store: StoreArg,
    },
}

#[derive(Subcommand)]
enum BackupCmd {
    /// List recorded changes and their status.
    List {
        #[command(flatten)]
        store: StoreArg,
    },
    /// Put a backed-up value back. WRITES to the module (and is itself backed up).
    Restore {
        /// Backup id from `backup list`, e.g. b0003.
        id: String,
        #[command(flatten)]
        target: Target,
        /// 24-bit workshop code the module expects (0 to 16777215).
        #[arg(long, value_parser = workshop_code_parser())]
        workshop_code: u32,
        #[command(flatten)]
        store: StoreArg,
    },
}

#[derive(Subcommand)]
enum DataCmd {
    /// Check every data file under a directory (default: ./data).
    Validate { path: Option<PathBuf> },
    /// Print the JSON Schema for data files.
    Schema,
}

#[derive(Subcommand)]
enum DtcCmd {
    /// List stored trouble codes (read-only).
    Read(Target),
    /// Clear all stored trouble codes. Saves a snapshot of them first, asks you to
    /// confirm exactly what was shown, then checks what the module reports. There is no --yes.
    Clear {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        store: StoreArg,
    },
}

#[derive(Args)]
struct Target {
    /// Use the built-in virtual UDS ECU (no car needed).
    #[arg(long)]
    simulate: bool,
    /// Use the built-in virtual KW1281 module (no car needed).
    #[arg(long)]
    simulate_kw1281: bool,
    /// Serial port of a K-line cable, e.g. COM3 or /dev/ttyUSB0 (see `opendiag ports`).
    #[arg(long)]
    port: Option<String>,
    /// With --port: use 5-baud slow init and the KW1281 protocol (older modules).
    #[arg(long)]
    slow_init: bool,
    /// Use the built-in virtual KWP2000 module on a K-line (no car needed).
    #[arg(long)]
    simulate_kwp: bool,
    /// With --port: use ISO 14230 fast init and KWP2000 (newer K-line modules).
    #[arg(long, conflicts_with = "slow_init")]
    fast_init: bool,
    /// CAN interface for KWP2000 over TP2.0, e.g. can0 (Linux only).
    #[arg(long)]
    can: Option<String>,
    /// Module address in hex, e.g. 0x01 for the engine ECU.
    #[arg(long, default_value = "0x01", value_parser = parse_hex_u8)]
    address: u8,
    /// Serial baud rate.
    #[arg(long, default_value_t = 10400)]
    baud: u32,
    /// With --slow-init: the cable does not echo transmitted bytes.
    #[arg(long)]
    no_echo: bool,
    /// Record every byte sent and received to this file (see docs/capture.md).
    #[arg(long)]
    trace: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        None => {
            println!(
                "opendiag {} - no command given; try --help",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
        Some(Cmd::Scan(t)) => scan(&t),
        Some(Cmd::Dtc {
            action: DtcCmd::Read(t),
        }) => dtc_read(&t),
        Some(Cmd::Dtc {
            action: DtcCmd::Clear { target, store },
        }) => dtc_clear(&target, &store),
        Some(Cmd::Adaptation {
            action: AdaptationCmd::Read { target, channel },
        }) => adaptation_read(&target, channel),
        Some(Cmd::Adaptation {
            action:
                AdaptationCmd::Set {
                    target,
                    channel,
                    value,
                    workshop_code,
                    store,
                },
        }) => adaptation_set(&target, channel, value, workshop_code, &store),
        Some(Cmd::Backup {
            action: BackupCmd::List { store },
        }) => backup_list(&store),
        Some(Cmd::Backup {
            action:
                BackupCmd::Restore {
                    id,
                    target,
                    workshop_code,
                    store,
                },
        }) => backup_restore(&id, &target, workshop_code, &store),
        Some(Cmd::Data {
            action: DataCmd::Validate { path },
        }) => data_validate(path.as_deref().unwrap_or_else(|| Path::new("data"))),
        Some(Cmd::Data {
            action: DataCmd::Schema,
        }) => {
            println!("{}", json_schema_pretty());
            Ok(())
        }
        Some(Cmd::Log {
            target,
            groups,
            samples,
            duration_secs,
            interval_ms,
            out,
        }) => {
            let limit = match duration_secs {
                Some(s) => Limit::Duration(Duration::from_secs(s)),
                None => Limit::Samples(samples.unwrap_or(20)),
            };
            let cfg = LogConfig {
                groups,
                interval: Duration::from_millis(interval_ms),
                limit,
            };
            log_cmd(&target, &cfg, out.as_deref())
        }
        Some(Cmd::Report { target, out }) => report_cmd(&target, out.as_deref()),
        Some(Cmd::Serve {
            target,
            listen_port,
        }) => serve_cmd(&target, listen_port),
        Some(Cmd::Ports) => ports(),
        Some(Cmd::Probe {
            port,
            address,
            baud,
            slow_init,
            no_echo,
            trace,
        }) => {
            if slow_init {
                probe_kw1281(&port, address, baud, !no_echo, trace.as_deref())
            } else {
                probe(&port, address, baud, trace.as_deref())
            }
        }
        Some(Cmd::Capture {
            port,
            can,
            baud,
            duration_secs,
            out,
        }) => capture_cmd(
            port.as_deref(),
            can.as_deref(),
            baud,
            duration_secs.map(Duration::from_secs),
            &out,
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn ports() -> Result<(), String> {
    let ports = list_serial_ports().map_err(|e| e.to_string())?;
    if ports.is_empty() {
        println!("No serial ports found. Is the cable plugged in and its driver installed?");
    }
    for p in ports {
        println!("{:<12} {}", p.name, p.description);
    }
    Ok(())
}

/// Prints how a trace went when dropped, i.e. after the session has been closed
/// and its last bytes recorded. Declare it before whatever owns the traced transport.
struct TraceReport {
    status: TraceStatus,
    path: PathBuf,
}

impl Drop for TraceReport {
    fn drop(&mut self) {
        eprintln!(
            "Trace: {} event(s) written to {}",
            self.status.events(),
            self.path.display()
        );
        if let Some(e) = self.status.error() {
            eprintln!("warning: the trace is incomplete: {e}");
        }
    }
}

/// Box `inner`, recording a trace of everything it does when a path is given.
fn trace_transport<T: Transport + 'static>(
    inner: T,
    path: Option<&Path>,
) -> Result<(Box<dyn Transport>, Option<TraceReport>), String> {
    let Some(path) = path else {
        return Ok((Box::new(inner), None));
    };
    let file = std::fs::File::create(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let tracing = Tracing::new(inner, io::BufWriter::new(file));
    let report = TraceReport {
        status: tracing.status(),
        path: path.to_path_buf(),
    };
    Ok((Box::new(tracing), Some(report)))
}

/// An open module plus the trace report that must be printed after it closes.
/// Fields drop in order, so the module (and its goodbye bytes) go first.
struct Connection {
    module: Box<dyn Module>,
    _trace: Option<TraceReport>,
}

/// Passive capture: record everything heard on a bus, send nothing.
fn capture_cmd(
    port: Option<&str>,
    can: Option<&str>,
    baud: u32,
    duration: Option<Duration>,
    out: &Path,
) -> Result<(), String> {
    let source: Box<dyn Transport> = match (port, can) {
        (Some(p), _) => Box::new(KLineSerial::open(p, baud).map_err(|e| e.to_string())?),
        (None, Some(i)) => open_can(i).map_err(|e| e.to_string())?,
        (None, None) => return Err("choose --port or --can".into()),
    };
    let file =
        std::fs::File::create(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    // Silence between frames is the normal state on a quiet bus; don't log it.
    let mut tracer = Tracing::new(source, io::BufWriter::new(file)).log_timeouts(false);
    let status = tracer.status();
    eprintln!(
        "Listening (send nothing). Writing to {}. {}",
        out.display(),
        if duration.is_some() {
            "Stops by itself."
        } else {
            "Press Ctrl-C to stop; the file is valid at any point."
        }
    );
    let deadline = duration.map(|d| Instant::now() + d);
    while !deadline.is_some_and(|d| Instant::now() >= d) {
        match tracer.recv(Duration::from_millis(100)) {
            Ok(_) | Err(Error::Timeout) => {}
            Err(e) => return Err(e.to_string()),
        }
        if let Some(e) = status.error() {
            return Err(format!("cannot write the trace: {e}"));
        }
    }
    eprintln!(
        "Captured {} event(s) to {}.",
        status.events(),
        out.display()
    );
    Ok(())
}

/// Read-only KW1281 session: 5-baud init, identification, fault codes, then end.
fn probe_kw1281(
    port: &str,
    address: u8,
    baud: u32,
    echo: bool,
    trace: Option<&Path>,
) -> Result<(), String> {
    let serial = KLineSerial::open(port, baud).map_err(|e| e.to_string())?;
    let (transport, _report) = trace_transport(serial, trace)?;
    let cfg = Kw1281Config {
        echo,
        ..Kw1281Config::default()
    };
    let mut kw = Kw1281Client::new(transport, SystemClock::new(), cfg);
    println!("5-baud slow init to module {address:#04X} on {port} at {baud} baud (about 2 s)...");
    let info = kw.connect(address).map_err(|e| e.to_string())?;
    println!(
        "Connected, keyword {:#04X} {:#04X}",
        info.key_bytes.kb1, info.key_bytes.kb2
    );
    for line in &info.ident {
        println!("  ident: {}", line.trim_end());
    }
    let faults = kw.read_fault_codes().map_err(|e| e.to_string())?;
    if faults.is_empty() {
        println!("No fault codes stored.");
    }
    for f in &faults {
        println!("  fault {} (status {:#04X})", f.text(), f.status);
    }
    // Best effort: the module times the session out by itself anyway.
    let _ = kw.end();
    Ok(())
}

/// Read-only handshake with one module over a K-line cable (fast init, KWP2000).
fn probe(port: &str, address: u8, baud: u32, trace: Option<&Path>) -> Result<(), String> {
    let serial = KLineSerial::open(port, baud).map_err(|e| e.to_string())?;
    let (transport, _report) = trace_transport(serial, trace)?;
    let mut kwp = KwpClient::new(transport, KwpConfig::new(address));
    println!("Fast init to module {address:#04X} on {port} at {baud} baud...");
    let kb = kwp.start_communication().map_err(|e| {
        format!(
            "{e}\nNo answer to fast init. Many older VAG modules use 5-baud slow init \
             (KW1281 / slow KWP2000), which is not implemented yet."
        )
    })?;
    println!(
        "StartCommunication OK, key bytes {:#04X} {:#04X}",
        kb.kb1, kb.kb2
    );
    kwp.tester_present().map_err(|e| e.to_string())?;
    println!("TesterPresent OK. The cable and module are talking.");
    Ok(())
}

/// KWP2000 over a TP2.0 channel on a CAN interface (Linux only).
fn connect_tp20(iface: &str, target: &Target) -> Result<Connection, String> {
    let can = open_can(iface).map_err(|e| e.to_string())?;
    let (transport, report) = trace_transport(can, target.trace.as_deref())?;
    eprintln!(
        "Opening a TP2.0 channel to module {:#04X} on {iface}...",
        target.address
    );
    let link = Tp20Link::open(
        transport,
        SystemClock::new(),
        &Tp20Config::new(target.address),
    )
    .map_err(|e| e.to_string())?;
    let client = Tp20KwpClient::new(link);
    Ok(Connection {
        module: Box::new(
            KwpModule::start(client, target.address, "kwp2000-tp20").map_err(|e| e.to_string())?,
        ),
        _trace: report,
    })
}

/// Open whichever module the flags select: a virtual module, KW1281 (`--slow-init`),
/// KWP2000 on K-line (`--fast-init`), or KWP2000 over TP2.0 on CAN (`--can`).
fn connect(target: &Target) -> Result<Connection, String> {
    let trace = target.trace.as_deref();
    if target.simulate {
        eprintln!("[simulated] virtual UDS ECU, tester id {TESTER_ID:#05X}");
        let (transport, report) = trace_transport(virtual_ecu_bus(), trace)?;
        return Ok(Connection {
            module: Box::new(virtual_uds_client_over(transport)),
            _trace: report,
        });
    }
    if target.simulate_kw1281 {
        eprintln!("[simulated] virtual KW1281 module");
        let (transport, report) = trace_transport(virtual_kw1281_bus(), trace)?;
        return Ok(Connection {
            module: Box::new(virtual_kw1281_module_over(transport).map_err(|e| e.to_string())?),
            _trace: report,
        });
    }
    if target.simulate_kwp {
        eprintln!("[simulated] virtual KWP2000 module (K-line fast init)");
        let (transport, report) = trace_transport(virtual_kwp_bus(), trace)?;
        return Ok(Connection {
            module: Box::new(virtual_kwp_module_over(transport).map_err(|e| e.to_string())?),
            _trace: report,
        });
    }
    if let Some(iface) = &target.can {
        if target.port.is_some() {
            return Err("choose either --port (K-line) or --can (CAN), not both".into());
        }
        return connect_tp20(iface, target);
    }
    let Some(port) = &target.port else {
        return Err(
            "no connection chosen: pass --simulate, --simulate-kw1281, --simulate-kwp, \
             --port <PORT> with --slow-init (KW1281) or --fast-init (KWP2000), or --can <IFACE>"
                .into(),
        );
    };
    if target.fast_init {
        let serial = KLineSerial::open(port, target.baud).map_err(|e| e.to_string())?;
        let (transport, report) = trace_transport(serial, trace)?;
        let mut kwp = KwpClient::new(transport, KwpConfig::new(target.address));
        eprintln!(
            "Fast init to module {:#04X} on {port} at {} baud...",
            target.address, target.baud
        );
        let kb = kwp.start_communication().map_err(|e| {
            format!("{e}\nNo answer to fast init. Older modules use 5-baud init: try --slow-init.")
        })?;
        eprintln!(
            "StartCommunication OK, key bytes {:#04X} {:#04X}",
            kb.kb1, kb.kb2
        );
        return Ok(Connection {
            module: Box::new(
                KwpModule::start(kwp, target.address, "kwp2000").map_err(|e| e.to_string())?,
            ),
            _trace: report,
        });
    }
    if !target.slow_init {
        return Err(
            "add --slow-init (older KW1281 modules) or --fast-init (KWP2000 modules). \
             UDS over hardware is not connected to scan/dtc yet"
                .into(),
        );
    }
    let serial = KLineSerial::open(port, target.baud).map_err(|e| e.to_string())?;
    let (transport, report) = trace_transport(serial, trace)?;
    let cfg = Kw1281Config {
        echo: !target.no_echo,
        ..Kw1281Config::default()
    };
    let client = Kw1281Client::new(transport, SystemClock::new(), cfg);
    eprintln!(
        "5-baud slow init to module {:#04X} on {port} at {} baud (about 2 s)...",
        target.address, target.baud
    );
    Ok(Connection {
        module: Box::new(Kw1281Module::connect(client, target.address).map_err(|e| e.to_string())?),
        _trace: report,
    })
}

fn scan(target: &Target) -> Result<(), String> {
    let mut conn = connect(target)?;
    let ident = conn.module.identify().map_err(|e| e.to_string())?;
    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "(not supported)".into());
    println!("Part number      : {}", show(&ident.part_number));
    // KW1281 and KWP2000 modules report free-form lines instead of the standard items.
    if ident.lines.is_empty() {
        println!("Software version : {}", show(&ident.software_version));
        println!("VIN              : {}", show(&ident.vin));
    }
    // When a part number was picked out, it is the first line; otherwise show every line.
    let already_shown = usize::from(ident.part_number.is_some());
    for line in ident.lines.iter().skip(already_shown) {
        println!("Ident            : {line}");
    }
    Ok(())
}

fn backup_store(arg: &StoreArg) -> BackupStore {
    let path = arg
        .backup_file
        .clone()
        .or_else(|| std::env::var_os("OPENDIAG_BACKUPS").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("opendiag-backups.jsonl"));
    BackupStore::new(path)
}

const NO_ADAPTATION: &str =
    "adaptation channels are only available for KW1281 modules so far (use --simulate-kw1281 or --slow-init)";

fn adaptation_read(target: &Target, channel: u8) -> Result<(), String> {
    let mut conn = connect(target)?;
    let access = conn.module.adaptation().ok_or(NO_ADAPTATION)?;
    let value = access.read(channel).map_err(|e| e.to_string())?;
    println!("Channel {channel}: {value}");
    Ok(())
}

fn print_write_warning(store: &BackupStore) {
    println!(
        "This WRITES to the module. The old value is saved to {} first, and the new value is read back to verify it.",
        store.path().display()
    );
    println!("Only continue with the vehicle stationary, ignition on, and a healthy battery.");
}

fn adaptation_set(
    target: &Target,
    channel: u8,
    value: u16,
    workshop_code: u32,
    store: &StoreArg,
) -> Result<(), String> {
    let store = backup_store(store);
    let mut conn = connect(target)?;
    let access = conn.module.adaptation().ok_or(NO_ADAPTATION)?;
    let change = plan_change(access, channel, value).map_err(|e| e.to_string())?;
    if change.old == change.new {
        println!("Channel {channel} already has value {value}; nothing to change.");
        return Ok(());
    }
    println!(
        "Adaptation channel {channel}: {} -> {}",
        change.old, change.new
    );
    print_write_warning(&store);
    if !confirm()? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    let confirmation = WriteConfirmation::for_change(&change);
    let applied = apply_change(access, &store, &change, workshop_code, &confirmation)
        .map_err(|e| e.to_string())?;
    println!(
        "Done and verified: channel {channel} now reads {}. Backup {} is in {}.",
        change.new,
        applied.backup_id,
        store.path().display()
    );
    println!(
        "To undo: opendiag backup restore {} (with the same connection options).",
        applied.backup_id
    );
    Ok(())
}

/// How the connection was made, in words for the report; and whether it is simulated.
fn describe_connection(t: &Target) -> (String, bool) {
    if t.simulate {
        return ("Simulated: built-in virtual UDS ECU".into(), true);
    }
    if t.simulate_kw1281 {
        return ("Simulated: built-in virtual KW1281 module".into(), true);
    }
    if t.simulate_kwp {
        return ("Simulated: built-in virtual KWP2000 module".into(), true);
    }
    let addr = t.address;
    if let Some(iface) = &t.can {
        return (
            format!("CAN interface {iface}, KWP2000 over TP2.0, module {addr:#04X}"),
            false,
        );
    }
    let port = t.port.as_deref().unwrap_or("?");
    let proto = if t.fast_init {
        "KWP2000 (fast init)"
    } else {
        "KW1281 (5-baud init)"
    };
    (
        format!(
            "K-line {port} at {} baud, {proto}, module {addr:#04X}",
            t.baud
        ),
        false,
    )
}

/// Random per-run token: 128 bits from the operating system, hex encoded.
fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("no secure random source: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Read-only local web interface. Binds to loopback only; every request goes
/// through `opendiag_web::App::handle`, which enforces the Host, token and
/// method rules. Single-threaded: requests are served one at a time, which also
/// keeps the diagnostic session from being used concurrently.
fn serve_cmd(target: &Target, listen_port: u16) -> Result<(), String> {
    let conn = connect(target)?;
    let Connection { module, _trace } = conn;
    let mut module = module;
    let module_name = module.faults().describe();
    let (connection, simulated) = describe_connection(target);

    let server = tiny_http::Server::http(("127.0.0.1", listen_port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{listen_port}: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or("cannot determine the listening port")?;
    let token = random_token()?;
    let info = opendiag_web::Info {
        version: env!("CARGO_PKG_VERSION").to_string(),
        connection,
        simulated,
        module: module_name,
    };
    let mut app = opendiag_web::App::new(module, load_data(), info, token.clone(), port);

    // First line on stdout, flushed: scripts and tests read the URL from here.
    println!("http://127.0.0.1:{port}/?t={token}");
    io::stdout().flush().ok();
    eprintln!("Serving read-only on 127.0.0.1:{port} (this computer only). Stop with Ctrl-C.");

    loop {
        match server.recv_timeout(Duration::from_secs(2)) {
            Ok(Some(request)) => answer(&mut app, request),
            // Idle: keep the diagnostic session alive.
            Ok(None) => app.tick(),
            Err(e) => return Err(format!("server error: {e}")),
        }
    }
}

fn answer(app: &mut opendiag_web::App, request: tiny_http::Request) {
    let method = request.method().to_string();
    let target = request.url().to_string();
    let header = |name: &'static str| {
        request
            .headers()
            .iter()
            .find(|h| h.field.equiv(name))
            .map(|h| h.value.as_str().to_string())
    };
    let host = header("Host");
    let token = header("X-OpenDiag-Token");
    let response = app.handle(&opendiag_web::Request {
        method: &method,
        target: &target,
        host: host.as_deref(),
        token_header: token.as_deref(),
    });
    let mut out = tiny_http::Response::from_data(response.body).with_status_code(response.status);
    let content_type = [("Content-Type", response.content_type)];
    for (name, value) in content_type
        .iter()
        .chain(opendiag_web::SECURITY_HEADERS.iter())
    {
        if let Ok(h) = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            out.add_header(h);
        }
    }
    // The client may have gone away; nothing to do about that.
    let _ = request.respond(out);
}

fn report_cmd(target: &Target, out: Option<&Path>) -> Result<(), String> {
    let mut conn = connect(target)?;
    let data = load_data();
    let ident = conn.module.identify().map_err(|e| e.to_string())?;
    let dtcs = conn.module.read_dtcs().map_err(|e| e.to_string())?;
    let module = conn.module.faults().describe();
    let (connection, simulated) = describe_connection(target);
    let faults = dtcs
        .iter()
        .map(|d| ReportFault {
            text: d.text.clone(),
            status: d.status,
            flags: d.flags.iter().map(|f| (*f).to_string()).collect(),
            description: describe(data.as_ref(), &d.text),
        })
        .collect();
    let report = Report {
        generated_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        connection,
        simulated,
        module: Some(module),
        ident,
        faults,
    };
    let html = render_html(&report);
    match out {
        Some(path) => {
            std::fs::write(path, html)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            eprintln!(
                "Report written to {}. It may contain a VIN and other identifying data; review it before sharing.",
                path.display()
            );
        }
        None => print!("{html}"),
    }
    Ok(())
}

fn backup_list(store: &StoreArg) -> Result<(), String> {
    let store = backup_store(store);
    let records = store.records().map_err(|e| e.to_string())?;
    if records.is_empty() {
        println!("No backups recorded in {}.", store.path().display());
        return Ok(());
    }
    println!("Backups in {}:", store.path().display());
    for r in &records {
        let restore = r
            .restores
            .as_ref()
            .map(|id| format!(" (restores {id})"))
            .unwrap_or_default();
        if r.kind == "dtc_clear" {
            println!(
                "{}  {} UTC  {} fault codes cleared ({} stored)  [{:?}]",
                r.id,
                format_unix_utc(r.created_unix),
                r.module.as_deref().unwrap_or(&r.protocol),
                r.codes.len(),
                r.status
            );
            for c in &r.codes {
                println!("      was: {c}");
            }
        } else {
            println!(
                "{}  {} UTC  {} module {:#04X} {} channel {}: {} -> {}  [{:?}]{restore}",
                r.id,
                format_unix_utc(r.created_unix),
                r.protocol,
                r.module_address,
                r.kind,
                r.channel,
                r.old_value,
                r.new_value,
                r.status
            );
        }
        if let Some(note) = &r.note {
            println!("      note: {note}");
        }
    }
    Ok(())
}

fn backup_restore(
    id: &str,
    target: &Target,
    workshop_code: u32,
    store: &StoreArg,
) -> Result<(), String> {
    let store = backup_store(store);
    let mut conn = connect(target)?;
    let access = conn.module.adaptation().ok_or(NO_ADAPTATION)?;
    let (record, change) = plan_restore(access, &store, id).map_err(|e| e.to_string())?;
    if change.old == change.new {
        println!(
            "Channel {} already has the backed-up value {}; nothing to restore.",
            change.channel, change.new
        );
        return Ok(());
    }
    println!(
        "Restore backup {id}: channel {} currently {}, will be set back to {}",
        change.channel, change.old, change.new
    );
    print_write_warning(&store);
    if !confirm()? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    let confirmation = WriteConfirmation::for_change(&change);
    let applied = apply_restore(
        access,
        &store,
        &record,
        &change,
        workshop_code,
        &confirmation,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "Restored and verified: channel {} reads {}. This restore is itself backup {}.",
        change.channel, change.new, applied.backup_id
    );
    Ok(())
}

fn log_cmd(target: &Target, cfg: &LogConfig, out: Option<&Path>) -> Result<(), String> {
    let mut conn = connect(target)?;
    let live = conn.module.live().ok_or(
        "live data is only available for KW1281 modules so far (use --simulate-kw1281 or --slow-init)",
    )?;
    let clock = SystemClock::new();
    let summary = match out {
        Some(path) => {
            let file = std::fs::File::create(path)
                .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
            let mut w = io::BufWriter::new(file);
            log_csv(live, &clock, cfg, &mut w).map_err(|e| e.to_string())?
        }
        None => log_csv(live, &clock, cfg, &mut io::stdout().lock()).map_err(|e| e.to_string())?,
    };
    eprintln!(
        "Logged {} round(s), {} row(s){}.",
        summary.samples,
        summary.rows,
        out.map(|p| format!(" to {}", p.display()))
            .unwrap_or_default()
    );
    Ok(())
}

fn data_validate(dir: &Path) -> Result<(), String> {
    match load_dir(dir) {
        Ok(set) => {
            if set.file_count() == 0 {
                println!("No data files found under {}.", dir.display());
            } else {
                println!(
                    "OK: {} file(s), {} module(s), {} code(s), {} measuring block(s).",
                    set.file_count(),
                    set.module_count(),
                    set.dtc_count(),
                    set.block_count()
                );
            }
            Ok(())
        }
        Err(issues) => {
            for i in &issues {
                eprintln!("{i}");
            }
            Err(format!(
                "{} problem(s) found in {}",
                issues.len(),
                dir.display()
            ))
        }
    }
}

/// Load descriptions from `$OPENDIAG_DATA` or `./data`, if present and valid.
fn load_data() -> Option<DataSet> {
    let dir = std::env::var_os("OPENDIAG_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"));
    if !dir.is_dir() {
        return None;
    }
    match load_dir(&dir) {
        Ok(set) => Some(set),
        Err(issues) => {
            eprintln!(
                "warning: ignoring invalid data in {} ({} problem(s)); run `opendiag data validate`",
                dir.display(),
                issues.len()
            );
            None
        }
    }
}

/// Look up a displayed code: `P0171-00` is an SAE code, a bare 5-digit code is VAG-style.
fn describe(data: Option<&DataSet>, text: &str) -> Option<String> {
    data?.describe_displayed_code(text).map(str::to_string)
}

fn print_dtcs(dtcs: &[DtcReport], data: Option<&DataSet>) {
    if dtcs.is_empty() {
        println!("No trouble codes stored.");
        return;
    }
    for d in dtcs {
        let meaning = describe(data, &d.text)
            .map(|m| format!("  {m}"))
            .unwrap_or_default();
        println!(
            "{}  status {:#04X}  [{}]{meaning}",
            d.text,
            d.status,
            d.flags.join(", ")
        );
    }
}

fn dtc_read(target: &Target) -> Result<(), String> {
    let mut conn = connect(target)?;
    let data = load_data();
    print_dtcs(
        &conn.module.read_dtcs().map_err(|e| e.to_string())?,
        data.as_ref(),
    );
    Ok(())
}

fn dtc_clear(target: &Target, store: &StoreArg) -> Result<(), String> {
    let store = backup_store(store);
    let mut conn = connect(target)?;
    let data = load_data();
    let access = conn.module.faults();
    let shown = plan_clear(access).map_err(|e| e.to_string())?;
    if shown.is_empty() {
        println!("No trouble codes stored; nothing to clear.");
        return Ok(());
    }
    println!("About to clear {} trouble code(s):", shown.len());
    print_dtcs(&shown, data.as_ref());
    println!(
        "Clearing erases these codes and their status for good and cannot be undone. A snapshot of them is saved to {} first.",
        store.path().display()
    );
    if !confirm()? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    let confirmation = ClearConfirmation::for_codes(&shown);
    let outcome = apply_clear(access, &store, &shown, &confirmation).map_err(|e| e.to_string())?;
    println!(
        "Cleared {} trouble code(s). Snapshot {} is in {}.",
        outcome.cleared.len(),
        outcome.backup_id,
        store.path().display()
    );
    if outcome.remaining.is_empty() {
        println!("The module now reports no trouble codes.");
    } else {
        println!(
            "Note: {} code(s) are still reported (active faults come straight back):",
            outcome.remaining.len()
        );
        print_dtcs(&outcome.remaining, data.as_ref());
    }
    Ok(())
}

fn confirm() -> Result<bool, String> {
    print!("Type YES to continue: ");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    // Windows PowerShell prepends a UTF-8 BOM when piping text into a program.
    Ok(line.trim_start_matches('\u{feff}').trim() == "YES")
}
