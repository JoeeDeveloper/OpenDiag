//! KW1281 client over a K-line [`Transport`] that supports 5-baud slow init.
//!
//! Safety: clearing fault codes refuses to run unless the client is in
//! [`WriteMode::Unlocked`]. Backup and confirmation live in the services layer.

use std::collections::VecDeque;
use std::time::Duration;

use opendiag_core::{Error, Result, WriteMode};
use opendiag_transport::{Frame, Transport};

use super::block::{parse_ascii, parse_faults, title, Block, Fault, END_BYTE};
use crate::clock::Clock;
use crate::kwp::KeyBytes;

/// The second keyword byte that identifies KW1281 (the first is 0x01).
const KW1281_KEYWORD: u8 = 0x8A;
/// Sync byte the module sends first so the tester can measure its baud rate.
const SYNC_BYTE: u8 = 0x55;
/// Safety cap on blocks in one response, so a misbehaving module cannot loop us forever.
const MAX_BLOCKS: usize = 64;

#[derive(Debug, Clone)]
pub struct Kw1281Config {
    /// The K-line cable echoes every byte we transmit back on RX (most do).
    pub echo: bool,
    /// Wait for the next byte from the module.
    pub byte_timeout: Duration,
    /// Wait for the module's sync byte after the 5-baud address.
    pub init_timeout: Duration,
    /// Pause before sending a byte complement (acknowledging a module byte).
    /// A conservative placeholder until measured on real hardware.
    pub ack_delay: Duration,
    /// Pause before sending a block, to give slow modules time to turn around.
    pub block_gap: Duration,
}

impl Default for Kw1281Config {
    fn default() -> Self {
        Self {
            echo: true,
            byte_timeout: Duration::from_millis(1000),
            init_timeout: Duration::from_millis(3000),
            ack_delay: Duration::from_millis(2),
            block_gap: Duration::from_millis(25),
        }
    }
}

/// What a module reports when a session starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectInfo {
    pub key_bytes: KeyBytes,
    /// Identification text lines (part number, component, coding, ...).
    pub ident: Vec<String>,
}

/// One measuring-block cell, undecoded: `(formula byte, a, b)`. Turning these
/// into engineering units needs per-formula data, which belongs in the open
/// data format and is not part of this layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawMeasurement {
    pub formula: u8,
    pub a: u8,
    pub b: u8,
}

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("kw1281: {}", msg.into()))
}

pub struct Kw1281Client<T: Transport, C: Clock> {
    transport: T,
    clock: C,
    cfg: Kw1281Config,
    rx: VecDeque<u8>,
    next_counter: u8,
    write_mode: WriteMode,
}

