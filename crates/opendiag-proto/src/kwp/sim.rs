//! A stateful virtual K-line KWP2000 module for tests and demos.
//!
//! It parses the tester's bytes as real ISO 14230 messages, answers the common
//! services, and keeps state (fault memory really clears). It also enforces the
//! session rules (fast init before StartCommunication, StartCommunication before
//! anything else) and records mistakes in [`KwpSim::violations`]. It models the
//! protocol as documented, not any real module.

use std::collections::VecDeque;
use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_transport::{Capabilities, Frame, Transport};

use super::frame::{Message, Parser};

pub struct KwpSim {
    ecu: u8,
    tester: u8,
    keywords: (u8, u8),
    ident: Vec<u8>,
    faults: Vec<(u16, u8)>,
    echo: bool,

    parser: Parser,
    inbox: VecDeque<Frame>,
    awaiting_start: bool,
    started: bool,

    fast_inits: usize,
    cleared: bool,
    requests: Vec<Vec<u8>>,
    violations: Vec<String>,
    /// Called the instant a modifying request arrives, before the module acts on it.
    write_hook: Option<WriteHook>,
}

/// Callback run when a modifying request arrives (see [`KwpSim::on_write`]).
type WriteHook = Box<dyn FnMut(&str)>;

impl KwpSim {
    pub fn new(ecu: u8) -> Self {
        Self {
            ecu,
            tester: 0xF1,
            keywords: (0xEF, 0x8F),
            ident: Vec::new(),
            faults: Vec::new(),
            echo: true,
            parser: Parser::new(),
            inbox: VecDeque::new(),
            awaiting_start: false,
            started: false,
            fast_inits: 0,
            cleared: false,
            requests: Vec::new(),
            violations: Vec::new(),
            write_hook: None,
        }
    }

    /// Run `hook` with a short label ("clear") the moment a modifying request arrives.
    pub fn on_write(mut self, hook: impl FnMut(&str) + 'static) -> Self {
        self.write_hook = Some(Box::new(hook));
        self
    }

    /// Whether the simulated cable echoes transmitted bytes (default true).
    pub fn echo(mut self, on: bool) -> Self {
        self.echo = on;
        self
    }

    /// Data returned for ReadECUIdentification option 0x9B.
    pub fn ident(mut self, data: &[u8]) -> Self {
        self.ident = data.to_vec();
        self
    }

    pub fn fault(mut self, code: u16, status: u8) -> Self {
        self.faults.push((code, status));
        self
    }

    /// How many fast-init wake-ups the tester performed.
    pub fn fast_inits(&self) -> usize {
        self.fast_inits
    }

    /// Whether the tester cleared the fault memory.
    pub fn cleared(&self) -> bool {
        self.cleared
    }

    /// Whether a communication session is open.
    pub fn is_started(&self) -> bool {
        self.started
    }

    /// Every service request the tester sent (SID first).
    pub fn requests(&self) -> &[Vec<u8>] {
        &self.requests
    }

    /// Protocol mistakes the tester made. Empty means a clean session.
    pub fn violations(&self) -> &[String] {
        &self.violations
    }

    fn violate(&mut self, msg: impl Into<String>) {
        self.violations.push(msg.into());
    }

    fn reply(&mut self, data: Vec<u8>) {
        let msg = Message {
            target: self.tester,
            source: self.ecu,
            data,
        };
        self.inbox
            .push_back(Frame::Raw(msg.encode().expect("sim replies are short")));
    }

    fn negative(&mut self, sid: u8, nrc: u8) {
        self.reply(vec![0x7F, sid, nrc]);
    }

    fn handle(&mut self, m: Message) {
        if m.target != self.ecu {
            return;
        }
        let Some(&sid) = m.data.first() else { return };
        let arg = |i: usize| m.data.get(i).copied().unwrap_or(0);
        self.requests.push(m.data.clone());

        if sid == 0x81 {
            if !self.awaiting_start {
                self.violate("StartCommunication without a fast-init wake-up");
            }
            self.awaiting_start = false;
            self.started = true;
            self.reply(vec![0xC1, self.keywords.0, self.keywords.1]);
            return;
        }
        if !self.started {
            self.violate(format!("service {sid:#04X} before StartCommunication"));
            return;
        }
        match sid {
            0x82 => {
                self.started = false;
                self.reply(vec![0xC2]);
            }
            0x10 => self.reply(vec![0x50, arg(1)]),
            0x3E => self.reply(vec![0x7E]),
            0x1A if arg(1) == 0x9B => {
                let mut r = vec![0x5A, 0x9B];
                r.extend_from_slice(&self.ident);
                self.reply(r);
            }
            0x1A => self.negative(sid, 0x12),
            0x18 => {
                let mut r = vec![0x58, self.faults.len() as u8];
                for &(code, status) in &self.faults {
                    r.extend_from_slice(&[(code >> 8) as u8, code as u8, status]);
                }
                self.reply(r);
            }
            0x14 => {
                if let Some(h) = self.write_hook.as_mut() {
                    h("clear");
                }
                self.faults.clear();
                self.cleared = true;
                self.reply(vec![0x54, arg(1), arg(2)]);
            }
            _ => self.negative(sid, 0x11),
        }
    }
}

impl Transport for KwpSim {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let Frame::Raw(bytes) = frame else {
            return Err(Error::Transport("K-line carries raw bytes only".into()));
        };
        if self.echo {
            self.inbox.push_back(Frame::Raw(bytes.clone()));
        }
        self.parser.push(bytes);
        loop {
            match self.parser.next_message() {
                Ok(Some(m)) => self.handle(m),
                Ok(None) => break,
                Err(e) => {
                    self.violate(e.to_string());
                    break;
                }
            }
        }
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Frame> {
        self.inbox.pop_front().ok_or(Error::Timeout)
    }

    fn set_baud(&mut self, _baud: u32) -> Result<()> {
        Ok(())
    }

    fn slow_init(&mut self, _address: u8) -> Result<()> {
        Err(Error::Transport("this module uses fast init".into()))
    }

    fn fast_init(&mut self) -> Result<()> {
        self.fast_inits += 1;
        self.awaiting_start = true;
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            kline: true,
            fast_init: true,
            set_baud: true,
            ..Capabilities::default()
        }
    }
}
