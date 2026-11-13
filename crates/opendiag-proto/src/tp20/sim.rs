//! A virtual TP2.0 module for tests and demos: answers channel setup, exchanges
//! parameters, then acts as a scripted KWP2000 responder. It models the protocol
//! as documented, not any real module, and records any mistakes the tester makes
//! in [`Tp20Sim::violations`].

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_transport::{Capabilities, Frame, Transport};

use super::frame::{
    decode_setup_request, encode_setup_reply, segment, ChannelMsg, ChannelParams, DataOp,
    SetupResponse, Timing, APP_KWP, SETUP_REQUEST_ID,
};

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    AwaitParams,
    Open,
    Closed,
}

pub struct Tp20Sim {
    address: u8,
    tester_tx_id: u16,
    params: ChannelParams,
    responses: HashMap<Vec<u8>, Vec<Vec<u8>>>,
    refuse_with: Option<u8>,
    drop_acks: bool,

    phase: Phase,
    module_tx_id: u16,
    inbox: VecDeque<Frame>,
    tx_queue: VecDeque<Vec<u8>>,
    awaiting_ack: bool,
    /// Sequence number the tester's next ACK must carry.
    expected_ack: u8,
    tx_seq: u8,
    rx_seq: u8,
    assembling: Vec<u8>,
    total: Option<usize>,

    sent: Vec<Frame>,
    messages: Vec<Vec<u8>>,
    violations: Vec<String>,
}

impl Tp20Sim {
    pub fn new(address: u8) -> Self {
        Self {
            address,
            tester_tx_id: 0x740,
            params: ChannelParams {
                block_size: 0x0F,
                t1: Timing(0x8A),
                t2: Timing::UNUSED,
                t3: Timing(0x4A), // 10 ms between packets, as in the documented capture
                t4: Timing::UNUSED,
            },
            responses: HashMap::new(),
            refuse_with: None,
            drop_acks: false,
            phase: Phase::Idle,
            module_tx_id: 0x300,
            inbox: VecDeque::new(),
            tx_queue: VecDeque::new(),
            awaiting_ack: false,
            expected_ack: 0,
            tx_seq: 0,
            rx_seq: 0,
            assembling: Vec::new(),
            total: None,
            sent: Vec::new(),
            messages: Vec::new(),
            violations: Vec::new(),
        }
    }

    /// Packets the tester must send before it waits for an ACK (and that this
    /// module sends before waiting for the tester's ACK).
    pub fn block_size(mut self, bs: u8) -> Self {
        self.params.block_size = bs;
        self
    }

    /// Answer `request` with one message.
    pub fn respond(self, request: &[u8], response: &[u8]) -> Self {
        self.respond_many(request, &[response])
    }

    /// Answer `request` with several messages in a row (e.g. ResponsePending, then the result).
    pub fn respond_many(mut self, request: &[u8], responses: &[&[u8]]) -> Self {
        self.responses.insert(
            request.to_vec(),
            responses.iter().map(|r| r.to_vec()).collect(),
        );
        self
    }

    /// Refuse channel setup with a negative opcode (0xD6..=0xD8).
    pub fn refuse_setup(mut self, opcode: u8) -> Self {
        self.refuse_with = Some(opcode);
        self
    }

    /// Ignore the tester's data packets entirely: no ACK and no reply.
    pub fn drop_acks(mut self) -> Self {
        self.drop_acks = true;
        self
    }

    /// Every frame the tester has sent.
    pub fn sent(&self) -> &[Frame] {
        &self.sent
    }

    /// Complete application messages received from the tester.
    pub fn received_messages(&self) -> &[Vec<u8>] {
        &self.messages
    }

    pub fn is_open(&self) -> bool {
        self.phase == Phase::Open
    }

    pub fn is_closed(&self) -> bool {
        self.phase == Phase::Closed
    }

    /// Protocol mistakes the tester made. Empty means a clean session.
    pub fn violations(&self) -> &[String] {
        &self.violations
    }

    fn push(&mut self, data: Vec<u8>) {
        self.inbox
            .push_back(Frame::can(u32::from(self.module_tx_id), &data));
    }

    /// Release queued packets until one that needs an ACK has gone out.
    fn pump(&mut self) {
        while !self.awaiting_ack {
            let Some(packet) = self.tx_queue.pop_front() else {
                break;
            };
            let wants_ack = matches!(packet[0] >> 4, 0 | 1);
            self.expected_ack = ((packet[0] & 0x0F) + 1) & 0x0F;
            self.push(packet);
            if wants_ack {
                self.awaiting_ack = true;
            }
        }
    }

