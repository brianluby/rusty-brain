//! `mcp` subcommand: run the MCP stdio adapter against the daemon.
//!
//! The namespace is resolved OFF the async runtime in `main.rs` (same as the
//! CLI path) and threaded into `run_mcp`. stdout carries ONLY JSON-RPC frames;
//! tracing goes to stderr.
//!
//! Transport resilience (W0.1): the daemon closes idle connections (60s
//! default), which is routine for an MCP session, so both the request proxy and
//! the background change subscriber reconnect instead of dying with the first
//! broken connection.

use crate::client::connect_or_start;
use async_trait::async_trait;
use rb_mcp::{serve_stdio_with_buffer, ChangeBuffer, DaemonProxy};
use rb_proto::{Client, Request, Response, SubscribeItem};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::sync::Mutex;

/// Sleeps between successive proxy reconnect attempts: one immediate attempt,
/// then one retry per entry. `connect_or_start` already retries auto-start
/// internally; this outer schedule only covers a daemon slow to die or rebind.
const RECONNECT_BACKOFF: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(300),
    Duration::from_millis(900),
];

/// Subscriber reconnect backoff: capped exponential, reset to the initial value
/// on each successful resubscribe.
const SUBSCRIBE_BACKOFF_INITIAL: Duration = Duration::from_millis(500);
const SUBSCRIBE_BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Adapts the daemon `rb_proto::Client` to the adapter's `DaemonProxy` seam,
/// reconnecting (with auto-start) when the connection has died between calls.
pub struct ClientProxy {
    /// `None` after a transport failure; rebuilt lazily by the next call.
    client: Option<Client>,
    socket_path: PathBuf,
    db_path: PathBuf,
    namespace: rb_types::Namespace,
    self_exe: PathBuf,
}

/// Which phase of a request round-trip failed. See the retry rule on
/// [`ClientProxy::call`].
enum CallFailure {
    Send(rb_types::Error),
    Recv(rb_types::Error),
}

impl CallFailure {
    fn into_error(self) -> rb_types::Error {
        match self {
            CallFailure::Send(e) | CallFailure::Recv(e) => e,
        }
    }
}

/// One send+recv round trip with the failure phase preserved.
async fn attempt_call(
    client: &mut Client,
    request: &Request,
) -> std::result::Result<Response, CallFailure> {
    client
        .send_request(request)
        .await
        .map_err(CallFailure::Send)?;
    client.recv_response().await.map_err(CallFailure::Recv)
}

/// Whether `request` is safe to retry after a SUCCESSFUL send with a lost
/// response: only ops the daemon cannot have mutated state for. `Remember`
/// creates a new row per execution and is the canonical non-retryable case;
/// `Update`/`Delete`/`RunJob`/`Reembed` are conservatively excluded too.
fn retry_safe_after_send(request: &Request) -> bool {
    matches!(
        request,
        Request::Ping
            | Request::Recall { .. }
            | Request::Get { .. }
            | Request::List { .. }
            | Request::Graph { .. }
            | Request::Context
    )
}

impl ClientProxy {
    /// Wrap a connected daemon client plus everything needed to reconnect.
    pub fn new(
        client: Client,
        socket_path: PathBuf,
        db_path: PathBuf,
        namespace: rb_types::Namespace,
        self_exe: PathBuf,
    ) -> Self {
        Self {
            client: Some(client),
            socket_path,
            db_path,
            namespace,
            self_exe,
        }
    }

