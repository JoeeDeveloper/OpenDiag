//! A virtual BMW vehicle and an HSFZ gateway server for it, for tests and
//! `--simulate`. No car needed.
//!
//! [`VirtualVehicle`] answers UDS requests per ECU (pure, unit-testable);
//! [`MockGateway`] serves it over real localhost TCP (diagnostics) and UDP
//! (discovery) on background threads, so the real socket code is exercised.
//!
//! This encodes our reading of the protocol (see `hsfz.rs`), so a shared
//! misunderstanding would not be caught. It is not evidence about a real car.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::hsfz::{take_frame, HsfzFrame, CTRL_ACK, CTRL_VEHICLE_IDENT};

/// A request seen by the gateway: target address and UDS bytes.
pub type Request = (u8, Vec<u8>);
type RequestLog = Arc<Mutex<Vec<Request>>>;

/// One simulated ECU.
#[derive(Debug, Clone, Default)]
pub struct VirtualEcu {
    pub address: u8,
    pub dids: BTreeMap<u16, Vec<u8>>,
    /// Stored DTCs: 3-byte code and status byte.
    pub dtcs: Vec<(u32, u8)>,
    /// DIDs that first answer "response pending" (0x78) once.
    pub slow_dids: BTreeSet<u16>,
}

/// A simulated car: a gateway plus ECUs, all addressed through the gateway.
#[derive(Debug, Clone)]
pub struct VirtualVehicle {
    pub vin: String,
    pub mac: String,
    pub gateway: u8,
    pub ecus: Vec<VirtualEcu>,
}

impl VirtualVehicle {
    /// A synthetic F-series-like car. Every value is invented for testing.
    pub fn demo() -> Self {
        let vin = "WBASIMULATED00001".to_string();
        let mut gw = VirtualEcu {
            address: 0x10,
            ..Default::default()
        };
        gw.dids.insert(0xF190, vin.clone().into_bytes());
        gw.dids.insert(0x100B, b"F020-00-00-000".to_vec());
        gw.dids.insert(0x3F06, vec![0x01, 0x02, 0x03, 0x04]);
        let mut dme = VirtualEcu {
            address: 0x12,
            dtcs: vec![(0x480A12, 0x09), (0x480B01, 0x24)],
            ..Default::default()
        };
        dme.dids.insert(0xF190, vin.clone().into_bytes());
        dme.slow_dids.insert(0x4001);
        dme.dids.insert(0x4001, vec![0x12, 0x34]);
        Self {
            vin,
            mac: "001A37B44210".into(),
            gateway: 0x10,
            ecus: vec![gw, dme],
        }
    }

    /// The identification string a gateway announces (layout unverified, see `discovery.rs`).
    pub fn ident_string(&self) -> String {
        format!(
            "DIAGADR{:X}BMWMAC{}BMWVIN{}",
            self.gateway, self.mac, self.vin
        )
    }

    /// UDS replies to one request for `target`, in order; `None` if no such ECU.
    pub fn respond(&mut self, target: u8, req: &[u8]) -> Option<Vec<Vec<u8>>> {
        let ecu = self.ecus.iter_mut().find(|e| e.address == target)?;
        let Some(&sid) = req.first() else {
            return Some(vec![]);
        };
        let nrc = |n: u8| vec![0x7F, sid, n];
        Some(match (sid, req) {
            (0x10, [_, sub, ..]) => vec![vec![0x50, *sub, 0x00, 0x32, 0x01, 0xF4]],
            (0x3E, [_, sub, ..]) => vec![vec![0x7E, *sub]],
            (0x22, [_, hi, lo]) => {
                let did = u16::from_be_bytes([*hi, *lo]);
                match ecu.dids.get(&did) {
                    None => vec![nrc(0x31)],
                    Some(data) => {
                        let mut ok = vec![0x62, *hi, *lo];
                        ok.extend_from_slice(data);
                        if ecu.slow_dids.remove(&did) {
                            vec![nrc(0x78), ok]
                        } else {
                            vec![ok]
                        }
                    }
                }
            }
            (0x19, [_, 0x02, mask]) => {
                let mut r = vec![0x59, 0x02, 0xFF];
                for (code, status) in ecu.dtcs.iter().filter(|(_, s)| s & mask != 0) {
                    let c = code.to_be_bytes();
                    r.extend_from_slice(&[c[1], c[2], c[3], *status]);
                }
                vec![r]
            }
            (0x14, [_, a, b, c]) => {
                if [*a, *b, *c] == [0xFF, 0xFF, 0xFF] {
                    ecu.dtcs.clear();
                    vec![vec![0x54]]
                } else {
                    vec![nrc(0x31)]
                }
            }
            (0x19 | 0x22 | 0x14, _) => vec![nrc(0x13)],
            _ => vec![nrc(0x11)],
        })
    }
}

