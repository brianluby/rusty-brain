//! `mcp` subcommand: run the MCP stdio adapter against the daemon.
//!
//! The namespace is resolved OFF the async runtime in `main.rs` (same as the
//! CLI path) and threaded into `run_mcp`. stdout carries ONLY JSON-RPC frames;
//! tracing goes to stderr.

use crate::client::connect_or_start;
use async_trait::async_trait;
use rb_mcp::{serve_stdio_with_buffer, ChangeBuffer, DaemonProxy};
use rb_proto::{Client, Request, Response, SubscribeItem};
use std::path::Path;
use std::sync::Arc;
use tokio::io::BufReader;
use tokio::sync::Mutex;

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

/// Run the MCP adapter: connect (auto-start) a proxy connection, spawn a
/// background subscriber on a SECOND connection that feeds a bounded change
/// ring, then serve stdio with `poll_changes` draining that ring.
///
/// The `namespace` is resolved off the async runtime in `main.rs` (shells out to
/// git / reads files), consistent with the CLI path.
pub async fn run_mcp(
    socket_path: &Path,
    db_path: &Path,
    namespace: rb_types::Namespace,
) -> anyhow::Result<()> {
    let self_exe =
        std::env::current_exe().map_err(|e| anyhow::anyhow!("locating own executable: {e}"))?;
    let client = connect_or_start(socket_path, db_path, namespace.clone(), self_exe.clone())
        .await
        .map_err(|e| anyhow::anyhow!("connecting to daemon: {e}"))?;

    // Bounded ring shared between the background subscriber and poll_changes.
    let buffer = Arc::new(Mutex::new(ChangeBuffer::new(1024)));

    // Background subscriber on a dedicated, read-only connection. If it cannot
    // connect or subscribe, poll_changes simply returns empty — the adapter must
    // still serve tools, so a subscriber failure is logged, not fatal.
    let sub_buffer = Arc::clone(&buffer);
    let sub_socket = socket_path.to_path_buf();
    let sub_db = db_path.to_path_buf();
    let sub_ns = namespace.clone();
    tokio::spawn(async move {
        run_subscriber(&sub_socket, &sub_db, sub_ns, self_exe, sub_buffer).await;
    });

    let proxy = ClientProxy::new(client);
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio_with_buffer(stdin, stdout, proxy, buffer)
        .await
        .map_err(|e| anyhow::anyhow!("mcp adapter failed: {e}"))?;
    Ok(())
}

/// Background loop: open a subscriber connection and push namespace-scoped change
/// events into the shared ring until the stream closes. Best-effort: connection
/// or stream errors end the loop quietly (logged to stderr), leaving poll_changes
/// to return whatever was already buffered.
async fn run_subscriber(
    socket_path: &Path,
    db_path: &Path,
    namespace: rb_types::Namespace,
    self_exe: std::path::PathBuf,
    buffer: Arc<Mutex<ChangeBuffer>>,
) {
    let mut client = match connect_or_start(socket_path, db_path, namespace, self_exe).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "mcp change subscriber could not connect; poll_changes will be empty");
            return;
        }
    };
    if let Err(e) = client.subscribe().await {
        tracing::warn!(error = %e, "mcp change subscriber could not subscribe; poll_changes will be empty");
        return;
    }
    loop {
        match client.recv_change().await {
            Ok(SubscribeItem::Change(evt)) => {
                buffer.lock().await.push(evt);
            }
            Ok(SubscribeItem::Lagged(n)) => {
                buffer.lock().await.record_dropped(n);
            }
            Err(e) => {
                tracing::debug!(error = %e, "mcp change subscriber stream ended");
                break;
            }
        }
    }
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

    // Compile-time guard: the buffer-aware stdio entrypoint must exist and be
    // importable from rb_mcp. If `serve_stdio_with_buffer` is removed or its
    // signature changes incompatibly, this fails to compile.
    #[test]
    fn buffer_aware_serve_symbol_is_available() {
        fn _assert_symbol_exists() {
            // Reference the function path without calling it.
            let _f = rb_mcp::serve_stdio_with_buffer::<
                tokio::io::BufReader<tokio::io::Stdin>,
                tokio::io::Stdout,
                crate::mcp::ClientProxy,
            >;
        }
        let _ = _assert_symbol_exists;
    }
}
