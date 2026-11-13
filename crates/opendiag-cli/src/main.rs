use std::io::{self, Write};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use opendiag_proto::clock::SystemClock;
use opendiag_proto::kw1281::{Kw1281Client, Kw1281Config};
use opendiag_proto::kwp::{KwpClient, KwpConfig};
use opendiag_services::demo::{virtual_kw1281_module, virtual_uds_client, TESTER_ID};
use opendiag_services::{ClearConfirmation, DtcReport, Kw1281Module, Module};
use opendiag_transport::{list_serial_ports, KLineSerial};

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
    },
}

fn parse_hex_u8(s: &str) -> Result<u8, String> {
    let digits = s.trim_start_matches("0x").trim_start_matches("0X");
    u8::from_str_radix(digits, 16).map_err(|_| format!("`{s}` is not a hex byte (e.g. 0x01)"))
}

#[derive(Subcommand)]
enum DtcCmd {
    /// List stored trouble codes (read-only).
    Read(Target),
    /// Clear all stored trouble codes. Asks for confirmation.
    Clear {
        #[command(flatten)]
        target: Target,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
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
    /// Module address in hex, e.g. 0x01 for the engine ECU.
    #[arg(long, default_value = "0x01", value_parser = parse_hex_u8)]
    address: u8,
    /// Serial baud rate.
    #[arg(long, default_value_t = 10400)]
    baud: u32,
    /// The cable does not echo transmitted bytes.
    #[arg(long)]
    no_echo: bool,
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
            action: DtcCmd::Clear { target, yes },
        }) => dtc_clear(&target, yes),
        Some(Cmd::Ports) => ports(),
        Some(Cmd::Probe {
            port,
            address,
            baud,
            slow_init,
            no_echo,
        }) => {
            if slow_init {
                probe_kw1281(&port, address, baud, !no_echo)
            } else {
                probe(&port, address, baud)
            }
        }
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

/// Read-only KW1281 session: 5-baud init, identification, fault codes, then end.
fn probe_kw1281(port: &str, address: u8, baud: u32, echo: bool) -> Result<(), String> {
    let transport = KLineSerial::open(port, baud).map_err(|e| e.to_string())?;
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
fn probe(port: &str, address: u8, baud: u32) -> Result<(), String> {
    let transport = KLineSerial::open(port, baud).map_err(|e| e.to_string())?;
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

/// Open whichever module the flags select. Only K-line KW1281 hardware is wired up so far.
fn connect(target: &Target) -> Result<Box<dyn Module>, String> {
    if target.simulate {
        eprintln!("[simulated] virtual UDS ECU, tester id {TESTER_ID:#05X}");
        return Ok(Box::new(virtual_uds_client()));
    }
    if target.simulate_kw1281 {
        eprintln!("[simulated] virtual KW1281 module");
        return Ok(Box::new(
            virtual_kw1281_module().map_err(|e| e.to_string())?,
        ));
    }
    let Some(port) = &target.port else {
        return Err(
            "no connection chosen: pass --simulate, --simulate-kw1281, or --port <PORT> --slow-init"
                .into(),
        );
    };
    if !target.slow_init {
        return Err(
            "only KW1281 (older modules) is wired up for hardware so far: add --slow-init. \
             KWP2000 and UDS over hardware are not connected to scan/dtc yet"
                .into(),
        );
    }
    let transport = KLineSerial::open(port, target.baud).map_err(|e| e.to_string())?;
    let cfg = Kw1281Config {
        echo: !target.no_echo,
        ..Kw1281Config::default()
    };
    let client = Kw1281Client::new(transport, SystemClock::new(), cfg);
    eprintln!(
        "5-baud slow init to module {:#04X} on {port} at {} baud (about 2 s)...",
        target.address, target.baud
    );
    Ok(Box::new(
        Kw1281Module::connect(client, target.address).map_err(|e| e.to_string())?,
    ))
}

fn scan(target: &Target) -> Result<(), String> {
    let mut module = connect(target)?;
    let ident = module.identify().map_err(|e| e.to_string())?;
    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "(not supported)".into());
    println!("Part number      : {}", show(&ident.part_number));
    // KW1281 modules report free-form lines instead of the standard items.
    if ident.lines.is_empty() {
        println!("Software version : {}", show(&ident.software_version));
        println!("VIN              : {}", show(&ident.vin));
    }
    for line in ident.lines.iter().skip(1) {
        println!("Ident            : {line}");
    }
    Ok(())
}

fn print_dtcs(dtcs: &[DtcReport]) {
    if dtcs.is_empty() {
        println!("No trouble codes stored.");
        return;
    }
    for d in dtcs {
        println!(
            "{}  status {:#04X}  [{}]",
            d.text,
            d.status,
            d.flags.join(", ")
        );
    }
}

fn dtc_read(target: &Target) -> Result<(), String> {
    let mut module = connect(target)?;
    print_dtcs(&module.read_dtcs().map_err(|e| e.to_string())?);
    Ok(())
}

fn dtc_clear(target: &Target, yes: bool) -> Result<(), String> {
    let mut module = connect(target)?;
    let stored = module.read_dtcs().map_err(|e| e.to_string())?;
    if stored.is_empty() {
        println!("No trouble codes stored; nothing to clear.");
        return Ok(());
    }
    println!("About to clear {} trouble code(s):", stored.len());
    print_dtcs(&stored);
    if !yes && !confirm()? {
        println!("Cancelled; nothing was changed.");
        return Ok(());
    }
    let cleared = module
        .clear_dtcs(&ClearConfirmation::confirmed_by_user())
        .map_err(|e| e.to_string())?;
    println!(
        "Cleared {} trouble code(s). Record of what was stored:",
        cleared.len()
    );
    print_dtcs(&cleared);
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
