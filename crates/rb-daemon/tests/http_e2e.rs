//! End-to-end tests for the opt-in loopback HTTP listener (HTTP PRD
//! HTTP-1/HTTP-2): op mirroring against the UDS path, and the fail-closed
//! security matrix (loopback-only bind, admin gating, Host/Origin checks,
//! body/timeout/connection bounds, zero footprint when disabled, graceful
//! shutdown).
//!
//! Requests are built and parsed RAW over `TcpStream` (no HTTP client
//! library) so hostile shapes — foreign Host, oversized bodies, stalled
//! connections — are exactly what goes on the wire.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use rb_daemon::{Daemon, DaemonConfig, HttpListenerConfig, SharedEmbedder};
use rb_embed::DeterministicProvider;
use rb_proto::{Client, Request, Response};
use rb_types::{MemoryType, Namespace};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DIM: usize = 8;

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir()).unwrap()
}

fn loopback_ephemeral() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

struct RunningDaemon {
    socket: PathBuf,
    http_addr: Option<SocketAddr>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl RunningDaemon {
    async fn start(http: Option<HttpListenerConfig>) -> RunningDaemon {
        let dir = tempdir();
        let socket = dir.path().join("runtime").join("sock");
        let db = dir.path().join("memory.db");
        let cfg = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db,
            read_pool_size: 4,
            jobs_config: rb_daemon::JobsConfig::default(),
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_engine::FusionMode::Linear,
            http,
        };
        let daemon = Daemon::bind(cfg, SharedEmbedder::new(DeterministicProvider::new(DIM)))
            .await
            .unwrap();
        let http_addr = daemon.http_addr();

        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            daemon
                .run(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });

        let mut ready = false;
        for _ in 0..200 {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(ready, "daemon socket not reachable at startup");

        RunningDaemon {
            socket,
            http_addr,
            shutdown: Some(tx),
            task: Some(task),
            _dir: dir,
        }
    }

    fn http_addr(&self) -> SocketAddr {
        self.http_addr.expect("daemon started with http enabled")
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("daemon task did not shut down within 5s")
                .expect("daemon task failed during shutdown");
        }
    }
}

/// Build a raw HTTP/1.1 request. `host: None` omits the Host header entirely.
fn build_request(
    method: &str,
    path: &str,
    host: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: Option<&str>,
) -> Vec<u8> {
    let mut req = format!("{method} {path} HTTP/1.1\r\n");
    if let Some(host) = host {
        req.push_str(&format!("Host: {host}\r\n"));
    }
    for (name, value) in extra_headers {
        req.push_str(&format!("{name}: {value}\r\n"));
    }
    req.push_str("Connection: close\r\n");
    if let Some(body) = body {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    let mut bytes = req.into_bytes();
    if let Some(body) = body {
        bytes.extend_from_slice(body.as_bytes());
    }
    bytes
}

/// Send raw bytes, read to EOF, return (status, head, body). `Connection:
/// close` makes read-to-EOF well-defined. `head` is the status line plus
/// response headers (lowercased for case-insensitive header assertions).
async fn raw_round_trip_full(addr: SocketAddr, request: &[u8]) -> (u16, String, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut buf))
        .await
        .expect("server did not respond within 10s")
        .unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status line in response: {text:?}"));
    let (head, body) = text
        .split_once("\r\n\r\n")
        .map(|(h, b)| (h.to_ascii_lowercase(), b.to_string()))
        .unwrap_or_default();
    (status, head, body)
}

/// Send raw bytes, read to EOF, return (status, body).
async fn raw_round_trip(addr: SocketAddr, request: &[u8]) -> (u16, String) {
    let (status, _head, body) = raw_round_trip_full(addr, request).await;
    (status, body)
}

/// JSON POST with well-formed loopback Host and JSON content type.
async fn post_json(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
    let host = addr.to_string();
    let req = build_request(
        "POST",
        path,
        Some(&host),
        &[("Content-Type", "application/json")],
        Some(body),
    );
    raw_round_trip(addr, &req).await
}

async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    let host = addr.to_string();
    let req = build_request("GET", path, Some(&host), &[], None);
    raw_round_trip(addr, &req).await
}

/// Serialize an `rb_proto::Request` to its tagged wire JSON (the `/ops` body
/// and — with the tag stripped — the shortcut-endpoint bodies), so tests never
/// hand-write wire shapes.
fn op_json(req: &Request) -> serde_json::Value {
    serde_json::to_value(req).unwrap()
}

