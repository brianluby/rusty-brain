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
/// W3.1 write-time near-dup suppression: cosine-similarity bound at or above
/// which a freshly stored hook capture absorbs an EXISTING hook capture (via
/// supersede). Deliberately high (near-identical only): two captures this
/// similar say the same thing, so collapsing to the newest is the intended
/// dedup of redundant automatic captures — INCLUDING two session summaries from
/// near-identical workflows (desired, not a bug; the newest is kept). Genuinely
/// distinct summaries differ well below 0.97 under real embeddings and stay
/// separate; looser clustering is the scheduled consolidation job's role. Gated
/// to `origin_source == "hook"` on BOTH the new write and each candidate, so
/// user/cli/mcp/job memories are never touched.
const HOOK_NEAR_DUP_THRESHOLD: f32 = 0.97;
/// Max existing hook near-dups absorbed per write (bounds the extra reads).
const HOOK_NEAR_DUP_LIMIT: usize = 8;
/// Default window for the stats aggregate when a `Stats` request omits one.
const STATS_DEFAULT_WINDOW_DAYS: u32 = 30;
/// Hard cap on the stats window so a client cannot force an unbounded scan.
const STATS_MAX_WINDOW_DAYS: u32 = 365;
/// Bound on the top-recalled id list in a stats reply (ids + counts only).
const STATS_TOP_RECALLED_LIMIT: usize = 5;
/// Hard cap on the history chain walk PER DIRECTION (PRD HIST-3: "default
/// unbounded but capped by an internal safety limit" — an absent depth uses
/// this cap, and a client cannot exceed it). Also the cycle-defense backstop
/// alongside the store's visited set.
const HISTORY_MAX_DEPTH: u32 = 100;
/// Hard cap on TOTAL chain members in a history reply. Depth bounds hops, not
/// width: near-dup fan-in can point many predecessors at one id, so one hop
/// could otherwise balloon the response. Overflow is reported via `truncated`.
const HISTORY_CHAIN_LIMIT: usize = 200;
/// Hard cap on the edge list in a history reply, so a heavily linked chain
/// cannot balloon one response (the stats window-clamp convention).
const HISTORY_EDGE_LIMIT: usize = 200;
/// Maximum number of simultaneous client connections.
const MAX_CONNECTIONS: usize = 256;
/// Oplog rows fetched per replay batch on a `subscribe --since` reconnect.
const OPLOG_REPLAY_BATCH: usize = 500;
/// Idle deadline for the initial handshake read (fail fast on stalled connects).
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Default idle deadline between consecutive request frames from an established
/// client; overridable via [`DaemonConfig::request_idle_timeout`] (the serve
/// binary resolves `RUSTY_BRAIN_IDLE_TIMEOUT_SECS` / config-file
/// `idle_timeout_secs` into it) so the idle/reconnect e2e runs in seconds.
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
///
/// C1 hermeticity rule: this library NEVER reads env vars or the user config
/// file for its knobs — the serve binary resolves them (CLI flag > env >
/// `~/.config/rusty-brain/config.toml` > default via `rb_config`) and passes
/// the result here, so embedded/test daemons cannot be steered by a
/// developer's real environment or config file.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub read_pool_size: usize,
    pub jobs_config: JobsConfig,
    /// Resolved `[retention]` policy from the user config (retention PRD).
    /// `None` = retention unconfigured: the scheduled job never spawns, the
    /// `RunJob(retention)` arm is a zero-work no-op, and the daemon-side
    /// policy is absent for stats. Following the C1 rule the daemon library
    /// never reads the config file itself — the serve binary resolves this.
    pub retention_policy: Option<rb_types::RetentionPolicy>,
    /// Per-connection request idle timeout between request frames; `None`
    /// uses the built-in 60s default.
    pub request_idle_timeout: Option<std::time::Duration>,
    /// Opt-in LLM enrichment endpoint; `None` falls back to heuristic
    /// enrichment. The API key (`RB_ENRICH_API_KEY`) is read from the daemon
    /// process env at bind — secrets are env-only and kept out of this
    /// `Debug`-printable struct.
    pub enrich: Option<EnrichEndpoint>,
    /// Recall fusion strategy (W2.2: `RB_FUSION_MODE` / `search.fusion`).
    /// `FusionMode::Linear` is the default; the default flip to RRF is
    /// deferred to W4.1 eval evidence.
    pub fusion_mode: rb_engine::FusionMode,
    /// Opt-in loopback HTTP listener (HTTP PRD HTTP-1/HTTP-2). `None` (the
    /// default posture) means ZERO footprint: no TCP socket is bound and no
    /// listener task is spawned. The bind address is re-validated at
    /// [`Daemon::bind`] — a non-loopback address fails closed even if the
    /// caller skipped config-layer validation. See docs/THREAT_MODEL.md,
    /// "The opt-in HTTP listener".
    pub http: Option<crate::http::HttpListenerConfig>,
}

/// An OpenAI-compatible enrichment endpoint (no credentials here; see
/// [`DaemonConfig::enrich`]).
#[derive(Clone, Debug)]
pub struct EnrichEndpoint {
    pub base_url: String,
    pub model: String,
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
    retention_policy: Option<rb_types::RetentionPolicy>,
    request_idle_timeout: std::time::Duration,
    fusion_mode: rb_engine::FusionMode,
    /// Bound opt-in HTTP listener + its config; `None` when disabled (the
    /// default): nothing is bound and `run` spawns no HTTP task.
    http: Option<(tokio::net::TcpListener, crate::http::HttpListenerConfig)>,
    /// Actual bound HTTP address (resolves port 0), for logs and callers.
    http_addr: Option<std::net::SocketAddr>,
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

        // Fail closed on a non-loopback HTTP bind BEFORE any filesystem or
        // socket work: the daemon re-validates the address itself and never
        // trusts the caller's config layer to have done so.
        if let Some(http_config) = config.http.as_ref() {
            crate::http::validate_bind(http_config)?;
        }

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

        // Opt-in HTTP listener (HTTP PRD): bound here (fail closed on any
        // bind error, e.g. a taken port), served in `run` beside the UDS
        // accept loop. Disabled means untouched: no socket, no task. Bound
        // BEFORE the store starts so a bind failure never strands the
        // writer thread.
        let (http, http_addr) = match config.http {
            None => (None, None),
            Some(http_config) => {
                let http_listener = crate::http::bind_listener(&http_config).await?;
                let addr = http_listener
                    .local_addr()
                    .map_err(|e| Error::Io(format!("http local_addr: {e}")))?;
                (Some((http_listener, http_config)), Some(addr))
            }
        };

        // Bind the embedder's model identity into every store open so a
        // same-dim provider swap fails closed instead of mixing vector spaces.
        let store = StoreHandle::start_with_model(
            config.db_path.clone(),
            dim,
            embedder.model_id().to_string(),
            config.read_pool_size,
        )?;

        // Build the opt-in LLM enricher once (reqwest client is reused across
        // all connections). Activation requires an endpoint in the config
        // (resolved by the serve binary from env > config.toml — C1); the API
        // key alone stays env-only (secret). Falls back to heuristic when no
        // endpoint is configured.
        let enricher: Option<Arc<dyn Enricher>> = match config.enrich.as_ref() {
            None => None,
            Some(endpoint) => match OpenAiCompatEnricher::from_settings(
                Some(endpoint.base_url.as_str()),
                Some(endpoint.model.as_str()),
                std::env::var("RB_ENRICH_API_KEY").ok().as_deref(),
            ) {
                Ok(Some(e)) => {
                    info!("LLM enrichment active");
                    Some(Arc::new(e))
                }
                Ok(None) => None,
                Err(e) => {
                    warn!(error = %e, "failed to build LLM enricher; falling back to heuristic");
                    None
                }
            },
        };