    /// Drop any dead client and rebuild the connection (auto-starting the
    /// daemon if needed), backing off per [`RECONNECT_BACKOFF`].
    async fn reconnect(&mut self) -> rb_types::Result<()> {
        self.client = None;
        let mut last_err: Option<rb_types::Error> = None;
        for (attempt, backoff) in std::iter::once(None)
            .chain(RECONNECT_BACKOFF.iter().map(Some))
            .enumerate()
        {
            if let Some(backoff) = backoff {
                tokio::time::sleep(*backoff).await;
            }
            match connect_or_start(
                &self.socket_path,
                &self.db_path,
                self.namespace.clone(),
                self.self_exe.clone(),
                Some(crate::client::client_identity("mcp")),
            )
            .await
            {
                Ok(client) => {
                    self.client = Some(client);
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(error = %e, attempt, "mcp proxy reconnect attempt failed");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| rb_types::Error::Io("reconnect failed".into())))
    }

    /// Borrow the live client, reconnecting first if a previous failure (or a
    /// non-retryable lost-response) left the proxy without one.
    async fn ensure_connected(&mut self) -> rb_types::Result<&mut Client> {
        if self.client.is_none() {
            self.reconnect().await?;
        }
        self.client
            .as_mut()
            .ok_or_else(|| rb_types::Error::Io("not connected".into()))
    }
}

#[async_trait]
impl DaemonProxy for ClientProxy {
    // The adapter already built a well-formed Request; forward it verbatim via
    // the raw send/recv methods (not the typed wrappers) so the daemon's
    // Response::Error stays a Response (surfaced as an isError tool result),
    // and only transport failures become Err.
    //
    // RETRY RULE (see rb_proto::Client::send_request / recv_response): a SEND
    // failure means the daemon never received the frame — Unix-socket writes
    // fail immediately once the peer has closed, which is exactly the daemon's
    // idle-timeout case — so one reconnect+retry is safe for EVERY op,
    // including Remember. A RECV failure after a successful send is ambiguous
    // for mutating ops (the daemon may have committed and only the response was
    // lost), so only read-only ops are retried; mutating ops return the error
    // and the NEXT call rebuilds the connection lazily.
    async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
        let failure = {
            let client = self.ensure_connected().await?;
            // Anchor-capability gate (typed code anchors): this raw path
            // bypasses the typed Client wrappers, so it must apply the same
            // fail-fast rule — a daemon that did not advertise the `anchors`
            // capability would silently DROP `Remember.anchors` / IGNORE
            // anchor filters instead of erroring.
            if rb_proto::request_uses_anchors(&request) && !client.supports_anchors() {
                return Err(rb_types::Error::InvalidArgument(
                    "this daemon does not support typed code anchors (no `anchors` \
                     capability in its handshake); upgrade and restart the daemon"
                        .to_string(),
                ));
            }
            match attempt_call(client, &request).await {
                Ok(resp) => return Ok(resp),
                Err(f) => f,
            }
        };

        // The connection is dead either way; never reuse a broken stream.
        self.client = None;

        let retryable = match &failure {
            CallFailure::Send(_) => true,
            CallFailure::Recv(_) => retry_safe_after_send(&request),
        };
        if !retryable {
            // Surface the ambiguous failure immediately. An eager reconnect
            // here can block ~20s (backoff attempts x auto-start retries) while
            // serial stdio dispatch stalls every other tool call — and buys
            // nothing: `self.client` is None, so the next call reconnects
            // lazily via `ensure_connected`.
            return Err(failure.into_error());
        }

        tracing::warn!(
            error = %failure.into_error(),
            "mcp proxy connection dead; reconnecting and retrying once"
        );
        let client = self.ensure_connected().await?;
        match attempt_call(client, &request).await {
            Ok(resp) => Ok(resp),
            Err(f) => {
                self.client = None;
                Err(f.into_error())
            }
        }
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
    let client = connect_or_start(
        socket_path,
        db_path,
        namespace.clone(),
        self_exe.clone(),
        Some(crate::client::client_identity("mcp")),
    )
    .await
    .map_err(|e| anyhow::anyhow!("connecting to daemon: {e}"))?;

    // Bounded ring shared between the background subscriber and poll_changes.
    let buffer = Arc::new(Mutex::new(ChangeBuffer::new(1024)));

    // Background subscriber on a dedicated, read-only connection. It reconnects
    // with capped backoff for the adapter's lifetime; while down, poll_changes
    // reports `subscriber.status = "disconnected"` instead of going silently
    // deaf. A subscriber outage is never fatal to tool serving.
    let sub_buffer = Arc::clone(&buffer);
    let sub_socket = socket_path.to_path_buf();
    let sub_db = db_path.to_path_buf();
    let sub_ns = namespace.clone();
    let sub_exe = self_exe.clone();
    tokio::spawn(async move {
        run_subscriber(&sub_socket, &sub_db, sub_ns, sub_exe, sub_buffer).await;
    });

