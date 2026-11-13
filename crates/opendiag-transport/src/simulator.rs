//! Simulator: virtual ECUs with scripted request -> response rules.

use std::collections::VecDeque;
use std::time::Duration;

use opendiag_core::{Error, Result};

use crate::{Capabilities, Frame, Transport};

struct Rule {
    request: Frame,
    responses: Vec<Frame>,
}

/// A scriptable fake bus. Sending a frame that matches a rule queues that
/// rule's responses for `recv`. Unmatched requests are ignored (the ECU
/// "does not answer"), so the caller sees a timeout.
#[derive(Default)]
pub struct Simulator {
    rules: Vec<Rule>,
    inbox: VecDeque<Frame>,
    sent: Vec<Frame>,
    baud: Option<u32>,
    slow_inits: Vec<u8>,
}

impl Simulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script a rule: when `request` is sent, answer with `responses` in order.
    /// Later rules for the same request override earlier ones.
    pub fn on(mut self, request: Frame, responses: Vec<Frame>) -> Self {
        self.rules.retain(|r| r.request != request);
        self.rules.push(Rule { request, responses });
        self
    }

    /// Every frame the code under test has sent, in order.
    pub fn sent(&self) -> &[Frame] {
        &self.sent
    }

    pub fn baud(&self) -> Option<u32> {
        self.baud
    }

    pub fn slow_inits(&self) -> &[u8] {
        &self.slow_inits
    }
}

impl Transport for Simulator {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.sent.push(frame.clone());
        if let Some(rule) = self.rules.iter().find(|r| &r.request == frame) {
            self.inbox.extend(rule.responses.iter().cloned());
        }
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Frame> {
        self.inbox.pop_front().ok_or(Error::Timeout)
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.baud = Some(baud);
        Ok(())
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        self.slow_inits.push(address);
        Ok(())
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can: true,
            kline: true,
            set_baud: true,
            slow_init: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Duration = Duration::from_millis(100);

    #[test]
    fn answers_scripted_request() {
        let req = Frame::can(0x7E0, &[0x02, 0x10, 0x03]);
        let resp = Frame::can(0x7E8, &[0x02, 0x50, 0x03]);
        let mut sim = Simulator::new().on(req.clone(), vec![resp.clone()]);
        sim.send(&req).unwrap();
        assert_eq!(sim.recv(T).unwrap(), resp);
    }

    #[test]
    fn unmatched_request_times_out() {
        let mut sim = Simulator::new();
        sim.send(&Frame::can(0x7E0, &[0x01])).unwrap();
        assert!(matches!(sim.recv(T), Err(Error::Timeout)));
    }

    #[test]
    fn multi_frame_responses_arrive_in_order() {
        let req = Frame::can(0x7E0, &[0x01]);
        let a = Frame::can(0x7E8, &[0xA]);
        let b = Frame::can(0x7E8, &[0xB]);
        let mut sim = Simulator::new().on(req.clone(), vec![a.clone(), b.clone()]);
        sim.send(&req).unwrap();
        assert_eq!(sim.recv(T).unwrap(), a);
        assert_eq!(sim.recv(T).unwrap(), b);
        assert!(matches!(sim.recv(T), Err(Error::Timeout)));
    }

    #[test]
    fn later_rule_overrides_earlier() {
        let req = Frame::can(0x1, &[1]);
        let mut sim = Simulator::new()
            .on(req.clone(), vec![Frame::can(0x2, &[1])])
            .on(req.clone(), vec![Frame::can(0x2, &[2])]);
        sim.send(&req).unwrap();
        assert_eq!(sim.recv(T).unwrap(), Frame::can(0x2, &[2]));
    }

    #[test]
    fn records_sent_frames_baud_and_slow_init() {
        let mut sim = Simulator::new();
        sim.send(&Frame::Raw(vec![0x55])).unwrap();
        sim.set_baud(10400).unwrap();
        sim.slow_init(0x01).unwrap();
        assert_eq!(sim.sent(), &[Frame::Raw(vec![0x55])]);
        assert_eq!(sim.baud(), Some(10400));
        assert_eq!(sim.slow_inits(), &[0x01]);
    }
}