        // Fixed at bind so every connection of this daemon instance agrees.
        let request_idle_timeout = config
            .request_idle_timeout
            .unwrap_or(DEFAULT_REQUEST_IDLE_TIMEOUT);

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
            retention_policy: config.retention_policy,
            request_idle_timeout,
            fusion_mode: config.fusion_mode,
            http,
            http_addr,
        })
    }

    /// The actual bound address of the opt-in HTTP listener (port 0 resolved),
    /// or `None` when HTTP is disabled — the zero-footprint contract.
    #[must_use]
    pub fn http_addr(&self) -> Option<std::net::SocketAddr> {
        self.http_addr
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
            retention_policy,
            request_idle_timeout,
            fusion_mode,
            http,
            http_addr: _http_addr,
        } = self;
        tokio::pin!(shutdown);
        // Writer-death signal (W1.6c): resolves only on an ABNORMAL writer
        // exit. Raced in the select! below so a daemon whose writer is gone
        // shuts down instead of zombieing — ponging Ping while every write
        // fails with "writer thread unavailable" (F17).
        let writer_died = store.writer_died();
        tokio::pin!(writer_died);
        let scheduler =
            jobs::scheduler::spawn(store.clone(), jobs_config.clone(), retention_policy.clone());
        let mut conns: JoinSet<()> = JoinSet::new();
        let conn_sem = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        // Daemon-lifetime recall channel counters, shared by every connection
        // (UDS and HTTP alike — one status truth).
        let recall_counters = Arc::new(RecallChannelCounters::default());

        // Opt-in HTTP listener task, beside the UDS accept loop. Zero
        // footprint when disabled: no channel subscriber, no task. Shutdown
        // is signalled through the watch channel below and JOINED before the
        // store shuts down, so the HTTP path is covered by graceful shutdown.
        let (http_shutdown_tx, http_shutdown_rx) = tokio::sync::watch::channel(false);
        let http_task = http.map(|(http_listener, http_config)| {
            let state = Arc::new(crate::http::HttpState {
                store: store.clone(),
                embedder: embedder.clone(),
                enricher: enricher.clone(),
                jobs_config: jobs_config.clone(),
                retention_policy: retention_policy.clone(),
                recall_counters: recall_counters.clone(),
                fusion_mode,
                provider_model: embedder.model_id().to_string(),
            });
            tokio::spawn(crate::http::run(
                http_listener,
                http_config,
                state,
                http_shutdown_rx,
            ))
        });

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
                            let retention_policy = retention_policy.clone();
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
                                    retention_policy,
                                    request_idle_timeout,
                                    recall_counters,
                                    fusion_mode,
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
        // Stop the HTTP listener FIRST and wait for it to finish: its accept
        // loop drops the TCP socket and aborts its connections, so no HTTP
        // request can race the store shutdown below.
        let _ = http_shutdown_tx.send(true);
        if let Some(task) = http_task {
            if let Err(e) = task.await {
                warn!(error = %e, "http listener task failed during shutdown");
            }
        }
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

/// Kernel-verified identity of a connecting peer (W2.6): `getpeereid` on
/// macOS/BSD, `SO_PEERCRED` on Linux, both via tokio's `peer_cred`. This — not
/// the client-declared handshake identity — is the connection's principal for
/// authorization decisions. The handshake identity remains provenance
/// metadata only.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PeerIdentity {
    uid: Option<u32>,
}

impl PeerIdentity {
    /// A peer with NO kernel-verified credential — never admin (fail
    /// closed). This is every HTTP connection's identity: TCP loopback has
    /// no `SO_PEERCRED`/`getpeereid`, so the HTTP surface is gated exactly
    /// like a UDS connection whose peer credentials could not be read.
    pub(crate) fn untrusted() -> Self {
        Self { uid: None }
    }

    fn from_stream(stream: &UnixStream) -> Self {
        match stream.peer_cred() {
            Ok(cred) => Self {
                uid: Some(cred.uid()),
            },
            Err(e) => {
                warn!(error = %e, "could not read peer credentials; treating peer as non-admin");
                Self { uid: None }
            }
        }
    }

    /// Admin = the kernel-verified peer uid equals the daemon's effective uid.
    /// Unreadable peer credentials are never admin (fail closed). Root is NOT
    /// special-cased: a root client gets no implicit admin grant from us (it
    /// does not need one — it can read the DB file directly).
    fn is_admin(&self) -> bool {
        self.uid.is_some_and(|uid| uid == process_euid())
    }
}

fn process_euid() -> u32 {
    // SAFETY: geteuid takes no arguments, cannot fail, and touches no memory.
    #[allow(unsafe_code)]
    unsafe {
        libc::geteuid()
    }
}

