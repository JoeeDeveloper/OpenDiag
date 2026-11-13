//! A byte-level virtual KW1281 module for tests and demos.
//!
//! Unlike the frame-matching `Simulator`, this one models the lock-step byte
//! protocol: it sends one byte at a time and waits for the tester's complement,
//! complements the tester's bytes in turn, and checks block counters. Anything
//! the tester gets wrong is recorded in [`Kw1281Sim::violations`]. It models the
//! protocol as documented, not any real module.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_transport::{Capabilities, Frame, Transport};

use super::block::{title, Block};

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    /// Waiting for slow init.
    Idle,
    /// Sent sync + keywords, waiting for the tester's complement of KW2.
    AwaitKeywordAck,
    Active,
    Ended,
}

pub struct Kw1281Sim {
    echo: bool,
    keywords: (u8, u8),
    ident: Vec<Vec<u8>>,
    faults: Vec<(u16, u8)>,
    groups: HashMap<u8, Vec<Vec<u8>>>,
    group_reads: HashMap<u8, usize>,

    phase: Phase,
    rx: VecDeque<u8>,
    tx_pending: VecDeque<u8>,
    awaiting_ack: Option<u8>,
    inbound: Vec<u8>,
    queue: VecDeque<Block>,
    last_counter: u8,

    slow_inits: Vec<u8>,
    bytes_received: usize,
    cleared: bool,
    violations: Vec<String>,
}

impl Default for Kw1281Sim {
    fn default() -> Self {
        Self::new()
    }
}

impl Kw1281Sim {
    pub fn new() -> Self {
        Self {
            echo: true,
            keywords: (0x01, 0x8A),
            ident: Vec::new(),
            faults: Vec::new(),
            groups: HashMap::new(),
            group_reads: HashMap::new(),
            phase: Phase::Idle,
            rx: VecDeque::new(),
            tx_pending: VecDeque::new(),
            awaiting_ack: None,
            inbound: Vec::new(),
            queue: VecDeque::new(),
            last_counter: 0,
            slow_inits: Vec::new(),
            bytes_received: 0,
            cleared: false,
            violations: Vec::new(),
        }
    }

    /// Whether the simulated cable echoes transmitted bytes (default true).
    pub fn echo(mut self, on: bool) -> Self {
        self.echo = on;
        self
    }

    pub fn keywords(mut self, kb1: u8, kb2: u8) -> Self {
        self.keywords = (kb1, kb2);
        self
    }

    /// Add one identification text block.
    pub fn ident_line(mut self, text: &str) -> Self {
        self.ident.push(text.as_bytes().to_vec());
        self
    }

    pub fn fault(mut self, code: u16, status: u8) -> Self {
        self.faults.push((code, status));
        self
    }

    /// Script a measuring-block group as `(formula, a, b)` cells.
    pub fn group(self, group: u8, cells: &[(u8, u8, u8)]) -> Self {
        self.group_cycle(group, &[cells])
    }

    /// Script a group whose value changes: each read returns the next frame, wrapping around.
    pub fn group_cycle(mut self, group: u8, frames: &[&[(u8, u8, u8)]]) -> Self {
        let frames = frames
            .iter()
            .map(|cells| cells.iter().flat_map(|&(f, a, b)| [f, a, b]).collect())
            .collect();
        self.groups.insert(group, frames);
        self
    }

    /// Addresses the tester sent 5-baud init to.
    pub fn slow_inits(&self) -> &[u8] {
        &self.slow_inits
    }

    /// Total bytes the tester has transmitted.
    pub fn bytes_received(&self) -> usize {
        self.bytes_received
    }

    /// Whether the tester cleared the fault memory.
    pub fn cleared(&self) -> bool {
        self.cleared
    }

    /// Protocol mistakes the tester made. Empty means a clean session.
    pub fn violations(&self) -> &[String] {
        &self.violations
    }

    fn violate(&mut self, msg: impl Into<String>) {
        self.violations.push(msg.into());
    }

    /// Start sending the next queued block, or an ACK when there is nothing queued.
    fn emit_next(&mut self) {
        let mut block = self.queue.pop_front().unwrap_or(Block {
            counter: 0,
            title: title::ACK,
            data: Vec::new(),
        });
        block.counter = self.last_counter.wrapping_add(1);
        self.last_counter = block.counter;
        let bytes = block.encode().expect("sim blocks are short");
        self.tx_pending = bytes.into();
        self.send_next_byte();
    }

    fn send_next_byte(&mut self) {
        if let Some(b) = self.tx_pending.pop_front() {
            self.rx.push_back(b);
            // Every byte but the block end expects the tester's complement.
            self.awaiting_ack = if self.tx_pending.is_empty() {
                None
            } else {
                Some(!b)
            };
        }
    }