fn shortcut_body(req: &Request) -> String {
    let mut v = op_json(req);
    v.as_object_mut().unwrap().remove("op");
    v.to_string()
}

fn recall_request(query: &str) -> Request {
    Request::Recall {
        query: query.to_string(),
        memory_type: None,
        tags: vec![],
        limit: 10,
        filter: rb_types::RecallFilter::default(),
    }
}

fn remember_request(content: &str) -> Request {
    Request::Remember {
        content: content.to_string(),
        context: None,
        memory_type: MemoryType::Insight,
        importance: 6,
        keywords: vec![],
        tags: vec![],
        related_files: vec![],
        confidence: None,
        supersedes: None,
        anchors: vec![],
    }
}

// ---------------------------------------------------------------------------
// Off-by-default / bind validation
// ---------------------------------------------------------------------------

/// SECURITY: no HTTP config means ZERO footprint — no TCP socket is bound and
/// no listener task exists. The UDS path is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_http_has_zero_footprint() {
    let daemon = RunningDaemon::start(None).await;
    assert!(
        daemon.http_addr.is_none(),
        "no http config must mean no bound address"
    );
    // UDS still serves.
    let mut client = Client::connect(&daemon.socket, Namespace::Global)
        .await
        .unwrap();
    client.ping().await.unwrap();
    daemon.stop().await;
}

/// SECURITY (fail closed): the daemon re-validates the bind address itself —
/// a non-loopback address is refused at bind even if a caller bypassed the
/// config-layer validation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_loopback_bind_fails_closed_at_daemon_bind() {
    let dir = tempdir();
    let cfg = DaemonConfig {
        socket_path: dir.path().join("runtime").join("sock"),
        db_path: dir.path().join("memory.db"),
        read_pool_size: 2,
        jobs_config: rb_daemon::JobsConfig::default(),
        retention_policy: None,
        request_idle_timeout: None,
        enrich: None,
        fusion_mode: rb_engine::FusionMode::Linear,
        http: Some(HttpListenerConfig {
            bind: "0.0.0.0:0".parse().unwrap(),
            ..Default::default()
        }),
    };
    let err = Daemon::bind(cfg, SharedEmbedder::new(DeterministicProvider::new(DIM)))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("loopback"),
        "must refuse non-loopback bind: {err}"
    );
}

// ---------------------------------------------------------------------------
// Op mirroring (parity with UDS)
// ---------------------------------------------------------------------------

/// PRD acceptance: `POST /recall` returns the same ranked results as the same
/// recall over UDS, and the generic `/ops` mirror agrees with the shortcut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_recall_matches_uds_recall() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();

    let mut uds = Client::connect(&daemon.socket, Namespace::Global)
        .await
        .unwrap();
    for content in [
        "the daemon binds a unix socket with 0600 permissions",
        "sqlite-vec partitions vectors by namespace",
        "retention policy is opt-in and fail-closed",
    ] {
        uds.request(remember_request(content)).await.unwrap();
    }

    let recall = recall_request("unix socket permissions");
    let uds_resp = uds.request(recall.clone()).await.unwrap();
    let uds_ids: Vec<String> = match &uds_resp {
        Response::Recalled { results, .. } => {
            results.iter().map(|r| r.memory.id.to_string()).collect()
        }
        other => panic!("expected Recalled over UDS, got {other:?}"),
    };
    assert!(!uds_ids.is_empty(), "corpus must produce recall hits");

    // Shortcut endpoint.
    let (status, body) = post_json(addr, "/recall", &shortcut_body(&recall)).await;
    assert_eq!(status, 200, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["result"], "Recalled");
    let http_ids: Vec<String> = parsed["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["memory"]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(http_ids, uds_ids, "HTTP recall must rank exactly like UDS");

    // Generic /ops mirror carries the same tagged shape as the wire.
    let (status, ops_body) = post_json(addr, "/ops", &op_json(&recall).to_string()).await;
    assert_eq!(status, 200, "{ops_body}");
    let ops_parsed: serde_json::Value = serde_json::from_str(&ops_body).unwrap();
    let ops_ids: Vec<String> = ops_parsed["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["memory"]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ops_ids, uds_ids, "/ops recall must rank exactly like UDS");

    daemon.stop().await;
}

/// The PRD-named REST surface round-trips: remember → get → context →
/// feedback, all over HTTP, serialized as the existing `Response` types.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_rest_surface_round_trips() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();

    // POST /remember
    let (status, body) = post_json(
        addr,
        "/remember",
        &shortcut_body(&remember_request("http surface stores a memory")),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["result"], "Remembered");
    let id = parsed["id"].as_str().unwrap().to_string();

    // GET /memories/:id
    let (status, body) = get(addr, &format!("/memories/{id}")).await;
    assert_eq!(status, 200, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        parsed["memory"]["content"].as_str().unwrap(),
        "http surface stores a memory"
    );

    // GET /context
    let (status, body) = get(addr, "/context").await;
    assert_eq!(status, 200, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["result"], "ContextResult");

    // GET /ping
    let (status, body) = get(addr, "/ping").await;
    assert_eq!(status, 200, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["result"], "Pong");

    // POST /feedback
    let feedback = Request::Feedback {
        id: id.parse().unwrap(),
        kind: rb_types::FeedbackKind::Helpful,
    };
    let (status, body) = post_json(addr, "/feedback", &shortcut_body(&feedback)).await;
    assert_eq!(status, 200, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["result"], "FeedbackRecorded");

    daemon.stop().await;
}

