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
}
