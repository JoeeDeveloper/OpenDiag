//! Robustness: nothing in the protocol layer may panic or hang on garbage.
//!
//! Decoders get random bytes. Clients run complete sessions through a transport
//! that corrupts, truncates, duplicates and invents frames. Errors are expected
//! and fine; panics and hangs are not. Everything is deterministic (seeded).

use std::collections::VecDeque;
use std::time::Duration;

use opendiag_core::Result;
use opendiag_proto::clock::FakeClock;
use opendiag_proto::isotp::{Config, IsoTpLink};
use opendiag_proto::kw1281::sim::Kw1281Sim;
use opendiag_proto::kw1281::{parse_ascii, parse_faults, Block, Kw1281Client, Kw1281Config};
use opendiag_proto::kwp::{KwpClient, KwpConfig, Message, Parser};
use opendiag_proto::tp20::sim::Tp20Sim;
use opendiag_proto::tp20::{
    decode_setup_request, ChannelMsg, SetupReply, Tp20Config, Tp20KwpClient, Tp20Link,
};
use opendiag_proto::uds::{UdsClient, UdsConfig};
use opendiag_transport::{Capabilities, Frame, Simulator, Transport};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn bytes(&mut self, max: u64) -> Vec<u8> {
        (0..self.below(max)).map(|_| self.next() as u8).collect()
    }
}

#[test]
fn decoders_survive_random_bytes() {
    let mut rng = Rng(0xC0FFEE);
    let mut parser = Parser::new();
    for _ in 0..60_000 {
        let b = rng.bytes(14);
        let _ = ChannelMsg::decode(&b);
        let _ = SetupReply::decode(&b);
        let _ = decode_setup_request(&b);
        let _ = Block::decode(&b);
        let _ = parse_faults(&b);
        let _ = parse_ascii(&Block {
            counter: 0,
            title: 0xF6,
            data: b.clone(),
        });
        parser.push(&b);
        while let Ok(Some(_)) = parser.next_message() {}
    }
}

/// Wraps a transport and damages what it receives.
struct Mutating<T: Transport> {
    inner: T,
    rng: Rng,
    /// Percent chance that a received frame is damaged.
    rate: u64,
    queue: VecDeque<Frame>,
    /// Receives allowed before the bus "goes quiet" (guarantees termination).
    budget: u32,
}

impl<T: Transport> Mutating<T> {
    fn new(inner: T, seed: u64, rate: u64) -> Self {
        Self {
            inner,
            rng: Rng(seed),
            rate,
            queue: VecDeque::new(),
            budget: 4000,
        }
    }

    fn damage(&mut self, mut f: Frame) -> Frame {
        let duplicate = f.clone();
        let data = match &mut f {
            Frame::Raw(d) => d,
            Frame::Can { data, .. } => data,
        };
        match self.rng.below(6) {
            0 if !data.is_empty() => {
                let i = self.rng.below(data.len() as u64) as usize;
                data[i] ^= 1 << self.rng.below(8);
            }
            1 => data.truncate(self.rng.below(data.len() as u64 + 1) as usize),
            2 => data.push(self.rng.next() as u8),
            3 => self.queue.push_back(duplicate),
            4 => *data = self.rng.bytes(9),
            _ => data.iter_mut().for_each(|b| *b = 0xFF),
        }
        f
    }
}

