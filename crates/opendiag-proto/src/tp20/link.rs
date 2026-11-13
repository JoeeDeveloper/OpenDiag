//! A TP2.0 channel over a CAN [`Transport`].

use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_transport::{Frame, Transport};

use super::frame::{
    segment, ChannelMsg, ChannelParams, DataOp, SetupReply, SetupRequest, Timing, APP_KWP,
    SETUP_REQUEST_ID,
};
use crate::clock::Clock;

fn protocol(msg: impl Into<String>) -> Error {
    Error::Protocol(format!("tp20: {}", msg.into()))
}

#[derive(Debug, Clone)]
pub struct Tp20Config {
    /// Logical address of the module, e.g. 0x01 for the engine.
    pub dest: u8,
    /// CAN id we ask the module to transmit on.
    pub module_tx_id: u16,
    /// Packets we ask the module to send before expecting an ACK.
    pub block_size: u8,
    pub t1: Timing,
    pub t3: Timing,
    /// How long to wait for the module's answer during channel setup.
    pub setup_timeout: Duration,
    /// Floor for the ACK timeout, whatever the module reports.
    pub min_ack_timeout: Duration,
}

impl Tp20Config {
    pub fn new(dest: u8) -> Self {
        Self {
            dest,
            module_tx_id: 0x300,
            // The values shown in the documented capture.
            block_size: 0x0F,
            t1: Timing(0x8A),
            t3: Timing(0x32),
            setup_timeout: Duration::from_millis(1000),
            min_ack_timeout: Duration::from_millis(50),
        }
    }
}

/// An open channel to one module. The channel is closed (best effort) on drop.
pub struct Tp20Link<T: Transport, C: Clock> {
    transport: T,
    clock: C,
    tx_id: u32,
    rx_id: u32,
    peer: ChannelParams,
    ack_timeout: Duration,
    tx_seq: u8,
    rx_seq: u8,
    last_send: Option<Duration>,
}

impl<T: Transport, C: Clock> Tp20Link<T, C> {
    /// Open a channel: setup request, then parameter exchange.
    pub fn open(mut transport: T, clock: C, cfg: &Tp20Config) -> Result<Self> {
        let reply_id = SETUP_REQUEST_ID + u32::from(cfg.dest);
        let request = SetupRequest {
            dest: cfg.dest,
            rx: None,
            tx: Some(cfg.module_tx_id),
            app: APP_KWP,
        };
        transport.send(&Frame::can(SETUP_REQUEST_ID, &request.encode()))?;

        let deadline = clock.now() + cfg.setup_timeout;
        let data = recv_on(&mut transport, &clock, reply_id, deadline)?;
        let reply = match SetupReply::decode(&data)? {
            SetupReply::Positive(r) => r,
            SetupReply::Refused { opcode } => {
                return Err(protocol(format!("channel setup refused ({opcode:#04X})")));
            }
        };
        let (Some(rx), Some(tx)) = (reply.rx, reply.tx) else {
            return Err(protocol("setup reply did not give both CAN ids"));
        };
        let (rx_id, tx_id) = (u32::from(rx), u32::from(tx));

        let ours = ChannelParams {
            block_size: cfg.block_size,
            t1: cfg.t1,
            t2: Timing::UNUSED,
            t3: cfg.t3,
            t4: Timing::UNUSED,
        };
        transport.send(&Frame::can(
            tx_id,
            &ChannelMsg::ParamsRequest(ours).encode(),
        ))?;
        let deadline = clock.now() + cfg.setup_timeout;
        let peer = loop {
            let data = recv_on(&mut transport, &clock, rx_id, deadline)?;
            if let ChannelMsg::ParamsResponse(p) = ChannelMsg::decode(&data)? {
                break p;
            }
        };
        let ack_timeout = peer
            .t1
            .duration()
            .unwrap_or(cfg.setup_timeout)
            .max(cfg.min_ack_timeout);
        Ok(Self {
            transport,
            clock,
            tx_id,
            rx_id,
            peer,
            ack_timeout,
            tx_seq: 0,
            rx_seq: 0,
            last_send: None,
        })
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    /// The parameters the module reported.
    pub fn peer_params(&self) -> ChannelParams {
        self.peer
    }

    fn send_msg(&mut self, msg: &ChannelMsg) -> Result<()> {
        self.transport.send(&Frame::can(self.tx_id, &msg.encode()))
    }

    fn recv_msg(&mut self, deadline: Duration) -> Result<ChannelMsg> {
        let data = recv_on(&mut self.transport, &self.clock, self.rx_id, deadline)?;
        ChannelMsg::decode(&data)
    }

    /// Wait for the module's ACK of everything sent so far.
    fn wait_ack(&mut self) -> Result<()> {
        let mut deadline = self.clock.now() + self.ack_timeout;
        loop {
            match self.recv_msg(deadline)? {
                ChannelMsg::Ack { ready: true, seq } if seq == self.tx_seq => return Ok(()),
                ChannelMsg::Ack { ready: true, seq } => {
                    return Err(protocol(format!(
                        "ACK for sequence {seq}, expected {}",
                        self.tx_seq
                    )))
                }
                // "Not ready": keep waiting for the real ACK.
                ChannelMsg::Ack { ready: false, .. } => {
                    deadline = self.clock.now() + self.ack_timeout;
                }
                ChannelMsg::Disconnect => return Err(protocol("module closed the channel")),
                ChannelMsg::Break => return Err(protocol("module sent a break")),
                ChannelMsg::Data { .. } => return Err(protocol("data while waiting for an ACK")),
                _ => {}
            }
        }
    }

    /// Honour the module's minimum interval between our packets.
    fn wait_packet_gap(&self) {
        if let (Some(last), Some(gap)) = (self.last_send, self.peer.t3.duration()) {
            self.clock.wait_until(last + gap);
        }
    }

    /// Send one message, splitting it into packets and waiting for ACKs.
    pub fn send(&mut self, payload: &[u8]) -> Result<()> {
        let packets = segment(payload)?;
        let block = usize::from(self.peer.block_size.max(1));
        let mut in_block = 0;
        for (i, body) in packets.iter().enumerate() {
            self.wait_packet_gap();
            in_block += 1;
            let op = if i == packets.len() - 1 {
                DataOp::WaitAckLast
            } else if in_block >= block {
                DataOp::WaitAckMore
            } else {
                DataOp::NoAckMore
            };
            let msg = ChannelMsg::Data {
                op,
                seq: self.tx_seq,
                payload: body.clone(),
            };
            self.tx_seq = (self.tx_seq + 1) & 0x0F;
            self.send_msg(&msg)?;
            self.last_send = Some(self.clock.now());
            if op.wants_ack() {
                self.wait_ack()?;
                in_block = 0;
            }
        }
        Ok(())
    }

    /// Receive one message, acknowledging packets as the module asks.
    pub fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let mut deadline = self.clock.now() + timeout;
        let mut buf = Vec::new();
        let mut total: Option<usize> = None;
        loop {
            match self.recv_msg(deadline)? {
                ChannelMsg::Data { op, seq, payload } => {
                    if seq != self.rx_seq {
                        return Err(protocol(format!(
                            "packet sequence {seq}, expected {}",
                            self.rx_seq
                        )));
                    }
                    self.rx_seq = (self.rx_seq + 1) & 0x0F;
                    let body = match total {
                        None => {
                            let [hi, lo, rest @ ..] = payload.as_slice() else {
                                return Err(protocol("first packet too short for a length"));
                            };
                            total = Some(usize::from(u16::from_be_bytes([*hi, *lo])));
                            rest
                        }
                        Some(_) => payload.as_slice(),
                    };
                    buf.extend_from_slice(body);
                    if op.wants_ack() {
                        let ack = ChannelMsg::Ack {
                            ready: true,
                            seq: self.rx_seq,
                        };
                        self.send_msg(&ack)?;
                    }
                    if total.is_some_and(|t| buf.len() >= t) {
                        buf.truncate(total.unwrap_or(0));
                        return Ok(buf);
                    }
                    // Between packets of one message, allow the usual ACK window.
                    deadline = self.clock.now() + self.ack_timeout;
                }
                ChannelMsg::Disconnect => return Err(protocol("module closed the channel")),
                ChannelMsg::Break => return Err(protocol("module sent a break")),
                _ => {}
            }
        }
    }

