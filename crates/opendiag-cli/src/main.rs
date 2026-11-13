use std::io::{self, Write};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use opendiag_services::demo::{virtual_uds_client, VirtualClient, TESTER_ID};
use opendiag_services::{clear_dtcs, identify, read_dtcs, ClearConfirmation, DtcReport};

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
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
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