/// A running mock gateway. Threads stop when this is dropped.
pub struct MockGateway {
    tcp: SocketAddr,
    udp: SocketAddr,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    /// Every UDS request received, as `(target, bytes)`, for assertions.
    requests: RequestLog,
}

const TICK: Duration = Duration::from_millis(20);

impl MockGateway {
    /// Serve `vehicle` on 127.0.0.1 with OS-chosen ports.
    pub fn start(vehicle: VirtualVehicle) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let tcp = listener.local_addr()?;
        let udp_sock = UdpSocket::bind("127.0.0.1:0")?;
        udp_sock.set_read_timeout(Some(TICK))?;
        let udp = udp_sock.local_addr()?;

        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let vehicle = Arc::new(Mutex::new(vehicle));
        let mut threads = Vec::new();

        // Discovery responder.
        {
            let (stop, vehicle) = (stop.clone(), vehicle.clone());
            threads.push(thread::spawn(move || {
                let mut buf = [0u8; 512];
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, from)) = udp_sock.recv_from(&mut buf) else {
                        continue;
                    };
                    let mut d = buf[..n].to_vec();
                    if let Ok(Some(f)) = take_frame(&mut d) {
                        if f.control == CTRL_VEHICLE_IDENT && f.payload.is_empty() {
                            let ident = vehicle.lock().unwrap().ident_string();
                            let reply =
                                HsfzFrame::new(CTRL_VEHICLE_IDENT, ident.into_bytes()).encode();
                            let _ = udp_sock.send_to(&reply, from);
                        }
                    }
                }
            }));
        }

        // Diagnostic server: one thread accepts, one per connection serves.
        {
            let (stop, requests) = (stop.clone(), requests.clone());
            threads.push(thread::spawn(move || {
                let mut conns: Vec<JoinHandle<()>> = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((sock, _)) => {
                            let (stop, vehicle, requests) =
                                (stop.clone(), vehicle.clone(), requests.clone());
                            conns.push(thread::spawn(move || {
                                serve(sock, &stop, &vehicle, &requests)
                            }));
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(TICK),
                        Err(_) => break,
                    }
                }
                for c in conns {
                    let _ = c.join();
                }
            }));
        }

        Ok(Self {
            tcp,
            udp,
            stop,
            threads,
            requests,
        })
    }

    /// Address of the diagnostic (TCP) service.
    pub fn tcp_addr(&self) -> SocketAddr {
        self.tcp
    }

    /// Address of the discovery (UDP) responder.
    pub fn udp_addr(&self) -> SocketAddr {
        self.udp
    }

    pub fn requests(&self) -> Vec<(u8, Vec<u8>)> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockGateway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn serve(
    mut sock: TcpStream,
    stop: &AtomicBool,
    vehicle: &Mutex<VirtualVehicle>,
    requests: &Mutex<Vec<Request>>,
) {
    let _ = sock.set_nonblocking(false);
    let _ = sock.set_read_timeout(Some(TICK));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !stop.load(Ordering::Relaxed) {
        match sock.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        loop {
            let frame = match take_frame(&mut buf) {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(_) => return,
            };
            let Some((src, dst, data)) = frame.as_diagnostic() else {
                let _ = sock.write_all(&HsfzFrame::new(0x41, vec![]).encode());
                continue;
            };
            requests.lock().unwrap().push((dst, data.to_vec()));
            let replies = vehicle.lock().unwrap().respond(dst, data);
            let out = match replies {
                None => HsfzFrame::new(0x43, vec![dst]).encode(),
                Some(list) => {
                    let mut o = HsfzFrame::new(CTRL_ACK, vec![src, dst]).encode();
                    for r in list {
                        o.extend(HsfzFrame::diagnostic(dst, src, &r).encode());
                    }
                    o
                }
            };
            if sock.write_all(&out).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_a_did_and_rejects_an_unknown_one() {
        let mut v = VirtualVehicle::demo();
        let r = v.respond(0x10, &[0x22, 0xF1, 0x90]).unwrap();
        assert_eq!(&r[0][..3], [0x62, 0xF1, 0x90]);
        assert_eq!(
            v.respond(0x10, &[0x22, 0xAB, 0xCD]).unwrap()[0],
            [0x7F, 0x22, 0x31]
        );
    }

    #[test]
    fn an_unknown_ecu_gets_no_replies() {
        assert!(VirtualVehicle::demo().respond(0x99, &[0x3E, 0]).is_none());
    }
}
