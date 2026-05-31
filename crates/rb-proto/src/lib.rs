//! `rb_proto`: daemon wire protocol for rusty-brain.
//!
//! Length-delimited JSON frames over a Unix domain socket, a versioned
//! handshake, the `Request`/`Response` enums, and an async `Client`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod client;
mod error;
mod frame;
mod messages;

pub use client::Client;
pub use error::{error_to_response, response_error_to_error};
pub use frame::{read_frame, write_frame};
pub use messages::{Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION};
