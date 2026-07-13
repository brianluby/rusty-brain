use rb_types::{
    FeedbackKind, JobKind, LinkType, MemoryChanged, MemoryId, MemoryNote, MemoryType,
    MemoryUpdates, Namespace, RecallFilter, SearchResult,
};
use serde::{Deserialize, Serialize};

/// Wire contract version carried in the handshake. Clients and the daemon must
/// agree on this exact value; mismatch is rejected at connect time.
///
/// v2 (P5 Feature C): result rows (recall/list/context) and the `get` payload
/// carry an additive `MemoryNote.contested` boolean. The field is
/// `#[serde(default)]`, so a v1 payload without it deserializes to `false` — but
/// the version bump lets clients detect (and rely on) the richer shape.
pub const CONTRACT_VERSION: u32 = 2;

/// First frame the client sends after connecting.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Handshake {
    pub contract_version: u32,
    pub namespace: Namespace,
    /// Optional client-declared identity (W0.5). Additive + `#[serde(default)]`
    /// — an old client omits it (`None`) and an old daemon ignores it (serde
    /// tolerates unknown fields), so NO contract-version bump is needed.
    #[serde(default)]
    pub identity: Option<ClientIdentity>,
}

/// Who/where/what is on the other end of a connection, declared by the client
/// at handshake and stamped onto every memory it writes. All fields optional:
/// the daemon falls back to its own whoami for `user`/`host` (same-host UDS),
/// while `agent`/`session_id`/`source` are client-knowledge only.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientIdentity {
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    /// The agent CLI driving the write (e.g. `claude-code`), when known.
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// Producer surface, declared per binary: `hook` | `mcp` | `cli`.
    #[serde(default)]
    pub source: Option<String>,
}

/// Capability string a daemon advertises when it evaluates typed code
/// anchors (the `memory_anchors` table + anchor filters, PRD 2026-07-02).
/// Pre-anchor daemons never send it, so clients can distinguish "this daemon
/// evaluates anchors" from "this daemon would silently drop/ignore them"
/// WITHOUT a CONTRACT_VERSION bump.
pub const CAP_ANCHORS: &str = "anchors";

/// Daemon reply to a `Handshake`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HandshakeAck {
    pub contract_version: u32,
    pub ok: bool,
    pub message: Option<String>,
    /// Feature capabilities this daemon supports beyond the bare contract
    /// version (first: [`CAP_ANCHORS`]). Additive + `#[serde(default,
    /// skip_serializing_if)]`: an old daemon's ack (no key) decodes to an
    /// empty list — the client then treats anchor-bearing requests as
    /// unsupported and fails fast locally — and an empty list serializes to
    /// nothing, byte-identical to the pre-capability ack (the
    /// `Handshake.identity` precedent; NO CONTRACT_VERSION bump).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// One request per engine operation. Internally tagged on `op`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "op")]
