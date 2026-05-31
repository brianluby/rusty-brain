//! Unix-domain-socket server for the daemon.

use std::fs;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
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

        if config.socket_path.exists() {
            if probe_live(&config.socket_path).await {
                return Err(Error::Io(format!(
                    "another rusty-brain daemon is already listening at {}",
                    config.socket_path.display()
                )));
            }
            let _ = fs::remove_file(&config.socket_path);
        }

        let socket_dir = config
            .socket_path
            .parent()
            .ok_or_else(|| Error::Io("socket path has no parent dir".to_string()))?
            .to_path_buf();
        fs::create_dir_all(&socket_dir)
            .map_err(|e| Error::Io(format!("create socket dir {}: {e}", socket_dir.display())))?;
        fs::set_permissions(&socket_dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| Error::Io(format!("chmod 0700 {}: {e}", socket_dir.display())))?;

        let listener = UnixListener::bind(&config.socket_path)
            .map_err(|e| Error::Io(format!("bind {}: {e}", config.socket_path.display())))?;
        fs::set_permissions(&config.socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::Io(format!("chmod 0600 {}: {e}", config.socket_path.display())))?;

        let store = StoreHandle::start(config.db_path.clone(), dim, config.read_pool_size)?;
        let pidfile_path = config.socket_path.with_extension("pid");
        fs::write(&pidfile_path, std::process::id().to_string())
            .map_err(|e| Error::Io(format!("write pidfile: {e}")))?;

        info!(socket = %config.socket_path.display(), "daemon bound");
        Ok(Self {
            listener,
            store,
            embedder,
            socket_path: config.socket_path,
            pidfile_path,
        })
    }

    /// Run until `shutdown` resolves, then drain connections and clean up.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<()> {
        let Daemon {
            listener,
            store,
            embedder,
            socket_path,
            pidfile_path,
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

        let _ = fs::remove_file(&socket_path);
        let _ = fs::remove_file(&pidfile_path);
        info!("daemon shut down cleanly");
        Ok(())
    }
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

    let namespace = handshake.namespace;
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
            scope: _,
            memory_type,
            tags,
            limit,
        } => match engine.recall(&query, limit, memory_type, &tags).await {
            Ok(results) => Response::Recalled { results },
            Err(e) => error_to_response(e),
        },
        Request::Get { id } => match engine.get(id).await {
            Ok(memory) => Response::Got { memory },
            Err(e) => error_to_response(e),
        },
        Request::List {
            scope: _,
            min_importance,
            limit,
        } => match engine.list(min_importance, limit).await {
            Ok(memories) => Response::Listed { memories },
            Err(e) => error_to_response(e),
        },
        Request::Graph { id, depth } => match engine.graph(id, depth).await {
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
