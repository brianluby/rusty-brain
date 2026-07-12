//! Async client for the rusty-brain daemon over a Unix domain socket.

use crate::codec::bounded_framed;
use crate::frame::{read_frame, write_frame};
use crate::messages::{Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION};
use crate::{response_error_to_error, Response as Resp};
use rb_types::{
    Error, FeedbackKind, MemoryChanged, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace,
    Result, SearchResult,
};
use std::path::Path;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// A connected, handshaken client. Sends one `Request`, reads one `Response`.
#[derive(Debug)]
pub struct Client {
    framed: Framed<UnixStream, LengthDelimitedCodec>,
    /// Whether the daemon advertised [`crate::CAP_ANCHORS`] at handshake —
    /// i.e. it evaluates typed code anchors instead of silently dropping
    /// (`Remember.anchors`) or ignoring (pre-filter daemons) them. Gates the
    /// anchor-bearing wrappers so the fail-fast anchor guarantee holds across
    /// daemon versions.
    supports_anchors: bool,
}

/// One item yielded by a live subscribe stream: either a change event or a
/// notice that the broadcast dropped `n` events for this slow subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeItem {
    /// A committed change in the connection's namespace.
    Change(MemoryChanged),
    /// The subscriber fell behind; `n` events were dropped by the broadcast.
    Lagged(u64),
}

impl Client {
    /// Connect to the daemon socket, perform the versioned handshake, and verify
    /// the daemon speaks `CONTRACT_VERSION`. Fails closed on any version drift or
    /// a non-ok ack. Sends no identity; provenance-aware callers use
    /// [`connect_with_identity`](Self::connect_with_identity).
    pub async fn connect(socket_path: &Path, namespace: Namespace) -> Result<Client> {
        Self::connect_with_identity(socket_path, namespace, None).await
    }

    /// [`connect`](Self::connect) carrying an optional client identity on the
    /// handshake (W0.5): the daemon stamps it as provenance on every memory this
    /// connection writes. `None` matches the pre-W0.5 wire shape exactly.
    pub async fn connect_with_identity(
        socket_path: &Path,
        namespace: Namespace,
        identity: Option<crate::ClientIdentity>,
    ) -> Result<Client> {
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| Error::from_io(&e))?;
        let mut framed = bounded_framed(stream);

        let handshake = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace,
            identity,
        };
        write_frame(&mut framed, &handshake).await?;

        let ack: HandshakeAck = read_frame(&mut framed).await?;
        if ack.contract_version != CONTRACT_VERSION {
            return Err(Error::Storage(format!(
                "contract version mismatch: client {CONTRACT_VERSION}, daemon {}",
                ack.contract_version
            )));
        }
        if !ack.ok {
            let detail = ack
                .message
                .unwrap_or_else(|| "handshake rejected".to_string());
            return Err(Error::Storage(format!("handshake rejected: {detail}")));
        }

        // A pre-anchor daemon sends no capabilities (serde default: empty),
        // so `supports_anchors` correctly degrades to false against it.
        let supports_anchors = ack.capabilities.iter().any(|c| c == crate::CAP_ANCHORS);
        Ok(Client {
            framed,
            supports_anchors,
        })
    }

    /// Whether the daemon advertised typed-code-anchor support
    /// ([`crate::CAP_ANCHORS`]) at handshake. Raw-`Request` callers (the MCP
    /// adapter) combine this with [`crate::request_uses_anchors`] to fail
    /// fast instead of letting an old daemon silently drop/ignore anchors.
    #[must_use]
    pub fn supports_anchors(&self) -> bool {
        self.supports_anchors
    }

    /// Send one request frame WITHOUT reading the response. Split from
    /// [`request`](Self::request) so retrying callers can distinguish the two
    /// failure phases: a SEND failure means the daemon never received the frame
    /// (a Unix-socket write fails immediately once the peer has closed), so a
    /// retry on a fresh connection is safe for any op; a RECV failure after a
    /// successful send means the daemon may have executed the request and only
    /// the response was lost, so only idempotent ops may be retried.
    pub async fn send_request(&mut self, req: &Request) -> Result<()> {
        write_frame(&mut self.framed, req).await
    }

    /// Read one response frame for a request previously sent with
    /// [`send_request`](Self::send_request).
    pub async fn recv_response(&mut self) -> Result<Response> {
        read_frame(&mut self.framed).await
    }

    /// Send one request and read one response.
    pub async fn request(&mut self, req: Request) -> Result<Response> {
        self.send_request(&req).await?;
        self.recv_response().await
    }

    /// Open a live change-notification stream on this connection. After this
    /// returns `Ok`, the connection no longer follows request/response cadence;
    /// call [`recv_change`](Self::recv_change) in a loop to read streamed frames.
    /// The stream is scoped server-side to the handshake namespace.
    pub async fn subscribe(&mut self) -> Result<()> {
        self.subscribe_since(None).await
    }

    /// Like [`subscribe`](Self::subscribe), but resume from an oplog cursor
    /// (W2.7): every change with `seq > since` is replayed from the durable
    /// oplog before live streaming begins, so a reconnecting consumer misses
    /// nothing. Track the cursor as `max(seq)` over received changes.
    pub async fn subscribe_since(&mut self, since: Option<u64>) -> Result<()> {
        write_frame(&mut self.framed, &Request::Subscribe { since }).await?;
        // Block until the daemon confirms the broadcast receiver is registered.
        // Returning before the ack would let the caller commit a change that
        // races ahead of an active subscription and is silently missed.
        let resp: Response = read_frame(&mut self.framed).await?;
        match resp {
            Resp::SubscribeAck => Ok(()),
            Resp::Error { kind, message } => Err(response_error_to_error(&kind, &message)),
            other => Err(Error::Storage(format!(
                "unexpected frame awaiting subscribe ack: {other:?}"
            ))),
        }
    }

    /// Read the next streamed item from a subscribe stream. Blocks until the
    /// daemon emits the next `Change`/`Lagged` frame; a closed stream surfaces as
    /// `Error::Io`. Any non-streamed response is a protocol violation.
    pub async fn recv_change(&mut self) -> Result<SubscribeItem> {
        let resp: Response = read_frame(&mut self.framed).await?;
        match resp {
            Response::Change(evt) => Ok(SubscribeItem::Change(evt)),
            Response::Lagged { dropped } => Ok(SubscribeItem::Lagged(dropped)),
            Resp::Error { kind, message } => Err(response_error_to_error(&kind, &message)),
            other => Err(Error::Storage(format!(
                "unexpected frame on subscribe stream: {other:?}"
            ))),
        }
    }
}