pub enum Request {
    Remember {
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_files: Vec<String>,
        /// Explicit trust prior in `0.0..=1.0` (W0.5 / F39 producer
        /// down-payment), or `None` when the caller expressed no prior.
        /// `None` means "use the full-trust default (1.0), but let an enricher
        /// fill it if one runs"; `Some(x)` is an explicit caller prior that an
        /// enricher must NOT override (fix #4 — distinguishing an explicit 1.0
        /// from the default, which a bare `f32` could not). Hook captures send
        /// `Some(0.7)`. `#[serde(default, skip_serializing_if)]`: an old client
        /// omits the field (`None`), and a `None` serializes to nothing —
        /// byte-identical to the pre-field frame, so no CONTRACT_VERSION bump.
        /// Range-validated by the engine.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<f32>,
        /// When `Some(old)`, the daemon ATOMICALLY supersedes `old` with the
        /// memory this request stores — archiving `old`, stamping its
        /// `superseded_by`, and pruning its vector — exactly the "store a
        /// replacement memory that supersedes the old one" path the `Update`
        /// and `Link` rejections point at (W0.4 / W3.1 update-as-supersede).
        /// The replacement is written first; the supersede follows in a second
        /// writer transaction (the existing atomic supersede), so a supersede
        /// failure leaves the new memory stored and `old` simply un-archived —
        /// never a partial/corrupt state. `#[serde(default,
        /// skip_serializing_if)]`: an old client omits the field (`None`) and a
        /// `None` serializes to nothing — byte-identical to the pre-field
        /// frame, so NO CONTRACT_VERSION bump (the `confidence` precedent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<MemoryId>,
        /// Typed code anchors to store with the memory (PRD 2026-07-02).
        /// Additive + `#[serde(default, skip_serializing_if)]`: an old
        /// client's frame (no key) decodes to no anchors, and an empty list
        /// serializes to nothing — byte-identical to the pre-anchor frame,
        /// so NO CONTRACT_VERSION bump. A PRE-ANCHOR daemon would silently
        /// DROP this field, so new clients gate on the daemon's advertised
        /// [`CAP_ANCHORS`] capability before sending non-empty anchors
        /// (see `Client::remember_anchored`).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        anchors: Vec<rb_types::MemoryAnchor>,
    },
    Recall {
        query: String,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
        limit: usize,
        /// Unified recall filter (PRD 2026-07-02 search-filter parity):
        /// confidence/date ranges, sources, contested, archived state, and the
        /// anchor plumbing. `memory_type`/`tags` above stay the legacy wire
        /// slots for the subset old daemons honor; the daemon FOLDS them into
        /// this filter (`RecallFilter::fold_recall_legacy`). Additive +
        /// `#[serde(default, skip_serializing_if)]`: an old client's frame (no
        /// key) decodes to the unconstrained default, and an empty filter
        /// serializes to nothing — byte-identical to the pre-filter frame, so
        /// NO CONTRACT_VERSION bump (the `Remember.confidence` precedent). An
        /// old daemon ignores the unknown field, so a new client degrades
        /// gracefully to the legacy-slot subset.
        #[serde(default, skip_serializing_if = "RecallFilter::is_empty")]
        filter: RecallFilter,
    },
    Get {
        id: MemoryId,
    },
    List {
        min_importance: Option<u8>,
        limit: usize,
        /// Unified list filter — same model, semantics, and wire-compat story
        /// as `Recall::filter`; `min_importance` above stays the legacy slot
        /// (folded via `RecallFilter::fold_list_legacy`).
        #[serde(default, skip_serializing_if = "RecallFilter::is_empty")]
        filter: RecallFilter,
    },
    Graph {
        id: MemoryId,
        depth: u8,
    },
    Update {
        id: MemoryId,
        updates: MemoryUpdates,
    },
    Delete {
        id: MemoryId,
    },
    Context,
    RunJob {
        job: JobKind,
    },
    /// Re-embed up to `limit` active memories whose stored
    /// `(embedding_model, embedding_input_version)` stamp is stale (P5 Feature
    /// A). `None` uses the daemon's configured batch default. Replies with
    /// `Response::JobRan { scanned, changed, skipped }`; bounded + idempotent.
    Reembed {
        limit: Option<usize>,
    },
    Ping,
    /// Open a live change-notification stream. The daemon stops the
    /// request/response cadence for this connection and streams `Response::Change`
    /// (and `Response::Lagged` on broadcast overflow) until the client disconnects.
    /// The stream is scoped to the connection's handshake namespace, filtered
    /// server-side.
    ///
    /// `since` (W2.7): when set, the daemon first REPLAYS every oplog change in
    /// the namespace with `seq > since` (oldest first), then continues with
    /// live events — a reconnecting subscriber resumes from its cursor instead
    /// of silently missing whatever happened while it was away. Additive +
    /// `#[serde(default)]`/`skip_serializing_if`: an old client's frame (no
    /// key) decodes to `None`, and a `None` from a new client serializes
    /// byte-identical to the old unit-variant frame.
    Subscribe {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
    },
    /// One-time namespace rename (W0.3 carryover): re-scope every memory row
    /// from `old` to `new` in ONE writer transaction (memories, vec0 partition
    /// rows, one oplog entry). Refuses a non-empty `new` unless `merge` is set.
    /// Additive variant per the `Handshake.identity` precedent: an old daemon
    /// fails to decode it and closes the connection (no CONTRACT_VERSION
    /// bump); `merge` is `#[serde(default)]` so its absence decodes to false.
    NamespaceRename {
        old: Namespace,
        new: Namespace,
        #[serde(default)]
        merge: bool,
    },
    /// Create an explicit link between two memories (W2.2: the user-facing
    /// producer for `contradicts`; the read side already computes `contested`
    /// from active contradicts edges). Additive variant per the
    /// `NamespaceRename` precedent: an old daemon fails to decode it and
    /// closes the connection (no CONTRACT_VERSION bump); `reason` is
    /// `#[serde(default)]` so its absence decodes to `None`.
    Link {
        from: MemoryId,
        to: MemoryId,
        link_type: LinkType,
        #[serde(default)]
        reason: Option<String>,
    },
    /// Retroactively redact secrets from every stored memory (W2.4
    /// `rusty-brain scrub`). Admin op, peer-gated server-side like RunJob /
    /// Reembed / NamespaceRename. Replies with `Response::Scrubbed`. Additive
    /// variant per the `NamespaceRename` precedent (no CONTRACT_VERSION bump).
    Scrub,
    /// Record a usefulness signal about a recalled memory (W3.7 / F37): the
    /// distinct correctness/usefulness signal `access_count` is not (it counts
    /// "returned", not "useful"). Namespace-scoped like `Update`/`Link` (NOT an
    /// admin op) — the engine verifies `id` lives in the connection's namespace.
    /// The daemon records a durable event row + an oplog entry and nudges the
    /// memory's `confidence` (single-axis coupling, see `FeedbackKind`),
    /// replying with `Response::FeedbackRecorded { confidence }`. Additive
    /// variant per the `Link`/`Scrub` precedent: an old daemon fails to decode
    /// it and closes the connection (no CONTRACT_VERSION bump — the handshake
    /// version gates the shape of SHARED result types, and a new op the old
    /// daemon simply does not implement is not such a change).
    Feedback {
        id: MemoryId,
        kind: FeedbackKind,
    },
    /// Namespace-scoped observability aggregate (doctor/stats PRD): recall
    /// volume, feedback ratios, top/never-recalled, contested count, corpus
    /// growth, re-embed backlog. Scoped to the connection's handshake
    /// namespace like `Context` (NOT an admin op); computed entirely on the
    /// daemon's read pool — the stats path issues ZERO writer ops (W1.8).
    /// `window_days` bounds the windowed fields; `None` uses the daemon
    /// default. Additive variant per the `Feedback` precedent: an old daemon
    /// fails to decode it and closes the connection (no CONTRACT_VERSION
    /// bump), and the `#[serde(default)]`/`skip_serializing_if` pair keeps a
    /// window-less frame byte-identical to a bare unit variant.
    Stats {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window_days: Option<u32>,
    },
    /// Retention/forget pass (retention PRD RET-2). Carries the client's
    /// RESOLVED `[retention]` policy verbatim so the daemon evaluates exactly
    /// what the user configured (the daemon re-validates it fail-closed —
    /// never trust a wire-supplied policy). Namespace-scoped by the
    /// handshake. `dry_run: true` returns the plan without writes;
    /// `dry_run: false` executes a bounded sweep. The serde defaults are the
    /// safety contract: an absent `mode` decodes to Apply (never Hard) and an
    /// absent `dry_run` decodes to a preview (never an execute). Hard-EXECUTE
    /// (`mode: hard, dry_run: false`) is admin-gated like `Scrub`. Additive
    /// variant per the Stats precedent: an old daemon fails to decode it and
    /// closes the connection (no CONTRACT_VERSION bump).
    Forget {
        policy: rb_types::RetentionPolicy,
        #[serde(default)]
        mode: rb_types::ForgetMode,
        #[serde(default = "default_forget_dry_run")]
        dry_run: bool,
    },
    /// Decision-history timeline for one memory (PRD 2026-07-02): the
    /// supersede chain in both directions plus active
    /// contradicts/extends/references edges, derived entirely from existing
    /// rows. Scoped to the connection's handshake namespace like `Get`/`Stats`
    /// (NOT an admin op); computed on the daemon's read pool — the history
    /// path issues ZERO writer ops (W1.8). `depth` bounds the chain walk per
    /// direction; `None` uses the daemon's safety cap. Additive variant per
    /// the `Stats` precedent: an old daemon fails to decode it and closes the
    /// connection (no CONTRACT_VERSION bump), and the
    /// `#[serde(default)]`/`skip_serializing_if` pair keeps a depth-less frame
    /// minimal.
    History {
        id: MemoryId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        depth: Option<u32>,
    },
    /// Review-queue generation / policy sweep (PRD 2026-07-02
    /// contradiction/dedup review). Namespace-scoped by the handshake.
    /// `dry_run: true` (the serde default — an absent flag must PREVIEW,
    /// never execute) returns the priority-ordered queue plus, when `policy`
    /// is named, the per-item plan; `dry_run: false` requires a `policy` and
    /// executes one bounded apply pass. Every action is reversible
    /// (supersede/archive are soft, demote is an update), so apply is NOT
    /// admin-gated — the `Forget` apply precedent. `since`/`limit`/
    /// `threshold` are optional knobs, server-clamped. Additive variant per
    /// the `Forget` precedent: an old daemon fails to decode it and closes
    /// the connection (no CONTRACT_VERSION bump).
    Review {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<rb_types::ReviewPolicy>,
        #[serde(default = "default_review_dry_run")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threshold: Option<f32>,
    },
    /// Apply ONE review resolution (REV-2 interactive mode): keep / merge /
    /// archive / demote / snooze on the item identified by `reason` + member
    /// `ids` (the daemon recomputes the canonical key server-side — the key
    /// never travels as free text). Namespace-scoped like `Update`/`Link`
    /// (NOT an admin op): the daemon verifies every id lives in the
    /// connection's namespace, validates the action shape fail-closed
    /// (`ReviewAction::validate`), and orchestrates the existing atomic
    /// supersede/archive/confidence primitives. Additive variant per the
    /// `Review` precedent (no CONTRACT_VERSION bump).
    Resolve {
        reason: rb_types::ReviewReason,
        ids: Vec<MemoryId>,
        action: rb_types::ReviewAction,
        /// Near-dup similarity bound for the resolve-time MERGE revalidation
        /// (the plan->resolve TOCTOU fix): the daemon re-checks that the pair
        /// still qualifies as near-duplicates at this threshold inside the
        /// atomic merge transaction. `None` uses the conservative default;
        /// server-clamped like `Review.threshold`. Additive +
        /// `#[serde(default, skip_serializing_if)]` — an old frame decodes to
        /// the default and a `None` stays off the wire.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threshold: Option<f32>,
    },
}

/// An absent `dry_run` on the wire must PREVIEW, never execute.
fn default_forget_dry_run() -> bool {
    true
}

/// An absent review `dry_run` must PREVIEW, never execute (the Forget rule).
fn default_review_dry_run() -> bool {
    true
}

/// True when `req` carries a typed-code-anchor payload that a PRE-ANCHOR
/// daemon would silently DROP (`Remember.anchors`) or, on a pre-filter
/// daemon, silently IGNORE (`Recall`/`List` filter anchors). Raw-`Request`
/// callers (the MCP adapter) gate on this plus the daemon's advertised
/// [`CAP_ANCHORS`]; the typed `Client` wrappers gate internally.
#[must_use]
pub fn request_uses_anchors(req: &Request) -> bool {
    match req {
        Request::Remember { anchors, .. } => !anchors.is_empty(),
        Request::Recall { filter, .. } | Request::List { filter, .. } => !filter.anchors.is_empty(),
        _ => false,
    }
}

/// Aggregate per-channel recall hit-contribution totals (W1.0), surfaced on
/// the daemon status path (`Ping` -> `Pong`). Counts are cumulative since
/// daemon start and cheap: `recalls` is the number of recall requests served;
/// the per-channel fields count returned results that channel contributed (a
/// result surfaced by several channels increments each of them).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecallChannelTotals {
    #[serde(default)]
    pub recalls: u64,
    #[serde(default)]
    pub fts_hits: u64,
    #[serde(default)]
    pub vector_hits: u64,
    #[serde(default)]
    pub graph_hits: u64,
}

/// Result of the truncating WAL checkpoint performed after a scrub.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrubCheckpoint {
    /// Whether a concurrent reader or writer prevented complete truncation.
    pub busy: bool,
    /// Frames observed in the WAL (`-1` when the database has no WAL).
    pub log_frames: i64,
    /// Frames copied into the main database (`-1` when there is no WAL).
    pub checkpointed_frames: i64,
}