impl<T: Transport, C: Clock> Kw1281Client<T, C> {
    /// New client, read-only by default.
    pub fn new(transport: T, clock: C, cfg: Kw1281Config) -> Self {
        Self {
            transport,
            clock,
            cfg,
            rx: VecDeque::new(),
            next_counter: 0,
            write_mode: WriteMode::ReadOnly,
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    pub fn write_mode(&self) -> WriteMode {
        self.write_mode
    }

    pub fn set_write_mode(&mut self, mode: WriteMode) {
        self.write_mode = mode;
    }

    fn require_unlocked(&self, what: &str) -> Result<()> {
        match self.write_mode {
            WriteMode::Unlocked => Ok(()),
            WriteMode::ReadOnly => Err(Error::WritesLocked(what.into())),
        }
    }

    fn pause(&self, d: Duration) {
        self.clock.wait_until(self.clock.now() + d);
    }

    // ---- byte level -------------------------------------------------------

    fn read_byte(&mut self, timeout: Duration) -> Result<u8> {
        loop {
            if let Some(b) = self.rx.pop_front() {
                return Ok(b);
            }
            if let Frame::Raw(bytes) = self.transport.recv(timeout)? {
                self.rx.extend(bytes);
            }
        }
    }

    /// Transmit one byte, consume its echo, and (when `expect_complement`) the
    /// module's complement acknowledgement.
    fn write_byte(&mut self, b: u8, expect_complement: bool) -> Result<()> {
        self.transport.send(&Frame::Raw(vec![b]))?;
        if self.cfg.echo {
            let echo = self.read_byte(self.cfg.byte_timeout)?;
            if echo != b {
                return Err(protocol(format!(
                    "echo mismatch: sent {b:#04X}, read back {echo:#04X} (try --no-echo?)"
                )));
            }
        }
        if expect_complement {
            let ack = self.read_byte(self.cfg.byte_timeout)?;
            if ack != !b {
                return Err(protocol(format!(
                    "module acknowledged {b:#04X} with {ack:#04X}, expected {:#04X}",
                    !b
                )));
            }
        }
        Ok(())
    }

    /// Read a module byte and acknowledge it with its complement.
    fn read_and_ack(&mut self) -> Result<u8> {
        let b = self.read_byte(self.cfg.byte_timeout)?;
        self.pause(self.cfg.ack_delay);
        self.write_byte(!b, false)?;
        Ok(b)
    }

    // ---- block level ------------------------------------------------------

    fn receive_block(&mut self) -> Result<Block> {
        let len = self.read_and_ack()?;
        if len < 3 {
            return Err(protocol(format!("block length {len} is too short")));
        }
        let mut bytes = vec![len];
        for i in 0..len {
            // The final byte is the block end, which is not acknowledged.
            let b = if i == len - 1 {
                self.read_byte(self.cfg.byte_timeout)?
            } else {
                self.read_and_ack()?
            };
            bytes.push(b);
        }
        let block = Block::decode(&bytes)?;
        self.next_counter = block.counter.wrapping_add(1);
        Ok(block)
    }

    fn send_block(&mut self, title: u8, data: &[u8]) -> Result<()> {
        self.pause(self.cfg.block_gap);
        let bytes = Block {
            counter: self.next_counter,
            title,
            data: data.to_vec(),
        }
        .encode()?;
        self.next_counter = self.next_counter.wrapping_add(1);
        let last = bytes.len() - 1;
        for (i, b) in bytes.into_iter().enumerate() {
            debug_assert!(i != last || b == END_BYTE);
            self.write_byte(b, i != last)?;
        }
        Ok(())
    }

    fn send_ack(&mut self) -> Result<()> {
        self.send_block(title::ACK, &[])
    }

    /// Receive blocks, acknowledging each, until the module sends its own ACK
    /// block to say it has nothing more (it is the tester's turn).
    fn receive_blocks(&mut self) -> Result<Vec<Block>> {
        let mut out = Vec::new();
        for _ in 0..MAX_BLOCKS {
            let block = self.receive_block()?;
            match block.title {
                title::ACK => return Ok(out),
                title::NAK => return Err(protocol("module answered NAK (request refused)")),
                _ => {
                    out.push(block);
                    self.send_ack()?;
                }
            }
        }
        Err(protocol("module kept sending blocks without a final ACK"))
    }

    fn request(&mut self, title: u8, data: &[u8]) -> Result<Vec<Block>> {
        self.send_block(title, data)?;
        self.receive_blocks()
    }

    // ---- session ----------------------------------------------------------

    /// 5-baud slow init to `address`, keyword exchange, then read identification.
    pub fn connect(&mut self, address: u8) -> Result<ConnectInfo> {
        self.rx.clear();
        self.transport.slow_init(address)?;

        let sync = self.read_byte(self.cfg.init_timeout)?;
        if sync != SYNC_BYTE {
            return Err(protocol(format!(
                "expected sync byte 0x55, got {sync:#04X} (wrong baud rate?)"
            )));
        }
        let kb1 = self.read_byte(self.cfg.byte_timeout)?;
        let kb2 = self.read_byte(self.cfg.byte_timeout)?;
        if kb2 != KW1281_KEYWORD {
            return Err(protocol(format!(
                "keyword {kb1:#04X} {kb2:#04X} is not KW1281 (0x01 0x8A); \
                 this module probably speaks KWP2000 after slow init"
            )));
        }
        self.pause(self.cfg.ack_delay);
        self.write_byte(!kb2, false)?;

        let mut ident = Vec::new();
        let mut blocks = self.receive_blocks()?;
        for _ in 0..8 {
            let mut more = false;
            for b in blocks.iter().filter(|b| b.title == title::ASCII_DATA) {
                let (text, flag) = parse_ascii(b);
                ident.push(text);
                more |= flag;
            }
            if !more {
                break;
            }
            blocks = self.request(title::READ_IDENT, &[])?;
        }
        Ok(ConnectInfo {
            key_bytes: KeyBytes { kb1, kb2 },
            ident,
        })
    }

    /// Keep the session alive (the module drops it after a few idle seconds).
    pub fn keep_alive(&mut self) -> Result<()> {
        self.send_ack()?;
        let block = self.receive_block()?;
        if block.title != title::ACK {
            return Err(protocol(format!(
                "expected ACK to keep-alive, got title {:#04X}",
                block.title
            )));
        }
        Ok(())
    }

    /// Politely end the session. The module does not reply.
    pub fn end(&mut self) -> Result<()> {
        self.send_block(title::END, &[])
    }

    fn fault_blocks(blocks: Vec<Block>) -> Result<Vec<Fault>> {
        let mut faults = Vec::new();
        for b in blocks {
            if b.title != title::FAULT_CODES_RESPONSE {
                return Err(protocol(format!(
                    "unexpected block {:#04X} in fault code response",
                    b.title
                )));
            }
            faults.extend(parse_faults(&b.data)?);
        }
        Ok(faults)
    }

    /// Read all stored fault codes.
    pub fn read_fault_codes(&mut self) -> Result<Vec<Fault>> {
        let blocks = self.request(title::FAULT_CODES_READ, &[])?;
        Self::fault_blocks(blocks)
    }

    /// Clear fault codes; returns any the module still reports afterwards.
    /// Requires unlocked writes.
    pub fn clear_fault_codes(&mut self) -> Result<Vec<Fault>> {
        self.require_unlocked("clear fault codes")?;
        let blocks = self.request(title::FAULT_CODES_DELETE, &[])?;
        Self::fault_blocks(blocks)
    }

    /// Read a measuring-block group as raw `(formula, a, b)` cells.
    pub fn read_group(&mut self, group: u8) -> Result<Vec<RawMeasurement>> {
        let blocks = self.request(title::GROUP_READ, &[group])?;
        let [block] = blocks.as_slice() else {
            return Err(protocol(format!(
                "expected one group response block, got {}",
                blocks.len()
            )));
        };
        if block.title != title::GROUP_READ_RESPONSE {
            return Err(protocol(format!(
                "unexpected block {:#04X} in group response",
                block.title
            )));
        }
        if block.data.len() % 3 != 0 {
            return Err(protocol("group data is not a multiple of 3 bytes"));
        }
        Ok(block
            .data
            .chunks_exact(3)
            .map(|c| RawMeasurement {
                formula: c[0],
                a: c[1],
                b: c[2],
            })
            .collect())
    }
}