impl Client {
    /// Helper: turn an unexpected response (including a wire `Error`) into an
    /// `Err`. The daemon's `Response::Error` is mapped back to a domain error;
    /// any other unexpected variant is a protocol violation -> `Error::Storage`.
    fn unexpected(resp: Resp) -> Error {
        match resp {
            Resp::Error { kind, message } => response_error_to_error(&kind, &message),
            other => Error::Storage(format!("unexpected response: {other:?}")),
        }
    }

    /// Store a new memory; returns its id. `confidence` is the EXPLICIT trust
    /// prior in `0.0..=1.0`, or `None` to express no prior (the daemon applies
    /// the full-trust 1.0 baseline and lets an enricher fill it). Hook captures
    /// pass `Some(0.7)`. An explicit out-of-range value is rejected client-side
    /// with [`Error::InvalidArgument`] before anything touches the wire.
    ///
    /// To store a replacement that supersedes an existing memory, use
    /// [`Client::remember_superseding`].
    #[allow(clippy::too_many_arguments)]
    pub async fn remember(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_files: Vec<String>,
        confidence: Option<f32>,
    ) -> Result<MemoryId> {
        self.remember_inner(
            content,
            context,
            memory_type,
            importance,
            keywords,
            tags,
            related_files,
            confidence,
            Vec::new(),
            None,
        )
        .await
    }

    /// [`Client::remember`] carrying typed code anchors (PRD 2026-07-02) and
    /// an optional atomic supersede. Fails fast with
    /// [`Error::InvalidArgument`] when `anchors` is non-empty but the daemon
    /// did NOT advertise anchor support at handshake — a pre-anchor daemon
    /// would silently DROP the additive `anchors` field, storing an
    /// un-anchored memory the caller believes is anchored.
    #[allow(clippy::too_many_arguments)]
    pub async fn remember_anchored(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_files: Vec<String>,
        confidence: Option<f32>,
        anchors: Vec<rb_types::MemoryAnchor>,
        supersedes: Option<MemoryId>,
    ) -> Result<MemoryId> {
        self.ensure_anchor_capable(!anchors.is_empty())?;
        for anchor in &anchors {
            anchor.validate()?;
        }
        self.remember_inner(
            content,
            context,
            memory_type,
            importance,
            keywords,
            tags,
            related_files,
            confidence,
            anchors,
            supersedes,
        )
        .await
    }

