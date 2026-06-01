//! `mcp` subcommand: run the MCP stdio adapter against the daemon.
//!
//! Detects the namespace (Part L), connects to the daemon (auto-starting it if
//! the socket is absent), and serves newline-delimited JSON-RPC on stdin/stdout.
//! stdout carries ONLY JSON-RPC frames; tracing goes to stderr.

use crate::client::connect_or_start;
use crate::namespace_detect::detect_namespace;
use async_trait::async_trait;
use rb_mcp::{serve_stdio, DaemonProxy};
use rb_proto::{Client, Request, Response};
use std::path::Path;
use tokio::io::BufReader;

/// Adapts the daemon `rb_proto::Client` to the adapter's `DaemonProxy` seam.
pub struct ClientProxy {
    client: Client,
}

impl ClientProxy {
    /// Wrap a connected daemon client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DaemonProxy for ClientProxy {
    async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
        // The adapter already built a well-formed Request; forward it verbatim
        // via the raw `request` method (not the typed wrappers) so the daemon's
        // Response::Error stays a Response (surfaced as an isError tool result),
        // and only transport failures become Err.
        self.client.request(request).await
    }
}

/// Run the MCP adapter: resolve namespace, connect (auto-start), serve stdio.
///
/// NOTE: `detect_namespace()` runs a synchronous `git` lookup once at startup.
/// Moving that off the runtime is the Part L / Part P-6 should-fix; this wiring
/// reuses the same call the bin's `run_client` already makes, for consistency.
pub async fn run_mcp(socket_path: &Path, db_path: &Path) -> anyhow::Result<()> {
    let namespace = detect_namespace();
    let self_exe =
        std::env::current_exe().map_err(|e| anyhow::anyhow!("locating own executable: {e}"))?;
    let client = connect_or_start(socket_path, db_path, namespace, self_exe)
        .await
        .map_err(|e| anyhow::anyhow!("connecting to daemon: {e}"))?;

    let proxy = ClientProxy::new(client);
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio(stdin, stdout, proxy)
        .await
        .map_err(|e| anyhow::anyhow!("mcp adapter failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    // The proxy is a thin newtype over rb_proto::Client; there is no offline way
    // to construct a connected Client without a daemon, so behavior is proven in
    // the daemon-backed e2e (Task 18). Here we only assert the type implements
    // the rb_mcp::DaemonProxy trait (a compile-time guarantee via this fn).
    fn _assert_impls_daemon_proxy<T: rb_mcp::DaemonProxy>() {}

    #[test]
    fn client_proxy_implements_daemon_proxy() {
        // If ClientProxy did not implement DaemonProxy this would not compile.
        let _ = _assert_impls_daemon_proxy::<ClientProxy>;
    }
}
