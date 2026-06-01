//! The single-writer store handle: one dedicated OS thread owns the write
//! connection (rusqlite is `!Sync`, so it must never be shared); a bounded
//! pool of read connections serves concurrent reads via `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rb_engine::MemoryBackend;
use rb_store::{SqliteStore, Store};
use rb_types::{Error, MemoryId, MemoryNote, MemoryUpdates, Namespace, Result};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Semaphore};

use crate::change::{ChangeKind, MemoryChanged};

const BROADCAST_CAPACITY: usize = 256;
const WRITE_QUEUE_CAPACITY: usize = 256;

enum WriteCommand {
    Insert {
        note: Box<MemoryNote>,
        embedding: Option<Vec<f32>>,
        reply: oneshot::Sender<Result<()>>,
    },
    Update {
        namespace: Namespace,
        id: MemoryId,
        updates: Box<MemoryUpdates>,
        reply: oneshot::Sender<Result<()>>,
    },
    Archive {
        namespace: Namespace,
        id: MemoryId,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Cloneable handle to the daemon's single-writer storage core.
#[derive(Clone)]
pub struct StoreHandle {
    writer_tx: mpsc::Sender<WriteCommand>,
    pool: Arc<ReadPool>,
    events: broadcast::Sender<MemoryChanged>,
    writer_join: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    shutting_down: Arc<AtomicBool>,
}

struct ReadPool {
    permits: Arc<Semaphore>,
    stores: Arc<Mutex<Vec<SqliteStore>>>,
}

impl ReadPool {
    fn open(db_path: &Path, dim: usize, size: usize) -> Result<Self> {
        let mut stores = Vec::with_capacity(size);
        for _ in 0..size {
            stores.push(SqliteStore::open(db_path, dim)?);
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(size)),
            stores: Arc::new(Mutex::new(stores)),
        })
    }
}

/// RAII guard that holds a popped `SqliteStore` and pushes it back to the pool
/// in `Drop`, ensuring the connection is returned even if the closure panics.
struct PoolGuard {
    store: Option<SqliteStore>,
    pool: Arc<Mutex<Vec<SqliteStore>>>,
}

impl Drop for PoolGuard {
    fn drop(&mut self) {
        if let Some(store) = self.store.take() {
            self.pool.blocking_lock().push(store);
        }
    }
}

impl StoreHandle {
    /// Start the writer thread and open the read pool.
    pub fn start(db_path: PathBuf, embedding_dim: usize, read_pool_size: usize) -> Result<Self> {
        let pool = Arc::new(ReadPool::open(
            &db_path,
            embedding_dim,
            read_pool_size.max(1),
        )?);
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (writer_tx, writer_rx) = mpsc::channel::<WriteCommand>(WRITE_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();

        let writer_events = events.clone();
        let writer_path = db_path;
        let writer_join = std::thread::Builder::new()
            .name("rb-writer".to_string())
            .spawn(move || {
                writer_loop(
                    writer_path,
                    embedding_dim,
                    writer_rx,
                    writer_events,
                    ready_tx,
                );
            })
            .map_err(|e| Error::Io(format!("spawn writer thread: {e}")))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = writer_join.join();
                return Err(e);
            }
            Err(_) => {
                let _ = writer_join.join();
                return Err(Error::Storage(
                    "writer thread exited before ready".to_string(),
                ));
            }
        }

        Ok(Self {
            writer_tx,
            pool,
            events,
            writer_join: Arc::new(Mutex::new(Some(writer_join))),
            shutting_down: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Subscribe to best-effort memory change notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<MemoryChanged> {
        self.events.subscribe()
    }

    /// Gracefully close the write queue and join the dedicated writer thread.
    pub async fn shutdown(self) {
        let StoreHandle {
            writer_tx,
            pool,
            events,
            writer_join,
            shutting_down,
        } = self;

        if !shutting_down.swap(true, Ordering::SeqCst) {
            let (reply, rx) = oneshot::channel();
            if writer_tx
                .send(WriteCommand::Shutdown { reply })
                .await
                .is_ok()
            {
                let _ = rx.await;
            }
        }

        drop(pool);
        drop(events);
        drop(writer_tx);

        let join = { writer_join.lock().await.take() };
        if let Some(handle) = join {
            let _ = tokio::task::spawn_blocking(move || handle.join()).await;
        }
    }

    async fn send_write(&self, cmd: WriteCommand, rx: oneshot::Receiver<Result<()>>) -> Result<()> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(Error::Storage("writer thread unavailable".to_string()));
        }

        self.writer_tx
            .send(cmd)
            .await
            .map_err(|_| Error::Storage("writer thread unavailable".to_string()))?;
        rx.await
            .map_err(|_| Error::Storage("writer dropped reply".to_string()))?
    }

