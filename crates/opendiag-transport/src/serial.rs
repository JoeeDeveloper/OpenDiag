//! K-line over a USB serial adapter (FTDI-style K-line cable).
//!
//! The wake-up pattern for ISO 14230 fast init is made by holding the TX line
//! in break (low) for 25 ms, then high for 25 ms. K-line cables echo every
//! transmitted byte back on RX; this driver passes the echo through and the
//! protocol layer ignores it. 5-baud slow init is not implemented yet.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

use opendiag_core::{Error, Result};

use crate::{Capabilities, Frame, Transport};

/// Time TX is held low during fast init (TiniL).
const FAST_INIT_LOW: Duration = Duration::from_millis(25);
/// Time TX is held high before StartCommunication (TWuP - TiniL).
const FAST_INIT_HIGH: Duration = Duration::from_millis(25);

/// The few serial-port operations the driver needs. Implemented for real
/// ports and for test mocks.
pub trait SerialIo {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Read up to `buf.len()` bytes, waiting at most `timeout`.
    /// Must return `ErrorKind::TimedOut` (or `Ok(0)`) when nothing arrived.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
    fn set_baud(&mut self, baud: u32) -> io::Result<()>;
    /// Hold the TX line low (`true`) or release it (`false`).
    fn set_break(&mut self, on: bool) -> io::Result<()>;
    /// Discard anything received but not yet read.
    fn clear_input(&mut self) -> io::Result<()>;
}

impl SerialIo for Box<dyn serialport::SerialPort> {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        io::Write::write_all(self.as_mut(), data)?;
        io::Write::flush(self.as_mut())
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        self.set_timeout(timeout).map_err(to_io)?;
        io::Read::read(self.as_mut(), buf)
    }

    fn set_baud(&mut self, baud: u32) -> io::Result<()> {
        self.set_baud_rate(baud).map_err(to_io)
    }

    fn set_break(&mut self, on: bool) -> io::Result<()> {
        if on {
            serialport::SerialPort::set_break(self.as_mut())
        } else {
            serialport::SerialPort::clear_break(self.as_mut())
        }
        .map_err(to_io)
    }

    fn clear_input(&mut self) -> io::Result<()> {
        self.clear(serialport::ClearBuffer::Input).map_err(to_io)
    }
}

fn to_io(e: serialport::Error) -> io::Error {
    io::Error::other(e)
}

fn transport_err(e: io::Error) -> Error {
    Error::Transport(e.to_string())
}

/// Sleep for `d` with sub-millisecond accuracy: sleep most of it, spin the rest.
/// OS sleep granularity (especially on Windows) is too coarse for 25 ms pulses.
fn precise_sleep(d: Duration) {
    let end = Instant::now() + d;
    let coarse = d.saturating_sub(Duration::from_millis(2));
    if !coarse.is_zero() {
        thread::sleep(coarse);
    }
    while Instant::now() < end {
        std::hint::spin_loop();
    }
}

/// K-line transport over any [`SerialIo`].
pub struct KLine<P: SerialIo> {
    port: P,
}

/// K-line on a real serial port.
pub type KLineSerial = KLine<Box<dyn serialport::SerialPort>>;

impl KLineSerial {
    /// Open `path` (e.g. `COM3` or `/dev/ttyUSB0`) at `baud` (10400 is typical), 8N1.
    pub fn open(path: &str, baud: u32) -> Result<Self> {
        let port = serialport::new(path, baud)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| Error::Transport(format!("cannot open {path}: {e}")))?;
        Ok(Self::new(port))
    }
}

impl<P: SerialIo> KLine<P> {
    pub fn new(port: P) -> Self {
        Self { port }
    }

    pub fn port(&self) -> &P {
        &self.port
    }
}

impl<P: SerialIo> Transport for KLine<P> {
    fn send(&mut self, frame: &Frame) -> Result<()> {
        match frame {
            Frame::Raw(bytes) => self.port.write_all(bytes).map_err(transport_err),
            Frame::Can { .. } => Err(Error::Transport("K-line cannot carry CAN frames".into())),
        }
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame> {
        let mut buf = [0u8; 256];
        match self.port.read(&mut buf, timeout) {
            Ok(0) => Err(Error::Timeout),
            Ok(n) => Ok(Frame::Raw(buf[..n].to_vec())),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Err(Error::Timeout),
            Err(e) => Err(transport_err(e)),
        }
    }

    fn set_baud(&mut self, baud: u32) -> Result<()> {
        self.port.set_baud(baud).map_err(transport_err)
    }

    fn slow_init(&mut self, _address: u8) -> Result<()> {
        Err(Error::Transport(
            "5-baud slow init is not implemented yet".into(),
        ))
    }

    fn fast_init(&mut self) -> Result<()> {
        self.port.set_break(true).map_err(transport_err)?;
        precise_sleep(FAST_INIT_LOW);
        self.port.set_break(false).map_err(transport_err)?;
        precise_sleep(FAST_INIT_HIGH);
        // The break shows up on RX as a framing-error byte; drop it.
        self.port.clear_input().map_err(transport_err)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            can: false,
            kline: true,
            set_baud: true,
            slow_init: false,
            fast_init: true,
        }
    }
}