impl<T: Transport> Transport for Mutating<T> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.inner.send(frame)
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame> {
        if self.budget == 0 {
            return Err(opendiag_core::Error::Timeout);
        }
        self.budget -= 1;
        if let Some(f) = self.queue.pop_front() {
            return Ok(f);
        }
        let f = self.inner.recv(timeout)?;
        if self.rng.below(100) < self.rate {
            Ok(self.damage(f))
        } else {
            Ok(f)
        }
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.inner.set_baud(baud)
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        self.inner.slow_init(address)
    }

    fn fast_init(&mut self) -> Result<()> {
        self.inner.fast_init()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

const SEEDS: u64 = 250;

#[test]
fn kw1281_client_survives_corrupted_sessions() {
    for seed in 0..SEEDS {
        let sim = Kw1281Sim::new()
            .ident_line("1J0906018AB")
            .ident_line("ENGINE")
            .fault(525, 0x20)
            .group(1, &[(1, 2, 3), (4, 5, 6)]);
        let t = Mutating::new(sim, seed, 8 + seed % 30);
        let mut c = Kw1281Client::new(t, FakeClock::new(), Kw1281Config::default());
        let _ = c.connect(0x01);
        let _ = c.read_fault_codes();
        let _ = c.read_group(1);
        let _ = c.keep_alive();
        let _ = c.end();
    }
}

#[test]
fn tp20_link_and_kwp_survive_corrupted_sessions() {
    for seed in 0..SEEDS {
        let sim = Tp20Sim::new(0x01)
            .block_size(2)
            .respond(&[0x10, 0x89], &[0x50, 0x89])
            .respond(&[0x21, 0x01], &(0..30).collect::<Vec<u8>>())
            .respond(&[0x18, 0x00, 0xFF, 0x00], &[0x58, 0x01, 0x01, 0x23, 0x20]);
        let t = Mutating::new(sim, seed, 5 + seed % 25);
        let Ok(link) = Tp20Link::open(t, FakeClock::new(), &Tp20Config::new(0x01)) else {
            continue;
        };
        let mut c = Tp20KwpClient::new(link);
        let _ = c.start_diagnostic_session(0x89);
        let _ = c.read_data_by_local_identifier(0x01);
        let _ = c.read_dtcs_by_status(0x00, 0xFF00);
        let _ = c.tester_present();
    }
}

fn frames_for_uds() -> Simulator {
    // Single-frame request -> multi-frame reply, as in the UDS tests.
    let req = Frame::can(0x7E0, &[0x03, 0x22, 0xF1, 0x90]);
    let reply = vec![
        Frame::can(0x7E8, &[0x10, 0x0A, 0x62, 0xF1, 0x90, 1, 2, 3]),
        Frame::can(0x7E8, &[0x21, 4, 5, 6, 7, 0, 0, 0]),
    ];
    Simulator::new()
        .on(req, reply)
        .on(
            Frame::can(0x7E0, &[0x02, 0x3E, 0x00]),
            vec![Frame::can(0x7E8, &[0x02, 0x7E, 0x00])],
        )
        .on(
            Frame::can(0x7E0, &[0x03, 0x19, 0x02, 0xFF]),
            vec![Frame::can(0x7E8, &[0x07, 0x59, 0x02, 0xFF, 1, 0x71, 0, 9])],
        )
}

#[test]
fn uds_over_isotp_survives_corrupted_traffic() {
    for seed in 0..SEEDS {
        let t = Mutating::new(frames_for_uds(), seed, 10 + seed % 30);
        let link = IsoTpLink::new(t, FakeClock::new(), 0x7E0, 0x7E8, Config::default());
        let mut c = UdsClient::new(link, UdsConfig::default());
        let _ = c.read_data_by_identifier(0xF190);
        let _ = c.tester_present();
        let _ = c.read_dtcs_by_status_mask(0xFF);
        let _ = c.session_control(0x03);
    }
}

fn kwp_sim() -> Simulator {
    let enc = |data: &[u8], to_ecu: bool| {
        let (target, source) = if to_ecu { (0x01, 0xF1) } else { (0xF1, 0x01) };
        Frame::Raw(
            Message {
                target,
                source,
                data: data.to_vec(),
            }
            .encode()
            .unwrap(),
        )
    };
    Simulator::new()
        .on(enc(&[0x81], true), vec![enc(&[0xC1, 0xEF, 0x8F], false)])
        .on(enc(&[0x3E], true), vec![enc(&[0x7E], false)])
        .on(
            enc(&[0x18, 0x00, 0xFF, 0x00], true),
            vec![enc(&[0x58, 0x02, 1, 2, 3, 4, 5, 6], false)],
        )
        .on(
            enc(&[0x21, 0x01], true),
            vec![
                enc(&[0x7F, 0x21, 0x78], false),
                enc(&[0x61, 0x01, 9], false),
            ],
        )
}

#[test]
fn kwp_k_line_client_survives_corrupted_traffic() {
    for seed in 0..SEEDS {
        let t = Mutating::new(kwp_sim(), seed, 10 + seed % 40);
        let mut c = KwpClient::new(t, KwpConfig::new(0x01));
        let _ = c.start_communication();
        let _ = c.tester_present();
        let _ = c.read_dtcs_by_status(0x00, 0xFF00);
        let _ = c.read_data_by_local_identifier(0x01);
        let _ = c.read_ecu_identification(0x9B);
    }
}
