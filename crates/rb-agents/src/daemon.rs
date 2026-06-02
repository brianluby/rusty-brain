//! Strictly fail-open best-effort client over `rb_proto::Client`. Every method
//! wraps the underlying call in a timeout and maps ANY error (connect failure,
//! contract-version mismatch, timeout, wire error) to `None`. The hook surface
//! must NEVER block the CLI or surface a failure: degrade silently.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use rb_proto::Client;
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};

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

/// The minimal set of parent env vars an auto-start daemon child may inherit.
/// Everything else is cleared before spawn (no parent-env leak into a long-lived
/// detached process).
const FORWARD_ENV: &[&str] = &[
    "VOYAGE_API_KEY",
    "RB_ENRICH_BASE_URL",
    "RB_ENRICH_MODEL",
    "RB_ENRICH_API_KEY",
    "HOME",
    "PATH",
    "XDG_RUNTIME_DIR",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
];

const SOCKET_ENV: &str = "RUSTY_BRAIN_SOCKET";
const DB_ENV: &str = "RUSTY_BRAIN_DB";

impl DaemonClient {
    /// Connect with the rb-proto handshake inside `timeout`. ANY failure (IO,
    /// timeout, contract-version mismatch) yields `None`. When `auto_start` is
    /// `Some` (SessionStart only) and the first connect fails, spawn a detached
    /// daemon then retry the connect briefly; otherwise never spawn.
    pub async fn connect(
        socket: &Path,
        namespace: Namespace,
        timeout: Duration,
        auto_start: Option<AutoStart>,
    ) -> Option<DaemonClient> {
        if let Some(client) = try_connect(socket, &namespace, timeout).await {
            return Some(DaemonClient { client, timeout });
        }
        let auto = auto_start?;
        // SessionStart-only path: spawn a detached daemon, then retry connect a
        // bounded number of times. Spawn failure => degrade to None.
        if spawn_daemon(&auto.self_exe, socket, &auto.db).is_err() {
            return None;
        }
        for _ in 0..50 {
            if let Some(client) = try_connect(socket, &namespace, timeout).await {
                return Some(DaemonClient { client, timeout });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        None
    }

    /// Best-effort `remember`. Returns the new id, or `None` on any error/timeout.
    pub async fn remember(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        tags: Vec<String>,
    ) -> Option<MemoryId> {
        let fut = self.client.remember(
            content,
            context,
            memory_type,
            importance,
            Vec::new(),
            tags,
            Vec::new(),
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
}

/// Connect + handshake within `timeout`; any error or timeout => `None`.
async fn try_connect(socket: &Path, namespace: &Namespace, timeout: Duration) -> Option<Client> {
    match tokio::time::timeout(timeout, Client::connect(socket, namespace.clone())).await {
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
    for key in FORWARD_ENV {
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
            )
            .await
            .expect("remember must return an id");
        assert_eq!(id.to_string().len(), 36, "id is a uuid");

        let (recent, important, total) = client
            .context()
            .await
            .expect("context must return a triple");
        assert!(total >= 1, "the stored memory must be counted");
        assert!(
            !recent.is_empty() || !important.is_empty(),
            "stored memory must appear in recent or important"
        );

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
        )
        .await;
        assert!(result.is_none(), "dead socket must degrade to None");
    }
}
