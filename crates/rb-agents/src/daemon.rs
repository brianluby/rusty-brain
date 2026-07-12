//! Strictly fail-open best-effort client over `rb_proto::Client`. Every method
//! wraps the underlying call in a timeout and maps ANY error (connect failure,
//! contract-version mismatch, timeout, wire error) to `None`. The hook surface
//! must NEVER block the CLI or surface a failure: degrade silently.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

// Auto-start env allowlist + override var names are owned by rb-config so the
// hooks spawner and the CLI spawner can never disagree. The forwarded set is
// secrets + identity + XDG/HOME plus the frozen legacy knobs (C1): config-file
// knobs need no forwarding — the spawned daemon re-reads config.toml itself.
use rb_config::{spawn_forward_env, DB_ENV, SOCKET_ENV};
use rb_proto::{Client, ClientIdentity};
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace, SearchResult};

/// Auto-start parameters. Provided ONLY for `SessionStart`; any other event
/// passes `None` so non-session hooks never spawn a daemon.
#[derive(Debug, Clone)]
pub struct AutoStart {
    pub self_exe: PathBuf,
    pub db: PathBuf,
}

/// A connected, fail-open daemon client. Holds the live `rb_proto::Client` and a
/// per-call timeout. All methods return `Option`, never `Result`.
pub struct DaemonClient {
    client: Client,
    timeout: Duration,
}

impl DaemonClient {
    /// Connect with the rb-proto handshake inside `timeout`. ANY failure (IO,
    /// timeout, contract-version mismatch) yields `None`. When `auto_start` is
    /// `Some` (SessionStart only) and the first connect fails, spawn a detached
    /// daemon then retry the connect briefly; otherwise never spawn. `identity`
    /// is the hook's provenance declaration (source/agent/session), stamped by
    /// the daemon onto every memory this connection writes.
    pub async fn connect(
        socket: &Path,
        namespace: Namespace,
        timeout: Duration,
        auto_start: Option<AutoStart>,
        identity: Option<ClientIdentity>,
    ) -> Option<DaemonClient> {
        if let Some(client) = try_connect(socket, &namespace, timeout, identity.clone()).await {
            return Some(DaemonClient { client, timeout });
        }
        let auto = auto_start?;
        // SessionStart-only path: spawn a detached daemon, then retry connect a
        // bounded number of times. Spawn failure => degrade to None.
        if spawn_daemon(&auto.self_exe, socket, &auto.db).is_err() {
            return None;
        }
        for _ in 0..50 {
            if let Some(client) = try_connect(socket, &namespace, timeout, identity.clone()).await {
                return Some(DaemonClient { client, timeout });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        None
    }

    /// Best-effort `remember`. Returns the new id, or `None` on any error/timeout.
    /// `confidence` is the EXPLICIT trust prior in `0.0..=1.0`, or `None` for no
    /// prior (the daemon applies the 1.0 baseline). Hook captures pass
    /// `Some(0.7)`.
    pub async fn remember(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        tags: Vec<String>,
        confidence: Option<f32>,
    ) -> Option<MemoryId> {
        self.remember_anchored(
            content,
            context,
            memory_type,
            importance,
            tags,
            confidence,
            Vec::new(),
            None,
        )
        .await
    }

    /// Best-effort `remember` that ATOMICALLY supersedes `supersedes` with the
    /// new memory (W3.1 update-as-supersede): the SessionEnd flow uses it to
    /// keep ONE live summary per session as the session is re-summarized.
    /// Returns the NEW memory's id, or `None` on any error/timeout.
    #[allow(clippy::too_many_arguments)]
    pub async fn remember_superseding(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        tags: Vec<String>,
        confidence: Option<f32>,
        supersedes: MemoryId,
    ) -> Option<MemoryId> {
        self.remember_anchored(
            content,
            context,
            memory_type,
            importance,
            tags,
            confidence,
            Vec::new(),
            Some(supersedes),
        )
        .await
    }

    /// Best-effort `remember` carrying typed code anchors (PRD 2026-07-02)
    /// and an optional atomic supersede. STRICTLY FAIL-OPEN like the rest of
    /// the hook surface: when the daemon did not advertise anchor support,
    /// the anchors are DROPPED (logged at debug) and the memory is stored
    /// un-anchored — a summary must never be lost over an old daemon.
    #[allow(clippy::too_many_arguments)]
    pub async fn remember_anchored(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        tags: Vec<String>,
        confidence: Option<f32>,
        anchors: Vec<rb_types::MemoryAnchor>,
        supersedes: Option<MemoryId>,
    ) -> Option<MemoryId> {
        let anchors = if anchors.is_empty() || self.client.supports_anchors() {
            anchors
        } else {
            tracing::debug!(
                dropped = anchors.len(),
                "daemon lacks anchor support; storing without anchors"
            );
            Vec::new()
        };
        let fut = self.client.remember_anchored(
            content,
            context,
            memory_type,
            importance,
            Vec::new(),
            tags,
            Vec::new(),
            confidence,
            anchors,
            supersedes,
        );
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(id)) => Some(id),
            Ok(Err(_)) | Err(_) => None,
        }
    }