/// The namespace header scopes requests server-side exactly like the UDS
/// handshake namespace (namespace is organization, not auth — but scoping
/// must not leak across namespaces).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn namespace_header_scopes_http_requests() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let host = addr.to_string();

    let body = shortcut_body(&remember_request("namespaced memory over http"));
    let req = build_request(
        "POST",
        "/remember",
        Some(&host),
        &[
            ("Content-Type", "application/json"),
            ("x-rusty-brain-namespace", "project:alpha"),
        ],
        Some(&body),
    );
    let (status, body) = raw_round_trip(addr, &req).await;
    assert_eq!(status, 200, "{body}");

    // Same recall in another namespace: no hits.
    let recall_body = shortcut_body(&recall_request("namespaced memory"));
    let req = build_request(
        "POST",
        "/recall",
        Some(&host),
        &[
            ("Content-Type", "application/json"),
            ("x-rusty-brain-namespace", "project:beta"),
        ],
        Some(&recall_body),
    );
    let (status, body) = raw_round_trip(addr, &req).await;
    assert_eq!(status, 200, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["results"].as_array().unwrap().len(), 0);

    // And in the writing namespace: hit.
    let req = build_request(
        "POST",
        "/recall",
        Some(&host),
        &[
            ("Content-Type", "application/json"),
            ("x-rusty-brain-namespace", "project:alpha"),
        ],
        Some(&recall_body),
    );
    let (status, body) = raw_round_trip(addr, &req).await;
    assert_eq!(status, 200, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(!parsed["results"].as_array().unwrap().is_empty());

    daemon.stop().await;
}

/// An invalid namespace header fails closed with a 400, before any dispatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_namespace_header_is_400() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let host = addr.to_string();
    let req = build_request(
        "GET",
        "/ping",
        Some(&host),
        &[("x-rusty-brain-namespace", "not/a/namespace")],
        None,
    );
    let (status, body) = raw_round_trip(addr, &req).await;
    assert_eq!(status, 400, "{body}");
    daemon.stop().await;
}

// ---------------------------------------------------------------------------
// Admin gating (peer-cred equivalence)
// ---------------------------------------------------------------------------

/// SECURITY: TCP loopback provides no kernel-verified peer credential, so an
/// HTTP connection is NEVER admin (fail closed, W2.6 semantics: unreadable
/// peer credentials are non-admin). Every admin op is denied over HTTP even
/// though the same user would be allowed over UDS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_ops_over_http_are_denied() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();

    let admin_ops: Vec<serde_json::Value> = vec![
        serde_json::json!({"op": "Scrub"}),
        serde_json::json!({"op": "Reembed", "limit": 10}),
        serde_json::json!({"op": "RunJob", "job": "consolidation"}),
        serde_json::json!({
            "op": "NamespaceRename",
            "old": {"Project": "a"},
            "new": {"Project": "b"},
        }),
        // Hard-EXECUTE forget is admin-gated like Scrub.
        serde_json::json!({
            "op": "Forget",
            "policy": {
                "enabled": true,
                "max_age_days": 30,
                "archive_after_days": null,
                "importance_floor": 6,
                "protected_tags": [],
                "batch_limit": 100,
            },
            "mode": "hard",
            "dry_run": false,
        }),
    ];
    for op in admin_ops {
        let (status, body) = post_json(addr, "/ops", &op.to_string()).await;
        assert_eq!(status, 403, "admin op {op} must be denied: {body}");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["result"], "Error");
        assert_eq!(parsed["kind"], "permission_denied", "{body}");
    }

    // Sanity: the SAME admin op over UDS (same uid as the daemon) succeeds —
    // HTTP is strictly more gated, not differently gated.
    let mut uds = Client::connect(&daemon.socket, Namespace::Global)
        .await
        .unwrap();
    let resp = uds.request(Request::Scrub).await.unwrap();
    assert!(
        !matches!(&resp, Response::Error { kind, .. } if kind == "permission_denied"),
        "same-uid UDS admin op must not be permission-denied: {resp:?}"
    );

    daemon.stop().await;
}

