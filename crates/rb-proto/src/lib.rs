//! `rb_proto`: daemon wire protocol for rusty-brain.
//!
//! Length-delimited JSON frames over a Unix domain socket, a versioned
//! handshake, the `Request`/`Response` enums, and an async `Client`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod client;
pub mod codec;
mod error;
mod frame;
mod messages;

pub use client::{Client, SubscribeItem};
pub use codec::{bounded_codec, bounded_framed, MAX_FRAME_BYTES};
pub use error::{error_to_response, response_error_to_error};
pub use frame::{read_frame, write_frame};
pub use messages::{
    request_uses_anchors, ClientIdentity, Handshake, HandshakeAck, RecallChannelTotals, Request,
    Response, ScrubCheckpoint, ScrubResult, CAP_ANCHORS, CONTRACT_VERSION,
};