    /// Store a new memory that ATOMICALLY supersedes `supersedes` daemon-side
    /// (W3.1 update-as-supersede): the replacement is written, then the prior
    /// memory is archived and stamped `superseded_by` through the existing
    /// atomic supersede. Returns the NEW memory's id. Same `confidence`
    /// semantics as [`Client::remember`]. See [`Request::Remember::supersedes`].
    #[allow(clippy::too_many_arguments)]
    pub async fn remember_superseding(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_files: Vec<String>,
        confidence: Option<f32>,
        supersedes: MemoryId,
    ) -> Result<MemoryId> {
        self.remember_inner(
            content,
            context,
            memory_type,
            importance,
            keywords,
            tags,
            related_files,
            confidence,
            Vec::new(),
            Some(supersedes),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn remember_inner(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_files: Vec<String>,
        confidence: Option<f32>,
        anchors: Vec<rb_types::MemoryAnchor>,
        supersedes: Option<MemoryId>,
    ) -> Result<MemoryId> {
        // Mirrors the engine-side check so a bad EXPLICIT value fails fast and
        // with the same error class the daemon would round-trip back. None
        // carries no prior and is always valid.
        if let Some(c) = confidence {
            if !c.is_finite() || !(0.0..=1.0).contains(&c) {
                return Err(Error::InvalidArgument(format!(
                    "confidence {c} out of range 0.0..=1.0"
                )));
            }
        }
        let resp = self
            .request(Request::Remember {
                content,
                context,
                memory_type,
                importance,
                keywords,
                tags,
                related_files,
                confidence,
                supersedes,
                anchors,
            })
            .await?;
        match resp {
            Resp::Remembered { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Hybrid recall; returns ranked results. Callers that surface the W1.6d
    /// degraded flag should use [`Client::recall_with_status`] (the
    /// `ping`/`ping_stats` pattern); this convenience wrapper drops it.
    pub async fn recall(
        &mut self,
        query: String,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        Ok(self
            .recall_with_status(query, memory_type, tags, limit)
            .await?
            .0)
    }

    /// Hybrid recall; returns ranked results plus the W1.6d `degraded` flag —
    /// `true` when recall fell back to keyword+graph because the embedding
    /// provider errored, so the caller can warn that results are vector-blind
    /// (rb-mcp consumes the raw `Response` frame for the same purpose).
    pub async fn recall_with_status(
        &mut self,
        query: String,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
        limit: usize,
    ) -> Result<(Vec<SearchResult>, bool)> {
        let filter = rb_types::RecallFilter::default().fold_recall_legacy(memory_type, tags);
        self.recall_filtered_with_status(query, filter, limit).await
    }

    /// Fail-fast anchor-capability gate: anchor payloads may only ride the
    /// wire when the daemon advertised [`crate::CAP_ANCHORS`] at handshake.
    /// A PRE-ANCHOR contract-v2 daemon either silently IGNORES the additive
    /// `filter` field (pre-parity daemons — anchor-filtered recall would come
    /// back unfiltered) or rejects it server-side; a pre-anchor daemon
    /// silently DROPS `Remember.anchors`. Rejecting locally keeps the
    /// fail-fast guarantee independent of the daemon version.
    fn ensure_anchor_capable(&self, uses_anchors: bool) -> Result<()> {
        if uses_anchors && !self.supports_anchors {
            return Err(Error::InvalidArgument(
                "this daemon does not support typed code anchors (no `anchors` capability \
                 in its handshake); upgrade and restart the daemon to use anchor \
                 filters/anchored memories"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Fail-fast gate shared by the filtered wrappers: non-empty anchor
    /// filters require the daemon's advertised anchor capability (see
    /// [`Client::ensure_anchor_capable`]).
    fn ensure_filter_sendable(&self, filter: &rb_types::RecallFilter) -> Result<()> {
        self.ensure_anchor_capable(!filter.anchors.is_empty())
    }

    /// [`Client::recall_with_status`] over the unified [`rb_types::RecallFilter`]
    /// (PRD 2026-07-02 search-filter parity). Wire strategy: the dimensions a
    /// pre-filter frame can express (a single type + tags) are SPLIT into the
    /// legacy request slots so an old daemon still honors them; only the new
    /// dimensions ride the additive `filter` field (which an old daemon
    /// ignores — graceful degradation to the legacy subset). Non-empty anchor
    /// filters are rejected BEFORE sending unless the daemon advertised
    /// anchor support (see `ensure_filter_sendable`).
    pub async fn recall_filtered_with_status(
        &mut self,
        query: String,
        filter: rb_types::RecallFilter,
        limit: usize,
    ) -> Result<(Vec<SearchResult>, bool)> {
        self.ensure_filter_sendable(&filter)?;
        let (memory_type, tags, filter) = filter.split_recall_legacy();
        let resp = self
            .request(Request::Recall {
                query,
                memory_type,
                tags,
                limit,
                filter,
            })
            .await?;
        match resp {
            Resp::Recalled { results, degraded } => Ok((results, degraded)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Fetch a single memory by id.
    pub async fn get(&mut self, id: MemoryId) -> Result<Option<MemoryNote>> {
        let resp = self.request(Request::Get { id }).await?;
        match resp {
            Resp::Got { memory } => Ok(memory),
            other => Err(Self::unexpected(other)),
        }
    }

    /// List memories in scope.
    pub async fn list(
        &mut self,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        let filter = rb_types::RecallFilter::default().fold_list_legacy(min_importance);
        self.list_filtered(filter, limit).await
    }

    /// [`Client::list`] over the unified [`rb_types::RecallFilter`] — the same
    /// legacy-slot split as [`Client::recall_filtered_with_status`]:
    /// `min_importance` rides the pre-filter wire slot (old daemons honor it),
    /// the new dimensions ride the additive `filter` field. Non-empty anchor
    /// filters are rejected BEFORE sending unless the daemon advertised
    /// anchor support (see `ensure_filter_sendable`).
    pub async fn list_filtered(
        &mut self,
        filter: rb_types::RecallFilter,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        self.ensure_filter_sendable(&filter)?;
        let (min_importance, filter) = filter.split_list_legacy();
        let resp = self
            .request(Request::List {
                min_importance,
                limit,
                filter,
            })
            .await?;
        match resp {
            Resp::Listed { memories } => Ok(memories),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Walk the link graph from a memory.
    pub async fn graph(&mut self, id: MemoryId, depth: u8) -> Result<Vec<MemoryNote>> {
        let resp = self.request(Request::Graph { id, depth }).await?;
        match resp {
            Resp::GraphResult { memories } => Ok(memories),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Apply a partial update to a memory.
    pub async fn update(&mut self, id: MemoryId, updates: MemoryUpdates) -> Result<()> {
        let resp = self.request(Request::Update { id, updates }).await?;
        match resp {
            Resp::Updated => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Create an explicit link between two memories (W2.2). `reason` defaults
    /// daemon-side to "user-asserted" when `None`.
    pub async fn link(
        &mut self,
        from: MemoryId,
        to: MemoryId,
        link_type: rb_types::LinkType,
        reason: Option<String>,
    ) -> Result<()> {
        // Fast-fail the one link type the engine rejects outright (supersede is
        // its own atomic op), so the caller gets the same InvalidArgument
        // without a guaranteed-failing round-trip.
        if link_type == rb_types::LinkType::Supersedes {
            return Err(Error::InvalidArgument(
                "supersedes links are created by storing a replacement memory, \
                 not by linking"
                    .to_string(),
            ));
        }
        let resp = self
            .request(Request::Link {
                from,
                to,
                link_type,
                reason,
            })
            .await?;
        match resp {
            Resp::Linked => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Record a usefulness signal about a recalled memory (W3.7). Returns the
    /// memory's `confidence` after the bounded nudge.
    pub async fn feedback(&mut self, id: MemoryId, kind: FeedbackKind) -> Result<f32> {
        let resp = self.request(Request::Feedback { id, kind }).await?;
        match resp {
            Resp::FeedbackRecorded { confidence } => Ok(confidence),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Soft-archive a memory.
    pub async fn delete(&mut self, id: MemoryId) -> Result<()> {
        let resp = self.request(Request::Delete { id }).await?;
        match resp {
            Resp::Deleted => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Fetch the project context payload (recent, important, total).
    pub async fn context(&mut self) -> Result<(Vec<MemoryNote>, Vec<MemoryNote>, usize)> {
        let resp = self.request(Request::Context).await?;
        match resp {
            Resp::ContextResult {
                recent,
                important,
                total,
            } => Ok((recent, important, total)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Round-trip ping; returns the daemon's contract version.
    pub async fn ping(&mut self) -> Result<u32> {
        Ok(self.ping_stats().await?.0)
    }

    /// Round-trip ping; returns the daemon's contract version plus the
    /// aggregate per-channel recall hit counters (W1.0). `None` from a daemon
    /// predating the counters.
    pub async fn ping_stats(
        &mut self,
    ) -> Result<(u32, Option<crate::messages::RecallChannelTotals>)> {
        let resp = self.request(Request::Ping).await?;
        match resp {
            Resp::Pong {
                contract_version,
                recall_channels,
            } => Ok((contract_version, recall_channels)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Trigger one bounded evolution-job pass on the daemon; returns the summary
    /// `(scanned, changed, skipped)`. The daemon performs all mutations through
    /// its single writer; this is just the wire trigger.
    pub async fn run_job(&mut self, job: rb_types::JobKind) -> Result<(u64, u64, u64)> {
        let resp = self.request(Request::RunJob { job }).await?;
        match resp {
            Resp::JobRan {
                scanned,
                changed,
                skipped,
            } => Ok((scanned, changed, skipped)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Trigger one bounded, idempotent re-embed pass on the daemon (P5 Feature
    /// A): up to `limit` active memories with a stale embedding stamp are
    /// re-embedded through the single writer. `None` uses the daemon default.
    /// Returns `(scanned, changed, skipped)`.
    pub async fn reembed(&mut self, limit: Option<usize>) -> Result<(u64, u64, u64)> {
        let resp = self.request(Request::Reembed { limit }).await?;
        match resp {
            Resp::JobRan {
                scanned,
                changed,
                skipped,
            } => Ok((scanned, changed, skipped)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Retroactively redact secrets from every stored memory (W2.4). Returns
    /// `(scanned, redacted, reembed_pending)`; run `reembed` until `changed=0`
    /// afterward to recompute vectors for the changed rows. Admin op: a
    /// non-admin peer is rejected with `Error::PermissionDenied`.
    pub async fn scrub(&mut self) -> Result<(u64, u64, u64)> {
        let resp = self.request(Request::Scrub).await?;
        match resp {
            Resp::Scrubbed {
                scanned,
                redacted,
                reembed_pending,
            } => Ok((scanned, redacted, reembed_pending)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Namespace-scoped observability aggregate (doctor/stats PRD). Returns
    /// `(stats, provider_model, writer_alive)`: the read-pool aggregate for
    /// this connection's namespace, the daemon's running embedding-model
    /// identity, and writer-thread liveness. `window_days` bounds the windowed
    /// fields (`None` = daemon default). Read-only server-side: the stats path
    /// issues zero writer ops (W1.8).
    pub async fn stats(
        &mut self,
        window_days: Option<u32>,
    ) -> Result<(rb_types::MemoryStats, String, bool)> {
        let resp = self.request(Request::Stats { window_days }).await?;
        match resp {
            Resp::Stats {
                stats,
                provider_model,
                writer_alive,
            } => Ok((stats, provider_model, writer_alive)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// One-time namespace rename (W0.3 carryover): re-scope every memory from
    /// `old` to `new` in one daemon writer transaction. Refused with
    /// `Error::InvalidArgument` when `new` already has rows unless `merge` is
    /// set. Returns `(moved_memories, moved_vectors)`.
    pub async fn rename_namespace(
        &mut self,
        old: Namespace,
        new: Namespace,
        merge: bool,
    ) -> Result<(u64, u64)> {
        let resp = self
            .request(Request::NamespaceRename { old, new, merge })
            .await?;
        match resp {
            Resp::NamespaceRenamed { moved, vectors } => Ok((moved, vectors)),
            other => Err(Self::unexpected(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
    };
    use rb_types::Namespace;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sock");
        (dir, path)
    }

    // Accept ONE connection, read the handshake, ack it (optionally with a
    // forced ack contract version to simulate drift), then echo one request as
    // a canned Pong response. Returns after serving a single connection.
    async fn run_responder(listener: UnixListener, ack_version: u32, ok: bool) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());

        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        let ack = HandshakeAck {
            contract_version: ack_version,
            ok,
            message: if ok {
                None
            } else {
                Some("version mismatch".into())
            },
            capabilities: vec![],
        };
        write_frame(&mut framed, &ack).await.unwrap();
        if !ok {
            return;
        }

        // Serve exactly one request: reply Pong regardless of the request.
        let _req: Request = read_frame(&mut framed).await.unwrap();
        let resp = Response::Pong {
            contract_version: CONTRACT_VERSION,
            recall_channels: None,
        };
        write_frame(&mut framed, &resp).await.unwrap();
    }

    #[tokio::test]
    async fn connect_handshake_and_request_round_trip() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(run_responder(listener, CONTRACT_VERSION, true));

        let mut client = Client::connect(&path, Namespace::Project("rusty-brain".into()))
            .await
            .unwrap();
        let resp = client.request(Request::Ping).await.unwrap();
        match resp {
            Response::Pong {
                contract_version, ..
            } => {
                assert_eq!(contract_version, CONTRACT_VERSION);
            }
            other => panic!("expected Pong, got {other:?}"),
        }
        server.await.unwrap();
    }

    // The split send/recv phases must compose to the same round trip as
    // `request` — they exist so retrying callers can tell a dead-on-send
    // connection from a lost response.
    #[tokio::test]
    async fn send_request_then_recv_response_round_trips() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(run_responder(listener, CONTRACT_VERSION, true));

        let mut client = Client::connect(&path, Namespace::Global).await.unwrap();
        client.send_request(&Request::Ping).await.unwrap();
        match client.recv_response().await.unwrap() {
            Response::Pong {
                contract_version, ..
            } => {
                assert_eq!(contract_version, CONTRACT_VERSION);
            }
            other => panic!("expected Pong, got {other:?}"),
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_with_identity_carries_identity_on_the_handshake() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        // Inline responder that captures the handshake identity for assertion.
        let server = tokio::spawn(async move {
            let (stream, _addr) = listener.accept().await.unwrap();
            let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
                Framed::new(stream, LengthDelimitedCodec::new());
            let hs: Handshake = read_frame(&mut framed).await.unwrap();
            write_frame(
                &mut framed,
                &HandshakeAck {
                    contract_version: CONTRACT_VERSION,
                    ok: true,
                    message: None,
                    capabilities: vec![],
                },
            )
            .await
            .unwrap();
            hs.identity
        });

        let identity = crate::ClientIdentity {
            agent: Some("claude-code".into()),
            session_id: Some("s-7".into()),
            source: Some("hook".into()),
            ..Default::default()
        };
        let _client =
            Client::connect_with_identity(&path, Namespace::Global, Some(identity.clone()))
                .await
                .unwrap();
        let got = server.await.unwrap();
        assert_eq!(got, Some(identity), "identity must reach the daemon");
    }

    #[tokio::test]
    async fn connect_rejects_contract_version_mismatch() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        // Responder acks with a different contract version and ok=false.
        let server = tokio::spawn(run_responder(listener, CONTRACT_VERSION + 1, false));

        let err = Client::connect(&path, Namespace::Global).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Storage(_)),
            "version mismatch must fail connect, got {err:?}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_to_missing_socket_is_io_error() {
        let (_dir, path) = socket_path();
        // Never bind a listener -> connect must fail with an IO error.
        let err = Client::connect(&path, Namespace::Global).await.unwrap_err();
        // Connect failure now carries the io::ErrorKind via Error::IoKind so the
        // client can classify auto-start policy without string matching.
        assert!(
            err.io_kind().is_some(),
            "missing socket should carry an io::ErrorKind, got {err:?}"
        );
    }
}

#[cfg(test)]
mod wrapper_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        error_to_response, read_frame, write_frame, Handshake, HandshakeAck, Request, Response,
        CONTRACT_VERSION,
    };
    use rb_types::{Error, MemoryId, MemoryNote, MemoryType, Namespace, SearchResult};
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrap.sock");
        (dir, path)
    }

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Global,
            "remembered body".into(),
            MemoryType::Insight,
            6,
        )
    }

    // Accept one connection, handshake (advertising NO capabilities — the
    // pre-anchor daemon shape), then answer each incoming Request with a
    // matching canned Response until the client disconnects.
    async fn serve(listener: UnixListener, fixed_id: MemoryId) {
        serve_with_capabilities(listener, fixed_id, vec![]).await;
    }

    // `serve` with an explicit advertised-capability list, so tests cover both
    // the old-daemon (empty) and anchor-capable daemon shapes.
    async fn serve_with_capabilities(
        listener: UnixListener,
        fixed_id: MemoryId,
        capabilities: Vec<String>,
    ) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());

        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        write_frame(
            &mut framed,
            &HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: true,
                message: None,
                capabilities,
            },
        )
        .await
        .unwrap();

        loop {
            let req: Request = match read_frame(&mut framed).await {
                Ok(r) => r,
                Err(_) => break, // client closed
            };
            let resp = match req {
                Request::Remember {
                    content, anchors, ..
                } => {
                    if content == "anchored-probe" {
                        // The anchored-remember test: prove the anchors rode
                        // the frame (a panic here fails the joined task).
                        assert_eq!(anchors.len(), 2, "anchors must ride the wire");
                        assert_eq!(anchors[0].value, "src/a.rs");
                        assert_eq!(anchors[0].start_line, Some(3));
                    }
                    Response::Remembered {
                        id: fixed_id.clone(),
                    }
                }
                Request::Recall {
                    query,
                    memory_type,
                    tags,
                    filter,
                    ..
                } => {
                    let hit = || {
                        vec![SearchResult {
                            memory: note(),
                            score: 0.5,
                            channels: rb_types::ChannelHits::default(),
                        }]
                    };
                    // Filter-parity probes: a hit comes back ONLY when the
                    // dimensions rode the wire in the expected slot, proving
                    // `recall_filtered_with_status` splits legacy vs additive.
                    let results = match query.as_str() {
                        "probe-legacy" => {
                            if memory_type == Some(MemoryType::BugFix)
                                && tags == vec!["t".to_string()]
                                && filter.is_empty()
                            {
                                hit()
                            } else {
                                Vec::new()
                            }
                        }
                        "probe-filter" => {
                            if memory_type.is_none()
                                && filter.min_confidence == Some(0.9)
                                && filter.sources == vec!["hook".to_string()]
                            {
                                hit()
                            } else {
                                Vec::new()
                            }
                        }
                        "probe-anchors" => {
                            // Anchor filters ride the ADDITIVE filter field.
                            if filter.anchors.len() == 1
                                && filter.anchors[0].kind == rb_types::AnchorKind::File
                                && filter.anchors[0].value == "src/a.rs"
                            {
                                hit()
                            } else {
                                Vec::new()
                            }
                        }
                        _ => hit(),
                    };
                    Response::Recalled {
                        results,
                        // Keyed on the query so the typed-wrapper test can prove
                        // the W1.6d flag rides the wire to `recall_with_status`.
                        degraded: query == "degraded",
                    }
                }
                Request::Get { .. } => Response::Got {
                    memory: Some(note()),
                },
                Request::List {
                    min_importance,
                    limit,
                    filter,
                } => {
                    // Filter-parity probe (keyed on limit=42): a note comes
                    // back ONLY when min_importance rode the legacy slot and
                    // the new dimensions rode the additive filter.
                    let memories = if limit == 42 {
                        if min_importance == Some(6)
                            && filter.min_importance.is_none()
                            && filter.state == rb_types::MemoryState::Archived
                        {
                            vec![note()]
                        } else {
                            Vec::new()
                        }
                    } else {
                        vec![note()]
                    };
                    Response::Listed { memories }
                }
                Request::Graph { .. } => Response::GraphResult {
                    memories: vec![note()],
                },
                Request::Update { .. } => Response::Updated,
                Request::Link { .. } => Response::Linked,
                Request::Feedback { .. } => Response::FeedbackRecorded { confidence: 0.7 },
                Request::Delete { .. } => Response::Deleted,
                Request::Context => Response::ContextResult {
                    recent: vec![note()],
                    important: vec![note()],
                    total: 2,
                },
                Request::Ping => Response::Pong {
                    contract_version: CONTRACT_VERSION,
                    recall_channels: None,
                },
                Request::Subscribe { .. } => Response::Pong {
                    contract_version: CONTRACT_VERSION,
                    recall_channels: None,
                },
                Request::RunJob { .. } => Response::JobRan {
                    scanned: 4,
                    changed: 2,
                    skipped: 2,
                },
                Request::Reembed { .. } => Response::JobRan {
                    scanned: 6,
                    changed: 5,
                    skipped: 1,
                },
                Request::NamespaceRename { merge, .. } => Response::NamespaceRenamed {
                    // Canned counts keyed on `merge` so the typed-wrapper test
                    // can prove the flag rides the wire.
                    moved: if merge { 7 } else { 3 },
                    vectors: 2,
                },
                Request::Scrub => Response::Scrubbed {
                    scanned: 0,
                    redacted: 0,
                    reembed_pending: 0,
                },
                // Canned payload keyed on `window_days` so the typed-wrapper
                // test can prove the window rides the wire.
                Request::Stats { window_days } => Response::Stats {
                    stats: rb_types::MemoryStats {
                        window_days: window_days.unwrap_or(30),
                        live: 5,
                        ..Default::default()
                    },
                    provider_model: "deterministic".to_string(),
                    writer_alive: true,
                },
            };
            write_frame(&mut framed, &resp).await.unwrap();
        }
    }

    async fn connect(path: &std::path::Path) -> Client {
        Client::connect(path, Namespace::Global).await.unwrap()
    }

    #[tokio::test]
    async fn anchor_payloads_are_rejected_against_a_daemon_without_the_capability() {
        // Old-daemon compatibility: a pre-anchor daemon advertises no
        // `anchors` capability. It would silently IGNORE anchor filters on
        // the additive `filter` field (pre-parity daemons) and silently DROP
        // `Remember.anchors` — so the client must reject anchor-bearing calls
        // LOCALLY, before any frame is sent. (The fake server here — acking
        // with NO capabilities — would happily return a hit/ack for any
        // request, so an Err proves the request never round-tripped.)
        use rb_types::{AnchorFilter, AnchorKind, RecallFilter};
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(serve(listener, MemoryId::new()));

        let mut c = connect(&path).await;
        assert!(
            !c.supports_anchors(),
            "a capability-less ack must read as unsupported"
        );
        let anchored = RecallFilter {
            anchors: vec![AnchorFilter {
                kind: AnchorKind::File,
                value: "src/lib.rs".into(),
            }],
            ..Default::default()
        };

        let err = c
            .recall_filtered_with_status("q".into(), anchored.clone(), 5)
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "recall must fail fast client-side, got {err:?}"
        );

        let err = c.list_filtered(anchored, 5).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "list must fail fast client-side, got {err:?}"
        );

        let err = c
            .remember_anchored(
                "anchored body".into(),
                None,
                MemoryType::Insight,
                5,
                vec![],
                vec![],
                vec![],
                None,
                vec![rb_types::MemoryAnchor::parse_file_spec("src/a.rs").unwrap()],
                None,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "anchored remember must fail fast client-side, got {err:?}"
        );

        // Anchor-FREE calls still work against the old daemon (graceful
        // degradation, not a hard cut).
        let listed = c.list_filtered(RecallFilter::default(), 5).await.unwrap();
        assert_eq!(listed.len(), 1);
        let id = c
            .remember_anchored(
                "plain body".into(),
                None,
                MemoryType::Insight,
                5,
                vec![],
                vec![],
                vec![],
                None,
                vec![],
                None,
            )
            .await
            .unwrap();
        assert_eq!(id.to_string().len(), 36);

        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn anchor_payloads_flow_when_the_daemon_advertises_the_capability() {
        use rb_types::{AnchorFilter, AnchorKind, RecallFilter};
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let fixed_id = MemoryId::new();
        let server = tokio::spawn(serve_with_capabilities(
            listener,
            fixed_id.clone(),
            vec![crate::CAP_ANCHORS.to_string()],
        ));

        let mut c = connect(&path).await;
        assert!(
            c.supports_anchors(),
            "the capability must be read off the ack"
        );

        // Anchored remember: the fake server ASSERTS the anchors rode the
        // frame for this content (a mismatch panics the joined task).
        let id = c
            .remember_anchored(
                "anchored-probe".into(),
                None,
                MemoryType::Insight,
                5,
                vec![],
                vec![],
                vec![],
                None,
                vec![
                    rb_types::MemoryAnchor::parse_file_spec("src/a.rs:3-9").unwrap(),
                    rb_types::MemoryAnchor::new(AnchorKind::Symbol, "Foo::bar").unwrap(),
                ],
                None,
            )
            .await
            .unwrap();
        assert_eq!(id, fixed_id);

        // Anchor filters ride the additive filter field: a hit comes back
        // ONLY when the fake server saw them there.
        let (results, _) = c
            .recall_filtered_with_status(
                "probe-anchors".into(),
                RecallFilter {
                    anchors: vec![AnchorFilter {
                        kind: AnchorKind::File,
                        value: "src/a.rs".into(),
                    }],
                    ..Default::default()
                },
                5,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "anchor filter must ride the wire");

        // An invalid anchor is still rejected locally (validation before send).
        let err = c
            .remember_anchored(
                "bad".into(),
                None,
                MemoryType::Insight,
                5,
                vec![],
                vec![],
                vec![],
                None,
                vec![rb_types::MemoryAnchor {
                    kind: AnchorKind::Commit,
                    value: "abc".into(),
                    start_line: Some(1),
                    end_line: None,
                }],
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::InvalidArgument(_)));

        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn filtered_wrappers_split_legacy_slots_from_the_additive_filter() {
        use rb_types::{MemoryState, RecallFilter};
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(serve(listener, MemoryId::new()));

        let mut c = connect(&path).await;

        // A single type + tags are expressible pre-filter, so they must ride
        // the LEGACY wire slots (old daemons keep honoring them) and the
        // additive filter must stay off the frame.
        let (results, _) = c
            .recall_filtered_with_status(
                "probe-legacy".into(),
                RecallFilter {
                    types: vec![MemoryType::BugFix],
                    tags: vec!["t".into()],
                    ..Default::default()
                },
                5,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "legacy slots must carry type+tags");

        // New dimensions ride the additive filter field.
        let (results, _) = c
            .recall_filtered_with_status(
                "probe-filter".into(),
                RecallFilter {
                    min_confidence: Some(0.9),
                    sources: vec!["hook".into()],
                    ..Default::default()
                },
                5,
            )
            .await
            .unwrap();
        assert_eq!(results.len(), 1, "new dimensions must ride the filter");

        // List: min_importance rides the legacy slot; state rides the filter.
        let listed = c
            .list_filtered(
                RecallFilter {
                    min_importance: Some(6),
                    state: MemoryState::Archived,
                    ..Default::default()
                },
                42,
            )
            .await
            .unwrap();
        assert_eq!(listed.len(), 1, "legacy min_importance + filtered state");

        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn typed_wrappers_return_domain_types() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let fixed_id = MemoryId::new();
        let server = tokio::spawn(serve(listener, fixed_id.clone()));

        let mut c = connect(&path).await;

        let id = c
            .remember(
                "body".into(),
                Some("ctx".into()),
                MemoryType::Insight,
                6,
                vec!["k".into()],
                vec!["t".into()],
                vec![],
                Some(1.0),
            )
            .await
            .unwrap();
        assert_eq!(id, fixed_id);

        let results = c.recall("q".into(), None, vec![], 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!((results[0].score - 0.5).abs() < f32::EPSILON);

        // The W1.6d degraded flag reaches the typed client (mock keys it on
        // the query string).
        let (results, degraded) = c
            .recall_with_status("q".into(), None, vec![], 5)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(!degraded, "ordinary recall is not degraded");
        let (_, degraded) = c
            .recall_with_status("degraded".into(), None, vec![], 5)
            .await
            .unwrap();
        assert!(degraded, "the degraded flag must ride the wire");

        let got = c.get(fixed_id.clone()).await.unwrap();
        assert!(got.is_some());

        let listed = c.list(Some(5), 10).await.unwrap();
        assert_eq!(listed.len(), 1);

        let graphed = c.graph(fixed_id.clone(), 2).await.unwrap();
        assert_eq!(graphed.len(), 1);

        c.update(fixed_id.clone(), Default::default())
            .await
            .unwrap();
        c.delete(fixed_id.clone()).await.unwrap();

        let (recent, important, total) = c.context().await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(important.len(), 1);
        assert_eq!(total, 2);

        let version = c.ping().await.unwrap();
        assert_eq!(version, CONTRACT_VERSION);

        let (scanned, changed, skipped) = c.run_job(rb_types::JobKind::LinkDecay).await.unwrap();
        assert_eq!((scanned, changed, skipped), (4, 2, 2));

        let (rs, rc, rk) = c.reembed(Some(10)).await.unwrap();
        assert_eq!((rs, rc, rk), (6, 5, 1));

        // The fake server keys `moved` on the merge flag, proving the flag
        // (and both namespaces) ride the wire and the counts come back typed.
        let (moved, vectors) = c
            .rename_namespace(
                Namespace::Project("scratch".into()),
                Namespace::Project("rb".into()),
                false,
            )
            .await
            .unwrap();
        assert_eq!((moved, vectors), (3, 2));
        let (moved, vectors) = c
            .rename_namespace(
                Namespace::Project("scratch".into()),
                Namespace::Project("rb".into()),
                true,
            )
            .await
            .unwrap();
        assert_eq!((moved, vectors), (7, 2));

        // The fake server echoes the window into the payload, proving the
        // window rides the wire and the payload comes back typed.
        let (stats, provider_model, writer_alive) = c.stats(Some(7)).await.unwrap();
        assert_eq!(stats.window_days, 7);
        assert_eq!(stats.live, 5);
        assert_eq!(provider_model, "deterministic");
        assert!(writer_alive);
        let (stats, _, _) = c.stats(None).await.unwrap();
        assert_eq!(stats.window_days, 30, "None uses the daemon default");

        drop(c);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn remember_rejects_invalid_confidence_before_any_io() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        // `serve` answers ANY Remember with Remembered, so an Err here can only
        // come from the client-side validation: the frame never hit the wire.
        let server = tokio::spawn(serve(listener, MemoryId::new()));

        let mut c = connect(&path).await;
        for bad in [f32::NAN, 1.5] {
            let err = c
                .remember(
                    "body".into(),
                    None,
                    MemoryType::Insight,
                    5,
                    vec![],
                    vec![],
                    vec![],
                    Some(bad),
                )
                .await
                .unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument(_)),
                "confidence {bad} must be InvalidArgument, got {err:?}"
            );
        }
        drop(c);
        server.await.unwrap();
    }

    // A responder that always returns a Response::Error, to prove wrappers map
    // wire errors back into rb_types::Error.
    async fn serve_error(listener: UnixListener) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());
        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        write_frame(
            &mut framed,
            &HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: true,
                message: None,
                capabilities: vec![],
            },
        )
        .await
        .unwrap();
        let _req: Request = read_frame(&mut framed).await.unwrap();
        let resp = error_to_response(&Error::NotFound(MemoryId::new()));
        write_frame(&mut framed, &resp).await.unwrap();
    }

    #[tokio::test]
    async fn wrapper_maps_wire_error_to_domain_error() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(serve_error(listener));

        let mut c = connect(&path).await;
        let err = c.get(MemoryId::new()).await.unwrap_err();
        // NotFound degrades to Storage on the wire (see error.rs), but it is an
        // Err, not a falsely-successful None.
        assert!(matches!(err, Error::Storage(_)), "got {err:?}");
        server.await.unwrap();
    }
}

#[cfg(test)]
mod subscribe_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
    };
    use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub.sock");
        (dir, path)
    }

    // Accept one connection, handshake, read exactly one Subscribe request, then
    // stream a Change frame followed by a Lagged frame, then close.
    async fn serve_stream(listener: UnixListener, change_id: MemoryId) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());
        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        write_frame(
            &mut framed,
            &HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: true,
                message: None,
                capabilities: vec![],
            },
        )
        .await
        .unwrap();

        let req: Request = read_frame(&mut framed).await.unwrap();
        assert!(
            matches!(req, Request::Subscribe { since: None }),
            "expected Subscribe"
        );

        // The client's subscribe() blocks on this ack before returning.
        write_frame(&mut framed, &Response::SubscribeAck)
            .await
            .unwrap();

        write_frame(
            &mut framed,
            &Response::Change(MemoryChanged {
                id: change_id,
                namespace: Namespace::Global,
                kind: ChangeKind::Created,
                seq: Some(1),
            }),
        )
        .await
        .unwrap();
        write_frame(&mut framed, &Response::Lagged { dropped: 5 })
            .await
            .unwrap();
        // Drop closes the stream; the client's next recv_change returns Err(Io).
    }

    #[tokio::test]
    async fn subscribe_then_recv_change_and_lagged() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let change_id = MemoryId::new();
        let server = tokio::spawn(serve_stream(listener, change_id.clone()));

        let mut client = Client::connect(&path, Namespace::Global).await.unwrap();
        client.subscribe().await.unwrap();

        match client.recv_change().await.unwrap() {
            SubscribeItem::Change(evt) => {
                assert_eq!(evt.id, change_id);
                assert_eq!(evt.kind, ChangeKind::Created);
            }
            other => panic!("expected Change, got {other:?}"),
        }
        match client.recv_change().await.unwrap() {
            SubscribeItem::Lagged(n) => assert_eq!(n, 5),
            other => panic!("expected Lagged, got {other:?}"),
        }
        // Stream closed -> the next recv is a transport error, not a hang.
        let err = client.recv_change().await.unwrap_err();
        assert!(matches!(err, Error::Io(_)), "closed stream -> Io: {err:?}");

        server.await.unwrap();
    }
}
