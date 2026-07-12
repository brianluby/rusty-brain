//! Opt-in loopback HTTP listener (HTTP PRD 2026-07-02, HTTP-1/HTTP-2).
//!
//! REST endpoints MIRROR the UDS wire ops — every request is decoded into the
//! same `rb_proto::Request` and served by the same `dispatch` as a UDS frame,
//! so the two surfaces cannot drift. Security posture (docs/THREAT_MODEL.md,
//! "The opt-in HTTP listener"):
//!
//! - **Default-off**: no `[http]` config → no socket bound, no task spawned.
//! - **Loopback-only, fail closed**: the bind address is re-validated here
//!   (not just at config resolution) — a non-loopback address is refused.
//! - **Never admin**: TCP gives no kernel-verified peer credential, so every
//!   HTTP request dispatches as [`PeerIdentity::untrusted`] — admin ops fail
//!   with `permission_denied` exactly as an unreadable-peer-cred UDS
//!   connection would (W2.6 fail-closed semantics).
//! - **Browser defenses**: the Host header must name a loopback literal
//!   (DNS-rebinding defense), a present Origin header must be a loopback
//!   origin (cross-origin defense; no CORS headers are ever sent), and POST
//!   bodies must declare `application/json` (forces a preflight, which fails
//!   without CORS headers).
//! - **Bounded**: request bodies are capped at the UDS frame bound
//!   ([`MAX_FRAME_BYTES`]), header reads have a deadline, each request has an
//!   overall deadline, and concurrent connections are capped by a semaphore
//!   SEPARATE from the UDS cap so HTTP load cannot starve the UDS path.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use rb_engine::{Enricher, MemoryEngine};
use rb_proto::{Request, Response, MAX_FRAME_BYTES};
use rb_types::{Error, Namespace, Result};
use tokio::net::TcpListener;
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::server::{dispatch, validate_namespace, PeerIdentity, RecallChannelCounters};
use crate::{JobsConfig, SharedEmbedder, StoreHandle};

/// Default deadline for reading a request's headers (also bounds keep-alive
/// idle time between requests). Overridable per-config for tests only.
const DEFAULT_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Default overall per-request deadline (headers already read): body read +
/// dispatch + response serialization. Bounds a client that trickles its body
/// (the slowloris phase the header deadline does not cover). Overridable via
/// [`HttpListenerConfig::request_timeout`] for tests.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Default cap on simultaneous HTTP connections. Deliberately SEPARATE from
/// (and smaller than) the UDS `MAX_CONNECTIONS` so a saturated HTTP surface
/// cannot consume UDS connection slots.
const DEFAULT_MAX_CONNECTIONS: usize = 64;
/// Request header selecting the namespace (DB string form, e.g. `global`,
/// `project:name`). Absent means `global`. Namespace is organization, not an
/// auth boundary — same as the UDS handshake namespace.
const NAMESPACE_HEADER: &str = "x-rusty-brain-namespace";

/// Static configuration for the opt-in HTTP listener. Constructed by the
/// serve binary from the resolved `[http]` config (C1: this library never
/// reads env or config files itself).
#[derive(Clone, Debug)]
pub struct HttpListenerConfig {
    /// Loopback address to bind (port 0 = ephemeral). Re-validated at bind:
    /// a non-loopback address fails closed.
    pub bind: SocketAddr,
    /// Header-read deadline override; `None` uses the built-in 10s default.
    /// Exposed for tests (the `request_idle_timeout` precedent).
    pub header_read_timeout: Option<Duration>,
    /// Overall per-request deadline override (body read + dispatch +
    /// response serialization — the slowloris-on-body bound); `None` uses
    /// the built-in 30s default. Exposed for tests (the
    /// `request_idle_timeout` precedent); deliberately not a config-file
    /// knob.
    pub request_timeout: Option<Duration>,
    /// Connection-cap override; `None` uses the built-in default. Exposed
    /// for tests.
    pub max_connections: Option<usize>,
}

impl Default for HttpListenerConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            header_read_timeout: None,
            request_timeout: None,
            max_connections: None,
        }
    }
}