    async fn with_read<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&SqliteStore) -> Result<T> + Send + 'static,
    {
        let permits = Arc::clone(&self.pool.permits);
        let stores = Arc::clone(&self.pool.stores);
        let permit = permits
            .acquire_owned()
            .await
            .map_err(|_| Error::Storage("read pool closed".to_string()))?;

        tokio::task::spawn_blocking(move || {
            // The semaphore permit is held for the lifetime of this closure.
            // If the task is cancelled the permit is dropped; that is fine
            // because spawn_blocking tasks run to completion.
            let _permit = permit;

            let store = stores
                .blocking_lock()
                .pop()
                .ok_or_else(|| Error::Storage("read pool exhausted despite permit".to_string()))?;

            // RAII guard: returns `store` to the pool in Drop, even on panic.
            let guard = PoolGuard {
                store: Some(store),
                pool: Arc::clone(&stores),
            };

            // Safety: guard.store is Some; we set it back to None in Drop.
            let result = f(guard.store.as_ref().ok_or_else(|| {
                Error::Storage("pool guard store unexpectedly missing".to_string())
            })?);
            // Explicit drop to be clear about ordering (not strictly necessary).
            drop(guard);
            result
        })
        .await
        .map_err(|e| Error::Storage(format!("read task panicked or cancelled: {e}")))?
    }

    /// Test-only helper: checks that the RAII guard returns the connection even
    /// when the read closure panics. Returns `Ok(())` if the panic is correctly
    /// mapped to `Error::Storage`, so the test can `.unwrap()` it.
    ///
    /// This method is `pub` only to be reachable from `tests/`. Do NOT call
    /// it in production code.
    #[doc(hidden)]
    pub async fn with_read_panicking_for_test(&self) -> Result<()> {
        #[allow(clippy::panic)]
        let result = self
            .with_read(|_store| -> Result<()> {
                panic!("deliberate test panic inside read closure");
            })
            .await;
        // The JoinError from the panic is mapped to Error::Storage by with_read.
        match result {
            Err(Error::Storage(_)) => Ok(()),
            other => Err(Error::Storage(format!(
                "expected Storage error from panic, got {other:?}"
            ))),
        }
    }
}

/// Execute a store operation inside `catch_unwind` so a panic in the rusqlite
/// layer does not unwind and kill the writer thread.
///
/// `SqliteStore` is not `UnwindSafe` (it wraps a raw FFI connection), so we
/// use `AssertUnwindSafe`. A rusqlite panic leaves the connection in an
/// undefined state; after catching one we log at error level and return a
/// `Storage` error. The writer thread itself survives and continues serving
/// subsequent commands.
#[allow(clippy::panic)]
fn catch_store_op<F>(store: &SqliteStore, op: F) -> Result<()>
where
    F: FnOnce(&SqliteStore) -> Result<()> + std::panic::UnwindSafe,
{
    use std::panic::{self, AssertUnwindSafe};
    match panic::catch_unwind(AssertUnwindSafe(|| op(store))) {
        Ok(result) => result,
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            tracing::error!(panic = %msg, "writer-thread store op panicked; connection may be degraded");
            Err(Error::Storage(format!("writer thread panic: {msg}")))
        }
    }
}

