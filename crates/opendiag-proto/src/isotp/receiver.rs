//! ISO-TP receiving state machine.

use std::time::Duration;

use opendiag_core::{Error, Result};

use super::frame::{FlowStatus, IsoTpFrame};
use super::Config;

/// Result of feeding one frame to the receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RxEvent {
    /// Frame consumed; more are needed (or it was ignored).
    Pending,
    /// A complete message.
    Message(Vec<u8>),
    /// Transmit these bytes (a Flow Control frame) to the sender.
    SendFlowControl(Vec<u8>),
}

#[derive(Debug)]
enum State {
    Idle,
    Receiving {
        expected: usize,
        next_sn: u8,
        left_in_block: u8,
        deadline: Duration,
    },
}

/// Reassembles messages. Feed every frame from the peer to [`Receiver::on_frame`]
/// and call [`Receiver::poll`] as time passes to detect a stalled transfer.
#[derive(Debug)]
pub struct Receiver {
    cfg: Config,
    buf: Vec<u8>,
    state: State,
}

impl Receiver {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            buf: Vec::new(),
            state: State::Idle,
        }
    }

    fn flow_control(&self, status: FlowStatus) -> RxEvent {
        RxEvent::SendFlowControl(
            IsoTpFrame::FlowControl {
                status,
                block_size: self.cfg.block_size,
                st_min: self.cfg.st_min,
            }
            .encode(self.cfg.padding),
        )
    }

    pub fn on_frame(&mut self, bytes: &[u8], now: Duration) -> Result<RxEvent> {
        let frame = IsoTpFrame::decode(bytes)?;
        match frame {
            IsoTpFrame::Single { data } => {
                // A new message abandons any transfer in progress.
                self.state = State::Idle;
                Ok(RxEvent::Message(data))
            }
            IsoTpFrame::First { total_len, data } => {
                if total_len > self.cfg.max_len {
                    self.state = State::Idle;
                    return Ok(self.flow_control(FlowStatus::Overflow));
                }
                self.buf.clear();
                self.buf.extend_from_slice(&data);
                self.state = State::Receiving {
                    expected: total_len,
                    next_sn: 1,
                    left_in_block: self.cfg.block_size,
                    deadline: now + self.cfg.n_cr,
                };
                Ok(self.flow_control(FlowStatus::ContinueToSend))
            }
            IsoTpFrame::Consecutive { sn, data } => {
                let State::Receiving {
                    expected,
                    next_sn,
                    left_in_block,
                    deadline,
                } = &mut self.state
                else {
                    return Ok(RxEvent::Pending);
                };
                if sn != *next_sn {
                    let want = *next_sn;
                    self.state = State::Idle;
                    return Err(Error::Protocol(format!(
                        "isotp: sequence error, expected {want} got {sn}"
                    )));
                }
                let take = data.len().min(*expected - self.buf.len());
                self.buf.extend_from_slice(&data[..take]);
                *next_sn = (*next_sn + 1) & 0x0F;
                *deadline = now + self.cfg.n_cr;
                if self.buf.len() >= *expected {
                    self.state = State::Idle;
                    return Ok(RxEvent::Message(std::mem::take(&mut self.buf)));
                }
                if self.cfg.block_size > 0 {
                    *left_in_block -= 1;
                    if *left_in_block == 0 {
                        *left_in_block = self.cfg.block_size;
                        return Ok(self.flow_control(FlowStatus::ContinueToSend));
                    }
                }
                Ok(RxEvent::Pending)
            }
            IsoTpFrame::FlowControl { .. } => Ok(RxEvent::Pending),
        }
    }

    /// Detect a stalled transfer. Errors with `Timeout` and resets to idle.
    pub fn poll(&mut self, now: Duration) -> Result<()> {
        if let State::Receiving { deadline, .. } = self.state {
            if now >= deadline {
                self.state = State::Idle;
                self.buf.clear();
                return Err(Error::Timeout);
            }
        }
        Ok(())
    }
}