/// Validate the configured bind address WITHOUT touching the network.
/// Loopback-only, fail closed: the config layer already validates this, but
/// the daemon must not trust its caller to have done so (an embedded daemon
/// constructs `DaemonConfig` directly).
pub(crate) fn validate_bind(config: &HttpListenerConfig) -> Result<()> {
    if !config.bind.ip().is_loopback() {
        return Err(Error::InvalidArgument(format!(
            "refusing non-loopback http bind {}: the HTTP listener is \
             loopback-only (127.0.0.1/::1); multi-host exposure is not \
             supported (see docs/THREAT_MODEL.md)",
            config.bind
        )));
    }
    Ok(())
}

/// Bind the TCP listener (after [`validate_bind`]). Fails closed on any bind
/// error; never falls back to another address.
pub(crate) async fn bind_listener(config: &HttpListenerConfig) -> Result<TcpListener> {
    validate_bind(config)?;
    let listener = TcpListener::bind(config.bind)
        .await
        .map_err(|e| Error::Io(format!("bind http {}: {e}", config.bind)))?;
    let addr = listener
        .local_addr()
        .map_err(|e| Error::Io(format!("http local_addr: {e}")))?;
    info!(addr = %addr, "http listener bound (loopback-only, opt-in)");
    Ok(listener)
}

/// Everything a request needs to dispatch — shared by every HTTP connection,
/// mirroring what `handle_connection` threads through the UDS path.
pub(crate) struct HttpState {
    pub store: StoreHandle,
    pub embedder: SharedEmbedder,
    pub enricher: Option<Arc<dyn Enricher>>,
    pub jobs_config: JobsConfig,
    pub retention_policy: Option<rb_types::RetentionPolicy>,
    pub recall_counters: Arc<RecallChannelCounters>,
    pub fusion_mode: rb_engine::FusionMode,
    pub provider_model: String,
}

/// Accept loop: runs beside the UDS accept loop until `shutdown` flips, then
/// drops the listener and aborts remaining connections. Connection concurrency
/// is bounded by a dedicated semaphore; over-cap connections are closed
/// immediately (the UDS accept-loop convention).
pub(crate) async fn run(
    listener: TcpListener,
    config: HttpListenerConfig,
    state: Arc<HttpState>,
    mut shutdown: watch::Receiver<bool>,
) {
    let header_read_timeout = config
        .header_read_timeout
        .unwrap_or(DEFAULT_HEADER_READ_TIMEOUT);
    let request_timeout = config.request_timeout.unwrap_or(DEFAULT_REQUEST_TIMEOUT);
    let max_connections = config.max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS);
    let conn_sem = Arc::new(Semaphore::new(max_connections));
    let mut conns: JoinSet<()> = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => {
                info!("shutdown signal received; stopping http accept loop");
                break;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _peer)) => {
                        let permit = match conn_sem.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => {
                                warn!("http connection cap ({max_connections}) reached; dropping connection");
                                drop(stream);
                                continue;
                            }
                        };
                        let state = state.clone();
                        conns.spawn(async move {
                            let _permit = permit; // released when the task completes
                            let io = TokioIo::new(stream);
                            let service = service_fn(move |req| {
                                let state = state.clone();
                                async move {
                                    Ok::<_, std::convert::Infallible>(
                                        handle_request(req, state, request_timeout).await,
                                    )
                                }
                            });
                            let served = hyper::server::conn::http1::Builder::new()
                                .timer(TokioTimer::new())
                                .header_read_timeout(header_read_timeout)
                                .serve_connection(io, service)
                                .await;
                            if let Err(e) = served {
                                // Routine client-side conditions (early close,
                                // header timeout) — not server errors.
                                debug!(error = %e, "http connection ended with error");
                            }
                        });
                    }
                    Err(e) => {
                        warn!(error = %e, "http accept failed");
                    }
                }
            }
            Some(_joined) = conns.join_next() => {}
        }
    }

    drop(listener);
    conns.shutdown().await;
    info!("http listener shut down");
}

/// Serve one request under the overall request deadline (the trickled-body
/// bound; pinned by `trickled_body_is_closed_at_request_deadline`). Every
/// path returns a `Response::Error`-shaped JSON body on failure so clients
/// parse ONE error shape across transports.
async fn handle_request(
    req: hyper::Request<Incoming>,
    state: Arc<HttpState>,
    request_timeout: Duration,
) -> HttpResponse {
    match tokio::time::timeout(request_timeout, process_request(req, state)).await {
        Ok(resp) => resp,
        Err(_elapsed) => error_response(StatusCode::SERVICE_UNAVAILABLE, "io", "request timed out"),
    }
}

