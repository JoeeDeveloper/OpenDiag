//! KW1281 (VAG "KWP1281") on K-line: the block protocol used by older modules
//! (roughly 1990s to mid-2000s, including many PQ24 / 9N-era controllers).
//!
//! Session: a 5-baud address init, then blocks `[len, counter, title, data.., 0x03]`.
//! Both sides acknowledge every byte except the block-end byte by sending its
//! bitwise complement, so the protocol is strictly byte-by-byte lock-step.
//! Sources are listed in `docs/sources.md`. Not yet verified against a real module.

mod block;
mod client;
pub mod sim;

pub use block::{parse_ascii, parse_faults, title, Block, Fault, END_BYTE};
pub use client::{ConnectInfo, Kw1281Client, Kw1281Config, RawMeasurement};