/// Whether a request is a cross-namespace maintenance (admin) operation.
/// These mutate or scan EVERY namespace, so they are gated on the
/// kernel-verified peer identity rather than the handshake namespace.
///
/// EXHAUSTIVE on purpose (no `_` arm): a newly added `Request` variant fails
/// to compile here until its author makes a deliberate admin/non-admin
/// decision. A wildcard default would silently ship a new cross-namespace op
/// ungated, defeating the W2.6 admin boundary.
fn is_admin_op(req: &Request) -> bool {
    match req {
        // Cross-namespace maintenance: peer-gated.
        Request::RunJob { .. }
        | Request::Reembed { .. }
        | Request::NamespaceRename { .. }
        | Request::Scrub => true,
        // Forget: only the HARD EXECUTE purges irreversibly — peer-gated
        // like Scrub. Apply-execute archives (reversible, namespace-scoped
        // like Delete) and dry-runs of either mode are read-only, so none of
        // those are admin.
        Request::Forget { mode, dry_run, .. } => *mode == rb_types::ForgetMode::Hard && !dry_run,
        // Namespace-scoped by the handshake: not admin. Review/Resolve
        // (PRD 2026-07-02) are deliberately non-admin: every review action is
        // reversible (supersede/archive are soft, demote is an update) and
        // namespace-scoped — the Forget APPLY precedent, not the hard purge.
        Request::Remember { .. }
        | Request::Recall { .. }
        | Request::Get { .. }
        | Request::List { .. }
        | Request::Graph { .. }
        | Request::Update { .. }
        | Request::Link { .. }
        | Request::Feedback { .. }
        | Request::Delete { .. }
        | Request::Context
        | Request::Subscribe { .. }
        | Request::Stats { .. }
        | Request::History { .. }
        | Request::Review { .. }
        | Request::Resolve { .. }
        | Request::Ping => false,
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    stream: UnixStream,
    store: StoreHandle,
    embedder: SharedEmbedder,
    enricher: Option<Arc<dyn Enricher>>,
    jobs_config: JobsConfig,
    retention_policy: Option<rb_types::RetentionPolicy>,
    request_idle_timeout: std::time::Duration,
    recall_counters: Arc<RecallChannelCounters>,
    fusion_mode: rb_engine::FusionMode,
) -> Result<()> {
    // W2.6 peer identity: read the kernel-verified peer credentials
    // (`getpeereid`/`SO_PEERCRED` via tokio's `peer_cred`) BEFORE any frame is
    // parsed. This is the connection's principal — nothing the client declares
    // in the handshake can claim it. Admin ops are gated on it below;
    // fail-closed: an unreadable peer cred is a non-admin connection.
    let peer = PeerIdentity::from_stream(&stream);

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
            capabilities: vec![],
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
                capabilities: vec![],
            };
            write_frame(&mut framed, &ack).await?;
            return Ok(());
        }
    };
    let ack = HandshakeAck {
        contract_version: CONTRACT_VERSION,
        ok: true,
        message: None,
        // Typed code anchors (PRD 2026-07-02): this daemon evaluates anchor
        // payloads, so advertise the capability — clients gate anchor-bearing
        // requests on it (pre-anchor daemons never send it). Old clients
        // ignore the additive field.
        capabilities: vec![rb_proto::CAP_ANCHORS.to_string()],
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
    // Snapshot the provider identity before the embedder moves into the
    // engine; the stats path reports it alongside the DB's recorded model.
    let provider_model = embedder.model_id().to_string();
    let engine = {
        let base =
            MemoryEngine::new(store, embedder, namespace.clone()).with_fusion_mode(fusion_mode);
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
        if let Request::Subscribe { since } = req {
            stream_changes(&mut framed, &store_for_stream, &namespace, since).await;
            break;
        }
        let resp = dispatch(
            &engine,
            &job_store,
            &jobs_config,
            retention_policy.as_ref(),
            &provenance,
            &recall_counters,
            &namespace,
            &provider_model,
            peer,
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
    since: Option<u64>,
) {
    let mut rx = store.subscribe();
    // Acknowledge the subscription now that the broadcast receiver is registered.
    // The client blocks in `subscribe()` until it sees this frame, so it cannot
    // commit (or unblock a peer that commits) a change that races ahead of an
    // active receiver and is silently missed.
    if write_frame(framed, &Response::SubscribeAck).await.is_err() {
        return; // client already gone
    }
    // W2.7 replay-on-reconnect: with a cursor, replay every oplog change after
    // it BEFORE live streaming. The broadcast receiver above is already
    // registered, so a write landing during replay is buffered, not lost; the
    // `replayed_to` watermark below drops the overlap (a live event whose seq
    // the replay already delivered).
    let mut replayed_to = since.unwrap_or(0);
    if let Some(cursor) = since {
        let mut after = cursor;
        loop {
            let page = match store
                .oplog_changes_since(namespace.clone(), after, OPLOG_REPLAY_BATCH)
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    // Fail open to live-only streaming: the consumer keeps its
                    // cursor and can retry; killing the stream would lose more.
                    warn!(error = %e, "oplog replay failed; continuing with live stream only");
                    break;
                }
            };
            // Paginate on ROWS SCANNED, not events emitted: a page can return
            // fewer events than scanned rows (bulk-admin rows are filtered),
            // so `changes.len() < BATCH` would terminate early and drop every
            // change after a page that contained filtered rows (W2.7 bug #2).
            let done = page.scanned < OPLOG_REPLAY_BATCH;
            // Advance the dedup watermark to the highest seq SCANNED (not just
            // emitted): every overlap event with seq ≤ this was covered by the
            // replay, so the live loop can safely drop it. No live event ever
            // carries a filtered bulk-row seq (those publish no broadcast).
            replayed_to = replayed_to.max(page.last_seq);
            for evt in page.changes {
                if write_frame(framed, &Response::Change(evt)).await.is_err() {
                    return; // client disconnected mid-replay
                }
            }
            if done {
                break;
            }
            // Advance past every SCANNED row (incl. skipped ones) or a full
            // page of skipped rows would re-scan forever / drop later rows.
            after = page.last_seq;
        }
    }
    loop {
        match rx.recv().await {
            Ok(evt) => {
                if &evt.namespace != namespace {
                    continue; // cross-namespace event: never leak it
                }
                // Overlap dedup: this event committed before (or during) the
                // replay window and was already delivered from the oplog. An
                // event without a seq cannot be compared — deliver it
                // (over-notify is safe; silent drop is not).
                if evt.seq.is_some_and(|s| s <= replayed_to) {
                    continue;
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

pub(crate) fn validate_namespace(namespace: rb_types::Namespace) -> Result<rb_types::Namespace> {
    let encoded = namespace.as_db_string();
    let parsed = rb_types::Namespace::parse_db_string(&encoded)?;
    if parsed == namespace {
        Ok(namespace)
    } else {
        Err(Error::InvalidNamespace(encoded))
    }
}

/// W3.1 write-time near-dup suppression. After a hook-sourced memory `new_id`
/// is stored, absorb every EXISTING active hook capture whose vector is a
/// near-duplicate (cosine >= [`HOOK_NEAR_DUP_THRESHOLD`]) into it by superseding
/// the older row through the existing atomic supersede. Strictly best-effort and
/// bounded ([`HOOK_NEAR_DUP_LIMIT`] candidates): every read/write error is logged
/// and skipped, and ONLY `origin_source == "hook"` candidates are superseded —
/// user/cli/mcp/job memories are never collapsed automatically. Namespace
/// isolation is guaranteed by both `near_duplicates` and the engine read.
async fn suppress_hook_near_duplicates<P>(
    engine: &MemoryEngine<StoreHandle, P>,
    job_store: &StoreHandle,
    namespace: &rb_types::Namespace,
    new_id: &rb_types::MemoryId,
) where
    P: EmbeddingProvider,
{
    let dups = match job_store
        .near_duplicates(
            namespace.clone(),
            new_id.clone(),
            HOOK_NEAR_DUP_THRESHOLD,
            HOOK_NEAR_DUP_LIMIT,
        )
        .await
    {
        Ok(dups) => dups,
        Err(e) => {
            warn!(error = %e, "near-dup scan failed; skipping write-time suppression");
            return;
        }
    };
    for (dup_id, _similarity) in dups {
        if &dup_id == new_id {
            continue;
        }
        // Only collapse OTHER automatic hook captures into the new memory; a
        // non-hook (or already-archived/absent) candidate is left untouched.
        // `peek` (not `get`) so inspecting a candidate's provenance never
        // records an access — a maintenance scan must not pollute the W3.7
        // usefulness signal on memories it deliberately leaves alone.
        match engine.peek(dup_id.clone()).await {
            Ok(Some(note)) if note.origin_source.as_deref() == Some("hook") => {
                if let Err(e) = job_store
                    .supersede(namespace.clone(), dup_id, new_id.clone())
                    .await
                {
                    warn!(error = %e, "near-dup supersede failed; leaving the duplicate live");
                }
            }
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, "near-dup candidate fetch failed; skipping it");
            }
        }
    }
}

/// Apply ONE review resolution (PRD 2026-07-02 REV-2). Fail-closed order:
/// validate the action shape against the item's REASON and member ids
/// (merge is near-duplicate-only), re-validate the planned relationship at
/// resolve time (the PR #63 TOCTOU fix — a contradiction item must still be
/// an active contradiction pair; a near-dup merge re-proves similarity
/// inside the writer transaction), resolve every member through the
/// namespace-scoped engine (`peek` — a maintenance read must not pollute the
/// W3.7 access signal; a foreign or missing id is `NotFound` before any
/// write), mutate, then record the `review_resolve` audit row (REV-4).
///
/// Merge is ONE writer transaction end to end (the PR #63 atomicity fix):
/// the combined memory is COMPOSED through the engine (validated, enriched,
/// embedded, provenance-stamped — `MemoryEngine::compose_note`, the same
/// construction `remember` commits) and handed to
/// `StoreHandle::review_merge`, whose single store transaction re-validates,
/// inserts, copies the originals' external edges, supersedes both originals
/// behind the pointer guard, and writes the audit row. Any failure rolls the
/// WHOLE merge back — no orphaned combined memory, no split chain; a raced
/// resolution loses with the distinct [`Error::StalePlan`], which the policy
/// sweep treats as skip-and-continue.
#[allow(clippy::too_many_arguments)] // the resolve context is irreducible here
async fn apply_review_action<P>(
    engine: &MemoryEngine<StoreHandle, P>,
    job_store: &StoreHandle,
    provenance: &rb_engine::Provenance,
    namespace: &rb_types::Namespace,
    reason: rb_types::ReviewReason,
    ids: &[rb_types::MemoryId],
    action: &rb_types::ReviewAction,
    threshold: f32,
) -> Result<rb_types::ReviewResolution>
where
    P: EmbeddingProvider,
{
    use rb_types::{MemberConfidence, ReviewAction};

    action.validate(reason, ids)?;
    let key = rb_types::review_item_key(reason, ids);

    // Resolve-time revalidation of the claimed relationship (PR #63 TOCTOU
    // fix). Contradiction items are re-proved here for EVERY action; a
    // near-dup MERGE is re-proved inside the atomic store transaction below
    // (where the guard is race-free). Non-merge actions on other reasons are
    // individually-targeted, reversible primitives.
    if reason == rb_types::ReviewReason::Contradiction {
        if let [a, b] = ids {
            let still = job_store
                .contradiction_pair_active(namespace.clone(), a.clone(), b.clone())
                .await?;
            if !still {
                return Err(Error::StalePlan(format!(
                    "{a} and {b} no longer form an active contradiction; re-run \
                     review for a fresh queue"
                )));
            }
        }
    }

    let mut notes = Vec::with_capacity(ids.len());
    for id in ids {
        match engine.peek(id.clone()).await? {
            Some(note) => notes.push(note),
            None => return Err(Error::NotFound(id.clone())),
        }
    }

    let mut resolution = rb_types::ReviewResolution {
        key: key.clone(),
        action: action.kind_str().to_string(),
        ..Default::default()
    };

    match action {
        ReviewAction::Keep { bump } => {
            if *bump {
                for note in &notes {
                    let confidence = (note.confidence + rb_types::REVIEW_KEEP_BUMP).clamp(0.0, 1.0);
                    engine
                        .update(
                            note.id.clone(),
                            rb_types::MemoryUpdates {
                                confidence: Some(confidence),
                                ..Default::default()
                            },
                        )
                        .await?;
                    resolution.confidence.push(MemberConfidence {
                        id: note.id.clone(),
                        confidence,
                    });
                }
            }
        }
        ReviewAction::Merge => {
            let (a, b) = (&notes[0], &notes[1]);
            // Deterministic combine: identical contents collapse to one;
            // otherwise both bodies are kept, first member first. Metadata
            // keeps the strongest signal (max importance/confidence) and the
            // union of keywords/tags/files/anchors; the graph edges are
            // unioned inside the store transaction.
            let content = if a.content == b.content {
                a.content.clone()
            } else {
                format!("{}\n\n---\n\n{}", a.content, b.content)
            };
            let mut keywords = a.keywords.clone();
            for k in &b.keywords {
                if !keywords.contains(k) {
                    keywords.push(k.clone());
                }
            }
            let mut tags = a.tags.clone();
            for t in &b.tags {
                if !tags.contains(t) {
                    tags.push(t.clone());
                }
            }
            let mut related_files = a.related_files.clone();
            for f in &b.related_files {
                if !related_files.contains(f) {
                    related_files.push(f.clone());
                }
            }
            let mut anchors = a.anchors.clone();
            for an in &b.anchors {
                if !anchors.contains(an) {
                    anchors.push(an.clone());
                }
            }
            let context = [&a.context, &b.context]
                .into_iter()
                .find(|c| !c.is_empty())
                .cloned();
            let memory_type = if b.importance > a.importance {
                b.memory_type
            } else {
                a.memory_type
            };
            let (note, embedding) = engine
                .compose_note(rb_engine::RememberInput {
                    content,
                    context,
                    memory_type,
                    importance: a.importance.max(b.importance),
                    keywords,
                    tags,
                    related_files,
                    confidence: Some(a.confidence.max(b.confidence)),
                    provenance: provenance.clone(),
                    anchors,
                })
                .await?;
            let new_id = note.id.clone();
            let details = serde_json::json!({
                "key": key,
                "action": "merge",
                "ids": ids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
                "merged_into": new_id.to_string(),
            })
            .to_string();
            job_store
                .review_merge(
                    namespace.clone(),
                    a.id.clone(),
                    b.id.clone(),
                    threshold,
                    note,
                    embedding,
                    details,
                )
                .await?;
            resolution.merged_into = Some(new_id);
            // The audit row was written INSIDE the merge transaction.
            return Ok(resolution);
        }
        ReviewAction::Archive { id } => {
            engine.delete(id.clone()).await?;
        }
        ReviewAction::Demote { id } => {
            // `validate` proved membership; the note is in `notes`.
            if let Some(note) = notes.iter().find(|n| &n.id == id) {
                let confidence = (note.confidence - rb_types::REVIEW_DEMOTE_STEP).clamp(0.0, 1.0);
                engine
                    .update(
                        id.clone(),
                        rb_types::MemoryUpdates {
                            confidence: Some(confidence),
                            ..Default::default()
                        },
                    )
                    .await?;
                resolution.confidence.push(MemberConfidence {
                    id: id.clone(),
                    confidence,
                });
            }
        }
        ReviewAction::Snooze { days } => {
            let details = serde_json::json!({
                "key": key,
                "action": "snooze",
                "days": days,
            })
            .to_string();
            // The snooze op writes its own `review_resolve` oplog row inside
            // the same transaction as the review_state upsert.
            let until = job_store
                .review_snooze(namespace.clone(), key.clone(), *days, details)
                .await?;
            resolution.snoozed_until = Some(until);
            return Ok(resolution);
        }
    }

    // REV-4: every non-snooze resolution stamps reviewed_at (clearing any
    // snooze — the user acted) and appends one `review_resolve` oplog row.
    // The per-memory mutations above already wrote their own oplog rows
    // through the existing primitives.
    let details = serde_json::json!({
        "key": key,
        "action": action.kind_str(),
        "ids": ids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
    })
    .to_string();
    job_store
        .record_review_resolution(namespace.clone(), key, details)
        .await?;
    Ok(resolution)
}

/// Execute an already-generated review plan under `policy` (the apply half of
/// `run_review_sweep`, split out so plan staleness is testable): derive each
/// item's action through the pure `ReviewPolicy::plan_action` (the SAME
/// mapping a dry-run shows) and apply it via [`apply_review_action`].
///
/// Failure semantics (PR #60 partial-outcome shape + the PR #63 refinement):
/// a distinct [`Error::StalePlan`] — the item was resolved concurrently or
/// its relationship dissolved between plan and apply — is a benign collision:
/// counted as `skipped`, the pass continues. Any OTHER error stops the pass
/// re-runnably; completed items stay committed. The bulk `review_sweep` oplog
/// row is written unconditionally.
async fn execute_review_plan<P>(
    engine: &MemoryEngine<StoreHandle, P>,
    job_store: &StoreHandle,
    provenance: &rb_engine::Provenance,
    namespace: &rb_types::Namespace,
    policy: rb_types::ReviewPolicy,
    plan: &rb_types::ReviewPlan,
    threshold: f32,
) -> Result<rb_types::ReviewOutcome>
where
    P: EmbeddingProvider,
{
    let mut outcome = rb_types::ReviewOutcome {
        policy: Some(policy),
        total_items: plan.items.len() as u64,
        ..Default::default()
    };
    let mut failure: Option<String> = None;
    for item in &plan.items {
        let Some(action) = policy.plan_action(item) else {
            outcome.skipped += 1;
            continue;
        };
        let ids: Vec<rb_types::MemoryId> = item.members.iter().map(|m| m.id.clone()).collect();
        match apply_review_action(
            engine,
            job_store,
            provenance,
            namespace,
            item.reason,
            &ids,
            &action,
            threshold,
        )
        .await
        {
            Ok(_) => match action {
                rb_types::ReviewAction::Keep { .. } => outcome.kept += 1,
                rb_types::ReviewAction::Merge => outcome.merged += 1,
                rb_types::ReviewAction::Archive { .. } => outcome.archived += 1,
                rb_types::ReviewAction::Demote { .. } => outcome.demoted += 1,
                rb_types::ReviewAction::Snooze { .. } => outcome.snoozed += 1,
            },
            Err(Error::StalePlan(reason)) => {
                // Benign collision: someone resolved the item (or its
                // relationship dissolved) between plan and apply. Skip it —
                // the rest of the batch must still complete.
                tracing::debug!(key = %item.key, %reason, "review item went stale; skipping");
                outcome.skipped += 1;
            }
            Err(e) => {
                // The failed item's own steps rolled back or stand alone;
                // earlier items are committed. Stop (re-runnable), report
                // the partial pass.
                failure = Some(format!("{} of {} failed: {e}", action.kind_str(), item.key));
                break;
            }
        }
    }
    // The bulk row records the RUN unconditionally (the retention_sweep
    // precedent): a zero-change or partial pass is still a durable, auditable
    // run. A failure recording it never erases the completed batch.
    let details = serde_json::json!({
        "policy": policy.as_str(),
        "merged": outcome.merged,
        "archived": outcome.archived,
        "demoted": outcome.demoted,
        "kept": outcome.kept,
        "snoozed": outcome.snoozed,
        "skipped": outcome.skipped,
        "total_items": outcome.total_items,
        "failure": failure,
    })
    .to_string();
    if let Err(e) = job_store
        .record_review_sweep(namespace.clone(), details)
        .await
    {
        let msg = format!("recording the sweep run: {e}");
        failure = Some(match failure {
            Some(prior) => format!("{prior}; {msg}"),
            None => msg,
        });
    }
    outcome.failure = failure;
    Ok(outcome)
}

/// One bounded review apply pass (REV-3 `--apply --policy`): recompute the
/// queue, then execute it via [`execute_review_plan`].
async fn run_review_sweep<P>(
    engine: &MemoryEngine<StoreHandle, P>,
    job_store: &StoreHandle,
    provenance: &rb_engine::Provenance,
    namespace: &rb_types::Namespace,
    policy: rb_types::ReviewPolicy,
    params: rb_store::ReviewQueueParams,
) -> Result<rb_types::ReviewOutcome>
where
    P: EmbeddingProvider,
{
    let plan = job_store.review_plan(namespace.clone(), params).await?;
    execute_review_plan(
        engine,
        job_store,
        provenance,
        namespace,
        policy,
        &plan,
        params.threshold,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch<P>(
    engine: &MemoryEngine<StoreHandle, P>,
    job_store: &StoreHandle,
    jobs_config: &JobsConfig,
    retention_policy: Option<&rb_types::RetentionPolicy>,
    provenance: &rb_engine::Provenance,
    recall_counters: &RecallChannelCounters,
    namespace: &rb_types::Namespace,
    provider_model: &str,
    peer: PeerIdentity,
    req: Request,
) -> Response
where
    P: EmbeddingProvider,
{
    // W2.6: cross-namespace maintenance ops are admin-gated on the
    // kernel-verified peer identity. Everything else stays namespace-scoped by
    // the handshake (namespace is NOT an auth boundary — see
    // docs/THREAT_MODEL.md).
    if is_admin_op(&req) && !peer.is_admin() {
        return error_to_response(Error::PermissionDenied(
            "this is an admin operation: it requires a kernel-verified peer \
             uid matching the daemon's user"
                .to_string(),
        ));
    }
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
            supersedes,
            anchors,
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
                anchors,
            };
            match engine.remember(input).await {
                Ok(id) => {
                    // W3.1 update-as-supersede: store the replacement first, then
                    // archive the prior memory through the existing atomic
                    // supersede (separate writer tx). Best-effort — a supersede
                    // failure leaves the new memory stored and `old` un-archived,
                    // never a partial state. That includes the #501 guard misses
                    // (StalePlan: `old` was already resolved by an earlier fold or
                    // another writer) — the established lineage pointer is kept
                    // and only this warn records the refusal. `old == id` is
                    // impossible (the id is freshly minted) but guarded for
                    // clarity.
                    if let Some(old) = supersedes {
                        if old != id {
                            if let Err(e) = job_store
                                .supersede(namespace.clone(), old, id.clone())
                                .await
                            {
                                warn!(error = %e, "supersede after remember failed (new memory kept)");
                            }
                        }
                    }
                    // W3.1 write-time near-dup suppression: collapse OTHER active
                    // hook captures that are near-identical to this one, so
                    // automatic capture cannot pile up redundant rows between
                    // scheduled consolidation runs. Gated to hook-source on the
                    // write AND each candidate; never touches user/cli/mcp/job rows.
                    if provenance.origin_source.as_deref() == Some("hook") {
                        suppress_hook_near_duplicates(engine, job_store, namespace, &id).await;
                    }
                    Response::Remembered { id }
                }
                Err(e) => error_to_response(e),
            }
        }
        Request::Recall {
            query,
            memory_type,
            tags,
            limit,
            filter,
        } => match engine
            .recall_with_status(
                &query,
                limit.min(MAX_LIMIT),
                // One effective filter: the legacy wire slots (type/tags) fold
                // into the additive unified filter (PRD 2026-07-02 parity), so
                // old and new clients hit the same engine path.
                &filter.fold_recall_legacy(memory_type, tags),
            )
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
            filter,
        } => match engine
            .list(
                // The legacy min_importance slot folds into the unified filter
                // (stricter bound wins), mirroring the Recall dispatch above.
                &filter.fold_list_legacy(min_importance),
                limit.min(MAX_LIMIT),
            )
            .await
        {
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
        Request::Subscribe { .. } => error_to_response(Error::InvalidArgument(
            "Subscribe is a streaming op, not a single request".to_string(),
        )),
        Request::RunJob { job } => {
            match jobs::run_once(job, job_store, jobs_config, retention_policy).await {
                Ok(summary) => Response::JobRan {
                    scanned: summary.scanned,
                    changed: summary.changed,
                    skipped: summary.skipped,
                },
                Err(e) => error_to_response(e),
            }
        }
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
        Request::Link {
            from,
            to,
            link_type,
            reason,
        } => match engine.link(from, to, link_type, reason).await {
            Ok(()) => Response::Linked,
            Err(e) => error_to_response(e),
        },
        // Namespace-scoped usefulness signal (W3.7): the engine verifies the
        // memory lives in this connection's namespace; `provenance` supplies the
        // giver (`origin_user`) recorded for the W5c per-author rollup.
        Request::Feedback { id, kind } => match engine.feedback(id, kind, provenance).await {
            Ok(confidence) => Response::FeedbackRecorded { confidence },
            Err(e) => error_to_response(e),
        },
        // Namespace-scoped read-only observability aggregate (doctor/stats
        // PRD). Runs entirely on the read pool — zero writer ops (W1.8). The
        // window is clamped server-side (never trusted raw); the re-embed
        // backlog compares against the LIVE provider identity and the current
        // input composition, mirroring the reembed scan.
        Request::Stats { window_days } => {
            let window = window_days
                .unwrap_or(STATS_DEFAULT_WINDOW_DAYS)
                .clamp(1, STATS_MAX_WINDOW_DAYS);
            match job_store
                .namespace_stats(
                    namespace.clone(),
                    window,
                    provider_model.to_string(),
                    rb_engine::EMBEDDING_INPUT_VERSION.to_string(),
                    STATS_TOP_RECALLED_LIMIT,
                    // RET-4: eligible-forgetting gauge under the daemon's
                    // resolved [retention] policy (None when unconfigured).
                    retention_policy.cloned(),
                )
                .await
            {
                Ok(stats) => Response::Stats {
                    stats,
                    provider_model: provider_model.to_string(),
                    writer_alive: job_store.writer_alive(),
                },
                Err(e) => error_to_response(e),
            }
        }
        // Retention/forget pass (retention PRD RET-2), namespace-scoped by
        // the handshake. Hard-EXECUTE was peer-gated above (is_admin_op);
        // everything below holds for any peer. The wire policy is validated
        // fail-closed FIRST — never trust a client-supplied policy — and the
        // execute path additionally requires enabled=true here (the store
        // refuses a disabled sweep too: defense in depth for off-by-default).
        Request::Forget {
            policy,
            mode,
            dry_run,
        } => {
            if let Err(e) = policy.validate() {
                return error_to_response(e);
            }
            if dry_run {
                match job_store
                    .retention_plan(namespace.clone(), policy, mode)
                    .await
                {
                    Ok(plan) => Response::ForgetPlanned { plan },
                    Err(e) => error_to_response(e),
                }
            } else if !policy.enabled {
                error_to_response(Error::InvalidArgument(
                    "retention is not enabled (retention.enabled = false): refusing to \
                     mutate; use a dry-run to preview, or enable it in [retention]"
                        .to_string(),
                ))
            } else {
                match job_store
                    .retention_sweep(namespace.clone(), policy, mode)
                    .await
                {
                    Ok(outcome) => Response::ForgetDone { outcome },
                    Err(e) => error_to_response(e),
                }
            }
        }
        // Namespace-scoped read-only decision-history timeline (PRD
        // 2026-07-02). Runs entirely on the read pool — zero writer ops
        // (W1.8). The depth is clamped server-side (never trusted raw) and
        // the chain/edge lists are capped; namespace purity (both chain hops
        // and edge endpoints) is enforced inside the store query.
        Request::History { id, depth } => {
            let depth = depth
                .unwrap_or(HISTORY_MAX_DEPTH)
                .clamp(1, HISTORY_MAX_DEPTH);
            match job_store
                .memory_history(
                    namespace.clone(),
                    id,
                    depth,
                    HISTORY_CHAIN_LIMIT,
                    HISTORY_EDGE_LIMIT,
                )
                .await
            {
                Ok(history) => Response::History { history },
                Err(e) => error_to_response(e),
            }
        }
        // Guided review (PRD 2026-07-02), namespace-scoped by the handshake.
        // Dry-run generates the queue on the read pool (zero writer ops) and,
        // when a policy is named, derives the per-item plan through the pure
        // `ReviewPolicy::plan_action` — the SAME mapping the apply pass
        // executes, so preview and mutation cannot diverge (REV-3). Execute
        // requires an explicit policy (never auto-resolve without consent).
        // The knobs are clamped server-side: a wire threshold can never drop
        // below the conservative floor (false-positive merge risk).
        Request::Review {
            policy,
            dry_run,
            since,
            limit,
            threshold,
        } => {
            let params = rb_store::ReviewQueueParams {
                threshold: match threshold {
                    Some(t) if t.is_finite() => t.clamp(rb_types::REVIEW_MIN_THRESHOLD, 1.0),
                    _ => rb_types::REVIEW_DEFAULT_THRESHOLD,
                },
                limit: limit
                    .unwrap_or(rb_types::REVIEW_DEFAULT_LIMIT)
                    .clamp(1, rb_types::REVIEW_MAX_LIMIT) as usize,
                since,
            };
            if dry_run {
                match job_store.review_plan(namespace.clone(), params).await {
                    Ok(mut plan) => {
                        if let Some(policy) = policy {
                            plan.policy = Some(policy);
                            plan.planned = plan
                                .items
                                .iter()
                                .filter_map(|item| {
                                    policy.plan_action(item).map(|action| {
                                        rb_types::PlannedResolution {
                                            key: item.key.clone(),
                                            action,
                                        }
                                    })
                                })
                                .collect();
                        }
                        Response::ReviewPlanned { plan }
                    }
                    Err(e) => error_to_response(e),
                }
            } else {
                let Some(policy) = policy else {
                    return error_to_response(Error::InvalidArgument(
                        "review execute requires an explicit policy (REV-3: never \
                         auto-resolve without consent); name one or use the dry-run \
                         preview"
                            .to_string(),
                    ));
                };
                match run_review_sweep(engine, job_store, provenance, namespace, policy, params)
                    .await
                {
                    Ok(outcome) => Response::ReviewDone { outcome },
                    Err(e) => error_to_response(e),
                }
            }
        }
        // One per-item resolution (REV-2 interactive mode). Validation is
        // fail-closed at this boundary: the action shape against the item
        // reason and member ids, the resolve-time relationship revalidation
        // (PR #63 TOCTOU fix), then every member resolved through the
        // namespace-scoped engine (a foreign or missing id is NotFound
        // before any write). The merge-revalidation threshold is clamped
        // exactly like Review's.
        Request::Resolve {
            reason,
            ids,
            action,
            threshold,
        } => {
            let threshold = match threshold {
                Some(t) if t.is_finite() => t.clamp(rb_types::REVIEW_MIN_THRESHOLD, 1.0),
                _ => rb_types::REVIEW_DEFAULT_THRESHOLD,
            };
            match apply_review_action(
                engine, job_store, provenance, namespace, reason, &ids, &action, threshold,
            )
            .await
            {
                Ok(resolution) => Response::Resolved { resolution },
                Err(e) => error_to_response(e),
            }
        }
        // Admin op (peer-gated above): retroactive secret redaction across all
        // namespaces through the single writer (W2.4).
        Request::Scrub => match job_store.scrub().await {
            Ok(outcome) => Response::Scrubbed {
                scanned: outcome.scanned,
                redacted: outcome.redacted,
                reembed_pending: outcome.reembed_pending,
            },
            Err(e) => error_to_response(e),
        },
        Request::NamespaceRename { old, new, merge } => {
            // One-time admin op (W0.3 carryover), cross-namespace by nature
            // like RunJob/Reembed (peer-gated above, W2.6). Both
            // namespaces are round-trip validated before the writer sees them
            // so a malformed encoding can never land in the namespace column.
            match validate_namespace(old).and_then(|o| Ok((o, validate_namespace(new)?))) {
                Ok((old, new)) => match job_store.rename_namespace(old, new, merge).await {
                    Ok(outcome) => Response::NamespaceRenamed {
                        moved: outcome.memories,
                        vectors: outcome.vectors,
                    },
                    Err(e) => error_to_response(e),
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

    // W2.6: admin classification covers exactly the cross-namespace ops.
    #[test]
    fn admin_ops_are_runjob_reembed_namespace_rename_and_scrub() {
        assert!(is_admin_op(&Request::RunJob {
            job: rb_types::JobKind::LinkDecay
        }));
        assert!(is_admin_op(&Request::Reembed { limit: None }));
        assert!(is_admin_op(&Request::NamespaceRename {
            old: Namespace::Project("a".into()),
            new: Namespace::Project("b".into()),
            merge: false,
        }));
        // Scrub is the W2.4 cross-namespace admin op; it MUST be gated too.
        assert!(is_admin_op(&Request::Scrub));
        for not_admin in [
            Request::Ping,
            Request::Context,
            Request::Subscribe { since: None },
            Request::Recall {
                query: "q".into(),
                memory_type: None,
                tags: vec![],
                limit: 1,
                filter: rb_types::RecallFilter::default(),
            },
            // Stats is namespace-scoped by the handshake (read-only, W1.8),
            // like Context — deliberately NOT peer-gated.
            Request::Stats { window_days: None },
            // Forget apply-execute is namespace-scoped and reversible
            // (archive); dry-runs (either mode) are read-only. Only
            // hard-EXECUTE is admin (asserted below).
            Request::Forget {
                policy: rb_types::RetentionPolicy::default(),
                mode: rb_types::ForgetMode::Apply,
                dry_run: false,
            },
            Request::Forget {
                policy: rb_types::RetentionPolicy::default(),
                mode: rb_types::ForgetMode::Hard,
                dry_run: true,
            },
            // History is namespace-scoped and read-only too (PRD 2026-07-02),
            // like Get/Stats — deliberately NOT peer-gated.
            Request::History {
                id: rb_types::MemoryId::new(),
                depth: None,
            },
        ] {
            assert!(!is_admin_op(&not_admin), "{not_admin:?}");
        }
        // Hard-execute purges irreversibly: peer-gated like Scrub.
        assert!(is_admin_op(&Request::Forget {
            policy: rb_types::RetentionPolicy::default(),
            mode: rb_types::ForgetMode::Hard,
            dry_run: false,
        }));
    }

    // W2.6: the admin decision is the kernel-verified uid equaling the
    // daemon's euid — unreadable peer creds and foreign uids fail closed.
    #[test]
    fn peer_identity_admin_is_same_euid_and_fails_closed() {
        let me = process_euid();
        assert!(PeerIdentity { uid: Some(me) }.is_admin());
        assert!(!PeerIdentity {
            uid: Some(me.wrapping_add(1))
        }
        .is_admin());
        assert!(!PeerIdentity { uid: None }.is_admin());
    }

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
    fn unset_request_idle_timeout_uses_the_60s_default() {
        // C1: the daemon library takes the timeout from DaemonConfig only
        // (validation/parsing of env + config-file values lives in rb-config);
        // an unset field is the fail-safe 60s default.
        let config = DaemonConfig {
            socket_path: PathBuf::from("/unused/sock"),
            db_path: PathBuf::from("/unused/db"),
            read_pool_size: 1,
            jobs_config: JobsConfig::default(),
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_engine::FusionMode::Linear,
            http: None,
        };
        assert_eq!(
            config
                .request_idle_timeout
                .unwrap_or(DEFAULT_REQUEST_IDLE_TIMEOUT),
            std::time::Duration::from_secs(60)
        );
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
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_engine::FusionMode::Linear,
            http: None,
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
    async fn stats_over_the_wire_reflects_seeded_feedback_and_issues_zero_writer_ops() {
        // Daemon e2e for the PRD verification clause: a seeded feedback
        // distribution comes back through `Request::Stats`, and serving it
        // enqueues NOTHING on the writer thread (zero writer ops == zero FTS
        // writes; every FTS write rides a writer op).
        use rb_embed::DeterministicProvider;
        use rb_types::FeedbackKind;

        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let config = DaemonConfig {
            socket_path: dir.path().join("rb.sock"),
            db_path: dir.path().join("rb.db"),
            read_pool_size: 2,
            jobs_config: JobsConfig::default(),
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_engine::FusionMode::Linear,
            http: None,
        };
        let socket = config.socket_path.clone();
        let daemon = Daemon::bind(
            config,
            crate::SharedEmbedder::new(DeterministicProvider::new(8)),
        )
        .await
        .unwrap();
        let store = daemon.store.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn(daemon.run(async {
            let _ = shutdown_rx.await;
        }));

        let ns = Namespace::Project("stats-e2e".to_string());
        let mut client = rb_proto::Client::connect(&socket, ns.clone())
            .await
            .unwrap();
        let mut ids = Vec::new();
        for body in ["alpha decision", "beta pattern", "gamma insight"] {
            let id = client
                .remember(
                    body.to_string(),
                    None,
                    rb_types::MemoryType::Insight,
                    5,
                    vec![],
                    vec![],
                    vec![],
                    None,
                )
                .await
                .unwrap();
            ids.push(id);
        }
        // Seeded distribution: 2 helpful, 1 wrong, 0 stale.
        client
            .feedback(ids[0].clone(), FeedbackKind::Helpful)
            .await
            .unwrap();
        client
            .feedback(ids[1].clone(), FeedbackKind::Helpful)
            .await
            .unwrap();
        client
            .feedback(ids[2].clone(), FeedbackKind::Wrong)
            .await
            .unwrap();

        let ops_before = store.writer_ops_count();
        let (stats, provider_model, writer_alive) = client.stats(Some(14)).await.unwrap();
        assert_eq!(
            store.writer_ops_count(),
            ops_before,
            "the stats path must issue ZERO writer ops (and hence zero FTS writes)"
        );

        assert_eq!(stats.namespace, ns.as_db_string());
        assert_eq!(stats.window_days, 14);
        assert_eq!(stats.live, 3);
        assert_eq!(stats.archived, 0);
        assert_eq!(stats.vectors, 3);
        assert_eq!(stats.feedback.helpful, 2);
        assert_eq!(stats.feedback.wrong, 1);
        assert_eq!(stats.feedback.stale, 0);
        assert_eq!(stats.never_recalled_live, 3, "nothing was recalled");
        assert_eq!(stats.reembed_pending, 0, "stamps match the live provider");
        assert_eq!(stats.db_embedding_model.as_deref(), Some("deterministic"));
        assert_eq!(provider_model, "deterministic");
        assert!(writer_alive);

        // The window is clamped server-side, never trusted raw.
        let (clamped, _, _) = client.stats(Some(100_000)).await.unwrap();
        assert_eq!(
            clamped.window_days, 365,
            "oversized windows clamp to a year"
        );
        let (defaulted, _, _) = client.stats(None).await.unwrap();
        assert_eq!(defaulted.window_days, 30, "absent window uses the default");

        drop(client);
        let _ = shutdown_tx.send(());
        tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .expect("daemon must shut down")
            .expect("run task must not panic")
            .expect("run must return Ok");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn history_over_the_wire_walks_the_chain_and_issues_zero_writer_ops() {
        // Daemon e2e for the decision-history PRD verification clause: a
        // 3-deep supersede chain (A -> B -> C) plus a contradiction on the
        // head comes back through `Request::History` as the rendered
        // timeline, and serving it enqueues NOTHING on the writer thread.
        use rb_embed::DeterministicProvider;

        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let config = DaemonConfig {
            socket_path: dir.path().join("rb.sock"),
            db_path: dir.path().join("rb.db"),
            read_pool_size: 2,
            jobs_config: JobsConfig::default(),
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_engine::FusionMode::Linear,
            retention_policy: None,
            http: None,
        };
        let socket = config.socket_path.clone();
        let daemon = Daemon::bind(
            config,
            crate::SharedEmbedder::new(DeterministicProvider::new(8)),
        )
        .await
        .unwrap();
        let store = daemon.store.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let run = tokio::spawn(daemon.run(async {
            let _ = shutdown_rx.await;
        }));

        let ns = Namespace::Project("history-e2e".to_string());
        let mut client = rb_proto::Client::connect(&socket, ns.clone())
            .await
            .unwrap();
        let a = client
            .remember(
                "decision v1".to_string(),
                None,
                rb_types::MemoryType::ArchitectureDecision,
                7,
                vec![],
                vec![],
                vec![],
                None,
            )
            .await
            .unwrap();
        // B supersedes A, C supersedes B (the W3.1 update-as-supersede path).
        let b = client
            .remember_superseding(
                "decision v2".to_string(),
                None,
                rb_types::MemoryType::ArchitectureDecision,
                7,
                vec![],
                vec![],
                vec![],
                None,
                a.clone(),
            )
            .await
            .unwrap();
        let c = client
            .remember_superseding(
                "decision v3".to_string(),
                None,
                rb_types::MemoryType::ArchitectureDecision,
                7,
                vec![],
                vec![],
                vec![],
                None,
                b.clone(),
            )
            .await
            .unwrap();
        let rival = client
            .remember(
                "rival claim".to_string(),
                None,
                rb_types::MemoryType::Insight,
                5,
                vec![],
                vec![],
                vec![],
                None,
            )
            .await
            .unwrap();
        client
            .link(
                rival.clone(),
                c.clone(),
                rb_types::LinkType::Contradicts,
                Some("disputes v3".to_string()),
            )
            .await
            .unwrap();

        let ops_before = store.writer_ops_count();
        let history = client.history(c.clone(), None).await.unwrap();
        assert_eq!(
            store.writer_ops_count(),
            ops_before,
            "the history path must issue ZERO writer ops (and hence zero FTS writes)"
        );

        // Acceptance criteria: `history C` lists A -> B -> C with C flagged
        // current and A/B flagged superseded (archived).
        assert_eq!(history.namespace, ns.as_db_string());
        assert_eq!(history.depth, 100, "absent depth uses the safety cap");
        let ids: Vec<String> = history.chain.iter().map(|e| e.id.to_string()).collect();
        assert_eq!(
            ids,
            vec![a.to_string(), b.to_string(), c.to_string()],
            "oldest first: A -> B -> C"
        );
        assert!(history.chain[0].archived && history.chain[1].archived);
        assert!(history.chain[2].current && history.chain[2].is_target);
        assert!(!history.truncated);
        // The contradicts edge appears with the far memory's summary and the
        // contested marker on the head.
        assert!(history.chain[2].contested, "the head is contested");
        assert_eq!(history.edges.len(), 1);
        assert_eq!(history.edges[0].other.to_string(), rival.to_string());
        assert_eq!(history.edges[0].reason, "disputes v3");
        assert_eq!(history.edges[0].other_summary, "rival claim");

        // The depth is clamped server-side, never trusted raw.
        let clamped = client.history(c.clone(), Some(100_000)).await.unwrap();
        assert_eq!(clamped.depth, 100, "oversized depth clamps to the cap");
        let bounded = client.history(c.clone(), Some(1)).await.unwrap();
        assert_eq!(bounded.chain.len(), 2, "depth 1 keeps one ancestor hop");
        assert!(bounded.truncated);

        // A missing id errors cleanly: the daemon maps NotFound to the stable
        // `not_found` wire kind with the message preserved (the client
        // reconstructs unstructured kinds under Storage — the Get precedent).
        let err = client
            .history(rb_types::MemoryId::new(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err:?}");

        drop(client);
        let _ = shutdown_tx.send(());
        tokio::time::timeout(std::time::Duration::from_secs(10), run)
            .await
            .expect("daemon must shut down")
            .expect("run task must not panic")
            .expect("run must return Ok");
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

        let summary = run_once(JobKind::Consolidation, &store, &config, None)
            .await
            .unwrap();
        assert_eq!(
            summary.changed, 1,
            "the RunJob(Consolidation) path must merge the duplicate"
        );

        store.shutdown().await;
    }

    /// PR #63 MEDIUM: a policy sweep must skip-and-continue past a BENIGN
    /// stale-plan collision (an item resolved concurrently between plan and
    /// apply) instead of aborting the whole batch; real errors still stop
    /// the pass (proved by the mid-batch-failure e2e).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn review_sweep_skips_stale_items_and_completes_the_rest() {
        use rb_embed::DeterministicProvider;

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, 8, 2).unwrap();
        let ns = Namespace::Project("sweep-stale".to_string());
        let engine = rb_engine::MemoryEngine::new(
            handle.clone(),
            SharedEmbedder::new(DeterministicProvider::new(8)),
            ns.clone(),
        );
        let provenance = rb_engine::Provenance {
            origin_source: Some("cli".to_string()),
            ..Default::default()
        };

        // Two independent dup pairs => two planned merges.
        let mut ids = Vec::new();
        for content in [
            "first duplicated remark",
            "first duplicated remark",
            "second duplicated remark",
            "second duplicated remark",
        ] {
            let id = engine
                .remember(rb_engine::RememberInput {
                    content: content.to_string(),
                    context: None,
                    memory_type: rb_types::MemoryType::Insight,
                    importance: 5,
                    keywords: vec![],
                    tags: vec![],
                    related_files: vec![],
                    confidence: Some(1.0),
                    provenance: provenance.clone(),
                    anchors: vec![],
                })
                .await
                .unwrap();
            ids.push(id);
        }

        let params = rb_store::ReviewQueueParams {
            threshold: rb_types::REVIEW_DEFAULT_THRESHOLD,
            limit: 50,
            since: None,
        };
        let plan = handle.review_plan(ns.clone(), params).await.unwrap();
        let dup_items = plan
            .items
            .iter()
            .filter(|i| i.reason == rb_types::ReviewReason::NearDuplicate)
            .count();
        assert_eq!(dup_items, 2, "both pairs planned: {plan:?}");

        // Concurrent-resolution simulation: archive one member of the FIRST
        // planned pair after the plan was generated.
        let stale_member = plan.items[0].members[0].id.clone();
        engine.delete(stale_member).await.unwrap();

        let outcome = execute_review_plan(
            &engine,
            &handle,
            &provenance,
            &ns,
            rb_types::ReviewPolicy::AutoMergeDups,
            &plan,
            params.threshold,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome.failure, None,
            "a benign collision must not fail the pass: {outcome:?}"
        );
        assert_eq!(outcome.merged, 1, "the live pair still merges");
        assert_eq!(outcome.skipped, 1, "the stale item is counted as skipped");

        handle.shutdown().await;
    }
}
