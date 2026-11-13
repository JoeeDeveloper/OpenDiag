use std::io::{self, Write};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use opendiag_proto::kwp::{KwpClient, KwpConfig};
use opendiag_services::demo::{virtual_uds_client, VirtualClient, TESTER_ID};
use opendiag_services::{clear_dtcs, identify, read_dtcs, ClearConfirmation, DtcReport};
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
    /// Use the built-in virtual ECU instead of hardware (no car needed).
    #[arg(long)]
    simulate: bool,
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
        }) => probe(&port, address, baud),
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

/// Read-only handshake with one module over a K-line cable.
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

type Uds = VirtualClient;

/// Hardware drivers do not exist yet, so only the virtual ECU can be opened.
fn connect(target: &Target) -> Result<Uds, String> {
    if !target.simulate {
        return Err("no hardware drivers yet; pass --simulate to use the virtual ECU".into());
    }
    eprintln!("[simulated] virtual ECU, tester id {TESTER_ID:#05X}");
    Ok(virtual_uds_client())
}

fn scan(target: &Target) -> Result<(), String> {
    let mut uds = connect(target)?;
    let ident = identify(&mut uds).map_err(|e| e.to_string())?;
    let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "(not supported)".into());
    println!("Part number      : {}", show(&ident.part_number));
    println!("Software version : {}", show(&ident.software_version));
    println!("VIN              : {}", show(&ident.vin));
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
    let mut uds = connect(target)?;
    print_dtcs(&read_dtcs(&mut uds).map_err(|e| e.to_string())?);
    Ok(())
}

fn dtc_clear(target: &Target, yes: bool) -> Result<(), String> {
    let mut uds = connect(target)?;
    let stored = read_dtcs(&mut uds).map_err(|e| e.to_string())?;
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
    let cleared =
        clear_dtcs(&mut uds, &ClearConfirmation::confirmed_by_user()).map_err(|e| e.to_string())?;
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