type HttpResponse = hyper::Response<Full<Bytes>>;

/// Decode, gate, and dispatch one request. Ordering is deliberate and MUST
/// be preserved by future edits: (1) the Host gate and (2) the Origin gate
/// run FIRST — before the namespace header is parsed, before routing, and
/// before a single body byte is read — so a browser-relayed or rebound
/// request is refused with zero processing; then (3) namespace validation,
/// (4) routing, (5) media-type + body bounds for POSTs, and only then
/// (6) the shared `dispatch`. Nothing is partially processed before its
/// gate passes.
async fn process_request(req: hyper::Request<Incoming>, state: Arc<HttpState>) -> HttpResponse {
    // DNS-rebinding defense: the request must be addressed to a loopback
    // literal (or `localhost`). A hostile page whose DNS re-resolves to
    // 127.0.0.1 arrives with its own hostname in Host — refuse it.
    //
    // RFC 7230 §5.4 hygiene first: more than one Host header field is a
    // hard 400, even when the values agree — duplicate Host is a classic
    // request-smuggling shape, and "which one applied" must never be a
    // question.
    let mut host_values = req.headers().get_all(hyper::header::HOST).iter();
    let host_header = match (host_values.next(), host_values.next()) {
        (Some(_), Some(_)) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "multiple Host headers are not allowed",
            )
        }
        (Some(value), None) => match value.to_str() {
            Ok(v) => Some(v.to_string()),
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "malformed Host header",
                )
            }
        },
        (None, _) => None,
    };
    // An absolute-form request-target carries its own authority. When a Host
    // header is ALSO present the two must agree (ASCII case-insensitively;
    // no port normalization — fail closed on any difference): a mismatched
    // pair is refused rather than resolved by silently preferring one side.
    let authority = match (req.uri().authority().map(|a| a.to_string()), host_header) {
        (Some(authority), Some(host)) => {
            if !authority.eq_ignore_ascii_case(&host) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "request-line authority and Host header disagree",
                );
            }
            Some(authority)
        }
        (Some(authority), None) => Some(authority),
        (None, host) => host,
    };
    match authority {
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "missing Host header",
            )
        }
        Some(host) if !host_is_loopback(&host) => {
            return error_response(
                StatusCode::FORBIDDEN,
                "permission_denied",
                "Host must be a loopback literal (127.0.0.1, [::1], or localhost)",
            )
        }
        Some(_) => {}
    }

    // Cross-origin defense: a present Origin header means a browser sent
    // this cross-context; only loopback origins may pass. No CORS headers
    // are ever emitted, so browsers refuse to share responses regardless.
    if let Some(origin) = header_str(&req, hyper::header::ORIGIN) {
        if !origin_is_loopback(origin) {
            return error_response(
                StatusCode::FORBIDDEN,
                "permission_denied",
                "cross-origin requests are not allowed",
            );
        }
    }

    // Namespace: header-selected, validated exactly like the UDS handshake
    // namespace (fail closed before any dispatch).
    let namespace = match header_str(&req, NAMESPACE_HEADER) {
        None => Namespace::Global,
        Some(raw) => match Namespace::parse_db_string(raw).and_then(validate_namespace) {
            Ok(ns) => ns,
            Err(e) => {
                return error_response(StatusCode::BAD_REQUEST, "invalid_namespace", &e.to_string())
            }
        },
    };

    // Route to a wire `Request` — the single-sourced op set.
    let wire_request = match route(&req) {
        Ok(Routed::Op(op)) => op,
        Ok(Routed::Body(kind)) => match read_json_body(req).await {
            Ok(value) => match decode_op(kind, value) {
                Ok(op) => op,
                Err(resp) => return *resp,
            },
            Err(resp) => return *resp,
        },
        Err(resp) => return *resp,
    };

    // Subscribe converts a UDS connection into a change stream; HTTP v1 has
    // no streaming responses. Reject instead of hanging the connection.
    if matches!(wire_request, Request::Subscribe { .. }) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            "Subscribe is not supported over HTTP; use the UDS client",
        );
    }

    // Dispatch through the SAME path as a UDS frame. The peer is untrusted
    // by construction (no kernel-verified credential over TCP), so admin ops
    // are denied exactly like an unreadable-peer-cred UDS connection.
    let provenance = rb_engine::Provenance {
        origin_user: rb_config::current_user(),
        origin_host: rb_config::current_hostname(),
        origin_agent: None,
        origin_source: Some("http".to_string()),
        session_id: None,
    };
    let engine = {
        let base = MemoryEngine::new(
            state.store.clone(),
            state.embedder.clone(),
            namespace.clone(),
        )
        .with_fusion_mode(state.fusion_mode);
        match state.enricher.clone() {
            Some(e) => base.with_enricher(e),
            None => base,
        }
    };
    let response = dispatch(
        &engine,
        &state.store,
        &state.jobs_config,
        state.retention_policy.as_ref(),
        &provenance,
        &state.recall_counters,
        &namespace,
        &state.provider_model,
        PeerIdentity::untrusted(),
        wire_request,
    )
    .await;

    json_response(status_for(&response), &response)
}