    fn fault_block(&self) -> Block {
        let data = if self.faults.is_empty() {
            vec![0xFF, 0xFF, 0x88]
        } else {
            self.faults
                .iter()
                .flat_map(|&(code, status)| [(code >> 8) as u8, code as u8, status])
                .collect()
        };
        Block {
            counter: 0,
            title: title::FAULT_CODES_RESPONSE,
            data,
        }
    }

    fn ident_blocks(&self) -> Vec<Block> {
        self.ident
            .iter()
            .map(|text| Block {
                counter: 0,
                title: title::ASCII_DATA,
                data: text.clone(),
            })
            .collect()
    }

    fn handle_block(&mut self, block: Block) {
        if block.counter != self.last_counter.wrapping_add(1) {
            self.violate(format!(
                "block counter {} after {}",
                block.counter, self.last_counter
            ));
        }
        self.last_counter = block.counter;
        match block.title {
            title::ACK => {}
            title::END => {
                self.phase = Phase::Ended;
                return;
            }
            title::READ_IDENT => self.queue.extend(self.ident_blocks()),
            title::FAULT_CODES_READ => {
                let b = self.fault_block();
                self.queue.push_back(b);
            }
            title::FAULT_CODES_DELETE => {
                self.faults.clear();
                self.cleared = true;
                let b = self.fault_block();
                self.queue.push_back(b);
            }
            title::GROUP_READ => {
                let group = block.data.first().copied().unwrap_or(0);
                let reply = if let Some(frames) = self.groups.get(&group).filter(|f| !f.is_empty())
                {
                    // Successive reads walk through the scripted frames, then wrap.
                    let n = self.group_reads.entry(group).or_insert(0);
                    let data = frames[*n % frames.len()].clone();
                    *n += 1;
                    Block {
                        counter: 0,
                        title: title::GROUP_READ_RESPONSE,
                        data,
                    }
                } else {
                    Block {
                        counter: 0,
                        title: title::NAK,
                        data: Vec::new(),
                    }
                };
                self.queue.push_back(reply);
            }
            other => self.queue.push_back(Block {
                counter: 0,
                title: if other == title::NAK {
                    title::ACK
                } else {
                    title::NAK
                },
                data: Vec::new(),
            }),
        }
        self.emit_next();
    }

    fn on_tester_byte(&mut self, b: u8) {
        self.bytes_received += 1;
        if self.echo {
            self.rx.push_back(b);
        }
        match self.phase {
            Phase::Idle | Phase::Ended => {
                self.violate(format!("byte {b:#04X} while no session is open"));
            }
            Phase::AwaitKeywordAck => {
                if b == !self.keywords.1 {
                    self.phase = Phase::Active;
                    self.queue.extend(self.ident_blocks());
                    self.emit_next();
                } else {
                    self.violate(format!("bad keyword acknowledgement {b:#04X}"));
                }
            }
            Phase::Active => {
                if let Some(expected) = self.awaiting_ack {
                    if b == expected {
                        self.awaiting_ack = None;
                        self.send_next_byte();
                    } else {
                        self.violate(format!("bad complement {b:#04X}, expected {expected:#04X}"));
                        self.tx_pending.clear();
                        self.awaiting_ack = None;
                    }
                    return;
                }
                self.inbound.push(b);
                let len = usize::from(self.inbound[0]);
                if self.inbound.len() == len + 1 {
                    // Block complete; the end byte is not acknowledged.
                    let bytes = std::mem::take(&mut self.inbound);
                    match Block::decode(&bytes) {
                        Ok(block) => self.handle_block(block),
                        Err(e) => self.violate(e.to_string()),
                    }
                } else {
                    self.rx.push_back(!b);
                }
            }
        }
    }
}

impl Transport for Kw1281Sim {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let Frame::Raw(bytes) = frame else {
            return Err(Error::Transport("K-line carries raw bytes only".into()));
        };
        for &b in bytes {
            self.on_tester_byte(b);
        }
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Frame> {
        if self.rx.is_empty() {
            return Err(Error::Timeout);
        }
        Ok(Frame::Raw(self.rx.drain(..).collect()))
    }

    fn set_baud(&mut self, _baud: u32) -> Result<()> {
        Ok(())
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        self.slow_inits.push(address);
        self.rx.clear();
        self.inbound.clear();
        self.tx_pending.clear();
        self.awaiting_ack = None;
        self.last_counter = 0;
        self.phase = Phase::AwaitKeywordAck;
        self.rx.extend([0x55, self.keywords.0, self.keywords.1]);
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            kline: true,
            slow_init: true,
            set_baud: true,
            ..Capabilities::default()
        }
    }
}
