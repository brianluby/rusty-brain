use rb_types::{
    JobKind, MemoryChanged, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace,
    SearchResult,
};
use serde::{Deserialize, Serialize};

/// Wire contract version carried in the handshake. Clients and the daemon must
/// agree on this exact value; mismatch is rejected at connect time.
pub const CONTRACT_VERSION: u32 = 1;

/// First frame the client sends after connecting.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Handshake {
    pub contract_version: u32,
    pub namespace: Namespace,
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
    Ping,
    /// Open a live change-notification stream. The daemon stops the
    /// request/response cadence for this connection and streams `Response::Change`
    /// (and `Response::Lagged` on broadcast overflow) until the client disconnects.
    /// The stream is scoped to the connection's handshake namespace, filtered
    /// server-side.
    Subscribe,
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
    fn contract_version_is_one() {
        assert_eq!(CONTRACT_VERSION, 1);
    }

    #[test]
    fn handshake_round_trip() {
        let hs = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace: Namespace::Project("rusty-brain".into()),
        };
        let json = serde_json::to_string(&hs).unwrap();
        let back: Handshake = serde_json::from_str(&json).unwrap();
        assert_eq!(back.contract_version, CONTRACT_VERSION);
        assert_eq!(back.namespace, Namespace::Project("rusty-brain".into()));
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
