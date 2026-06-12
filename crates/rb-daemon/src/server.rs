//! Unix-domain-socket server for the daemon.

use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use rb_embed::EmbeddingProvider;
use rb_engine::{Enricher, MemoryEngine, RememberInput};
use rb_enrich::OpenAiCompatEnricher;
use rb_proto::{
    bounded_framed, read_frame, write_frame, Handshake, HandshakeAck, RecallChannelTotals, Request,
    Response, CONTRACT_VERSION,
};
use rb_types::{Error, Result};
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{info, warn};

use crate::error_map::error_to_response;
use crate::jobs::{self, JobsConfig};
use crate::{SharedEmbedder, StoreHandle};

/// Maximum number of results returned per Recall or List request.
const MAX_LIMIT: usize = 1000;
/// Maximum graph traversal depth per Graph request.
const MAX_DEPTH: u8 = 8;
/// Default re-embed batch size when a `Reembed` request omits a limit. Bounded
/// so one pass converges a slice of the corpus; rerun until `changed == 0`.
const REEMBED_DEFAULT_LIMIT: usize = 1000;
/// Hard cap on a single re-embed pass to bound writer-thread work per request.
const REEMBED_MAX_LIMIT: usize = 10_000;
/// Maximum number of simultaneous client connections.
const MAX_CONNECTIONS: usize = 256;
/// Idle deadline for the initial handshake read (fail fast on stalled connects).
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Default idle deadline between consecutive request frames from an established
/// client; overridable via `RUSTY_BRAIN_IDLE_TIMEOUT_SECS` (see
/// [`parse_idle_timeout`]) so the idle/reconnect e2e runs in seconds.
const DEFAULT_REQUEST_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Daemon-lifetime per-channel recall hit-contribution counters (W1.0).
///
/// Cheap by construction: four relaxed atomic adds per served recall, no locks,
/// no per-namespace cardinality. Shared across every connection task and
/// snapshotted into `Response::Pong` on the status path (`Ping`). `Relaxed`
/// ordering is sufficient — these are monotone observability counters with no
/// cross-variable invariant.
#[derive(Debug, Default)]
pub struct RecallChannelCounters {
    recalls: std::sync::atomic::AtomicU64,
    fts_hits: std::sync::atomic::AtomicU64,
    vector_hits: std::sync::atomic::AtomicU64,
    graph_hits: std::sync::atomic::AtomicU64,
}

impl RecallChannelCounters {
    /// Fold one served recall's results into the totals.
    fn record(&self, results: &[rb_types::SearchResult]) {
        use std::sync::atomic::Ordering::Relaxed;
        let (mut fts, mut vector, mut graph) = (0u64, 0u64, 0u64);
        for r in results {
            fts += u64::from(r.channels.fts);
            vector += u64::from(r.channels.vector);
            graph += u64::from(r.channels.graph);
        }
        self.recalls.fetch_add(1, Relaxed);
        self.fts_hits.fetch_add(fts, Relaxed);
        self.vector_hits.fetch_add(vector, Relaxed);
        self.graph_hits.fetch_add(graph, Relaxed);
    }

    /// Snapshot the totals for the wire (`Pong.recall_channels`).
    fn snapshot(&self) -> RecallChannelTotals {
        use std::sync::atomic::Ordering::Relaxed;
        RecallChannelTotals {
            recalls: self.recalls.load(Relaxed),
            fts_hits: self.fts_hits.load(Relaxed),
            vector_hits: self.vector_hits.load(Relaxed),
            graph_hits: self.graph_hits.load(Relaxed),
        }
    }
}

/// Static configuration for a daemon instance.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub read_pool_size: usize,
    pub jobs_config: JobsConfig,
}

/// A bound, ready-to-run daemon.
pub struct Daemon {
    listener: UnixListener,
    store: StoreHandle,
    embedder: SharedEmbedder,
    enricher: Option<Arc<dyn Enricher>>,
    socket_path: PathBuf,
    pidfile_path: PathBuf,
    bind_guard: BindGuard,
    jobs_config: JobsConfig,
    request_idle_timeout: std::time::Duration,
}