/// Subscribe is a streaming op that cannot follow request/response over
/// HTTP v1: reject it cleanly instead of hanging the connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribe_over_http_is_rejected() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let (status, body) = post_json(addr, "/ops", "{\"op\":\"Subscribe\"}").await;
    assert_eq!(status, 400, "{body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["kind"], "invalid_argument", "{body}");
    daemon.stop().await;
}

// ---------------------------------------------------------------------------
// Fail-closed request hygiene
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_json_body_is_400() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let (status, body) = post_json(addr, "/recall", "{not json").await;
    assert_eq!(status, 400, "{body}");
    daemon.stop().await;
}

/// SECURITY: bodies over the wire-frame bound (1 MiB, the UDS
/// `MAX_FRAME_BYTES` parity) are refused up front from the declared length —
/// no buffering, no partial processing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_body_is_413() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let host = addr.to_string();

    // Declared oversized length: the server must answer without waiting for
    // the (never-sent) body.
    let mut req = format!("POST /recall HTTP/1.1\r\nHost: {host}\r\n");
    req.push_str("Content-Type: application/json\r\n");
    req.push_str("Connection: close\r\n");
    req.push_str(&format!("Content-Length: {}\r\n\r\n", 2 * 1024 * 1024));
    let (status, body) = raw_round_trip(addr, req.as_bytes()).await;
    assert_eq!(status, 413, "{body}");

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_method_is_405() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let (status, _) = get(addr, "/recall").await;
    assert_eq!(status, 405);
    let (status, _) = post_json(addr, "/context", "{}").await;
    assert_eq!(status, 405);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_path_is_404() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let (status, _) = get(addr, "/admin").await;
    assert_eq!(status, 404);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_memory_with_invalid_id_is_400() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let (status, _) = get(addr, "/memories/not-a-uuid").await;
    assert_eq!(status, 400);
    daemon.stop().await;
}

/// SECURITY: POST bodies must declare `application/json`. A browser can fire
/// a cross-site "simple request" (text/plain form POST) at loopback without
/// any preflight; requiring the JSON content type forces a CORS preflight,
/// which fails because the listener never sends CORS headers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_without_json_content_type_is_415() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let host = addr.to_string();
    let body = shortcut_body(&recall_request("q"));
    let req = build_request(
        "POST",
        "/recall",
        Some(&host),
        &[("Content-Type", "text/plain")],
        Some(&body),
    );
    let (status, resp_body) = raw_round_trip(addr, &req).await;
    assert_eq!(status, 415, "{resp_body}");

    // charset parameters on the JSON type are fine.
    let req = build_request(
        "POST",
        "/recall",
        Some(&host),
        &[("Content-Type", "application/json; charset=utf-8")],
        Some(&body),
    );
    let (status, resp_body) = raw_round_trip(addr, &req).await;
    assert_eq!(status, 200, "{resp_body}");

    daemon.stop().await;
}

/// SECURITY (DNS-rebinding defense): the Host header must name a loopback
/// literal. A hostile page at `evil.example` whose DNS re-resolves to
/// 127.0.0.1 makes the browser send `Host: evil.example` — refuse it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_or_missing_host_is_rejected() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();

    for host in [
        "evil.example.com",
        "evil.example.com:80",
        "127.0.0.1.evil.example",
    ] {
        let req = build_request("GET", "/ping", Some(host), &[], None);
        let (status, body) = raw_round_trip(addr, &req).await;
        assert_eq!(status, 403, "Host {host} must be refused: {body}");
    }

    // Loopback spellings are accepted.
    for host in [
        addr.to_string(),
        "127.0.0.1".to_string(),
        format!("localhost:{}", addr.port()),
        "localhost".to_string(),
    ] {
        let req = build_request("GET", "/ping", Some(&host), &[], None);
        let (status, body) = raw_round_trip(addr, &req).await;
        assert_eq!(status, 200, "Host {host} must be accepted: {body}");
    }

    // No Host at all on HTTP/1.1: refused (never processed).
    let req = build_request("GET", "/ping", None, &[], None);
    let (status, _) = raw_round_trip(addr, &req).await;
    assert!(
        status == 400 || status == 403,
        "missing Host must be refused, got {status}"
    );

    daemon.stop().await;
}

