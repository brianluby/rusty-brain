//! `rb_mcp`: a thin Model Context Protocol (MCP) stdio adapter for rusty-brain.
//!
//! Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout (stdout carries ONLY
//! JSON-RPC frames; all logging goes to stderr). Each `tools/call` is routed to
//! an `rb_proto::Request` and forwarded to the daemon over the Unix socket via a
//! `DaemonProxy`. The adapter holds no storage of its own.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod change_buffer;
pub mod jsonrpc;
pub mod proxy;
pub mod server;
pub mod tools;
pub mod transport;

pub use change_buffer::{ChangeBuffer, Drained};
pub use jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use proxy::{build_request, response_to_content, DaemonProxy};
pub use server::handle_request;
pub use tools::{tool_definitions, ToolDef};
pub use transport::serve_stdio;
