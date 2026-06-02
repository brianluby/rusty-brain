//! `mcp` subcommand: run the MCP stdio adapter against the daemon.
//!
//! The namespace is resolved OFF the async runtime in `main.rs` (same as the
//! CLI path) and threaded into `run_mcp`. stdout carries ONLY JSON-RPC frames;
//! tracing goes to stderr.

use crate::client::connect_or_start;
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

/// Run the MCP adapter: connect (auto-start), serve stdio.
///
/// The `namespace` is resolved off the async runtime in `main.rs` before
/// `block_on` (shells out to git / reads files), consistent with the CLI path.
pub async fn run_mcp(
    socket_path: &Path,
    db_path: &Path,
    namespace: rb_types::Namespace,
) -> anyhow::Result<()> {
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

    // Compile-time guard: run_mcp must accept a pre-resolved Namespace (threaded
    // in from main.rs off the runtime) rather than calling detect_namespace()
    // itself from inside the tokio runtime.
    #[test]
    fn run_mcp_signature_takes_preresolved_namespace() {
        // If run_mcp's signature changed back to not accept a Namespace this
        // closure would fail to compile.
        fn _assert_callable<'a>(
            sock: &'a std::path::Path,
            db: &'a std::path::Path,
            ns: rb_types::Namespace,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + 'a>> {
            Box::pin(run_mcp(sock, db, ns))
        }
        let _ = _assert_callable as fn(_, _, _) -> _;
    }
}