/// Which wire op a shortcut endpoint's body decodes into.
#[derive(Clone, Copy, Debug)]
enum BodyKind {
    /// Body is the FULL tagged wire `Request` (`{"op": "...", ...}`).
    Ops,
    /// Body is the variant's fields; the tag is injected server-side.
    Tagged(&'static str),
}

enum Routed {
    /// A complete `Request` needing no body.
    Op(Request),
    /// A POST whose JSON body decodes per `BodyKind`.
    Body(BodyKind),
}

/// Map `(method, path)` to a wire op. Unknown paths are 404; known paths
/// with the wrong method are 405.
fn route(req: &hyper::Request<Incoming>) -> std::result::Result<Routed, Box<HttpResponse>> {
    let path = req.uri().path();
    let method = req.method();

    // (path, allowed method) table for the flat endpoints. Each arm knows
    // the ONE method its path supports, so a 405 can advertise it exactly.
    let routed = match path {
        "/ping" => some_get(method, || Routed::Op(Request::Ping)),
        "/context" => some_get(method, || Routed::Op(Request::Context)),
        "/recall" => some_post(method, BodyKind::Tagged("Recall")),
        "/remember" => some_post(method, BodyKind::Tagged("Remember")),
        "/feedback" => some_post(method, BodyKind::Tagged("Feedback")),
        "/ops" => some_post(method, BodyKind::Ops),
        _ => match path.strip_prefix("/memories/") {
            None => {
                return Err(Box::new(error_response(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    "unknown path",
                )))
            }
            Some(id_part) => {
                if method != Method::GET {
                    return Err(Box::new(method_not_allowed(Method::GET)));
                }
                match id_part.parse::<rb_types::MemoryId>() {
                    Ok(id) => Some(Routed::Op(Request::Get { id })),
                    Err(_) => {
                        return Err(Box::new(error_response(
                            StatusCode::BAD_REQUEST,
                            "invalid_argument",
                            "invalid memory id",
                        )))
                    }
                }
            }
        },
    };
    routed.ok_or_else(|| {
        // Every flat endpoint supports exactly one method; GET paths built
        // an op above, so a miss on them advertises GET, else POST.
        let allowed = match path {
            "/ping" | "/context" => Method::GET,
            _ => Method::POST,
        };
        Box::new(method_not_allowed(allowed))
    })
}

fn some_get(method: &Method, build: impl FnOnce() -> Routed) -> Option<Routed> {
    (method == Method::GET).then(build)
}

fn some_post(method: &Method, kind: BodyKind) -> Option<Routed> {
    (method == Method::POST).then_some(Routed::Body(kind))
}

/// 405 advertising the single method `allowed` for the requested path
/// (per-path, not a blanket list — Copilot review, PR #62).
fn method_not_allowed(allowed: Method) -> HttpResponse {
    let mut resp = error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        "invalid_argument",
        "method not allowed for this path",
    );
    if let Ok(value) = hyper::header::HeaderValue::from_str(allowed.as_str()) {
        resp.headers_mut().insert(hyper::header::ALLOW, value);
    }
    resp
}

