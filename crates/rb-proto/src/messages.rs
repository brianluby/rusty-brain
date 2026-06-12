use rb_types::{
    JobKind, MemoryChanged, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace,
    SearchResult,
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

/// Daemon reply to a `Handshake`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HandshakeAck {
    pub contract_version: u32,
    pub ok: bool,
    pub message: Option<String>,
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
        /// Trust prior in `0.0..=1.0` (W0.5 / F39 producer down-payment).
        /// `#[serde(default)]` to 1.0 so old clients keep today's behavior;
        /// hook captures send 0.7. Range-validated by the engine.
        #[serde(default = "default_confidence")]
        confidence: f32,
    },
    Recall {
        query: String,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
        limit: usize,
    },
    Get {
        id: MemoryId,
    },
    List {
        min_importance: Option<u8>,
        limit: usize,
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
    Subscribe,
}

/// Serde default for `Request::Remember::confidence`: full trust, the
/// pre-W0.5 hardwired value.
fn default_confidence() -> f32 {
    1.0
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
    },
    JobRan {
        scanned: u64,
        changed: u64,
        skipped: u64,
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
    fn remember_without_confidence_defaults_to_full_trust() {
        // Pre-W0.5 Remember payloads carry no confidence: serde must default it
        // to 1.0 (today's hardwired value), keeping old clients byte-compatible.
        let req = Request::Remember {
            content: "c".into(),
            context: None,
            memory_type: MemoryType::Insight,
            importance: 5,
            keywords: vec![],
            tags: vec![],
            related_files: vec![],
            confidence: 0.3,
        };
        let mut value = serde_json::to_value(&req).unwrap();
        value.as_object_mut().unwrap().remove("confidence");
        let back: Request = serde_json::from_value(value).unwrap();
        match back {
            Request::Remember { confidence, .. } => {
                assert!((confidence - 1.0).abs() < f32::EPSILON);
            }
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn handshake_ack_round_trip() {
        let ack = HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: false,
            message: Some("version mismatch".into()),
        };
        let json = serde_json::to_string(&ack).unwrap();
        let back: HandshakeAck = serde_json::from_str(&json).unwrap();
        assert!(!back.ok);
        assert_eq!(back.message.as_deref(), Some("version mismatch"));
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
                confidence: 0.7,
            },
            Request::Recall {
                query: "q".into(),
                memory_type: Some(MemoryType::BugFix),
                tags: vec!["sqlite".into()],
                limit: 10,
            },
            Request::Get { id: id.clone() },
            Request::List {
                min_importance: Some(5),
                limit: 20,
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
            Request::Subscribe,
            Request::RunJob {
                job: JobKind::LinkDecay,
            },
            Request::Reembed { limit: Some(100) },
            Request::Reembed { limit: None },
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
                }],
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
            },
            Response::Error {
                kind: "not_found".into(),
                message: "no such memory".into(),
            },
            Response::Change(rb_types::MemoryChanged {
                id: MemoryId::new(),
                namespace: Namespace::Project("rusty-brain".into()),
                kind: rb_types::ChangeKind::Created,
            }),
            Response::Lagged { dropped: 3 },
            Response::SubscribeAck,
            Response::JobRan {
                scanned: 10,
                changed: 3,
                skipped: 7,
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
        })
        .unwrap();
        assert_eq!(json, r#"{"result":"Pong","contract_version":1}"#);
    }

    #[test]
    fn subscribe_request_round_trips_and_uses_op_tag() {
        let json = serde_json::to_string(&Request::Subscribe).unwrap();
        assert_eq!(json, r#"{"op":"Subscribe"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn change_and_lagged_responses_round_trip() {
        use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
        let change = Response::Change(MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Project("rusty-brain".into()),
            kind: ChangeKind::Created,
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
