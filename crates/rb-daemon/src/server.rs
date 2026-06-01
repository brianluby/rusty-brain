//! Unix-domain-socket server for the daemon.

use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use rb_embed::EmbeddingProvider;
use rb_engine::{MemoryEngine, RememberInput};
use rb_proto::{
    read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
};
use rb_types::{Error, Result};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{info, warn};

use crate::error_map::error_to_response;
use crate::{SharedEmbedder, StoreHandle};

/// Maximum number of results returned per Recall or List request.
const MAX_LIMIT: usize = 1000;
/// Maximum graph traversal depth per Graph request.
const MAX_DEPTH: u8 = 8;

/// Static configuration for a daemon instance.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    pub read_pool_size: usize,
}

/// A bound, ready-to-run daemon.
pub struct Daemon {
    listener: UnixListener,
    store: StoreHandle,
    embedder: SharedEmbedder,
    socket_path: PathBuf,
    pidfile_path: PathBuf,
    bind_guard: BindGuard,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("socket_path", &self.socket_path)
            .field("pidfile_path", &self.pidfile_path)
            .finish_non_exhaustive()
    }
}

impl Daemon {
    /// Bind the daemon socket and initialize the backing store.
    pub async fn bind(config: DaemonConfig, embedder: SharedEmbedder) -> Result<Self> {
        let dim = embedder.dim();

        if let Some(parent) = config.db_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| Error::Io(format!("create db dir {}: {e}", parent.display())))?;
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

        let store = StoreHandle::start(config.db_path.clone(), dim, config.read_pool_size)?;

        info!(socket = %config.socket_path.display(), "daemon bound");
        Ok(Self {
            listener,
            store,
            embedder,
            socket_path: config.socket_path,
            pidfile_path,
            bind_guard,
        })
    }

    /// Run until `shutdown` resolves, then drain connections and clean up.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let Daemon {
            listener,
            store,
            embedder,
            socket_path: _socket_path,
            pidfile_path: _pidfile_path,
            mut bind_guard,
        } = self;
        tokio::pin!(shutdown);
        let mut conns: JoinSet<()> = JoinSet::new();

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => {
                    info!("shutdown signal received; stopping accept loop");
                    break;
                }
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _addr)) => {
                            let store = store.clone();
                            let embedder = embedder.clone();
                            conns.spawn(async move {
                                if let Err(e) = handle_connection(stream, store, embedder).await {
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
        conns.shutdown().await;
        store.shutdown().await;

        bind_guard.cleanup();
        info!("daemon shut down cleanly");
        Ok(())
    }
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
            let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
            let hs = Handshake {
                contract_version: CONTRACT_VERSION,
                namespace: rb_types::Namespace::Global,
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

async fn handle_connection(
    stream: UnixStream,
    store: StoreHandle,
    embedder: SharedEmbedder,
) -> Result<()> {
    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
    let handshake: Handshake = read_frame(&mut framed).await?;

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

    let engine = MemoryEngine::new(store, embedder, namespace);
    loop {
        let req: Request = match read_frame::<_, Request>(&mut framed).await {
            Ok(req) => req,
            Err(_) => break,
        };
        let resp = dispatch(&engine, req).await;
        write_frame(&mut framed, &resp).await?;
    }

    Ok(())
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

async fn dispatch<P>(engine: &MemoryEngine<StoreHandle, P>, req: Request) -> Response
where
    P: EmbeddingProvider,
{
    match req {
        Request::Ping => Response::Pong {
            contract_version: CONTRACT_VERSION,
        },
        Request::Remember {
            content,
            context,
            memory_type,
            importance,
            keywords,
            tags,
            related_files,
        } => {
            let input = RememberInput {
                content,
                context,
                memory_type,
                importance,
                keywords,
                tags,
                related_files,
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
            .recall(&query, limit.min(MAX_LIMIT), memory_type, &tags)
            .await
        {
            Ok(results) => Response::Recalled { results },
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
}
