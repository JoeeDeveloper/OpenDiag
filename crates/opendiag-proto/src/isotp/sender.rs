//! ISO-TP sending state machine.

use std::time::Duration;

use opendiag_core::{Error, Result};

use super::frame::{FlowStatus, IsoTpFrame};
use super::{st_min_duration, Config, MAX_MESSAGE_LEN};

/// What the caller should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Put these bytes on the bus now.
    Send(Vec<u8>),
    /// Nothing to send until this absolute time (or until a frame arrives).
    WaitUntil(Duration),
    /// The whole message has been sent.
    Done,
}

#[derive(Debug)]
enum State {
    WaitFlowControl { deadline: Duration },
    Sending,
    Done,
    Failed,
}

/// Sends one message. Feed incoming frames to [`Sender::on_frame`] and call
/// [`Sender::poll`] whenever time passes.
#[derive(Debug)]
pub struct Sender {
    payload: Vec<u8>,
    offset: usize,
    next_sn: u8,
    state: State,
    block_size: u8,
    left_in_block: u8,
    st_min: Duration,
    next_send_at: Duration,
    wait_frames: u8,
    cfg: Config,
}

impl Sender {
    /// Start sending `payload`. Returns the sender and the first frame to transmit
    /// (a Single Frame, or a First Frame followed by Consecutive Frames after flow control).
    pub fn new(payload: &[u8], cfg: &Config, now: Duration) -> Result<(Self, Vec<u8>)> {
        if payload.is_empty() {
            return Err(Error::Protocol("isotp: empty message".into()));
        }
        if payload.len() > MAX_MESSAGE_LEN {
            return Err(Error::Protocol(format!(
                "isotp: message of {} bytes exceeds {MAX_MESSAGE_LEN}",
                payload.len()
            )));
        }
        let mut s = Self {
            payload: payload.to_vec(),
            offset: 0,
            next_sn: 1,
            state: State::Done,
            block_size: 0,
            left_in_block: 0,
            st_min: Duration::ZERO,
            next_send_at: now,
            wait_frames: 0,
            cfg: cfg.clone(),
        };
        let first = if payload.len() <= 7 {
            s.offset = payload.len();
            IsoTpFrame::Single {
                data: payload.to_vec(),
            }
        } else {
            s.offset = 6;
            s.state = State::WaitFlowControl {
                deadline: now + cfg.n_bs,
            };
            IsoTpFrame::First {
                total_len: payload.len(),
                data: payload[..6].to_vec(),
            }
        };
        Ok((s, first.encode(cfg.padding)))
    }

    /// Handle a frame received from the peer. Anything but a Flow Control
    /// frame while waiting for one is ignored as bus noise.
    pub fn on_frame(&mut self, bytes: &[u8], now: Duration) -> Result<()> {
        if !matches!(self.state, State::WaitFlowControl { .. }) {
            return Ok(());
        }
        let Ok(IsoTpFrame::FlowControl {
            status,
            block_size,
            st_min,
        }) = IsoTpFrame::decode(bytes)
        else {
            return Ok(());
        };
        match status {
            FlowStatus::ContinueToSend => {
                self.block_size = block_size;
                self.left_in_block = block_size;
                self.st_min = st_min_duration(st_min);
                self.wait_frames = 0;
                self.next_send_at = now;
                self.state = State::Sending;
            }
            FlowStatus::Wait => {
                self.wait_frames += 1;
                if self.wait_frames > self.cfg.max_wait_frames {
                    self.state = State::Failed;
                    return Err(Error::Protocol("isotp: too many flow control waits".into()));
                }
                self.state = State::WaitFlowControl {
                    deadline: now + self.cfg.n_bs,
                };
            }
            FlowStatus::Overflow => {
                self.state = State::Failed;
                return Err(Error::Protocol("isotp: receiver reported overflow".into()));
            }
        }
        Ok(())
    }

    /// Ask what to do at time `now`. Errors with `Timeout` if flow control never came.
    pub fn poll(&mut self, now: Duration) -> Result<Poll> {
        match self.state {
            State::Done => Ok(Poll::Done),
            State::Failed => Err(Error::Protocol("isotp: transfer already failed".into())),
            State::WaitFlowControl { deadline } => {
                if now >= deadline {
                    self.state = State::Failed;
                    Err(Error::Timeout)
                } else {
                    Ok(Poll::WaitUntil(deadline))
                }
            }
            State::Sending => {
                if now < self.next_send_at {
                    return Ok(Poll::WaitUntil(self.next_send_at));
                }
                let end = (self.offset + 7).min(self.payload.len());
                let frame = IsoTpFrame::Consecutive {
                    sn: self.next_sn,
                    data: self.payload[self.offset..end].to_vec(),
                };
                self.next_sn = (self.next_sn + 1) & 0x0F;
                self.offset = end;
                self.next_send_at = now + self.st_min;
                if self.offset >= self.payload.len() {
                    self.state = State::Done;
                } else if self.block_size > 0 {
                    self.left_in_block -= 1;
                    if self.left_in_block == 0 {
                        self.state = State::WaitFlowControl {
                            deadline: now + self.cfg.n_bs,
                        };
                    }
                }
                Ok(Poll::Send(frame.encode(self.cfg.padding)))
            }
        }
    }
}
