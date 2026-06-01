//! `rb_mcp`: a thin Model Context Protocol (MCP) stdio adapter for rusty-brain.
//!
//! Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout (stdout carries ONLY
//! JSON-RPC frames; all logging goes to stderr). Each `tools/call` is routed to
//! an `rb_proto::Request` and forwarded to the daemon over the Unix socket via a
//! `DaemonProxy`. The adapter holds no storage of its own.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod jsonrpc;