fn writer_loop(
    db_path: PathBuf,
    embedding_dim: usize,
    mut rx: mpsc::Receiver<WriteCommand>,
    events: broadcast::Sender<MemoryChanged>,
    ready_tx: std::sync::mpsc::Sender<Result<()>>,
) {
    let store = match SqliteStore::open(&db_path, embedding_dim) {
        Ok(store) => {
            let _ = ready_tx.send(Ok(()));
            store
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            WriteCommand::Insert {
                note,
                embedding,
                reply,
            } => {
                let namespace = note.namespace.clone();
                let id = note.id.clone();
                let result =
                    catch_store_op(&store, |s| s.insert_memory(&note, embedding.as_deref()));
                let changed = result.is_ok();
                let _ = reply.send(result);
                if changed {
                    let _ = events.send(MemoryChanged {
                        id,
                        namespace,
                        kind: ChangeKind::Created,
                    });
                }
            }
            WriteCommand::Update {
                namespace,
                id,
                updates,
                reply,
            } => {
                let result = catch_store_op(&store, |s| match s.get_memory(&id) {
                    Ok(Some(note)) if note.namespace == namespace => s.update_memory(&id, &updates),
                    Ok(Some(_)) | Ok(None) => Err(Error::NotFound(id.clone())),
                    Err(e) => Err(e),
                });
                let changed = result.is_ok();
                let _ = reply.send(result);
                if changed {
                    let _ = events.send(MemoryChanged {
                        id,
                        namespace,
                        kind: ChangeKind::Updated,
                    });
                }
            }
            WriteCommand::Archive {
                namespace,
                id,
                reply,
            } => {
                let result = catch_store_op(&store, |s| match s.get_memory(&id) {
                    Ok(Some(note)) if note.namespace == namespace => s.archive_memory(&id),
                    Ok(Some(_)) | Ok(None) => Err(Error::NotFound(id.clone())),
                    Err(e) => Err(e),
                });
                let changed = result.is_ok();
                let _ = reply.send(result);
                if changed {
                    let _ = events.send(MemoryChanged {
                        id,
                        namespace,
                        kind: ChangeKind::Archived,
                    });
                }
            }
            WriteCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
        }
    }

    drop(store);
}

#[async_trait]
impl MemoryBackend for StoreHandle {
    async fn write(&self, note: MemoryNote, embedding: Option<Vec<f32>>) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Insert {
            note: Box::new(note),
            embedding,
            reply,
        };
        self.send_write(cmd, rx).await
    }

    async fn get(&self, ns: Namespace, id: MemoryId) -> Result<Option<MemoryNote>> {
        self.with_read(move |store| Ok(store.get_memory(&id)?.filter(|note| note.namespace == ns)))
            .await
    }

    async fn keyword(&self, ns: Namespace, query: String, limit: usize) -> Result<Vec<MemoryId>> {
        self.with_read(move |store| store.keyword_search(&ns, &query, limit))
            .await
    }

    async fn vector(
        &self,
        ns: Namespace,
        embedding: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        self.with_read(move |store| store.vector_search(&ns, &embedding, limit))
            .await
    }

    async fn graph(&self, ns: Namespace, id: MemoryId, depth: u8) -> Result<Vec<MemoryId>> {
        self.with_read(move |store| {
            let Some(anchor) = store.get_memory(&id)? else {
                return Ok(Vec::new());
            };
            if anchor.namespace != ns || anchor.archived_at.is_some() {
                return Ok(Vec::new());
            }

            let ids = store.graph_neighbors(&id, depth)?;
            let mut filtered = Vec::with_capacity(ids.len());
            for graph_id in ids {
                let Some(note) = store.get_memory(&graph_id)? else {
                    continue;
                };
                if note.namespace == ns && note.archived_at.is_none() {
                    filtered.push(graph_id);
                }
            }
            Ok(filtered)
        })
        .await
    }

    async fn list(
        &self,
        ns: Namespace,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        self.with_read(move |store| store.list(&ns, min_importance, limit))
            .await
    }

    async fn update(&self, ns: Namespace, id: MemoryId, updates: MemoryUpdates) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Update {
            namespace: ns,
            id,
            updates: Box::new(updates),
            reply,
        };
        self.send_write(cmd, rx).await
    }

    async fn archive(&self, ns: Namespace, id: MemoryId) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Archive {
            namespace: ns,
            id,
            reply,
        };
        self.send_write(cmd, rx).await
    }
}
