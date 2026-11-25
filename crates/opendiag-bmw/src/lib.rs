//! BMW F-series diagnostics over ENET (HSFZ).
//!
//! **Simulator-tested only.** Nothing here has run against a real car or
//! gateway. Items marked NOT VERIFIED in the module docs and the README are
//! recalled or inferred, not confirmed from a public source.
//!
//! Layers: [`hsfz`] (framing), [`gateway`] (TCP session and per-ECU links that
//! implement the shared `MessageLink`, so the VAG-proven UDS client is reused),
//! [`discovery`] (UDP), [`ident`], [`scan`], [`live`] (read-only services), [`ecu`] (DTC read and the
//! guarded clear, via `opendiag-services`), [`mock`] (virtual car).

pub mod discovery;
pub mod ecu;
pub mod ecus;
pub mod gateway;
pub mod hsfz;
pub mod ident;
pub mod live;
pub mod mock;
pub mod scan;
pub mod trace;
pub mod vehicle;

pub use discovery::{broadcast_target, discover, Discovered};
pub use ecu::{format_bmw_dtc, BmwEcu};
pub use gateway::{ByteStream, HsfzGateway, HsfzLink, TcpByteStream};
pub use ident::{read_did, read_ident, BmwIdent};
pub use live::{log_dids_csv, DidLogConfig, DidLogSummary};
pub use scan::{scan, ScanEntry};
pub use trace::{RecordingStream, ReplayStream};
pub use vehicle::BmwVehicle;