/// Resolve the request idle timeout from an env value. Fail-safe: absent,
/// empty, non-numeric, or zero values all use the default — a misconfigured
/// override must never produce an instantly-dying or never-dying connection.
fn parse_idle_timeout(value: Option<&str>) -> std::time::Duration {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&secs| secs > 0)
        .map(std::time::Duration::from_secs)
        .unwrap_or(DEFAULT_REQUEST_IDLE_TIMEOUT)
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("socket_path", &self.socket_path)
            .field("pidfile_path", &self.pidfile_path)
            .field("enricher_active", &self.enricher.is_some())
            .finish_non_exhaustive()
    }
}

impl Daemon {
    /// Bind the daemon socket and initialize the backing store.
    pub async fn bind(config: DaemonConfig, embedder: SharedEmbedder) -> Result<Self> {
        let dim = embedder.dim();

        if let Some(parent) = config.db_path.parent() {
            prepare_db_dir(parent)?;
        }

        prepare_socket_dir(&config.socket_path)?;

        let pidfile_path = config.socket_path.with_extension("pid");
        let mut bind_guard = BindGuard::acquire(config.socket_path.clone(), pidfile_path.clone())?;

        if config.socket_path.exists() {
            if probe_live(&config.socket_path).await {
                return Err(Error::Io(format!(
                    "another rusty-brain daemon is already listening at {}",
                    config.socket_path.display()
                )));
            }
            fs::remove_file(&config.socket_path).map_err(|e| {
                Error::Io(format!(
                    "remove stale socket {}: {e}",
                    config.socket_path.display()
                ))
            })?;
        }

        let listener = UnixListener::bind(&config.socket_path)
            .map_err(|e| Error::Io(format!("bind {}: {e}", config.socket_path.display())))?;
        fs::set_permissions(&config.socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::Io(format!("chmod 0600 {}: {e}", config.socket_path.display())))?;
        bind_guard.mark_socket_bound();

        // Bind the embedder's model identity into every store open so a
        // same-dim provider swap fails closed instead of mixing vector spaces.
        let store = StoreHandle::start_with_model(
            config.db_path.clone(),
            dim,
            embedder.model_id().to_string(),
            config.read_pool_size,
        )?;

        // Build the opt-in LLM enricher once (reqwest client is reused across
        // all connections). Activation requires RB_ENRICH_BASE_URL +
        // RB_ENRICH_MODEL; falls back to heuristic when either is absent.
        let enricher: Option<Arc<dyn Enricher>> = match OpenAiCompatEnricher::from_env() {
            Ok(Some(e)) => {
                info!("LLM enrichment active");
                Some(Arc::new(e))
            }
            Ok(None) => None,
            Err(e) => {
                warn!(error = %e, "failed to build LLM enricher; falling back to heuristic");
                None
            }
        };

        // Read once at bind so every connection of this daemon instance agrees.
        let request_idle_timeout =
            parse_idle_timeout(std::env::var(rb_config::IDLE_TIMEOUT_ENV).ok().as_deref());

        info!(socket = %config.socket_path.display(), "daemon bound");
        Ok(Self {
            listener,
            store,
            embedder,
            enricher,
            socket_path: config.socket_path,
            pidfile_path,
            bind_guard,
            jobs_config: config.jobs_config,
            request_idle_timeout,
        })
    }