/// Typed result of a scrub request, including its at-rest checkpoint status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubResult {
    pub scanned: u64,
    pub redacted: u64,
    pub reembed_pending: u64,
    /// `None` when talking to an older daemon that predates checkpoint status.
    pub wal_checkpoint: Option<ScrubCheckpoint>,
    /// Checkpoint execution failure after the redaction transaction committed.
    pub wal_checkpoint_error: Option<String>,
}

impl ScrubResult {
    /// Conservatively reports risk when the checkpoint was busy or unavailable.
    #[must_use]
    pub fn plaintext_may_remain_in_wal(&self) -> bool {
        self.wal_checkpoint_error.is_some()
            || self.wal_checkpoint.is_none_or(|checkpoint| checkpoint.busy)
    }
}

/// One response per request. Internally tagged on `result`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "result")]
pub enum Response {
    Remembered {
        id: MemoryId,
    },
    Recalled {
        results: Vec<SearchResult>,
        /// `true` when recall DEGRADED to keyword+graph because the embedder
        /// errored (W1.6d / F19). Additive + `#[serde(default)]`: an old
        /// daemon omits it (`false`) and an old client ignores it, so NO
        /// contract-version bump is needed — the `Pong.recall_channels`
        /// precedent. `skip_serializing_if` keeps a non-degraded Recalled
        /// frame byte-identical to the pre-W1.6 shape.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        degraded: bool,
    },
    Got {
        memory: Option<MemoryNote>,
    },
    Listed {
        memories: Vec<MemoryNote>,
    },
    GraphResult {
        memories: Vec<MemoryNote>,
    },
    Updated,
    Deleted,
    ContextResult {
        recent: Vec<MemoryNote>,
        important: Vec<MemoryNote>,
        total: usize,
    },
    Pong {
        contract_version: u32,
        /// Aggregate per-channel recall hit-contribution counters since daemon
        /// start (W1.0). Additive + `#[serde(default)]`: an old daemon omits it
        /// (`None`) and an old client ignores it, so NO contract-version bump
        /// is needed — the `Handshake.identity` precedent. Only the daemon's
        /// real Ping path populates it; internal pseudo-Pongs leave it `None`.
        /// `skip_serializing_if` keeps a `None` Pong byte-identical to the
        /// pre-W1.0 frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recall_channels: Option<RecallChannelTotals>,
    },
    JobRan {
        scanned: u64,
        changed: u64,
        skipped: u64,
    },
    /// Reply to `Request::NamespaceRename`: `moved` memories rows were
    /// re-scoped and `vectors` vec0 rows were re-inserted under the new
    /// partition key. Additive variant; old clients never see it because they
    /// never send the request.
    NamespaceRenamed {
        moved: u64,
        vectors: u64,
    },
    /// Acknowledges a `Request::Link`. Additive variant; old clients never see
    /// it because they never send the request.
    Linked,
    /// Reply to `Request::Scrub` (W2.4): `redacted` of `scanned` rows had a
    /// secret removed; `reembed_pending` of those need a `reembed` pass to
    /// recompute their vector. Additive variant; old clients never see it.
    Scrubbed {
        scanned: u64,
        redacted: u64,
        reembed_pending: u64,
        /// Additive and optional for old-daemon/new-client compatibility.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wal_checkpoint: Option<ScrubCheckpoint>,
        /// Additive partial-success diagnostic when checkpoint execution failed
        /// after the redaction transaction committed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wal_checkpoint_error: Option<String>,
    },
    /// Acknowledges a `Request::Feedback` (W3.7): `confidence` is the target
    /// memory's trust prior AFTER the bounded nudge, so the caller can surface
    /// the effect. Additive variant; old clients never see it because they never
    /// send the request.
    FeedbackRecorded {
        confidence: f32,
    },
    /// Reply to `Request::Stats`: the namespace-scoped aggregate plus two
    /// daemon-side health facts the store cannot know — the running embedding
    /// provider's model identity and writer-thread liveness (both
    /// `#[serde(default)]` so the payload stays additive). Counts and ids
    /// only, never memory content. Additive variant; old clients never see it
    /// because they never send the request.
    Stats {
        stats: rb_types::MemoryStats,
        #[serde(default)]
        provider_model: String,
        #[serde(default)]
        writer_alive: bool,
    },
    /// Reply to a dry-run `Request::Forget`: the exact candidate set one
    /// sweep pass would touch, computed by the same query the sweep executes.
    /// Additive variant; old clients never see it because they never send
    /// the request.
    ForgetPlanned {
        plan: rb_types::ForgetPlan,
    },
    /// Reply to an executed `Request::Forget`: what the bounded pass did.
    /// Additive variant; old clients never see it.
    ForgetDone {
        outcome: rb_types::ForgetOutcome,
    },
    /// Reply to `Request::History`: the derived decision-history timeline.
    /// Every payload field is `#[serde(default)]` (the `MemoryStats`
    /// precedent) so the shape stays additive. Additive variant; old clients
    /// never see it because they never send the request.
    History {
        history: rb_types::MemoryHistory,
    },
    /// Reply to a dry-run `Request::Review`: the priority-ordered queue plus
    /// the per-item plan when a policy was named — computed by the same
    /// queue generator the apply pass executes. Every payload field is
    /// `#[serde(default)]` so the shape stays additive. Additive variant;
    /// old clients never see it because they never send the request.
    ReviewPlanned {
        plan: rb_types::ReviewPlan,
    },
    /// Reply to an executed `Request::Review`: what the bounded policy pass
    /// did (the `ForgetDone` shape, including the partial-failure slot).
    /// Additive variant; old clients never see it.
    ReviewDone {
        outcome: rb_types::ReviewOutcome,
    },
    /// Reply to a `Request::Resolve`: what the single resolution did (the
    /// merged-into id, post-nudge confidences, or the snooze expiry).
    /// Additive variant; old clients never see it.
    Resolved {
        resolution: rb_types::ReviewResolution,
    },
    Error {
        kind: String,
        message: String,
    },
    /// A streamed change event (only emitted on a `Subscribe` connection).
    Change(MemoryChanged),
    /// The subscriber fell behind and the broadcast channel dropped `dropped`
    /// events for it. Observability only; the stream continues.
    Lagged {
        dropped: u64,
    },
    /// Acknowledges a `Subscribe`: the daemon has registered the change-stream
    /// receiver and will deliver every event committed from now on. Sent exactly
    /// once, before any `Change`/`Lagged` frame, so the client cannot make (or
    /// unblock a peer that makes) a change that races ahead of an active receiver.
    SubscribeAck,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_types::{
        JobKind, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace, SearchResult,
    };

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rusty-brain".into()),
            "one db, one transaction".into(),
            MemoryType::ArchitectureDecision,
            8,
        )
    }

    #[test]
    fn contract_version_is_two() {
        // Bumped to 2 for the additive `contested` field (P5 Feature C).
        assert_eq!(CONTRACT_VERSION, 2);
    }

    #[test]
    fn handshake_round_trip() {
        let hs = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace: Namespace::Project("rusty-brain".into()),
            identity: None,
        };
        let json = serde_json::to_string(&hs).unwrap();
        let back: Handshake = serde_json::from_str(&json).unwrap();
        assert_eq!(back.contract_version, CONTRACT_VERSION);
        assert_eq!(back.namespace, Namespace::Project("rusty-brain".into()));
        assert!(back.identity.is_none());
    }

    #[test]
    fn handshake_identity_round_trips() {
        let hs = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace: Namespace::Global,
            identity: Some(ClientIdentity {
                user: Some("alice".into()),
                host: Some("devbox".into()),
                agent: Some("claude-code".into()),
                session_id: Some("s-1".into()),
                source: Some("hook".into()),
            }),
        };
        let json = serde_json::to_string(&hs).unwrap();
        let back: Handshake = serde_json::from_str(&json).unwrap();
        assert_eq!(back.identity, hs.identity);
    }

    #[test]
    fn old_client_handshake_without_identity_deserializes() {
        // A pre-W0.5 client sends exactly {contract_version, namespace}; the
        // daemon must accept it with identity == None (no CONTRACT_VERSION bump:
        // the field is additive and serde-default).
        let hs = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace: Namespace::Global,
            identity: None,
        };
        let mut value = serde_json::to_value(&hs).unwrap();
        value.as_object_mut().unwrap().remove("identity");
        let back: Handshake = serde_json::from_value(value).unwrap();
        assert!(back.identity.is_none());
    }

    #[test]
    fn old_daemon_tolerates_identity_field_on_the_wire() {
        // The reverse direction: a new client's handshake (with identity) must
        // still deserialize under a decoder that doesn't know the field — serde
        // ignores unknown fields by default, which this pins for Handshake.
        #[derive(serde::Deserialize)]
        struct OldHandshake {
            contract_version: u32,
            #[allow(dead_code)]
            namespace: Namespace,
        }
        let hs = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace: Namespace::Global,
            identity: Some(ClientIdentity {
                source: Some("cli".into()),
                ..Default::default()
            }),
        };
        let json = serde_json::to_string(&hs).unwrap();
        let back: OldHandshake = serde_json::from_str(&json).unwrap();
        assert_eq!(back.contract_version, CONTRACT_VERSION);
    }

    #[test]
    fn remember_without_confidence_decodes_to_none_and_none_omits_the_key() {
        // Wire compat (fix #4): an old payload with no `confidence` decodes to
        // None (the engine applies the 1.0 baseline), and a None serializes
        // WITHOUT the key — byte-identical to the pre-field frame.
        let explicit = Request::Remember {
            content: "c".into(),
            context: None,
            memory_type: MemoryType::Insight,
            importance: 5,
            keywords: vec![],
            tags: vec![],
            related_files: vec![],
            confidence: Some(0.3),
            supersedes: None,
            anchors: vec![],
        };
        // Some(x) serializes the key and round-trips back to Some(x).
        let json = serde_json::to_value(&explicit).unwrap();
        assert!(
            json.get("confidence").is_some(),
            "explicit confidence must serialize the key: {json}"
        );
        match serde_json::from_value::<Request>(json.clone()).unwrap() {
            Request::Remember { confidence, .. } => {
                assert!(confidence.is_some_and(|c| (c - 0.3).abs() < f32::EPSILON));
            }
            other => panic!("expected Remember, got {other:?}"),
        }

        // Removing the key decodes to None (old client / no explicit prior).
        let mut value = json;
        value.as_object_mut().unwrap().remove("confidence");
        let back: Request = serde_json::from_value(value).unwrap();
        match back {
            Request::Remember { confidence, .. } => {
                assert_eq!(confidence, None);
            }
            other => panic!("expected Remember, got {other:?}"),
        }

        // A None confidence serializes to nothing (no key) — pre-field shape.
        let none = Request::Remember {
            content: "c".into(),
            context: None,
            memory_type: MemoryType::Insight,
            importance: 5,
            keywords: vec![],
            tags: vec![],
            related_files: vec![],
            confidence: None,
            supersedes: None,
            anchors: vec![],
        };
        let json = serde_json::to_value(&none).unwrap();
        assert!(
            json.as_object().unwrap().get("confidence").is_none(),
            "None confidence must not serialize: {json}"
        );
    }

    #[test]
    fn remember_without_supersedes_decodes_to_none_and_none_omits_the_key() {
        // Wire compat (W3.1): an old payload with no `supersedes` decodes to
        // None (a plain store), and a None serializes WITHOUT the key —
        // byte-identical to the pre-field frame, so no CONTRACT_VERSION bump.
        let old = MemoryId::new();
        let explicit = Request::Remember {
            content: "c".into(),
            context: None,
            memory_type: MemoryType::Insight,
            importance: 5,
            keywords: vec![],
            tags: vec![],
            related_files: vec![],
            confidence: None,
            supersedes: Some(old.clone()),
            anchors: vec![],
        };
        // Some(id) serializes the key and round-trips back to the same id.
        let json = serde_json::to_value(&explicit).unwrap();
        assert!(
            json.get("supersedes").is_some(),
            "explicit supersedes must serialize the key: {json}"
        );
        match serde_json::from_value::<Request>(json.clone()).unwrap() {
            Request::Remember { supersedes, .. } => {
                assert_eq!(supersedes, Some(old));
            }
            other => panic!("expected Remember, got {other:?}"),
        }

        // Removing the key decodes to None (old client / a plain store).
        let mut value = json;
        value.as_object_mut().unwrap().remove("supersedes");
        match serde_json::from_value::<Request>(value).unwrap() {
            Request::Remember { supersedes, .. } => assert_eq!(supersedes, None),
            other => panic!("expected Remember, got {other:?}"),
        }

        // A None supersedes serializes to nothing (no key) — pre-field shape.
        let none = Request::Remember {
            content: "c".into(),
            context: None,
            memory_type: MemoryType::Insight,
            importance: 5,
            keywords: vec![],
            tags: vec![],
            related_files: vec![],
            confidence: None,
            supersedes: None,
            anchors: vec![],
        };
        let json = serde_json::to_value(&none).unwrap();
        assert!(
            json.as_object().unwrap().get("supersedes").is_none(),
            "None supersedes must not serialize: {json}"
        );
    }

    #[test]
    fn handshake_ack_round_trip() {
        let ack = HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: false,
            message: Some("version mismatch".into()),
            capabilities: vec![],
        };
        let json = serde_json::to_string(&ack).unwrap();
        let back: HandshakeAck = serde_json::from_str(&json).unwrap();
        assert!(!back.ok);
        assert_eq!(back.message.as_deref(), Some("version mismatch"));
    }

    #[test]
    fn handshake_ack_capabilities_are_additive_in_both_directions() {
        // OLD daemon -> NEW client: an ack without the `capabilities` key
        // decodes to an empty list (the client then treats anchors as
        // unsupported and fails fast locally).
        let old = serde_json::json!({
            "contract_version": CONTRACT_VERSION, "ok": true, "message": null
        });
        let back: HandshakeAck = serde_json::from_value(old).unwrap();
        assert!(back.capabilities.is_empty());

        // An empty capabilities list serializes to NOTHING — byte-identical
        // to the pre-capability ack shape.
        let bare = HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: true,
            message: None,
            capabilities: vec![],
        };
        let json = serde_json::to_value(&bare).unwrap();
        assert!(
            json.as_object().unwrap().get("capabilities").is_none(),
            "empty capabilities must not serialize: {json}"
        );

        // NEW daemon -> OLD client: a decoder that does not know the field
        // must still accept the ack (serde ignores unknown fields).
        #[derive(serde::Deserialize)]
        struct OldAck {
            contract_version: u32,
            ok: bool,
            #[allow(dead_code)]
            message: Option<String>,
        }
        let new = HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: true,
            message: None,
            capabilities: vec![CAP_ANCHORS.to_string()],
        };
        let json = serde_json::to_string(&new).unwrap();
        let back: OldAck = serde_json::from_str(&json).unwrap();
        assert_eq!(back.contract_version, CONTRACT_VERSION);
        assert!(back.ok);
    }

    #[test]
    fn remember_without_anchors_decodes_to_empty_and_empty_omits_the_key() {
        // Wire compat (typed code anchors): an old payload with no `anchors`
        // decodes to an empty list, and an empty list serializes WITHOUT the
        // key — byte-identical to the pre-anchor frame, so no
        // CONTRACT_VERSION bump (the `confidence`/`supersedes` precedent).
        let explicit = Request::Remember {
            content: "c".into(),
            context: None,
            memory_type: MemoryType::Insight,
            importance: 5,
            keywords: vec![],
            tags: vec![],
            related_files: vec![],
            confidence: None,
            supersedes: None,
            anchors: vec![rb_types::MemoryAnchor::parse_file_spec("src/a.rs:2-4").unwrap()],
        };
        let json = serde_json::to_value(&explicit).unwrap();
        assert!(
            json.get("anchors").is_some(),
            "non-empty anchors must serialize the key: {json}"
        );
        match serde_json::from_value::<Request>(json.clone()).unwrap() {
            Request::Remember { anchors, .. } => {
                assert_eq!(anchors.len(), 1);
                assert_eq!(anchors[0].value, "src/a.rs");
                assert_eq!(anchors[0].start_line, Some(2));
            }
            other => panic!("expected Remember, got {other:?}"),
        }

        // Removing the key decodes to an empty list (old client).
        let mut value = json;
        value.as_object_mut().unwrap().remove("anchors");
        match serde_json::from_value::<Request>(value).unwrap() {
            Request::Remember { anchors, .. } => assert!(anchors.is_empty()),
            other => panic!("expected Remember, got {other:?}"),
        }

        // Empty anchors serialize to nothing (no key) — pre-anchor shape.
        let none = Request::Remember {
            content: "c".into(),
            context: None,
            memory_type: MemoryType::Insight,
            importance: 5,
            keywords: vec![],
            tags: vec![],
            related_files: vec![],
            confidence: None,
            supersedes: None,
            anchors: vec![],
        };
        let json = serde_json::to_value(&none).unwrap();
        assert!(
            json.as_object().unwrap().get("anchors").is_none(),
            "empty anchors must not serialize: {json}"
        );
    }

    #[test]
    fn request_uses_anchors_flags_exactly_the_anchor_bearing_frames() {
        let anchored_filter = rb_types::RecallFilter {
            anchors: vec![rb_types::AnchorFilter {
                kind: rb_types::AnchorKind::File,
                value: "src/a.rs".into(),
            }],
            ..Default::default()
        };
        assert!(request_uses_anchors(&Request::Recall {
            query: "q".into(),
            memory_type: None,
            tags: vec![],
            limit: 5,
            filter: anchored_filter.clone(),
        }));
        assert!(request_uses_anchors(&Request::List {
            min_importance: None,
            limit: 5,
            filter: anchored_filter,
        }));
        assert!(request_uses_anchors(&Request::Remember {
            content: "c".into(),
            context: None,
            memory_type: MemoryType::Insight,
            importance: 5,
            keywords: vec![],
            tags: vec![],
            related_files: vec![],
            confidence: None,
            supersedes: None,
            anchors: vec![rb_types::MemoryAnchor::parse_file_spec("a.rs").unwrap()],
        }));
        // Anchor-free frames are never flagged.
        assert!(!request_uses_anchors(&Request::Recall {
            query: "q".into(),
            memory_type: None,
            tags: vec![],
            limit: 5,
            filter: rb_types::RecallFilter::default(),
        }));
        assert!(!request_uses_anchors(&Request::Ping));
    }

    fn all_requests() -> Vec<Request> {
        let id = MemoryId::new();
        vec![
            Request::Remember {
                content: "c".into(),
                context: Some("ctx".into()),
                memory_type: MemoryType::Insight,
                importance: 7,
                keywords: vec!["k".into()],
                tags: vec!["t".into()],
                related_files: vec!["src/lib.rs".into()],
                confidence: Some(0.7),
                supersedes: Some(id.clone()),
                anchors: vec![rb_types::MemoryAnchor::parse_file_spec("src/lib.rs:3-9").unwrap()],
            },
            Request::Recall {
                query: "q".into(),
                memory_type: Some(MemoryType::BugFix),
                tags: vec!["sqlite".into()],
                limit: 10,
                filter: rb_types::RecallFilter::default(),
            },
            Request::Recall {
                query: "q".into(),
                memory_type: None,
                tags: vec![],
                limit: 10,
                filter: rb_types::RecallFilter {
                    min_confidence: Some(0.4),
                    sources: vec!["hook".into()],
                    contested: Some(false),
                    state: rb_types::MemoryState::All,
                    ..Default::default()
                },
            },
            Request::Get { id: id.clone() },
            Request::List {
                min_importance: Some(5),
                limit: 20,
                filter: rb_types::RecallFilter::default(),
            },
            Request::List {
                min_importance: None,
                limit: 20,
                filter: rb_types::RecallFilter {
                    since: Some(chrono::Utc::now()),
                    state: rb_types::MemoryState::Archived,
                    ..Default::default()
                },
            },
            Request::Graph {
                id: id.clone(),
                depth: 2,
            },
            Request::Update {
                id: id.clone(),
                updates: MemoryUpdates {
                    importance: Some(9),
                    ..Default::default()
                },
            },
            Request::Delete { id },
            Request::Context,
            Request::Ping,
            Request::Subscribe { since: None },
            Request::Subscribe { since: Some(42) },
            Request::RunJob {
                job: JobKind::LinkDecay,
            },
            Request::Reembed { limit: Some(100) },
            Request::Reembed { limit: None },
            Request::NamespaceRename {
                old: Namespace::Project("scratch-dir".into()),
                new: Namespace::Project("rusty-brain".into()),
                merge: false,
            },
            Request::NamespaceRename {
                old: Namespace::Project("scratch-dir".into()),
                new: Namespace::Project("rusty-brain".into()),
                merge: true,
            },
            Request::Scrub,
            Request::Feedback {
                id: MemoryId::new(),
                kind: rb_types::FeedbackKind::Wrong,
            },
            Request::Stats { window_days: None },
            Request::Stats {
                window_days: Some(14),
            },
            Request::History {
                id: MemoryId::new(),
                depth: None,
            },
            Request::History {
                id: MemoryId::new(),
                depth: Some(3),
            },
        ]
    }

    #[test]
    fn every_request_variant_round_trips() {
        for req in all_requests() {
            let json = serde_json::to_string(&req).unwrap();
            let back: Request = serde_json::from_str(&json).unwrap();
            // Compare via JSON since Request is not PartialEq.
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
        }
    }

    #[test]
    fn request_uses_op_tag() {
        let json = serde_json::to_string(&Request::Ping).unwrap();
        assert_eq!(json, r#"{"op":"Ping"}"#);
        let json = serde_json::to_string(&Request::Context).unwrap();
        assert_eq!(json, r#"{"op":"Context"}"#);
    }

    fn all_responses() -> Vec<Response> {
        vec![
            Response::Remembered {
                id: MemoryId::new(),
            },
            Response::Recalled {
                results: vec![SearchResult {
                    memory: note(),
                    score: 0.9,
                    channels: rb_types::ChannelHits::default(),
                }],
                degraded: false,
            },
            Response::Recalled {
                results: Vec::new(),
                degraded: true,
            },
            Response::Got {
                memory: Some(note()),
            },
            Response::Got { memory: None },
            Response::Listed {
                memories: vec![note()],
            },
            Response::GraphResult {
                memories: vec![note()],
            },
            Response::Updated,
            Response::Deleted,
            Response::ContextResult {
                recent: vec![note()],
                important: vec![note()],
                total: 2,
            },
            Response::Pong {
                contract_version: CONTRACT_VERSION,
                recall_channels: None,
            },
            Response::Pong {
                contract_version: CONTRACT_VERSION,
                recall_channels: Some(RecallChannelTotals {
                    recalls: 4,
                    fts_hits: 9,
                    vector_hits: 11,
                    graph_hits: 2,
                }),
            },
            Response::Error {
                kind: "not_found".into(),
                message: "no such memory".into(),
            },
            Response::Change(rb_types::MemoryChanged {
                id: MemoryId::new(),
                namespace: Namespace::Project("rusty-brain".into()),
                kind: rb_types::ChangeKind::Created,
                seq: Some(12),
            }),
            Response::Lagged { dropped: 3 },
            Response::SubscribeAck,
            Response::JobRan {
                scanned: 10,
                changed: 3,
                skipped: 7,
            },
            Response::NamespaceRenamed {
                moved: 12,
                vectors: 9,
            },
            Response::Linked,
            Response::FeedbackRecorded { confidence: 0.4 },
            Response::Scrubbed {
                scanned: 200,
                redacted: 3,
                reembed_pending: 2,
                wal_checkpoint: Some(ScrubCheckpoint {
                    busy: false,
                    log_frames: 0,
                    checkpointed_frames: 0,
                }),
                wal_checkpoint_error: None,
            },
            Response::Stats {
                stats: rb_types::MemoryStats {
                    namespace: "global".to_string(),
                    window_days: 30,
                    live: 2,
                    top_recalled: vec![rb_types::TopRecalled {
                        id: MemoryId::new(),
                        access_count: 4,
                    }],
                    created_per_day: vec![rb_types::GrowthBucket {
                        day: "2026-07-10".to_string(),
                        count: 1,
                    }],
                    ..Default::default()
                },
                provider_model: "deterministic".to_string(),
                writer_alive: true,
            },
            Response::History {
                history: rb_types::MemoryHistory {
                    namespace: "global".to_string(),
                    depth: 100,
                    chain: vec![rb_types::HistoryEntry {
                        id: MemoryId::new(),
                        summary: "we use kafka".to_string(),
                        importance: 7,
                        confidence: 0.9,
                        created_at: chrono::Utc::now(),
                        archived: false,
                        contested: true,
                        current: true,
                        is_target: true,
                        superseded_by: None,
                        origin_user: Some("alice".to_string()),
                        origin_host: None,
                        origin_agent: None,
                        origin_source: Some("cli".to_string()),
                    }],
                    edges: vec![rb_types::HistoryEdge {
                        link_type: rb_types::LinkType::Contradicts,
                        local: MemoryId::new(),
                        other: MemoryId::new(),
                        outbound: false,
                        reason: "disagrees".to_string(),
                        other_summary: "kafka too slow".to_string(),
                        other_confidence: 0.5,
                        other_contested: true,
                        created_at: chrono::Utc::now(),
                    }],
                    truncated: false,
                },
            },
        ]
    }

    #[test]
    fn every_response_variant_round_trips() {
        for resp in all_responses() {
            let json = serde_json::to_string(&resp).unwrap();
            let back: Response = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
        }
    }

    #[test]
    fn response_uses_result_tag() {
        let json = serde_json::to_string(&Response::Updated).unwrap();
        assert_eq!(json, r#"{"result":"Updated"}"#);
        let json = serde_json::to_string(&Response::Pong {
            contract_version: 1,
            recall_channels: None,
        })
        .unwrap();
        // A `None` recall_channels keeps the pre-W1.0 byte shape (additive,
        // skip-serializing field), so old clients see an unchanged frame.
        assert_eq!(json, r#"{"result":"Pong","contract_version":1}"#);
    }

    #[test]
    fn recalled_without_degraded_field_decodes_to_false() {
        // Wire compat: a pre-W1.6 Recalled frame (no `degraded` key) must
        // decode, defaulting the flag off.
        let back: Response = serde_json::from_str(r#"{"result":"Recalled","results":[]}"#).unwrap();
        match back {
            Response::Recalled { results, degraded } => {
                assert!(results.is_empty());
                assert!(!degraded, "absent degraded key must default to false");
            }
            other => panic!("expected Recalled, got {other:?}"),
        }
    }

    #[test]
    fn non_degraded_recalled_keeps_the_pre_w16_byte_shape() {
        // `degraded: false` is skip-serialized, so old clients see an
        // unchanged frame; `degraded: true` rides along explicitly.
        let json = serde_json::to_string(&Response::Recalled {
            results: Vec::new(),
            degraded: false,
        })
        .unwrap();
        assert_eq!(json, r#"{"result":"Recalled","results":[]}"#);

        let json = serde_json::to_string(&Response::Recalled {
            results: Vec::new(),
            degraded: true,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"result":"Recalled","results":[],"degraded":true}"#
        );
    }

    #[test]
    fn pong_without_recall_channels_field_decodes_to_none() {
        // Wire compat: a pre-W1.0 Pong (no recall_channels key) must decode.
        let back: Response =
            serde_json::from_str(r#"{"result":"Pong","contract_version":2}"#).unwrap();
        match back {
            Response::Pong {
                contract_version,
                recall_channels,
            } => {
                assert_eq!(contract_version, 2);
                assert!(recall_channels.is_none());
            }
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    #[test]
    fn pong_recall_channel_totals_round_trip() {
        let totals = RecallChannelTotals {
            recalls: 4,
            fts_hits: 9,
            vector_hits: 11,
            graph_hits: 2,
        };
        let json = serde_json::to_string(&Response::Pong {
            contract_version: CONTRACT_VERSION,
            recall_channels: Some(totals),
        })
        .unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::Pong {
                recall_channels, ..
            } => assert_eq!(recall_channels, Some(totals)),
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    #[test]
    fn recall_and_list_filter_round_trips() {
        use rb_types::{MemoryState, RecallFilter};
        let filter = RecallFilter {
            min_confidence: Some(0.4),
            sources: vec!["hook".to_string()],
            contested: Some(true),
            state: MemoryState::All,
            ..Default::default()
        };
        let recall = Request::Recall {
            query: "q".into(),
            memory_type: None,
            tags: vec![],
            limit: 10,
            filter: filter.clone(),
        };
        let json = serde_json::to_string(&recall).unwrap();
        match serde_json::from_str::<Request>(&json).unwrap() {
            Request::Recall { filter: back, .. } => assert_eq!(back, filter),
            other => panic!("expected Recall, got {other:?}"),
        }

        let list = Request::List {
            min_importance: None,
            limit: 20,
            filter: filter.clone(),
        };
        let json = serde_json::to_string(&list).unwrap();
        match serde_json::from_str::<Request>(&json).unwrap() {
            Request::List { filter: back, .. } => assert_eq!(back, filter),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn recall_and_list_without_filter_key_decode_to_default() {
        // Wire compat (old client -> new daemon): a pre-filter frame carries no
        // `filter` key and must decode to the unconstrained default — the
        // `contested` additive-field precedent, NO CONTRACT_VERSION bump.
        use rb_types::RecallFilter;
        let recall: Request = serde_json::from_str(
            r#"{"op":"Recall","query":"q","memory_type":null,"tags":[],"limit":10}"#,
        )
        .unwrap();
        match recall {
            Request::Recall { filter, .. } => assert!(filter.is_empty()),
            other => panic!("expected Recall, got {other:?}"),
        }
        let list: Request =
            serde_json::from_str(r#"{"op":"List","min_importance":5,"limit":20}"#).unwrap();
        match list {
            Request::List {
                min_importance,
                filter,
                ..
            } => {
                assert_eq!(min_importance, Some(5));
                assert_eq!(filter, RecallFilter::default());
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn unfiltered_recall_and_list_keep_the_pre_filter_byte_shape() {
        // A default filter must serialize to NOTHING (skip_serializing_if), so
        // requests that use no new filters stay byte-identical to the frames an
        // old daemon already accepts (new client -> old daemon).
        let json = serde_json::to_string(&Request::Recall {
            query: "q".into(),
            memory_type: None,
            tags: vec![],
            limit: 10,
            filter: rb_types::RecallFilter::default(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"op":"Recall","query":"q","memory_type":null,"tags":[],"limit":10}"#
        );

        let json = serde_json::to_string(&Request::List {
            min_importance: Some(5),
            limit: 20,
            filter: rb_types::RecallFilter::default(),
        })
        .unwrap();
        assert_eq!(json, r#"{"op":"List","min_importance":5,"limit":20}"#);
    }

    #[test]
    fn old_daemon_shape_tolerates_the_filter_field_on_the_wire() {
        // The reverse direction (new client -> old daemon) when a filter IS
        // set: a decoder that does not know `filter` must still accept the
        // frame — serde ignores unknown fields by default, pinned here for the
        // Recall/List variants (the `Handshake.identity` precedent).
        #[derive(serde::Deserialize, Debug)]
        #[serde(tag = "op")]
        enum OldRequest {
            Recall {
                query: String,
                memory_type: Option<MemoryType>,
                tags: Vec<String>,
                limit: usize,
            },
            List {
                min_importance: Option<u8>,
                limit: usize,
            },
        }

        let json = serde_json::to_string(&Request::Recall {
            query: "q".into(),
            memory_type: Some(MemoryType::BugFix),
            tags: vec!["t".into()],
            limit: 10,
            filter: rb_types::RecallFilter {
                min_confidence: Some(0.4),
                ..Default::default()
            },
        })
        .unwrap();
        match serde_json::from_str::<OldRequest>(&json).unwrap() {
            OldRequest::Recall {
                query,
                memory_type,
                tags,
                limit,
            } => {
                // The legacy slots still carry the old-daemon-honorable subset.
                assert_eq!(query, "q");
                assert_eq!(memory_type, Some(MemoryType::BugFix));
                assert_eq!(tags, vec!["t".to_string()]);
                assert_eq!(limit, 10);
            }
            other => panic!("expected Recall, got {other:?}"),
        }

        let json = serde_json::to_string(&Request::List {
            min_importance: Some(5),
            limit: 20,
            filter: rb_types::RecallFilter {
                sources: vec!["hook".into()],
                ..Default::default()
            },
        })
        .unwrap();
        match serde_json::from_str::<OldRequest>(&json).unwrap() {
            OldRequest::List {
                min_importance,
                limit,
            } => {
                assert_eq!(min_importance, Some(5));
                assert_eq!(limit, 20);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn subscribe_request_round_trips_and_uses_op_tag() {
        // W2.7 wire compat: a cursorless Subscribe stays byte-identical to the
        // pre-W2.7 unit-variant frame, and the old frame decodes to
        // `since: None`.
        let json = serde_json::to_string(&Request::Subscribe { since: None }).unwrap();
        assert_eq!(json, r#"{"op":"Subscribe"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);

        let with_cursor = serde_json::to_string(&Request::Subscribe { since: Some(9) }).unwrap();
        assert_eq!(with_cursor, r#"{"op":"Subscribe","since":9}"#);
        let back: Request = serde_json::from_str(&with_cursor).unwrap();
        assert!(matches!(back, Request::Subscribe { since: Some(9) }));
    }

    #[test]
    fn change_and_lagged_responses_round_trip() {
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        let change = Response::Change(MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Project("rusty-brain".into()),
            kind: ChangeKind::Created,
            seq: Some(3),
        });
        let json = serde_json::to_string(&change).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        // The streamed Change frame carries `result: "Change"`.
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["result"], "Change");

        let lagged = Response::Lagged { dropped: 7 };
        let json = serde_json::to_string(&lagged).unwrap();
        assert_eq!(json, r#"{"result":"Lagged","dropped":7}"#);
        let back: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn stats_request_uses_op_tag_and_window_days_is_additive() {
        // A window-less Stats request stays byte-identical to a bare
        // unit-variant frame (the `Subscribe.since` precedent), and the bare
        // frame decodes to `None` — no CONTRACT_VERSION bump.
        let json = serde_json::to_string(&Request::Stats { window_days: None }).unwrap();
        assert_eq!(json, r#"{"op":"Stats"}"#);
        let back: Request = serde_json::from_str(r#"{"op":"Stats"}"#).unwrap();
        assert!(matches!(back, Request::Stats { window_days: None }));

        let json = serde_json::to_string(&Request::Stats {
            window_days: Some(7),
        })
        .unwrap();
        assert_eq!(json, r#"{"op":"Stats","window_days":7}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            Request::Stats {
                window_days: Some(7)
            }
        ));
    }

    #[test]
    fn forget_request_round_trips_and_absent_fields_are_safe() {
        // Retention PRD RET-2, additive op per the Stats precedent. Safety of
        // the serde defaults IS the wire contract here: a frame that omits
        // `mode` must decode to Apply (never Hard) and one that omits
        // `dry_run` must decode to a PREVIEW (never an execute).
        let policy = rb_types::RetentionPolicy {
            enabled: true,
            max_age_days: Some(365),
            ..rb_types::RetentionPolicy::default()
        };
        let req = Request::Forget {
            policy: policy.clone(),
            mode: rb_types::ForgetMode::Hard,
            dry_run: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["op"], "Forget");
        let back: Request = serde_json::from_str(&json).unwrap();
        match back {
            Request::Forget {
                policy: p,
                mode,
                dry_run,
            } => {
                assert_eq!(p, policy);
                assert_eq!(mode, rb_types::ForgetMode::Hard);
                assert!(!dry_run);
            }
            other => panic!("expected Forget, got {other:?}"),
        }

        let bare: Request =
            serde_json::from_str(r#"{"op":"Forget","policy":{"enabled":true}}"#).unwrap();
        match bare {
            Request::Forget { mode, dry_run, .. } => {
                assert_eq!(
                    mode,
                    rb_types::ForgetMode::Apply,
                    "absent mode must never escalate to Hard"
                );
                assert!(dry_run, "absent dry_run must preview, never execute");
            }
            other => panic!("expected Forget, got {other:?}"),
        }
    }

    #[test]
    fn forget_responses_round_trip_with_result_tag() {
        let planned = Response::ForgetPlanned {
            plan: rb_types::ForgetPlan {
                mode: rb_types::ForgetMode::Apply,
                candidates: vec![],
                total_eligible: 3,
            },
        };
        let json = serde_json::to_string(&planned).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["result"], "ForgetPlanned");
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::ForgetPlanned { plan } => assert_eq!(plan.total_eligible, 3),
            other => panic!("expected ForgetPlanned, got {other:?}"),
        }

        let done = Response::ForgetDone {
            outcome: rb_types::ForgetOutcome {
                mode: rb_types::ForgetMode::Hard,
                archived: 0,
                purged: 2,
                total_eligible: 2,
                remaining: 0,
                failure: None,
            },
        };
        let json = serde_json::to_string(&done).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["result"], "ForgetDone");
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::ForgetDone { outcome } => {
                assert_eq!(outcome.purged, 2);
                assert_eq!(outcome.mode, rb_types::ForgetMode::Hard);
            }
            other => panic!("expected ForgetDone, got {other:?}"),
        }
    }

    #[test]
    fn stats_response_round_trips_with_result_tag() {
        let resp = Response::Stats {
            stats: rb_types::MemoryStats {
                namespace: "project:rusty-brain".to_string(),
                window_days: 30,
                live: 10,
                feedback: rb_types::FeedbackTotals {
                    helpful: 3,
                    wrong: 1,
                    stale: 0,
                },
                ..Default::default()
            },
            provider_model: "deterministic".to_string(),
            writer_alive: true,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["result"], "Stats");
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::Stats {
                stats,
                provider_model,
                writer_alive,
            } => {
                assert_eq!(stats.live, 10);
                assert_eq!(stats.feedback.helpful, 3);
                assert_eq!(provider_model, "deterministic");
                assert!(writer_alive);
            }
            other => panic!("expected Stats, got {other:?}"),
        }
    }

    #[test]
    fn stats_response_daemon_fields_are_additive() {
        // A frame carrying only the stats payload (a peer predating the
        // daemon-side fields) must decode with zero-valued defaults.
        let back: Response = serde_json::from_str(r#"{"result":"Stats","stats":{}}"#).unwrap();
        match back {
            Response::Stats {
                stats,
                provider_model,
                writer_alive,
            } => {
                assert_eq!(stats, rb_types::MemoryStats::default());
                assert_eq!(provider_model, "");
                assert!(!writer_alive);
            }
            other => panic!("expected Stats, got {other:?}"),
        }
    }

    #[test]
    fn history_request_uses_op_tag_and_depth_is_additive() {
        // A depth-less History request carries only the op tag and the id
        // (the `Stats.window_days` precedent), and a frame without the depth
        // key decodes to `None` — no CONTRACT_VERSION bump.
        let id = MemoryId::new();
        let json = serde_json::to_string(&Request::History {
            id: id.clone(),
            depth: None,
        })
        .unwrap();
        assert_eq!(json, format!(r#"{{"op":"History","id":"{id}"}}"#));
        let back: Request = serde_json::from_str(&json).unwrap();
        match back {
            Request::History { id: got, depth } => {
                assert_eq!(got, id);
                assert_eq!(depth, None);
            }
            other => panic!("expected History, got {other:?}"),
        }

        let json = serde_json::to_string(&Request::History {
            id: id.clone(),
            depth: Some(3),
        })
        .unwrap();
        assert_eq!(json, format!(r#"{{"op":"History","id":"{id}","depth":3}}"#));
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::History { depth: Some(3), .. }));
    }

    #[test]
    fn scrub_checkpoint_status_is_additive_and_old_payload_defaults_to_none() {
        let old = r#"{"result":"Scrubbed","scanned":10,"redacted":2,"reembed_pending":1}"#;
        let decoded: Response = serde_json::from_str(old).unwrap();
        assert!(matches!(
            decoded,
            Response::Scrubbed {
                wal_checkpoint: None,
                wal_checkpoint_error: None,
                ..
            }
        ));

        let response = Response::Scrubbed {
            scanned: 10,
            redacted: 2,
            reembed_pending: 1,
            wal_checkpoint: Some(ScrubCheckpoint {
                busy: true,
                log_frames: 7,
                checkpointed_frames: 3,
            }),
            wal_checkpoint_error: None,
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["wal_checkpoint"]["busy"], true);
        assert_eq!(value["wal_checkpoint"]["log_frames"], 7);
        assert_eq!(value["wal_checkpoint"]["checkpointed_frames"], 3);
    }

    #[test]
    fn history_response_round_trips_with_result_tag() {
        let id = MemoryId::new();
        let resp = Response::History {
            history: rb_types::MemoryHistory {
                namespace: "project:rusty-brain".to_string(),
                depth: 100,
                chain: vec![rb_types::HistoryEntry {
                    id: id.clone(),
                    summary: "we use kafka".to_string(),
                    importance: 7,
                    confidence: 0.9,
                    created_at: chrono::Utc::now(),
                    archived: false,
                    contested: true,
                    current: true,
                    is_target: true,
                    superseded_by: None,
                    origin_user: None,
                    origin_host: None,
                    origin_agent: None,
                    origin_source: None,
                }],
                edges: Vec::new(),
                truncated: true,
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["result"], "History");
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::History { history } => {
                assert_eq!(history.chain.len(), 1);
                assert_eq!(history.chain[0].id, id);
                assert!(history.chain[0].current);
                assert!(history.truncated);
            }
            other => panic!("expected History, got {other:?}"),
        }
    }

    #[test]
    fn history_response_payload_fields_are_additive() {
        // A frame carrying an empty history object (a peer predating — or
        // postdating — any field) must decode to the zero-valued default:
        // old-daemon/new-client tolerance without a CONTRACT_VERSION bump.
        let back: Response = serde_json::from_str(r#"{"result":"History","history":{}}"#).unwrap();
        match back {
            Response::History { history } => {
                assert_eq!(history, rb_types::MemoryHistory::default());
                assert!(history.chain.is_empty());
            }
            other => panic!("expected History, got {other:?}"),
        }
    }

    #[test]
    fn run_job_uses_op_tag_with_snake_case_job() {
        let json = serde_json::to_string(&Request::RunJob {
            job: JobKind::LinkDecay,
        })
        .unwrap();
        assert_eq!(json, r#"{"op":"RunJob","job":"link_decay"}"#);
    }

    #[test]
    fn reembed_uses_op_tag_with_optional_limit() {
        let json = serde_json::to_string(&Request::Reembed { limit: Some(50) }).unwrap();
        assert_eq!(json, r#"{"op":"Reembed","limit":50}"#);
        let json = serde_json::to_string(&Request::Reembed { limit: None }).unwrap();
        assert_eq!(json, r#"{"op":"Reembed","limit":null}"#);
    }

    #[test]
    fn namespace_rename_uses_op_tag_and_merge_defaults_off() {
        // Wire shape is pinned: internally tagged on `op`, Namespace enum JSON.
        let req = Request::NamespaceRename {
            old: Namespace::Project("scratch".into()),
            new: Namespace::Project("rusty-brain".into()),
            merge: false,
        };
        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["op"], "NamespaceRename");
        assert_eq!(value["merge"], false);

        // A payload WITHOUT the merge key must decode to merge=false (the
        // serde-default additive pattern, mirroring Remember.confidence).
        let mut stripped = value;
        stripped.as_object_mut().unwrap().remove("merge");
        let back: Request = serde_json::from_value(stripped).unwrap();
        match back {
            Request::NamespaceRename { merge, old, new } => {
                assert!(!merge, "absent merge key must default to false");
                assert_eq!(old, Namespace::Project("scratch".into()));
                assert_eq!(new, Namespace::Project("rusty-brain".into()));
            }
            other => panic!("expected NamespaceRename, got {other:?}"),
        }
    }

    #[test]
    fn namespace_renamed_uses_result_tag_with_counts() {
        let json = serde_json::to_string(&Response::NamespaceRenamed {
            moved: 5,
            vectors: 3,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"result":"NamespaceRenamed","moved":5,"vectors":3}"#
        );
    }

    #[test]
    fn job_ran_uses_result_tag() {
        let json = serde_json::to_string(&Response::JobRan {
            scanned: 1,
            changed: 0,
            skipped: 1,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"result":"JobRan","scanned":1,"changed":0,"skipped":1}"#
        );
    }

    #[test]
    fn review_request_defaults_to_dry_run_and_minimal_frame_is_bare() {
        // SAFETY DEFAULT (the Forget precedent): an absent `dry_run` on the
        // wire must PREVIEW, never execute a policy.
        let back: Request = serde_json::from_str(r#"{"op":"Review"}"#).unwrap();
        match back {
            Request::Review {
                policy,
                dry_run,
                since,
                limit,
                threshold,
            } => {
                assert!(dry_run, "absent dry_run must decode to a preview");
                assert!(policy.is_none());
                assert!(since.is_none() && limit.is_none() && threshold.is_none());
            }
            other => panic!("expected Review, got {other:?}"),
        }
        // The default-shaped request keeps every optional key off the frame.
        let json = serde_json::to_string(&Request::Review {
            policy: None,
            dry_run: true,
            since: None,
            limit: None,
            threshold: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"op":"Review","dry_run":true}"#);
    }

    #[test]
    fn review_request_round_trips_with_every_field() {
        let req = Request::Review {
            policy: Some(rb_types::ReviewPolicy::AutoMergeDups),
            dry_run: false,
            since: Some(42),
            limit: Some(10),
            threshold: Some(0.97),
        };
        let json = serde_json::to_string(&req).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["op"], "Review");
        assert_eq!(value["policy"], "auto_merge_dups");
        let back: Request = serde_json::from_str(&json).unwrap();
        match back {
            Request::Review {
                policy,
                dry_run,
                since,
                limit,
                threshold,
            } => {
                assert_eq!(policy, Some(rb_types::ReviewPolicy::AutoMergeDups));
                assert!(!dry_run);
                assert_eq!(since, Some(42));
                assert_eq!(limit, Some(10));
                assert_eq!(threshold, Some(0.97));
            }
            other => panic!("expected Review, got {other:?}"),
        }
    }

    #[test]
    fn resolve_request_round_trips_reason_ids_and_action() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let req = Request::Resolve {
            reason: rb_types::ReviewReason::NearDuplicate,
            ids: vec![a.clone(), b.clone()],
            action: rb_types::ReviewAction::Archive { id: b.clone() },
            threshold: Some(0.9),
        };
        let json = serde_json::to_string(&req).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["op"], "Resolve");
        assert_eq!(value["action"]["action"], "archive");
        let back: Request = serde_json::from_str(&json).unwrap();
        match back {
            Request::Resolve {
                reason,
                ids,
                action,
                threshold,
            } => {
                assert_eq!(reason, rb_types::ReviewReason::NearDuplicate);
                assert_eq!(ids, vec![a, b.clone()]);
                assert_eq!(action, rb_types::ReviewAction::Archive { id: b });
                assert_eq!(threshold, Some(0.9));
            }
            other => panic!("expected Resolve, got {other:?}"),
        }
        // Additive: a frame WITHOUT the threshold key decodes to None (the
        // server then revalidates a merge at the conservative default).
        let mut value = value;
        value.as_object_mut().unwrap().remove("threshold");
        let back: Request = serde_json::from_value(value).unwrap();
        match back {
            Request::Resolve { threshold, .. } => assert_eq!(threshold, None),
            other => panic!("expected Resolve, got {other:?}"),
        }
    }

    #[test]
    fn review_responses_round_trip_and_payloads_are_additive() {
        let resp = Response::ReviewPlanned {
            plan: rb_types::ReviewPlan {
                totals: rb_types::ReviewTotals {
                    contradictions: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["result"], "ReviewPlanned");
        match serde_json::from_str::<Response>(&json).unwrap() {
            Response::ReviewPlanned { plan } => assert_eq!(plan.totals.contradictions, 2),
            other => panic!("expected ReviewPlanned, got {other:?}"),
        }

        // Additive payloads: empty-object payloads decode to defaults (the
        // Stats/History precedent).
        match serde_json::from_str::<Response>(r#"{"result":"ReviewPlanned","plan":{}}"#).unwrap() {
            Response::ReviewPlanned { plan } => assert_eq!(plan, rb_types::ReviewPlan::default()),
            other => panic!("expected ReviewPlanned, got {other:?}"),
        }
        match serde_json::from_str::<Response>(r#"{"result":"ReviewDone","outcome":{}}"#).unwrap() {
            Response::ReviewDone { outcome } => {
                assert_eq!(outcome, rb_types::ReviewOutcome::default());
            }
            other => panic!("expected ReviewDone, got {other:?}"),
        }
        match serde_json::from_str::<Response>(r#"{"result":"Resolved","resolution":{}}"#).unwrap()
        {
            Response::Resolved { resolution } => {
                assert_eq!(resolution, rb_types::ReviewResolution::default());
            }
            other => panic!("expected Resolved, got {other:?}"),
        }

        let done = Response::ReviewDone {
            outcome: rb_types::ReviewOutcome {
                policy: Some(rb_types::ReviewPolicy::DemoteLowConfidence),
                demoted: 3,
                failure: Some("injected".to_string()),
                ..Default::default()
            },
        };
        let back: Response = serde_json::from_str(&serde_json::to_string(&done).unwrap()).unwrap();
        match back {
            Response::ReviewDone { outcome } => {
                assert_eq!(outcome.demoted, 3);
                assert_eq!(outcome.failure.as_deref(), Some("injected"));
            }
            other => panic!("expected ReviewDone, got {other:?}"),
        }
    }
}
