//! Transport layer: the `Transport` trait plus hardware-free implementations
//! (`Simulator`, `Replay`). Real drivers (SocketCAN, K-line) come later.

mod replay;
mod serial;
mod simulator;
#[cfg(target_os = "linux")]
mod socketcan_driver;
mod trace;

use std::time::Duration;

use opendiag_core::Result;

pub use replay::Replay;
pub use serial::{list_serial_ports, KLine, KLineSerial, PortInfo, SerialIo};
pub use simulator::Simulator;
#[cfg(target_os = "linux")]
pub use socketcan_driver::SocketCan;
pub use trace::{
    format_frame, parse_trace, TraceEntry, TraceEvent, TraceReplay, TraceStatus, Tracing,
    TRACE_HEADER,
};

/// A unit of data on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A CAN frame. `extended` selects 29-bit identifiers.
    Can {
        id: u32,
        extended: bool,
        data: Vec<u8>,
    },
    /// Raw bytes on a serial line (K-line: KW1281 / KWP2000).
    Raw(Vec<u8>),
}

impl Frame {
    /// Convenience constructor for a standard (11-bit) CAN frame.
    pub fn can(id: u32, data: &[u8]) -> Self {
        Frame::Can {
            id,
            extended: false,
            data: data.to_vec(),
        }
    }
}

/// What a transport can do. Protocol code checks this before relying on a feature.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub can: bool,
    pub kline: bool,
    pub set_baud: bool,
    pub slow_init: bool,
    pub fast_init: bool,
}

/// Hardware abstraction. Protocol code depends only on this trait.
///
/// `recv` returns `Error::Timeout` when nothing arrives within `timeout`.
/// Hardware-free implementations never sleep: they report a timeout
/// immediately, which keeps tests deterministic.
pub trait Transport {
    fn send(&mut self, frame: &Frame) -> Result<()>;
    fn recv(&mut self, timeout: Duration) -> Result<Frame>;
    fn set_baud(&mut self, baud: u32) -> Result<()>;
    /// ISO 9141 5-baud initialisation addressed to `address`.
    fn slow_init(&mut self, address: u8) -> Result<()>;
    /// ISO 14230 fast init wake-up pattern (25 ms low, 25 ms high). The caller
    /// sends StartCommunication afterwards. Unsupported by default.
    fn fast_init(&mut self) -> Result<()> {
        Err(opendiag_core::Error::Transport(
            "fast init not supported by this transport".into(),
        ))
    }
    fn capabilities(&self) -> Capabilities;
}

/// A boxed transport is a transport, so code can pick one at run time.
impl<T: Transport + ?Sized> Transport for Box<T> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        (**self).send(frame)
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame> {
        (**self).recv(timeout)
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        (**self).set_baud(baud)
    }

    fn slow_init(&mut self, address: u8) -> Result<()> {
        (**self).slow_init(address)
    }

    fn fast_init(&mut self) -> Result<()> {
        (**self).fast_init()
    }

    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }
}

/// Open a CAN interface by name. SocketCAN exists only on Linux; elsewhere this
/// returns a clear error instead of failing to compile callers.
pub fn open_can(interface: &str) -> Result<Box<dyn Transport>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(SocketCan::open(interface)?))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = interface;
        Err(opendiag_core::Error::Transport(
            "CAN interfaces (SocketCAN) are only available on Linux".into(),
        ))
    }
}
