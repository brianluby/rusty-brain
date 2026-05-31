//! `rb_proto`: daemon wire protocol for rusty-brain.
//!
//! Length-delimited JSON frames over a Unix domain socket, a versioned
//! handshake, the `Request`/`Response` enums, and an async `Client`.
//! Concrete types are added in subsequent tasks.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
