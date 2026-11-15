//! `opendiag bmw ...`: BMW F-series over an ENET cable (HSFZ). Read-only.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use clap::{Args, Subcommand};
use opendiag_bmw::hsfz::{DEFAULT_GATEWAY, TCP_PORT};
use opendiag_bmw::mock::{MockGateway, VirtualVehicle};
use opendiag_bmw::{
    broadcast_target, discover, read_did, read_ident, Discovered, HsfzGateway, TcpByteStream,
};
use opendiag_proto::clock::SystemClock;
use opendiag_proto::uds::{UdsClient, UdsConfig};

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
