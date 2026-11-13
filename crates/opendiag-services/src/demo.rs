//! A scripted virtual ECU for demos and tests. Not a model of any real module.

use opendiag_core::Result;
use opendiag_proto::clock::FakeClock;
use opendiag_proto::isotp::{Config, IsoTpFrame, IsoTpLink};
use opendiag_proto::kw1281::sim::Kw1281Sim;
use opendiag_proto::kw1281::{Kw1281Client, Kw1281Config};
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_transport::{Frame, Simulator, Transport};

use opendiag_proto::kwp::sim::KwpSim;
use opendiag_proto::kwp::{KwpClient, KwpConfig};

use crate::{Kw1281Module, KwpModule};

/// A virtual K-line KWP2000 module: made-up identification and two made-up fault codes.
pub fn virtual_kwp_bus() -> KwpSim {
    KwpSim::new(0x01)
        .ident(b"VIRTUAL KWP2000 ECU")
        .fault(12345, 0x20)
        .fault(54321, 0x01)
}

/// The virtual KWP2000 module with its session open.
pub type VirtualKwp = KwpModule<KwpClient<KwpSim>>;

pub fn virtual_kwp_module() -> Result<VirtualKwp> {
    virtual_kwp_module_over(virtual_kwp_bus())
}

/// The same session over any transport that carries the virtual module's traffic.
pub fn virtual_kwp_module_over<T: Transport>(transport: T) -> Result<KwpModule<KwpClient<T>>> {
    let mut kwp = KwpClient::new(transport, KwpConfig::new(0x01));
    kwp.start_communication()?;
    KwpModule::start(kwp, 0x01, "kwp2000")
}

/// A virtual KW1281 module with made-up ident text and two fault codes.
pub fn virtual_kw1281_bus() -> Kw1281Sim {
    Kw1281Sim::new()
        .ident_line("DEMO-KW-0001")
        .ident_line("VIRTUAL ENGINE ECU")
        .fault(525, 0x20)
        .fault(17, 0x01)
        // Made-up measuring groups whose values change on every read.
        .group_cycle(
            1,
            &[
                &[(0x01, 0x10, 0x20), (0x07, 0x00, 0x64)],
                &[(0x01, 0x10, 0x28), (0x07, 0x00, 0x65)],
                &[(0x01, 0x10, 0x30), (0x07, 0x00, 0x66)],
            ],
        )
        .group(2, &[(0x05, 0x00, 0x80)])
        // Made-up adaptation channels.
        .adaptation(4, 100)
        .adaptation(7, 5)
}

/// The virtual KW1281 module with its session already open.
pub type VirtualKw1281 = Kw1281Module<Kw1281Sim, FakeClock>;

pub fn virtual_kw1281_module() -> Result<VirtualKw1281> {
    virtual_kw1281_module_over(virtual_kw1281_bus())
}

/// The same session over any transport that carries the virtual module's traffic
/// (for example the virtual bus wrapped in a tracer).
pub fn virtual_kw1281_module_over<T: Transport>(
    transport: T,
) -> Result<Kw1281Module<T, FakeClock>> {
    let client = Kw1281Client::new(transport, FakeClock::new(), Kw1281Config::default());
    Kw1281Module::connect(client, 0x01)
}

pub const TESTER_ID: u32 = 0x7E0;
pub const ECU_ID: u32 = 0x7E8;

/// A single-frame request as the tester sends it.
fn request(payload: &[u8]) -> Frame {
    let mut d = vec![payload.len() as u8];
    d.extend_from_slice(payload);
    Frame::can(TESTER_ID, &d)
}

/// An ECU reply of any length, segmented into ISO-TP frames.
fn reply(payload: &[u8]) -> Vec<Frame> {
    if payload.len() <= 7 {
        let mut d = vec![payload.len() as u8];
        d.extend_from_slice(payload);
        return vec![Frame::can(ECU_ID, &d)];
    }
    let first = IsoTpFrame::First {
        total_len: payload.len(),
        data: payload[..6].to_vec(),
    };
    let mut out = vec![Frame::can(ECU_ID, &first.encode(None))];
    for (i, chunk) in payload[6..].chunks(7).enumerate() {
        let cf = IsoTpFrame::Consecutive {
            sn: ((i + 1) & 0xF) as u8,
            data: chunk.to_vec(),
        };
        out.push(Frame::can(ECU_ID, &cf.encode(Some(0))));
    }
    out
}

fn did_reply(did: u16, data: &[u8]) -> Vec<Frame> {
    let mut p = vec![0x62, (did >> 8) as u8, did as u8];
    p.extend_from_slice(data);
    reply(&p)
}

/// The simulated bus: a UDS ECU with identification data and two stored DTCs.
pub fn virtual_ecu_bus() -> Simulator {
    let dtcs = [
        0x59, 0x02, 0xFF, // positive response, sub-function, availability mask
        0x01, 0x71, 0x00, 0x09, // P0171 (system too lean): testFailed + confirmed
        0x03, 0x00, 0x00, 0x24, // P0300 (random misfire): pending-style status
    ];
    Simulator::new()
        .on(
            request(&[0x22, 0xF1, 0x87]),
            did_reply(0xF187, b"DEMO-0001-A"),
        )
        .on(request(&[0x22, 0xF1, 0x89]), did_reply(0xF189, b"SW 1.0.3"))
        .on(
            request(&[0x22, 0xF1, 0x90]),
            did_reply(0xF190, b"DEMO00000000VIN01"),
        )
        .on(request(&[0x19, 0x02, 0xFF]), reply(&dtcs))
        .on(request(&[0x14, 0xFF, 0xFF, 0xFF]), reply(&[0x54]))
}

/// UDS client type used with the virtual ECU.
pub type VirtualClient = UdsClient<Simulator, FakeClock>;

/// A UDS client wired to the virtual ECU. Read-only until a service unlocks it.
pub fn virtual_uds_client() -> VirtualClient {
    virtual_uds_client_over(virtual_ecu_bus())
}

/// The same client over any transport carrying the virtual ECU's traffic.
pub fn virtual_uds_client_over<T: Transport>(transport: T) -> UdsClient<T, FakeClock> {
    let link = IsoTpLink::new(
        transport,
        FakeClock::new(),
        TESTER_ID,
        ECU_ID,
        Config::default(),
    );
    UdsClient::new(link, UdsConfig::default())
}