/// SECURITY (request-smuggling hygiene, RFC 7230 §5.4): a request with more
/// than one Host header field is refused 400 — even when the values agree —
/// so no downstream component can ever disagree about which one applied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_host_headers_are_rejected() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let host = addr.to_string();

    // Differing duplicates: the classic smuggling shape.
    let req = format!(
        "GET /ping HTTP/1.1\r\nHost: {host}\r\nHost: evil.example.com\r\nConnection: close\r\n\r\n"
    );
    let (status, body) = raw_round_trip(addr, req.as_bytes()).await;
    assert_eq!(status, 400, "differing duplicate Hosts must be 400: {body}");

    // Equal duplicates are ALSO refused (RFC 7230 §5.4 is unconditional).
    let req =
        format!("GET /ping HTTP/1.1\r\nHost: {host}\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let (status, body) = raw_round_trip(addr, req.as_bytes()).await;
    assert_eq!(status, 400, "equal duplicate Hosts must be 400: {body}");

    daemon.stop().await;
}

/// SECURITY: an absolute-form request-target carries its own authority; when
/// a Host header is ALSO present the two must agree, otherwise the request
/// is refused 400 before any processing — a mismatched pair must never be
/// resolved by silently preferring one of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absolute_uri_and_host_mismatch_is_rejected() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let host = addr.to_string();

    // Loopback authority in the request line, foreign Host header.
    let req = format!(
        "GET http://{host}/ping HTTP/1.1\r\nHost: evil.example.com\r\nConnection: close\r\n\r\n"
    );
    let (status, body) = raw_round_trip(addr, req.as_bytes()).await;
    assert_eq!(status, 400, "mismatched authority/Host must be 400: {body}");

    // Foreign authority in the request line, loopback Host header — the
    // mismatch is refused without ever trusting either value.
    let req = format!(
        "GET http://evil.example.com/ping HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    );
    let (status, body) = raw_round_trip(addr, req.as_bytes()).await;
    assert_eq!(status, 400, "mismatched authority/Host must be 400: {body}");

    // A MATCHING absolute-form pair is legitimate and passes.
    let req =
        format!("GET http://{host}/ping HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let (status, body) = raw_round_trip(addr, req.as_bytes()).await;
    assert_eq!(status, 200, "matching authority/Host must pass: {body}");

    daemon.stop().await;
}

/// SECURITY (browser-origin defense): a request carrying a non-loopback
/// Origin header is a cross-origin browser request — refuse it. Requests
/// without Origin (curl, scripts, native clients) and same-origin loopback
/// requests pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_origin_is_rejected() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let host = addr.to_string();

    for origin in ["https://evil.example", "http://evil.example:8080", "null"] {
        let req = build_request("GET", "/ping", Some(&host), &[("Origin", origin)], None);
        let (status, body) = raw_round_trip(addr, &req).await;
        assert_eq!(status, 403, "Origin {origin} must be refused: {body}");
    }

    let loopback_origin = format!("http://{host}");
    let req = build_request(
        "GET",
        "/ping",
        Some(&host),
        &[("Origin", &loopback_origin)],
        None,
    );
    let (status, body) = raw_round_trip(addr, &req).await;
    assert_eq!(status, 200, "loopback Origin must pass: {body}");

    daemon.stop().await;
}

// ---------------------------------------------------------------------------
// Resource bounds and lifecycle
// ---------------------------------------------------------------------------

/// SECURITY (slowloris defense): a connection that never finishes sending its
/// request headers is closed at the header-read deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_connection_is_closed_at_header_deadline() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig {
        bind: loopback_ephemeral(),
        header_read_timeout: Some(Duration::from_millis(200)),
        ..Default::default()
    }))
    .await;
    let addr = daemon.http_addr();

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(b"GET /ping HTT").await.unwrap(); // stall mid-request-line
    let mut buf = Vec::new();
    let n = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
        .await
        .expect("stalled connection must be closed by the header deadline")
        .unwrap_or(0);
    // Either a 408 response or a bare close is acceptable; hanging is not.
    let _ = n;
    daemon.stop().await;
}