    /// Run until `shutdown` resolves, then drain connections and clean up.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let Daemon {
            listener,
            store,
            embedder,
            enricher,
            socket_path: _socket_path,
            pidfile_path: _pidfile_path,
            mut bind_guard,
            jobs_config,
            request_idle_timeout,
        } = self;
        tokio::pin!(shutdown);
        // Writer-death signal (W1.6c): resolves only on an ABNORMAL writer
        // exit. Raced in the select! below so a daemon whose writer is gone
        // shuts down instead of zombieing — ponging Ping while every write
        // fails with "writer thread unavailable" (F17).
        let writer_died = store.writer_died();
        tokio::pin!(writer_died);
        let scheduler = jobs::scheduler::spawn(store.clone(), jobs_config.clone());
        let mut conns: JoinSet<()> = JoinSet::new();
        let conn_sem = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        // Daemon-lifetime recall channel counters, shared by every connection.
        let recall_counters = Arc::new(RecallChannelCounters::default());

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    info!("shutdown signal received; stopping accept loop");
                    break;
                }
                () = &mut writer_died => {
                    tracing::error!(
                        "writer thread died; shutting down daemon instead of zombieing"
                    );
                    break;
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _addr)) => {
                            let store = store.clone();
                            let embedder = embedder.clone();
                            let enricher = enricher.clone();
                            let jobs_config = jobs_config.clone();
                            let recall_counters = recall_counters.clone();
                            // Acquire a connection permit before spawning. If
                            // all permits are taken, drop the newly accepted
                            // stream immediately instead of queueing unbounded
                            // per-connection tasks.
                            let permit = match conn_sem.clone().try_acquire_owned() {
                                Ok(p) => p,
                                Err(_) => {
                                    warn!("connection cap ({MAX_CONNECTIONS}) reached; dropping connection");
                                    drop(stream);
                                    continue;
                                }
                            };
                            conns.spawn(async move {
                                let _permit = permit; // released when task completes
                                if let Err(e) = handle_connection(
                                    stream,
                                    store,
                                    embedder,
                                    enricher,
                                    jobs_config,
                                    request_idle_timeout,
                                    recall_counters,
                                )
                                .await
                                {
                                    warn!(error = %e, "connection ended with error");
                                }
                            });
                        }
                        Err(e) => {
                            warn!(error = %e, "accept failed");
                        }
                    }
                }
                Some(_joined) = conns.join_next() => {}
            }
        }

        drop(listener);
        scheduler.abort();
        conns.shutdown().await;
        store.shutdown().await;

        bind_guard.cleanup();
        info!("daemon shut down cleanly");
        Ok(())
    }
}

/// Create the DB's parent dir private (0700) when the daemon creates it —
/// parity with the socket dir (W0.5). Unlike the socket dir, an EXISTING dir is
/// accepted as-is: the daemon only owns dirs it created itself (the default
/// data dir), while `RUSTY_BRAIN_DB` overrides and tests point into
/// caller-owned dirs we must not chmod or reject. The DB file itself is always
/// tightened to 0600 by rb-store at open, so the file stays private either way.
fn prepare_db_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        // Mirror prepare_socket_dir: a non-directory here would otherwise
        // surface later as an opaque SQLite open failure.
        let metadata = fs::metadata(dir)
            .map_err(|e| Error::Io(format!("stat db dir {}: {e}", dir.display())))?;
        if !metadata.is_dir() {
            return Err(Error::Io(format!(
                "db parent {} is not a directory",
                dir.display()
            )));
        }
        return Ok(());
    }
    fs::create_dir_all(dir)
        .map_err(|e| Error::Io(format!("create db dir {}: {e}", dir.display())))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| Error::Io(format!("chmod 0700 {}: {e}", dir.display())))
}

fn prepare_socket_dir(socket_path: &Path) -> Result<()> {
    let socket_dir = socket_path
        .parent()
        .ok_or_else(|| Error::Io("socket path has no parent dir".to_string()))?;

    if socket_dir.exists() {
        let metadata = fs::metadata(socket_dir)
            .map_err(|e| Error::Io(format!("stat socket dir {}: {e}", socket_dir.display())))?;
        if !metadata.is_dir() {
            return Err(Error::Io(format!(
                "socket parent {} is not a directory",
                socket_dir.display()
            )));
        }
        let mode = metadata.permissions().mode() & 0o777;
        if mode != 0o700 {
            return Err(Error::Io(format!(
                "socket dir {} must already be private (0700), got {mode:03o}",
                socket_dir.display()
            )));
        }
        return Ok(());
    }

    fs::create_dir_all(socket_dir)
        .map_err(|e| Error::Io(format!("create socket dir {}: {e}", socket_dir.display())))?;
    fs::set_permissions(socket_dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| Error::Io(format!("chmod 0700 {}: {e}", socket_dir.display())))
}

