//! Linux SocketCAN transport (CANable, candleLight, gs_usb, vcan, ...).
//!
//! The interface must already be up with the right bitrate, e.g.
//! `sudo ip link set can0 up type can bitrate 500000`. This driver does not
//! configure the bus, so `set_baud` is unsupported.

use std::io;
use std::time::Duration;

use opendiag_core::{Error, Result};
use socketcan::{CanFrame, CanSocket, EmbeddedFrame, ExtendedId, Id, Socket, StandardId};

use crate::{Capabilities, Frame, Transport};

pub struct SocketCan {
    socket: CanSocket,
}

impl SocketCan {
    /// Open a CAN interface by name, e.g. `can0` or `vcan0`.
    pub fn open(interface: &str) -> Result<Self> {
        let socket = CanSocket::open(interface)
            .map_err(|e| Error::Transport(format!("cannot open {interface}: {e}")))?;
        Ok(Self { socket })
    }
}

fn to_can_frame(id: u32, extended: bool, data: &[u8]) -> Result<CanFrame> {
    let id: Id = if extended {
        ExtendedId::new(id)
            .ok_or_else(|| Error::Transport(format!("invalid 29-bit id {id:#X}")))?
            .into()
    } else {
        u16::try_from(id)
            .ok()
            .and_then(StandardId::new)
            .ok_or_else(|| Error::Transport(format!("invalid 11-bit id {id:#X}")))?
            .into()
    };
    CanFrame::new(id, data)
        .ok_or_else(|| Error::Transport(format!("invalid CAN data length {}", data.len())))
}

/// Convert a received frame; remote and error frames are not diagnostic traffic.
fn from_can_frame(frame: &CanFrame) -> Option<Frame> {
    let CanFrame::Data(d) = frame else {
        return None;
    };
    let (id, extended) = match d.id() {
        Id::Standard(s) => (u32::from(s.as_raw()), false),
        Id::Extended(e) => (e.as_raw(), true),
    };
    Some(Frame::Can {
        id,
        extended,
        data: d.data().to_vec(),
    })
}

impl Transport for SocketCan {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        let Frame::Can { id, extended, data } = frame else {
            return Err(Error::Transport("SocketCAN carries only CAN frames".into()));
        };
        let f = to_can_frame(*id, *extended, data)?;
        self.socket
            .write_frame(&f)
            .map_err(|e| Error::Transport(e.to_string()))
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame> {
        // A zero timeout would mean "block forever" to the OS.
        let timeout = timeout.max(Duration::from_millis(1));
        self.socket
            .set_read_timeout(timeout)
            .map_err(|e| Error::Transport(e.to_string()))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.socket.read_frame() {
                Ok(f) => {
                    if let Some(frame) = from_can_frame(&f) {
                        return Ok(frame);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    return Err(Error::Timeout)
                }
                Err(e) => return Err(Error::Transport(e.to_string())),
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
        }
    }

    fn set_baud(&mut self, _baud: u32) -> Result<()> {
        Err(Error::Transport(
            "set the CAN bitrate with `ip link`, not through OpenDiag".into(),
        ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_missing_interface_fails_cleanly() {
        let err = SocketCan::open("opendiag-no-such-if")
            .err()
            .expect("should fail");
        assert!(matches!(err, Error::Transport(m) if m.contains("cannot open")));
    }

    #[test]
    fn frame_conversion_roundtrips_standard_and_extended() {
        for (id, extended) in [(0x7E0u32, false), (0x18DA_F110, true)] {
            let can = to_can_frame(id, extended, &[1, 2, 3]).unwrap();
            assert_eq!(
                from_can_frame(&can).unwrap(),
                Frame::Can {
                    id,
                    extended,
                    data: vec![1, 2, 3]
                }
            );
        }
    }

    #[test]
    fn invalid_ids_and_lengths_are_rejected() {
        assert!(to_can_frame(0x800, false, &[]).is_err(), "11-bit overflow");
        assert!(
            to_can_frame(0x2000_0000, true, &[]).is_err(),
            "29-bit overflow"
        );
        assert!(
            to_can_frame(0x7E0, false, &[0; 9]).is_err(),
            "classic CAN max 8"
        );
    }
}