/// A serial port found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    pub name: String,
    pub description: String,
}

/// List serial ports so users can pick their cable.
pub fn list_serial_ports() -> Result<Vec<PortInfo>> {
    let ports = serialport::available_ports()
        .map_err(|e| Error::Transport(format!("cannot list serial ports: {e}")))?;
    Ok(ports
        .into_iter()
        .map(|p| {
            let description = match p.port_type {
                serialport::SerialPortType::UsbPort(u) => {
                    let name = [u.manufacturer, u.product]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("USB {:04X}:{:04X} {name}", u.vid, u.pid)
                        .trim_end()
                        .to_string()
                }
                serialport::SerialPortType::BluetoothPort => "Bluetooth".into(),
                serialport::SerialPortType::PciPort => "PCI".into(),
                serialport::SerialPortType::Unknown => "unknown".into(),
            };
            PortInfo {
                name: p.port_name,
                description,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Debug, PartialEq)]
    enum Event {
        Break(bool),
        ClearInput,
        Write(Vec<u8>),
        Baud(u32),
    }

    #[derive(Default)]
    struct MockPort {
        events: Vec<(Instant, Event)>,
        incoming: VecDeque<io::Result<Vec<u8>>>,
    }

    impl SerialIo for MockPort {
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            self.events
                .push((Instant::now(), Event::Write(data.to_vec())));
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8], _t: Duration) -> io::Result<usize> {
            match self.incoming.pop_front() {
                Some(Ok(data)) => {
                    buf[..data.len()].copy_from_slice(&data);
                    Ok(data.len())
                }
                Some(Err(e)) => Err(e),
                None => Err(io::ErrorKind::TimedOut.into()),
            }
        }
        fn set_baud(&mut self, baud: u32) -> io::Result<()> {
            self.events.push((Instant::now(), Event::Baud(baud)));
            Ok(())
        }
        fn set_break(&mut self, on: bool) -> io::Result<()> {
            self.events.push((Instant::now(), Event::Break(on)));
            Ok(())
        }
        fn clear_input(&mut self) -> io::Result<()> {
            self.events.push((Instant::now(), Event::ClearInput));
            Ok(())
        }
    }

    const T: Duration = Duration::from_millis(10);

    #[test]
    fn sends_raw_bytes_and_rejects_can_frames() {
        let mut k = KLine::new(MockPort::default());
        k.send(&Frame::Raw(vec![0x81, 0x01])).unwrap();
        assert_eq!(k.port().events[0].1, Event::Write(vec![0x81, 0x01]));
        assert!(k.send(&Frame::can(1, &[0])).is_err());
    }

    #[test]
    fn recv_returns_bytes_and_maps_timeouts() {
        let mut port = MockPort::default();
        port.incoming.push_back(Ok(vec![1, 2, 3]));
        port.incoming.push_back(Ok(vec![]));
        let mut k = KLine::new(port);
        assert_eq!(k.recv(T).unwrap(), Frame::Raw(vec![1, 2, 3]));
        assert!(
            matches!(k.recv(T), Err(Error::Timeout)),
            "Ok(0) is a timeout"
        );
        assert!(
            matches!(k.recv(T), Err(Error::Timeout)),
            "TimedOut is a timeout"
        );
    }

    #[test]
    fn other_read_errors_are_transport_errors() {
        let mut port = MockPort::default();
        port.incoming
            .push_back(Err(io::Error::new(io::ErrorKind::BrokenPipe, "unplugged")));
        let err = KLine::new(port).recv(T).unwrap_err();
        assert!(matches!(err, Error::Transport(m) if m.contains("unplugged")));
    }

    #[test]
    fn fast_init_pulses_break_then_flushes_input() {
        let mut k = KLine::new(MockPort::default());
        let start = Instant::now();
        k.fast_init().unwrap();
        let total = start.elapsed();

        let ev = &k.port().events;
        let kinds: Vec<&Event> = ev.iter().map(|(_, e)| e).collect();
        assert_eq!(
            kinds,
            vec![
                &Event::Break(true),
                &Event::Break(false),
                &Event::ClearInput
            ]
        );
        let low = ev[1].0 - ev[0].0;
        let high = ev[2].0 - ev[1].0;
        assert!(low >= FAST_INIT_LOW, "low pulse {low:?}");
        assert!(high >= FAST_INIT_HIGH, "high period {high:?}");
        assert!(total >= Duration::from_millis(50), "total {total:?}");
    }

    #[test]
    fn baud_is_forwarded_and_slow_init_is_unsupported() {
        let mut k = KLine::new(MockPort::default());
        k.set_baud(10400).unwrap();
        assert_eq!(k.port().events[0].1, Event::Baud(10400));
        assert!(k.slow_init(0x01).is_err());
        let caps = k.capabilities();
        assert!(caps.kline && caps.fast_init && !caps.can && !caps.slow_init);
    }
}