struct BindGuard {
    socket_path: PathBuf,
    pidfile_path: PathBuf,
    _pidfile: File,
    owns_socket: bool,
    active: bool,
}

impl BindGuard {
    fn acquire(socket_path: PathBuf, pidfile_path: PathBuf) -> Result<Self> {
        let pidfile = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&pidfile_path)
            .map_err(|e| Error::Io(format!("open pidfile {}: {e}", pidfile_path.display())))?;
        fs::set_permissions(&pidfile_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::Io(format!("chmod 0600 {}: {e}", pidfile_path.display())))?;
        lock_pidfile(&pidfile, &socket_path, &pidfile_path)?;
        write_pidfile(&pidfile, &pidfile_path)?;

        Ok(Self {
            socket_path,
            pidfile_path,
            _pidfile: pidfile,
            owns_socket: false,
            active: true,
        })
    }

    fn mark_socket_bound(&mut self) {
        self.owns_socket = true;
    }

    fn cleanup(&mut self) {
        if !self.active {
            return;
        }
        if self.owns_socket {
            let _ = fs::remove_file(&self.socket_path);
        }
        let _ = fs::remove_file(&self.pidfile_path);
        self.active = false;
    }
}

impl Drop for BindGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn write_pidfile(mut pidfile: &File, pidfile_path: &Path) -> Result<()> {
    pidfile
        .set_len(0)
        .map_err(|e| Error::Io(format!("truncate pidfile {}: {e}", pidfile_path.display())))?;
    pidfile
        .seek(SeekFrom::Start(0))
        .map_err(|e| Error::Io(format!("seek pidfile {}: {e}", pidfile_path.display())))?;
    write!(pidfile, "{}", std::process::id())
        .map_err(|e| Error::Io(format!("write pidfile {}: {e}", pidfile_path.display())))?;
    pidfile
        .flush()
        .map_err(|e| Error::Io(format!("flush pidfile {}: {e}", pidfile_path.display())))
}