/// Read and bound a JSON request body: `application/json` required (forces a
/// CORS preflight on browsers), declared and actual size capped at the UDS
/// frame bound ([`MAX_FRAME_BYTES`]) — fail closed BEFORE buffering an
/// oversized body.
async fn read_json_body(
    req: hyper::Request<Incoming>,
) -> std::result::Result<serde_json::Value, Box<HttpResponse>> {
    let is_json = header_str(&req, hyper::header::CONTENT_TYPE)
        .map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
        .unwrap_or(false);
    if !is_json {
        return Err(Box::new(error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_argument",
            "Content-Type must be application/json",
        )));
    }

    // Declared-length gate: refuse up front, without reading the body.
    if let Some(declared) =
        header_str(&req, hyper::header::CONTENT_LENGTH).and_then(|v| v.trim().parse::<u64>().ok())
    {
        if declared > MAX_FRAME_BYTES as u64 {
            return Err(Box::new(payload_too_large()));
        }
    }

    // Actual-length gate (covers chunked bodies): `Limited` errors once the
    // cap is crossed, so an unbounded stream is never buffered.
    let limited = Limited::new(req.into_body(), MAX_FRAME_BYTES);
    let bytes = match limited.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Err(Box::new(payload_too_large())),
    };

    serde_json::from_slice(&bytes).map_err(|e| {
        Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            &format!("malformed JSON body: {e}"),
        ))
    })
}

fn payload_too_large() -> HttpResponse {
    error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_argument",
        &format!("request body exceeds {MAX_FRAME_BYTES} bytes"),
    )
}

/// Decode a JSON body into the wire `Request`. Shortcut endpoints inject the
/// serde tag so their bodies are exactly the wire variant's fields — the
/// shapes stay single-sourced in `rb_proto::messages`.
fn decode_op(
    kind: BodyKind,
    value: serde_json::Value,
) -> std::result::Result<Request, Box<HttpResponse>> {
    let tagged = match kind {
        BodyKind::Ops => value,
        BodyKind::Tagged(tag) => match value {
            serde_json::Value::Object(mut map) => {
                map.insert("op".to_string(), serde_json::Value::String(tag.to_string()));
                serde_json::Value::Object(map)
            }
            _ => {
                return Err(Box::new(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_argument",
                    "request body must be a JSON object",
                )))
            }
        },
    };
    serde_json::from_value::<Request>(tagged).map_err(|e| {
        Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_argument",
            &format!("invalid request: {e}"),
        ))
    })
}

/// HTTP status for a wire `Response`: errors map by kind (validation → 400,
/// permission → 403, not-found → 404, internal → 500), everything else 200.
fn status_for(response: &Response) -> StatusCode {
    match response {
        Response::Error { kind, .. } => match kind.as_str() {
            "not_found" => StatusCode::NOT_FOUND,
            "permission_denied" => StatusCode::FORBIDDEN,
            "invalid_namespace"
            | "invalid_memory_type"
            | "invalid_link_type"
            | "invalid_argument"
            | "dimension_mismatch" => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        },
        _ => StatusCode::OK,
    }
}

/// Serialize a wire `Response` as the HTTP body. `no-store` because memory
/// content is sensitive; `nosniff` as defense-in-depth against content-type
/// confusion.
fn json_response(status: StatusCode, response: &Response) -> HttpResponse {
    let body = match serde_json::to_vec(response) {
        Ok(body) => body,
        Err(e) => {
            warn!(error = %e, "response serialization failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "serialization",
                "internal error",
            );
        }
    };
    let builder = hyper::Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff");
    match builder.body(Full::new(Bytes::from(body))) {
        Ok(resp) => resp,
        Err(e) => {
            // Static header construction cannot fail; guard anyway.
            warn!(error = %e, "response build failed");
            hyper::Response::new(Full::new(Bytes::from_static(b"{}")))
        }
    }
}

/// A `Response::Error`-shaped terminal response, so every HTTP failure body
/// parses like a wire error frame.
fn error_response(status: StatusCode, kind: &str, message: &str) -> HttpResponse {
    json_response(
        status,
        &Response::Error {
            kind: kind.to_string(),
            message: message.to_string(),
        },
    )
}