    /// Best-effort context fetch. Returns `(recent, important, total)`, or `None`.
    pub async fn context(&mut self) -> Option<(Vec<MemoryNote>, Vec<MemoryNote>, usize)> {
        match tokio::time::timeout(self.timeout, self.client.context()).await {
            Ok(Ok(triple)) => Some(triple),
            Ok(Err(_)) | Err(_) => None,
        }
    }

    /// Best-effort hybrid recall on `query` (W3.2(a): the user's prompt). Returns
    /// up to `limit` ranked results, or `None` on any error/timeout. Drops the
    /// W1.6d degraded flag like [`Self::context`] — the hook surface degrades
    /// silently and never blocks the CLI. Issues NO writer ops (W1.8: recall is
    /// read-only), so a UserPromptSubmit on every turn stays cheap.
    pub async fn recall(&mut self, query: String, limit: usize) -> Option<Vec<SearchResult>> {
        let fut = self.client.recall(query, None, Vec::new(), limit);
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(results)) => Some(results),
            Ok(Err(_)) | Err(_) => None,
        }
    }
}

/// Connect + handshake within `timeout`; any error or timeout => `None`.
async fn try_connect(
    socket: &Path,
    namespace: &Namespace,
    timeout: Duration,
    identity: Option<ClientIdentity>,
) -> Option<Client> {
    match tokio::time::timeout(
        timeout,
        Client::connect_with_identity(socket, namespace.clone(), identity),
    )
    .await
    {
        Ok(Ok(client)) => Some(client),
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Spawn `rusty-brain serve` as a detached child with a cleared environment
/// (only the resolved socket/db paths plus allowlisted vars are forwarded).
fn spawn_daemon(self_exe: &Path, socket: &Path, db: &Path) -> std::io::Result<()> {
    let mut cmd = Command::new(self_exe);
    cmd.arg("serve");
    cmd.env_clear();
    cmd.env(SOCKET_ENV, socket);
    cmd.env(DB_ENV, db);
    for key in spawn_forward_env() {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_child| ())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_daemon::{Daemon, DaemonConfig, JobsConfig, SharedEmbedder};
    use rb_embed::DeterministicProvider;
    use rb_types::{MemoryType, Namespace};
    use std::time::Duration;
    use tokio::sync::oneshot;

    const DIM: usize = 8;

    // Bind + run an in-process daemon on a temp UDS. Returns the dir guard, the
    // socket path, a shutdown sender, and the run JoinHandle.
    async fn start_daemon() -> (
        tempfile::TempDir,
        PathBuf,
        oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        // The daemon creates this `runtime/` subdir itself at 0700, so the parent
        // perms are guaranteed private regardless of the tempdir root's mode.
        let socket = dir.path().join("runtime").join("sock");
        let db = dir.path().join("rb.db");
        let config = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db,
            read_pool_size: 2,
            jobs_config: JobsConfig::default(),
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_daemon::FusionMode::Linear,
        };
        let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
        let daemon = Daemon::bind(config, embedder).await.unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let _ = daemon
                .run(async move {
                    let _ = rx.await;
                })
                .await;
        });
        // Give the accept loop a moment to be ready.
        tokio::time::sleep(Duration::from_millis(50)).await;
        (dir, socket, tx, handle)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remember_then_context_round_trip_over_real_daemon() {
        let (_dir, socket, shutdown, handle) = start_daemon().await;

        let mut client = DaemonClient::connect(
            &socket,
            Namespace::Project("rb-agents-test".to_string()),
            Duration::from_secs(5),
            None,
            Some(ClientIdentity {
                agent: Some("claude-code".to_string()),
                session_id: Some("sess-42".to_string()),
                source: Some("hook".to_string()),
                ..Default::default()
            }),
        )
        .await
        .expect("connect must succeed against a live daemon");

        let id = client
            .remember(
                "always run one writer thread".to_string(),
                Some("daemon design".to_string()),
                MemoryType::ArchitectureDecision,
                9,
                vec!["daemon".to_string()],
                Some(0.7),
            )
            .await
            .expect("remember must return an id");
        assert_eq!(id.to_string().len(), 36, "id is a uuid");

        let (recent, important, total) = client
            .context()
            .await
            .expect("context must return a triple");
        assert!(total >= 1, "the stored memory must be counted");
        let note = recent
            .iter()
            .chain(important.iter())
            .find(|m| m.id == id)
            .expect("stored memory must appear in recent or important");

        // W0.5: the handshake identity is stamped onto the write, and the
        // declared confidence (0.7 for hook captures) is persisted.
        assert_eq!(note.origin_source.as_deref(), Some("hook"));
        assert_eq!(note.origin_agent.as_deref(), Some("claude-code"));
        assert_eq!(note.session_id.as_deref(), Some("sess-42"));
        assert!((note.confidence - 0.7).abs() < f32::EPSILON);
        // user/host were not declared: the daemon fell back to its own whoami.
        assert_eq!(note.origin_user, rb_config::current_user());
        assert_eq!(note.origin_host, rb_config::current_hostname());

        // Typed code anchors: the real daemon advertises the capability, so
        // an anchored best-effort remember persists its anchors.
        let anchors = vec![
            rb_types::MemoryAnchor::parse_file_spec("src/server.rs:1-9").unwrap(),
            rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "Server::run").unwrap(),
        ];
        let anchored_id = client
            .remember_anchored(
                "anchored session summary".to_string(),
                None,
                MemoryType::Insight,
                6,
                vec!["hook".to_string()],
                Some(0.7),
                anchors.clone(),
                None,
            )
            .await
            .expect("anchored remember must return an id");
        let (recent, important, _total) = client.context().await.expect("context");
        let anchored_note = recent
            .iter()
            .chain(important.iter())
            .find(|m| m.id == anchored_id)
            .expect("anchored memory must appear");
        assert_eq!(
            anchored_note.anchors, anchors,
            "anchors must persist through the fail-open hook client"
        );

        let _ = shutdown.send(());
        let _ = handle.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn old_client_without_identity_gets_whoami_fallback_provenance() {
        let (_dir, socket, shutdown, handle) = start_daemon().await;

        // identity=None is exactly the pre-W0.5 handshake wire shape.
        let mut client = DaemonClient::connect(
            &socket,
            Namespace::Project("rb-agents-old".to_string()),
            Duration::from_secs(5),
            None,
            None,
        )
        .await
        .expect("old-shape connect must still succeed");

        let id = client
            .remember(
                "legacy client write".to_string(),
                None,
                MemoryType::Insight,
                8,
                vec![],
                Some(1.0),
            )
            .await
            .expect("remember must return an id");

        let (recent, important, _total) = client.context().await.expect("context");
        let note = recent
            .iter()
            .chain(important.iter())
            .find(|m| m.id == id)
            .expect("note present");
        assert_eq!(note.origin_user, rb_config::current_user());
        assert_eq!(note.origin_host, rb_config::current_hostname());
        assert!(note.origin_agent.is_none());
        assert!(
            note.origin_source.is_none(),
            "source is client-declared only"
        );
        assert!(note.session_id.is_none());

        let _ = shutdown.send(());
        let _ = handle.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connect_to_dead_socket_returns_none_without_panic_or_hang() {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let socket = dir.path().join("absent.sock"); // never bound

        let result = DaemonClient::connect(
            &socket,
            Namespace::Global,
            Duration::from_millis(200),
            None, // no auto-start: must not spawn, must degrade to None
            None,
        )
        .await;
        assert!(result.is_none(), "dead socket must degrade to None");
    }
}