#[allow(unsafe_code)]
fn lock_pidfile(pidfile: &File, socket_path: &Path, pidfile_path: &Path) -> Result<()> {
    // SAFETY: flock only reads the borrowed file descriptor for the duration of
    // the call. The File outlives the lock and is held by BindGuard.
    let rc = unsafe { libc::flock(pidfile.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }

    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::WouldBlock {
        return Err(Error::Io(format!(
            "another rusty-brain daemon is already listening at {} (pidfile {} is locked)",
            socket_path.display(),
            pidfile_path.display()
        )));
    }

    Err(Error::Io(format!(
        "lock pidfile {}: {err}",
        pidfile_path.display()
    )))
}

async fn probe_live(path: &Path) -> bool {
    match UnixStream::connect(path).await {
        Ok(stream) => {
            let mut framed = bounded_framed(stream);
            let hs = Handshake {
                contract_version: CONTRACT_VERSION,
                namespace: rb_types::Namespace::Global,
                identity: None,
            };
            if write_frame(&mut framed, &hs).await.is_err() {
                return false;
            }
            matches!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    read_frame::<_, HandshakeAck>(&mut framed),
                )
                .await,
                Ok(Ok(_))
            )
        }
        Err(_) => false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    stream: UnixStream,
    store: StoreHandle,
    embedder: SharedEmbedder,
    enricher: Option<Arc<dyn Enricher>>,
    jobs_config: JobsConfig,
    request_idle_timeout: std::time::Duration,
    recall_counters: Arc<RecallChannelCounters>,
) -> Result<()> {
    let mut framed = bounded_framed(stream);

    // Enforce a short deadline for the handshake so a stalled client cannot
    // tie up a connection slot indefinitely before even identifying itself.
    let handshake: Handshake = match timeout(HANDSHAKE_TIMEOUT, read_frame(&mut framed)).await {
        Ok(Ok(hs)) => hs,
        Ok(Err(_)) | Err(_) => return Ok(()), // parse error or timeout: drop silently
    };

    if handshake.contract_version != CONTRACT_VERSION {
        let ack = HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: false,
            message: Some(format!(
                "contract mismatch: server {CONTRACT_VERSION}, client {}",
                handshake.contract_version
            )),
        };
        write_frame(&mut framed, &ack).await?;
        return Ok(());
    }

    let namespace = match validate_namespace(handshake.namespace) {
        Ok(namespace) => namespace,
        Err(e) => {
            let ack = HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: false,
                message: Some(e.to_string()),
            };
            write_frame(&mut framed, &ack).await?;
            return Ok(());
        }
    };
    let ack = HandshakeAck {
        contract_version: CONTRACT_VERSION,
        ok: true,
        message: None,
    };
    write_frame(&mut framed, &ack).await?;

    // Connection-scoped provenance (W0.5): client-declared identity with a
    // daemon-side whoami fallback for user/host (same-host UDS, so the daemon's
    // view is authoritative when the client stays silent). agent/session/source
    // are client knowledge only — an old client (no identity) leaves them None.
    let identity = handshake.identity.unwrap_or_default();
    let provenance = rb_engine::Provenance {
        origin_user: identity.user.or_else(rb_config::current_user),
        origin_host: identity.host.or_else(rb_config::current_hostname),
        origin_agent: identity.agent,
        origin_source: identity.source,
        session_id: identity.session_id,
    };

    let store_for_stream = store.clone();
    let job_store = store.clone();
    let engine = {
        let base = MemoryEngine::new(store, embedder, namespace.clone());
        match enricher {
            Some(e) => base.with_enricher(e),
            None => base,
        }
    };
    loop {
        // Break the loop if the client is idle for too long between requests.
        let req: Request = match timeout(request_idle_timeout, read_frame(&mut framed)).await {
            Ok(Ok(req)) => req,
            Ok(Err(_)) => break, // parse error or clean close
            Err(_) => {
                warn!("client idle timeout; closing connection");
                break;
            }
        };
        // Subscribe converts this connection into a one-way change stream. It
        // never returns to request/response cadence; it runs until the client
        // disconnects or the broadcast closes.
        if matches!(req, Request::Subscribe) {
            stream_changes(&mut framed, &store_for_stream, &namespace).await;
            break;
        }
        let resp = dispatch(
            &engine,
            &job_store,
            &jobs_config,
            &provenance,
            &recall_counters,
            req,
        )
        .await;
        write_frame(&mut framed, &resp).await?;
    }

    Ok(())
}