/// SECURITY (slowloris-on-body defense): a client that completes its headers
/// with an under-cap Content-Length and then TRICKLES the body is cut off at
/// the overall request deadline with a 503 — the header deadline alone does
/// not cover this phase. The UDS path is unaffected throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trickled_body_is_closed_at_request_deadline() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig {
        bind: loopback_ephemeral(),
        request_timeout: Some(Duration::from_millis(300)),
        ..Default::default()
    }))
    .await;
    let addr = daemon.http_addr();
    let host = addr.to_string();

    let mut stream = TcpStream::connect(addr).await.unwrap();
    // Complete headers, body cap respected on paper — then stall mid-body.
    let head = format!(
        "POST /recall HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n\
         Connection: close\r\nContent-Length: 1000\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(b"{\"query\":").await.unwrap(); // 9 of 1000 bytes, then silence

    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
        .await
        .expect("trickled body must be cut off at the request deadline")
        .unwrap_or(0);
    let text = String::from_utf8_lossy(&buf);
    assert!(
        text.starts_with("HTTP/1.1 503"),
        "expected a 503 at the request deadline, got {text:?}"
    );

    // The stalled HTTP request never touched the UDS path.
    let mut uds = Client::connect(&daemon.socket, Namespace::Global)
        .await
        .unwrap();
    uds.ping().await.unwrap();

    daemon.stop().await;
}

/// A 405 names the method the path actually supports — per path, not a
/// blanket list (Copilot review, PR #62).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn method_not_allowed_names_the_paths_allowed_method() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();
    let host = addr.to_string();

    // GET-only paths report Allow: GET.
    for path in [
        "/context",
        &format!("/memories/{}", rb_types::MemoryId::new()),
    ] {
        let req = build_request(
            "POST",
            path,
            Some(&host),
            &[("Content-Type", "application/json")],
            Some("{}"),
        );
        let (status, head, body) = raw_round_trip_full(addr, &req).await;
        assert_eq!(status, 405, "{path}: {body}");
        assert!(
            head.lines().any(|l| l.trim() == "allow: get"),
            "{path} must advertise exactly Allow: GET, got head {head:?}"
        );
    }

    // POST-only paths report Allow: POST.
    for path in ["/recall", "/ops"] {
        let req = build_request("GET", path, Some(&host), &[], None);
        let (status, head, body) = raw_round_trip_full(addr, &req).await;
        assert_eq!(status, 405, "{path}: {body}");
        assert!(
            head.lines().any(|l| l.trim() == "allow: post"),
            "{path} must advertise exactly Allow: POST, got head {head:?}"
        );
    }

    daemon.stop().await;
}

/// SECURITY (connection bound): concurrent HTTP connections are capped by a
/// dedicated semaphore so the HTTP path cannot starve the UDS path. Excess
/// connections are closed immediately, and the UDS path stays live.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn excess_http_connections_are_dropped_and_uds_stays_live() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig {
        bind: loopback_ephemeral(),
        max_connections: Some(2),
        ..Default::default()
    }))
    .await;
    let addr = daemon.http_addr();

    // Two held-open connections consume both permits.
    let held1 = TcpStream::connect(addr).await.unwrap();
    let held2 = TcpStream::connect(addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The third is closed without service.
    let mut third = TcpStream::connect(addr).await.unwrap();
    let mut buf = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(5), third.read_to_end(&mut buf)).await;
    assert!(
        matches!(read, Ok(Ok(0))),
        "over-cap connection must be closed empty, got {read:?} / {buf:?}"
    );

    // The UDS path is unaffected by HTTP saturation.
    let mut uds = Client::connect(&daemon.socket, Namespace::Global)
        .await
        .unwrap();
    uds.ping().await.unwrap();

    drop(held1);
    drop(held2);
    daemon.stop().await;
}

/// Graceful shutdown covers the HTTP listener: after shutdown resolves, the
/// port no longer accepts connections.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_shutdown_covers_http_listener() {
    let daemon = RunningDaemon::start(Some(HttpListenerConfig::default())).await;
    let addr = daemon.http_addr();

    // Serves before shutdown.
    let (status, _) = get(addr, "/ping").await;
    assert_eq!(status, 200);

    daemon.stop().await;

    // Refused (or immediately closed) after shutdown.
    match TcpStream::connect(addr).await {
        Err(_) => {}
        Ok(mut stream) => {
            let mut buf = Vec::new();
            let n = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut buf))
                .await
                .expect("post-shutdown connection must not be serviced")
                .unwrap_or(0);
            assert_eq!(n, 0, "post-shutdown connection must be closed empty");
        }
    }
}