    fn enqueue_message(&mut self, payload: &[u8]) {
        let bodies = segment(payload).expect("sim messages are valid");
        let block = usize::from(self.params.block_size.max(1));
        let mut in_block = 0;
        for (i, body) in bodies.iter().enumerate() {
            in_block += 1;
            let op = if i == bodies.len() - 1 {
                DataOp::WaitAckLast
            } else if in_block >= block {
                DataOp::WaitAckMore
            } else {
                DataOp::NoAckMore
            };
            if op.wants_ack() {
                in_block = 0;
            }
            let packet = ChannelMsg::Data {
                op,
                seq: self.tx_seq,
                payload: body.clone(),
            }
            .encode();
            self.tx_seq = (self.tx_seq + 1) & 0x0F;
            self.tx_queue.push_back(packet);
        }
    }

    fn violate(&mut self, msg: impl Into<String>) {
        self.violations.push(msg.into());
    }

    fn on_setup(&mut self, data: &[u8]) {
        let req = match decode_setup_request(data) {
            Ok(r) => r,
            Err(e) => return self.violate(e.to_string()),
        };
        if req.dest != self.address {
            return; // not for us; no answer
        }
        let reply_id = SETUP_REQUEST_ID + u32::from(self.address);
        if let Some(op) = self.refuse_with {
            let refusal = [req.dest, op, 0, 0x10, 0, 0x10, APP_KWP];
            self.inbox.push_back(Frame::can(reply_id, &refusal));
            return;
        }
        let Some(tx) = req.tx else {
            return self.violate("setup request gave no transmit id");
        };
        self.module_tx_id = tx;
        let reply = SetupResponse {
            dest: 0,
            rx: Some(tx),
            tx: Some(self.tester_tx_id),
            app: APP_KWP,
        };
        self.inbox
            .push_back(Frame::can(reply_id, &encode_setup_reply(&reply)));
        self.phase = Phase::AwaitParams;
    }

    fn on_channel(&mut self, data: &[u8]) {
        let msg = match ChannelMsg::decode(data) {
            Ok(m) => m,
            Err(e) => return self.violate(e.to_string()),
        };
        match msg {
            ChannelMsg::ParamsRequest(_) => {
                self.phase = Phase::Open;
                self.push(ChannelMsg::ParamsResponse(self.params).encode());
            }
            ChannelMsg::Test => self.push(ChannelMsg::ParamsResponse(self.params).encode()),
            ChannelMsg::Disconnect => {
                self.phase = Phase::Closed;
                self.push(ChannelMsg::Disconnect.encode());
            }
            ChannelMsg::Break => {
                self.assembling.clear();
                self.total = None;
            }
            ChannelMsg::Ack { ready, seq } => {
                if !self.awaiting_ack {
                    return self.violate("unexpected ACK");
                }
                if ready && seq != self.expected_ack {
                    self.violate(format!(
                        "ACK sequence {seq}, expected {}",
                        self.expected_ack
                    ));
                }
                if ready {
                    self.awaiting_ack = false;
                    self.pump();
                }
            }
            ChannelMsg::Data { op, seq, payload } => self.on_data(op, seq, &payload),
            ChannelMsg::ParamsResponse(_) => self.violate("tester sent a parameters response"),
        }
    }

    fn on_data(&mut self, op: DataOp, seq: u8, payload: &[u8]) {
        if self.drop_acks {
            return;
        }
        if self.phase != Phase::Open {
            return self.violate("data before the channel is open");
        }
        if seq != self.rx_seq {
            return self.violate(format!("tester sequence {seq}, expected {}", self.rx_seq));
        }
        self.rx_seq = (self.rx_seq + 1) & 0x0F;
        let body = match self.total {
            None => {
                let [hi, lo, rest @ ..] = payload else {
                    return self.violate("first packet too short");
                };
                self.total = Some(usize::from(u16::from_be_bytes([*hi, *lo])));
                rest
            }
            Some(_) => payload,
        };
        self.assembling.extend_from_slice(body);
        if op.wants_ack() {
            self.push(
                ChannelMsg::Ack {
                    ready: true,
                    seq: self.rx_seq,
                }
                .encode(),
            );
        }
        if self.total.is_some_and(|t| self.assembling.len() >= t) {
            let request = std::mem::take(&mut self.assembling);
            self.total = None;
            self.messages.push(request.clone());
            let replies = self.responses.get(&request).cloned().unwrap_or_else(|| {
                // serviceNotSupported
                vec![vec![0x7F, request.first().copied().unwrap_or(0), 0x11]]
            });
            for r in replies {
                self.enqueue_message(&r);
            }
            self.pump();
        }
    }
}

impl Transport for Tp20Sim {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.sent.push(frame.clone());
        let Frame::Can {
            id,
            extended: false,
            data,
        } = frame
        else {
            return Err(Error::Transport("TP2.0 runs on standard CAN frames".into()));
        };
        if *id == SETUP_REQUEST_ID {
            self.on_setup(data);
        } else if *id == u32::from(self.tester_tx_id) {
            self.on_channel(data);
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
        Err(Error::Transport("slow init is a K-line feature".into()))
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can: true,
            ..Capabilities::default()
        }
    }
}