/// Stream namespace-scoped change events to a subscriber until the client
/// disconnects or the broadcast closes.
///
/// HARD RULE: this must NEVER block the writer. It only reads from the broadcast
/// receiver (which drops oldest and reports `Lagged` for slow consumers) and
/// writes to this one connection's socket; a write error means the client is
/// gone, so we stop. Events outside `namespace` are filtered server-side (fail
/// closed: only exact-namespace events are forwarded).
async fn stream_changes(
    framed: &mut Framed<UnixStream, LengthDelimitedCodec>,
    store: &StoreHandle,
    namespace: &rb_types::Namespace,
) {
    let mut rx = store.subscribe();
    // Acknowledge the subscription now that the broadcast receiver is registered.
    // The client blocks in `subscribe()` until it sees this frame, so it cannot
    // commit (or unblock a peer that commits) a change that races ahead of an
    // active receiver and is silently missed.
    if write_frame(framed, &Response::SubscribeAck).await.is_err() {
        return; // client already gone
    }
    loop {
        match rx.recv().await {
            Ok(evt) => {
                if &evt.namespace != namespace {
                    continue; // cross-namespace event: never leak it
                }
                if write_frame(framed, &Response::Change(evt)).await.is_err() {
                    break; // client disconnected
                }
            }
            Err(RecvError::Lagged(dropped)) => {
                // The subscriber fell behind; the broadcast dropped `dropped`
                // events for it. Surface the count and keep streaming.
                if write_frame(framed, &Response::Lagged { dropped })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(RecvError::Closed) => break, // daemon shutting down
        }
    }
}

fn validate_namespace(namespace: rb_types::Namespace) -> Result<rb_types::Namespace> {
    let encoded = namespace.as_db_string();
    let parsed = rb_types::Namespace::parse_db_string(&encoded)?;
    if parsed == namespace {
        Ok(namespace)
    } else {
        Err(Error::InvalidNamespace(encoded))
    }
}

async fn dispatch<P>(
    engine: &MemoryEngine<StoreHandle, P>,
    job_store: &StoreHandle,
    jobs_config: &JobsConfig,
    provenance: &rb_engine::Provenance,
    recall_counters: &RecallChannelCounters,
    req: Request,
) -> Response
where
    P: EmbeddingProvider,
{
    match req {
        Request::Ping => Response::Pong {
            contract_version: CONTRACT_VERSION,
            // Status path (W1.0): surface the daemon-lifetime per-channel
            // recall hit-contribution totals.
            recall_channels: Some(recall_counters.snapshot()),
        },
        Request::Remember {
            content,
            context,
            memory_type,
            importance,
            keywords,
            tags,
            related_files,
            confidence,
        } => {
            let input = RememberInput {
                content,
                context,
                memory_type,
                importance,
                keywords,
                tags,
                related_files,
                confidence,
                provenance: provenance.clone(),
            };
            match engine.remember(input).await {
                Ok(id) => Response::Remembered { id },
                Err(e) => error_to_response(e),
            }
        }
        Request::Recall {
            query,
            memory_type,
            tags,
            limit,
        } => match engine
            .recall_with_status(&query, limit.min(MAX_LIMIT), memory_type, &tags)
            .await
        {
            Ok(outcome) => {
                // Per-channel hit-contribution counters (W1.0): four relaxed
                // atomic adds per served recall — never a failure path.
                recall_counters.record(&outcome.results);
                // `degraded` (W1.6d): an embedder outage downgraded this
                // recall to keyword+graph; the flag rides the wire so clients
                // can warn instead of silently serving vector-blind results.
                Response::Recalled {
                    results: outcome.results,
                    degraded: outcome.degraded,
                }
            }
            Err(e) => error_to_response(e),
        },
        Request::Get { id } => match engine.get(id).await {
            Ok(memory) => Response::Got { memory },
            Err(e) => error_to_response(e),
        },
        Request::List {
            min_importance,
            limit,
        } => match engine.list(min_importance, limit.min(MAX_LIMIT)).await {
            Ok(memories) => Response::Listed { memories },
            Err(e) => error_to_response(e),
        },
        Request::Graph { id, depth } => match engine.graph(id, depth.min(MAX_DEPTH)).await {
            Ok(memories) => Response::GraphResult { memories },
            Err(e) => error_to_response(e),
        },
        Request::Update { id, updates } => match engine.update(id, updates).await {
            Ok(()) => Response::Updated,
            Err(e) => error_to_response(e),
        },
        Request::Delete { id } => match engine.delete(id).await {
            Ok(()) => Response::Deleted,
            Err(e) => error_to_response(e),
        },
        Request::Context => match engine.context().await {
            Ok((recent, important, total)) => Response::ContextResult {
                recent,
                important,
                total,
            },
            Err(e) => error_to_response(e),
        },
        // Subscribe is handled by the streaming branch in `handle_connection`
        // before `dispatch` is called; reaching here is a protocol misuse.
        Request::Subscribe => error_to_response(Error::InvalidArgument(
            "Subscribe is a streaming op, not a single request".to_string(),
        )),
        Request::RunJob { job } => match jobs::run_once(job, job_store, jobs_config).await {
            Ok(summary) => Response::JobRan {
                scanned: summary.scanned,
                changed: summary.changed,
                skipped: summary.skipped,
            },
            Err(e) => error_to_response(e),
        },
        Request::Reembed { limit } => {
            // Bounded, idempotent re-embed batch (P5 Feature A). Cross-namespace
            // maintenance driven through the engine (it owns the embedder); the
            // vector UPDATE goes through the single writer. `None` uses the
            // daemon batch default.
            let batch = limit
                .unwrap_or(REEMBED_DEFAULT_LIMIT)
                .min(REEMBED_MAX_LIMIT);
            match engine.reembed(batch).await {
                Ok((scanned, changed, skipped)) => Response::JobRan {
                    scanned,
                    changed,
                    skipped,
                },
                Err(e) => error_to_response(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use rb_types::Namespace;

    #[test]
    fn prepare_socket_dir_creates_missing_parent_private() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("missing").join("sock");

        prepare_socket_dir(&socket).unwrap();

        let parent = socket.parent().unwrap();
        let mode = fs::metadata(parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn prepare_db_dir_creates_missing_dir_private_and_leaves_existing_alone() {
        let dir = tempfile::tempdir().unwrap();
        let created = dir.path().join("data").join("rusty-brain");
        prepare_db_dir(&created).unwrap();
        let mode = fs::metadata(&created).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "daemon-created db dir must be private");

        // An existing caller-owned dir is accepted untouched (the DB file is
        // still 0600 via rb-store; only daemon-created dirs are tightened).
        let existing = dir.path().join("caller-owned");
        fs::create_dir(&existing).unwrap();
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_db_dir(&existing).unwrap();
        let mode = fs::metadata(&existing).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "daemon must not chmod caller-owned db dirs");
    }

    #[test]
    fn prepare_db_dir_rejects_a_non_directory_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir");
        fs::write(&file, b"x").unwrap();
        let err = prepare_db_dir(&file).unwrap_err();
        assert!(
            err.to_string().contains("is not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn prepare_socket_dir_rejects_public_existing_parent_without_chmod() {
        let dir = tempfile::tempdir().unwrap();
        let socket_dir = dir.path().join("public");
        fs::create_dir(&socket_dir).unwrap();
        fs::set_permissions(&socket_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let socket = socket_dir.join("sock");

        let err = prepare_socket_dir(&socket).unwrap_err();
        assert!(
            err.to_string().contains("must already be private"),
            "unexpected error: {err}"
        );
        let mode = fs::metadata(&socket_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "daemon must not chmod caller-owned dirs");
    }

    #[test]
    fn parse_idle_timeout_honors_valid_seconds() {
        assert_eq!(
            parse_idle_timeout(Some("1")),
            std::time::Duration::from_secs(1)
        );
        assert_eq!(
            parse_idle_timeout(Some(" 120 ")),
            std::time::Duration::from_secs(120)
        );
    }

    #[test]
    fn parse_idle_timeout_falls_back_to_default_on_garbage() {
        // Fail-safe: absent, empty, non-numeric, negative, and zero all default.
        for bad in [
            None,
            Some(""),
            Some("abc"),
            Some("-5"),
            Some("0"),
            Some("1.5"),
        ] {
            assert_eq!(
                parse_idle_timeout(bad),
                DEFAULT_REQUEST_IDLE_TIMEOUT,
                "value {bad:?} must fall back to the default"
            );
        }
    }

    #[test]
    fn validate_namespace_rejects_non_persistable_namespaces() {
        assert!(validate_namespace(Namespace::Global).is_ok());
        assert!(validate_namespace(Namespace::Project("p".to_string())).is_ok());
        assert!(matches!(
            validate_namespace(Namespace::Project(String::new())),
            Err(Error::InvalidNamespace(_))
        ));
        assert!(matches!(
            validate_namespace(Namespace::Session {
                project: "p".to_string(),
                session_id: String::new(),
            }),
            Err(Error::InvalidNamespace(_))
        ));
    }

    #[test]
    fn recall_channel_counters_accumulate_and_snapshot() {
        use rb_types::{ChannelHits, MemoryNote, MemoryType, Namespace, SearchResult};

        let mk = |fts: bool, vector: bool, graph: bool| SearchResult {
            memory: MemoryNote::new(
                Namespace::Project("rb".into()),
                "note".into(),
                MemoryType::Insight,
                5,
            ),
            score: 0.5,
            channels: ChannelHits { fts, vector, graph },
        };

        let counters = RecallChannelCounters::default();
        assert_eq!(counters.snapshot(), RecallChannelTotals::default());

        // Recall 1: one fts+vector hit, one vector-only hit.
        counters.record(&[mk(true, true, false), mk(false, true, false)]);
        // Recall 2: one graph-only hit. Empty results still count the recall.
        counters.record(&[mk(false, false, true)]);
        counters.record(&[]);

        let snap = counters.snapshot();
        assert_eq!(snap.recalls, 3);
        assert_eq!(snap.fts_hits, 1);
        assert_eq!(snap.vector_hits, 2);
        assert_eq!(snap.graph_hits, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn writer_death_exits_the_accept_loop() {
        // W1.6c / F17: a daemon whose writer thread died must exit `run`
        // instead of zombieing — accepting connections and ponging Ping while
        // every write fails.
        use rb_embed::DeterministicProvider;

        let dir = tempfile::tempdir().unwrap();
        // The socket parent must be private (prepare_socket_dir fail-closed);
        // tempdir permissions are umask-derived, so tighten explicitly.
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let config = DaemonConfig {
            socket_path: dir.path().join("rb.sock"),
            db_path: dir.path().join("rb.db"),
            read_pool_size: 1,
            jobs_config: JobsConfig::default(),
        };
        let daemon = Daemon::bind(
            config,
            crate::SharedEmbedder::new(DeterministicProvider::new(8)),
        )
        .await
        .unwrap();
        let store = daemon.store.clone();

        // A shutdown future that never resolves: only the writer-death arm
        // can break the accept loop.
        let run = tokio::spawn(daemon.run(std::future::pending::<()>()));

        store.kill_writer_for_test().await;

        tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .expect("daemon must exit its accept loop after writer death")
            .expect("run task must not panic")
            .expect("run must return Ok on writer-death shutdown");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_job_consolidation_merges_via_store_handle() {
        use crate::jobs::{run_once, JobKind, JobsConfig};
        use crate::StoreHandle;
        use rb_engine::MemoryBackend;
        use rb_types::{MemoryNote, MemoryType, Namespace};

        const DIM: usize = 8;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        // The RunJob dispatch arm operates on a StoreHandle clone (jobs are
        // cross-namespace maintenance, not engine-bound); build one directly.
        let store = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("a".to_string());

        let mut a = MemoryNote::new(ns.clone(), "twin a".to_string(), MemoryType::Insight, 9);
        a.id = rb_types::MemoryId::new();
        let mut b = MemoryNote::new(ns.clone(), "twin b".to_string(), MemoryType::Insight, 3);
        b.id = rb_types::MemoryId::new();
        store
            .write(a, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();
        store
            .write(b, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        // Defaults disable the job, but run_once runs ONE pass on demand
        // regardless of `enabled` (enabled only gates the scheduler). Provide an
        // explicit consolidation config so the threshold is the documented 0.95.
        let config = JobsConfig {
            consolidation: crate::jobs::ConsolidationConfig {
                enabled: true,
                interval_secs: 86_400,
                similarity_threshold: 0.95,
                batch_limit: 200,
            },
            ..Default::default()
        };

        let summary = run_once(JobKind::Consolidation, &store, &config)
            .await
            .unwrap();
        assert_eq!(
            summary.changed, 1,
            "the RunJob(Consolidation) path must merge the duplicate"
        );

        store.shutdown().await;
    }
}
