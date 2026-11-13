//! VW Transport Protocol 2.0 (TP2.0): the CAN transport older CAN-era VAG
//! modules use to carry KWP2000 messages.
//!
//! There is no public standard. This follows community documentation that was
//! reverse-engineered from captures (see `docs/sources.md`), so every detail
//! is unverified against official specifications and real modules.
//!
//! Flow: a channel is opened with a setup request on CAN id 0x200, parameters
//! are exchanged on the negotiated ids, then messages are carried as numbered
//! packets with ACKs. Each direction keeps its own 4-bit sequence counter.

mod frame;
mod kwp;
mod link;
pub mod sim;

pub use frame::{
    decode_setup_request, encode_setup_reply, segment, ChannelMsg, ChannelParams, DataOp,
    SetupReply, SetupRequest, SetupResponse, Timing, APP_KWP, SETUP_REQUEST_ID,
};
pub use kwp::Tp20KwpClient;
pub use link::{Tp20Config, Tp20Link};
