//! ISO-TP over a [`Transport`]: connects the state machines to a CAN bus.

use std::time::Duration;

use opendiag_core::{Error, Result};
use opendiag_transport::{Frame, Transport};

use super::{Config, Poll, Receiver, RxEvent, Sender};
use crate::clock::Clock;

/// A point-to-point ISO-TP channel using one CAN id each way (normal addressing).
pub struct IsoTpLink<T: Transport, C: Clock> {
    transport: T,
    clock: C,
    tx_id: u32,
    rx_id: u32,
    cfg: Config,
}

impl<T: Transport, C: Clock> IsoTpLink<T, C> {
    pub fn new(transport: T, clock: C, tx_id: u32, rx_id: u32, cfg: Config) -> Self {
        Self {
            transport,
            clock,
            tx_id,
            rx_id,
            cfg,
        }
    }

    /// CAN id requests are sent on.
    pub fn tx_id(&self) -> u32 {
        self.tx_id
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    fn send_can(&mut self, data: &[u8]) -> Result<()> {
        self.transport.send(&Frame::can(self.tx_id, data))
    }

    /// Next payload addressed to us within `wait`, or `None` on timeout.
    fn recv_payload(&mut self, wait: Duration) -> Result<Option<Vec<u8>>> {
        loop {
            match self.transport.recv(wait) {
                Ok(Frame::Can {
                    id,
                    extended: false,
                    data,
                }) if id == self.rx_id => return Ok(Some(data)),
                Ok(_) => continue, // other traffic on the bus
                Err(Error::Timeout) => return Ok(None),
                Err(e) => return Err(e),
            }
        }
    }

    /// Send one message, handling flow control, block size and STmin.
    pub fn send(&mut self, payload: &[u8]) -> Result<()> {
        let (mut sender, first) = Sender::new(payload, &self.cfg, self.clock.now())?;
        self.send_can(&first)?;
        loop {
            match sender.poll(self.clock.now())? {
                Poll::Done => return Ok(()),
                Poll::Send(frame) => self.send_can(&frame)?,
                Poll::WaitUntil(t) => {
                    let wait = t.saturating_sub(self.clock.now());
                    if let Some(data) = self.recv_payload(wait)? {
                        sender.on_frame(&data, self.clock.now())?;
                    } else {
                        self.clock.wait_until(t);
                    }
                }
            }
        }
    }

    /// Receive one message, sending flow control as needed.
    pub fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = self.clock.now() + timeout;
        let mut receiver = Receiver::new(self.cfg.clone());
        loop {
            let now = self.clock.now();
            receiver.poll(now)?;
            if now >= deadline {
                return Err(Error::Timeout);
            }
            let Some(data) = self.recv_payload(deadline - now)? else {
                self.clock.wait_until(deadline);
                continue;
            };
            match receiver.on_frame(&data, self.clock.now())? {
                RxEvent::Message(m) => return Ok(m),
                RxEvent::SendFlowControl(fc) => self.send_can(&fc)?,
                RxEvent::Pending => {}
            }
        }
    }

    /// Send a request and wait for the reply message.
    pub fn request(&mut self, payload: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        self.send(payload)?;
        self.recv(timeout)
    }
}