    /// Send a message and wait for the reply message.
    pub fn request(&mut self, payload: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        self.send(payload)?;
        self.recv(timeout)
    }

    /// Channel test: keeps the channel alive and checks the module still answers.
    pub fn channel_test(&mut self) -> Result<()> {
        self.send_msg(&ChannelMsg::Test)?;
        let deadline = self.clock.now() + self.ack_timeout;
        loop {
            if let ChannelMsg::ParamsResponse(_) = self.recv_msg(deadline)? {
                return Ok(());
            }
        }
    }

    /// Close the channel and wait briefly for the module's disconnect.
    pub fn close(&mut self) -> Result<()> {
        self.send_msg(&ChannelMsg::Disconnect)?;
        let deadline = self.clock.now() + self.ack_timeout;
        loop {
            if let ChannelMsg::Disconnect = self.recv_msg(deadline)? {
                return Ok(());
            }
        }
    }
}

impl<T: Transport, C: Clock> Drop for Tp20Link<T, C> {
    fn drop(&mut self) {
        // Best effort, and no waiting: the module also times the channel out.
        let _ = self.send_msg(&ChannelMsg::Disconnect);
    }
}

/// Next frame on `id` before `deadline`. On a timeout the clock is advanced to
/// the deadline, so fake-clock tests observe the wait.
fn recv_on<T: Transport, C: Clock>(
    transport: &mut T,
    clock: &C,
    id: u32,
    deadline: Duration,
) -> Result<Vec<u8>> {
    loop {
        let now = clock.now();
        if now >= deadline {
            return Err(Error::Timeout);
        }
        match transport.recv(deadline - now) {
            Ok(Frame::Can {
                id: got,
                extended: false,
                data,
            }) if got == id => return Ok(data),
            Ok(_) => {}
            Err(Error::Timeout) => {
                clock.wait_until(deadline);
                return Err(Error::Timeout);
            }
            Err(e) => return Err(e),
        }
    }
}