fn header_str<K>(req: &hyper::Request<Incoming>, name: K) -> Option<&str>
where
    K: hyper::header::AsHeaderName,
{
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Whether a Host header / URI authority names a loopback literal:
/// `127.0.0.1`, any `127.0.0.0/8` literal, `[::1]`, or `localhost`, with an
/// optional `:port`. Hostnames are NEVER resolved — an attacker-controlled
/// name that happens to resolve to loopback still fails this check (that is
/// the DNS-rebinding defense).
fn host_is_loopback(host: &str) -> bool {
    let host = host.trim();
    // Bracketed IPv6 literal, optionally with a port.
    if let Some(rest) = host.strip_prefix('[') {
        let Some((addr, after)) = rest.split_once(']') else {
            return false;
        };
        if !(after.is_empty() || is_port_suffix(after)) {
            return false;
        }
        return addr
            .parse::<std::net::Ipv6Addr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    }
    // Split a trailing `:port` (the only ':' form valid in a non-bracketed
    // authority).
    let bare = match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        Some(_) => return false,
        None => host,
    };
    if bare.eq_ignore_ascii_case("localhost") {
        return true;
    }
    bare.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn is_port_suffix(s: &str) -> bool {
    s.strip_prefix(':')
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether an Origin header names a loopback origin (`scheme://host[:port]`
/// with a loopback host). `null` and unparseable origins fail closed.
fn origin_is_loopback(origin: &str) -> bool {
    let Some((_scheme, rest)) = origin.trim().split_once("://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    !host.is_empty() && host_is_loopback(host)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn loopback_hosts_are_accepted_with_and_without_ports() {
        for host in [
            "127.0.0.1",
            "127.0.0.1:7777",
            "127.1.2.3:80",
            "localhost",
            "LOCALHOST:8080",
            "[::1]",
            "[::1]:7777",
        ] {
            assert!(host_is_loopback(host), "{host} must be loopback");
        }
    }

    #[test]
    fn non_loopback_hosts_are_rejected() {
        for host in [
            "evil.example.com",
            "evil.example.com:80",
            "127.0.0.1.evil.example",
            "192.168.1.10",
            "0.0.0.0",
            "[2001:db8::1]:80",
            "[::1",   // unterminated bracket
            "[::1]x", // junk after bracket
            "localhost:notaport",
            "",
        ] {
            assert!(!host_is_loopback(host), "{host} must be rejected");
        }
    }

    #[test]
    fn loopback_origins_pass_and_everything_else_fails_closed() {
        assert!(origin_is_loopback("http://127.0.0.1:7777"));
        assert!(origin_is_loopback("http://localhost"));
        assert!(origin_is_loopback("http://[::1]:9000"));
        for origin in [
            "https://evil.example",
            "http://evil.example:8080",
            "null",
            "",
            "127.0.0.1", // no scheme: not a well-formed origin
        ] {
            assert!(!origin_is_loopback(origin), "{origin} must fail closed");
        }
    }

    #[test]
    fn validate_bind_refuses_non_loopback() {
        for bad in ["0.0.0.0:0", "192.168.0.1:80", "[::]:80"] {
            let cfg = HttpListenerConfig {
                bind: bad.parse().unwrap(),
                ..Default::default()
            };
            let err = validate_bind(&cfg).unwrap_err();
            assert!(err.to_string().contains("loopback"), "{bad}: {err}");
        }
        let ok = HttpListenerConfig::default();
        assert!(validate_bind(&ok).is_ok());
    }

    #[test]
    fn error_statuses_map_by_wire_error_kind() {
        let mk = |kind: &str| Response::Error {
            kind: kind.to_string(),
            message: String::new(),
        };
        assert_eq!(status_for(&mk("not_found")), StatusCode::NOT_FOUND);
        assert_eq!(status_for(&mk("permission_denied")), StatusCode::FORBIDDEN);
        assert_eq!(status_for(&mk("invalid_argument")), StatusCode::BAD_REQUEST);
        assert_eq!(
            status_for(&mk("storage")),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_for(&Response::Pong {
                contract_version: rb_proto::CONTRACT_VERSION,
                recall_channels: None,
            }),
            StatusCode::OK
        );
    }
}