    let proxy = ClientProxy::new(
        client,
        socket_path.to_path_buf(),
        db_path.to_path_buf(),
        namespace,
        self_exe,
    );
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio_with_buffer(stdin, stdout, proxy, buffer)
        .await
        .map_err(|e| anyhow::anyhow!("mcp adapter failed: {e}"))?;
    Ok(())
}

/// Background loop: keep a subscriber connection alive for the adapter's
/// lifetime, pushing namespace-scoped change events into the shared ring.
/// Connect/subscribe/stream failures mark the buffer disconnected and retry
/// with capped exponential backoff (reset on each successful resubscribe);
/// the task ends only with the runtime.
async fn run_subscriber(
    socket_path: &Path,
    db_path: &Path,
    namespace: rb_types::Namespace,
    self_exe: std::path::PathBuf,
    buffer: Arc<Mutex<ChangeBuffer>>,
) {
    let mut backoff = SUBSCRIBE_BACKOFF_INITIAL;
    // W2.7 replay cursor: the highest oplog seq this subscriber has pushed
    // into the ring. On reconnect we resubscribe `--since cursor`, so the
    // daemon replays whatever committed during the outage instead of those
    // changes being silently missed. `None` until the first seq-stamped event
    // (a first connect has nothing to resume; a pre-seq daemon never advances
    // it, which degrades to today's live-only behavior).
    let mut cursor: Option<u64> = None;
    loop {
        match connect_and_subscribe(
            socket_path,
            db_path,
            namespace.clone(),
            self_exe.clone(),
            cursor,
        )
        .await
        {
            Ok(mut client) => {
                buffer.lock().await.set_connected();
                backoff = SUBSCRIBE_BACKOFF_INITIAL;
                pump_changes(&mut client, &buffer, &mut cursor).await;
                buffer.lock().await.set_disconnected();
            }
            Err(e) => {
                buffer.lock().await.set_disconnected();
                tracing::warn!(
                    error = %e,
                    retry_in = ?backoff,
                    "mcp change subscriber connect/subscribe failed"
                );
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(SUBSCRIBE_BACKOFF_MAX);
    }
}

/// Open a fresh connection (auto-starting the daemon if needed) and switch it
/// into the change stream, resuming from `since` when a cursor exists (W2.7).
async fn connect_and_subscribe(
    socket_path: &Path,
    db_path: &Path,
    namespace: rb_types::Namespace,
    self_exe: std::path::PathBuf,
    since: Option<u64>,
) -> rb_types::Result<Client> {
    let mut client = connect_or_start(
        socket_path,
        db_path,
        namespace,
        self_exe,
        Some(crate::client::client_identity("mcp")),
    )
    .await?;
    client.subscribe_since(since).await?;
    Ok(client)
}

/// Drain the live stream into the ring until it errors (daemon restart or
/// shutdown), advancing the replay cursor past every seq-stamped event; the
/// caller handles reconnect.
async fn pump_changes(
    client: &mut Client,
    buffer: &Arc<Mutex<ChangeBuffer>>,
    cursor: &mut Option<u64>,
) {
    loop {
        match client.recv_change().await {
            Ok(SubscribeItem::Change(evt)) => {
                if let Some(seq) = evt.seq {
                    *cursor = Some(cursor.unwrap_or(0).max(seq));
                }
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

    // The proxy is a thin wrapper over rb_proto::Client; there is no offline way
    // to construct a connected Client without a daemon, so reconnect behavior is
    // proven in the daemon-backed e2e (tests/mcp_e2e.rs). Here we only assert
    // the type implements the rb_mcp::DaemonProxy trait (a compile-time
    // guarantee via this fn).
    fn _assert_impls_daemon_proxy<T: rb_mcp::DaemonProxy>() {}

    #[test]
    fn client_proxy_implements_daemon_proxy() {
        // If ClientProxy did not implement DaemonProxy this would not compile.
        let _ = _assert_impls_daemon_proxy::<ClientProxy>;
    }

    // The retry rule's idempotency split: read-only ops may be retried after a
    // lost response; anything that writes (or streams) must not be, because the
    // daemon may already have executed it.
    #[test]
    fn lost_response_retry_is_limited_to_read_only_ops() {
        let id = rb_types::MemoryId::new;
        for req in [
            Request::Ping,
            Request::Recall {
                query: "q".into(),
                memory_type: None,
                tags: vec![],
                limit: 5,
                filter: rb_types::RecallFilter::default(),
            },
            Request::Get { id: id() },
            Request::List {
                min_importance: None,
                limit: 10,
                filter: rb_types::RecallFilter::default(),
            },
            Request::Graph { id: id(), depth: 2 },
            Request::Context,
        ] {
            assert!(retry_safe_after_send(&req), "read-only must retry: {req:?}");
        }
        for req in [
            Request::Remember {
                anchors: vec![],
                content: "c".into(),
                context: None,
                memory_type: rb_types::MemoryType::Insight,
                importance: 5,
                keywords: vec![],
                tags: vec![],
                related_files: vec![],
                confidence: None,
                supersedes: None,
            },
            Request::Update {
                id: id(),
                updates: Default::default(),
            },
            Request::Delete { id: id() },
            Request::Subscribe { since: None },
            Request::RunJob {
                job: rb_types::JobKind::LinkDecay,
            },
            Request::Reembed { limit: None },
        ] {
            assert!(
                !retry_safe_after_send(&req),
                "mutating/streaming must not retry: {req:?}"
            );
        }
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
