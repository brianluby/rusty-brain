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

/// Count of `MemoryChanged` broadcasts that could not be delivered (no live
/// receivers). Best-effort notification only — a non-zero value is
/// observability, not an error.
static DROPPED_BROADCASTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Read the cumulative count of dropped change-event broadcasts. Exposed for
/// observability and tests; production callers use it for metrics/logging only.
#[allow(dead_code)]
pub fn dropped_broadcast_count() -> u64 {
    DROPPED_BROADCASTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Publish a change event, counting + logging when there is no receiver to take
/// it. `broadcast::Sender::send` returns `Err(SendError)` precisely when there
/// are zero receivers; that is the signal we surface.
fn publish_change(events: &broadcast::Sender<MemoryChanged>, evt: MemoryChanged) {
    if events.send(evt).is_err() {
        let n = DROPPED_BROADCASTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        tracing::warn!(
            dropped_total = n,
            "MemoryChanged broadcast had no receivers; change notification dropped"
        );
    }
}

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
    AddLink {
        link: Box<rb_types::MemoryLink>,
        reply: oneshot::Sender<Result<()>>,
    },
    RecordAccess {
        id: MemoryId,
        reply: oneshot::Sender<Result<()>>,
    },
    RecordAccesses {
        ids: Vec<MemoryId>,
        reply: oneshot::Sender<Result<()>>,
    },
    SetLinkStrength {
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
        strength: f32,
        reply: oneshot::Sender<Result<()>>,
    },
    DeleteLink {
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
        reply: oneshot::Sender<Result<()>>,
    },
    Supersede {
        namespace: Namespace,
        old: MemoryId,
        new: MemoryId,
        reply: oneshot::Sender<Result<()>>,
    },
    #[cfg(test)]
    PanicForTest {
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
    /// when the read closure panics. The panic should surface as
    /// `Error::Storage`, while the guard's `Drop` still returns the connection.
    ///
    /// This method is `pub` only to be reachable from `tests/`. Do NOT call
    /// it in production code.
    #[doc(hidden)]
    pub async fn with_read_panicking_for_test(&self) -> Result<()> {
        #[allow(clippy::panic)]
        self.with_read(|_store| -> Result<()> {
            panic!("deliberate test panic inside read closure");
        })
        .await
    }

    /// Test-only helper: the number of idle connections currently in the read
    /// pool. After all reads have returned, this equals the configured pool
    /// size; a leaked connection would make it permanently smaller.
    ///
    /// This method is `pub` only to be reachable from `tests/`. Do NOT call
    /// it in production code.
    #[doc(hidden)]
    pub async fn read_pool_len_for_test(&self) -> usize {
        self.pool.stores.lock().await.len()
    }

    /// Read up to `limit` link edges (cross-namespace) via the read pool, for
    /// the link-decay job. Reads never go through the writer.
    pub async fn links_for_decay(&self, limit: usize) -> Result<Vec<rb_store::LinkRow>> {
        self.with_read(move |store| store.links_for_decay(limit))
            .await
    }

    /// Set the strength of a single link edge through the single writer.
    pub async fn set_link_strength(
        &self,
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
        strength: f32,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::SetLinkStrength {
            source,
            target,
            link_type,
            strength,
            reply,
        };
        self.send_write(cmd, rx).await
    }

    /// Delete a single link edge through the single writer.
    pub async fn delete_link(
        &self,
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::DeleteLink {
            source,
            target,
            link_type,
            reply,
        };
        self.send_write(cmd, rx).await
    }

    /// Read near-duplicate candidates for `id` within `ns` via the read pool.
    /// Namespace-isolated (see `SqliteStore::near_duplicates`); used by the
    /// consolidation job to find merge candidates without crossing namespaces.
    pub async fn near_duplicates(
        &self,
        ns: Namespace,
        id: MemoryId,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryId, f32)>> {
        self.with_read(move |store| store.near_duplicates(&ns, &id, threshold, limit))
            .await
    }

    /// Supersede `old` with `new`: archive `old` and point it at `new`, through
    /// the single writer. The `namespace` is carried only for the published
    /// `Archived` change event; the FK-guarded SQL keys on ids. Fails closed
    /// (rolls back) if `new` does not exist.
    pub async fn supersede(
        &self,
        namespace: Namespace,
        old: MemoryId,
        new: MemoryId,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Supersede {
            namespace,
            old,
            new,
            reply,
        };
        self.send_write(cmd, rx).await
    }

    /// Enumerate up to `limit` active, non-superseded memories across ALL
    /// namespaces, oldest first then by id, for the consolidation job to scan.
    /// Each candidate carries the id/namespace/importance/created_at the job and
    /// its survivor policy need. Reads via the pool.
    pub async fn candidates_for_consolidation(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::jobs::consolidation::Candidate>> {
        self.with_read(move |store| store.candidates_for_consolidation(limit))
            .await
    }
}

struct StoreOpReport {
    result: Result<()>,
    writer_usable: bool,
}

enum StoreOpOutcome {
    Completed(Result<()>),
    Panicked(String),
}

/// Execute a store operation inside `catch_unwind` so a panic in the store
/// layer can be converted into an explicit recovery decision.
///
/// `SqliteStore` is not `UnwindSafe` (it wraps a raw FFI connection), so we
/// use `AssertUnwindSafe`. A caught panic must not continue on the same
/// connection; the caller drops and reopens the writer store before serving
/// further commands.
#[allow(clippy::panic)]
fn catch_store_op<F>(store: &SqliteStore, op: F) -> StoreOpOutcome
where
    F: FnOnce(&SqliteStore) -> Result<()>,
{
    use std::panic::{self, AssertUnwindSafe};
    match panic::catch_unwind(AssertUnwindSafe(|| op(store))) {
        Ok(result) => StoreOpOutcome::Completed(result),
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            StoreOpOutcome::Panicked(msg)
        }
    }
}

/// Run one store operation on the writer thread, containing any panic so a
/// single bad command cannot take down the daemon.
///
/// GUARANTEE (tested by `caught_writer_panic_isolates_and_does_not_lose_later_writes`):
/// a caught panic (a) is reported to the caller as `Error::Storage`, (b) drops
/// and reopens the write connection so no partial transaction leaks into later
/// writes, and (c) keeps the writer loop alive so subsequent commands commit
/// normally. Only a failed REOPEN (not the panic itself) stops the writer.
fn run_store_op<F>(
    store: &mut Option<SqliteStore>,
    db_path: &Path,
    embedding_dim: usize,
    op: F,
) -> StoreOpReport
where
    F: FnOnce(&SqliteStore) -> Result<()>,
{
    let outcome = {
        let Some(active_store) = store.as_ref() else {
            return StoreOpReport {
                result: Err(Error::Storage("writer thread unavailable".to_string())),
                writer_usable: false,
            };
        };
        catch_store_op(active_store, op)
    };

    match outcome {
        StoreOpOutcome::Completed(result) => StoreOpReport {
            result,
            writer_usable: true,
        },
        StoreOpOutcome::Panicked(msg) => {
            tracing::error!(
                panic = %msg,
                "writer-thread store op panicked; reopening writer connection"
            );

            // A panic may have skipped SQLite rollback code or left rusqlite's
            // FFI state suspect. Drop the old connection first so any open
            // transaction is closed before the replacement connection is opened.
            drop(store.take());

            match SqliteStore::open(db_path, embedding_dim) {
                Ok(reopened) => {
                    *store = Some(reopened);
                    StoreOpReport {
                        result: Err(Error::Storage(format!("writer thread panic: {msg}"))),
                        writer_usable: true,
                    }
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "failed to reopen writer connection after panic; writer exiting"
                    );
                    StoreOpReport {
                        result: Err(Error::Storage(format!(
                            "writer thread panic: {msg}; reopen failed: {e}"
                        ))),
                        writer_usable: false,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
fn panic_for_test_store_op(_store: &SqliteStore) -> Result<()> {
    panic!("deliberate writer test panic");
}

fn writer_loop(
    db_path: PathBuf,
    embedding_dim: usize,
    mut rx: mpsc::Receiver<WriteCommand>,
    events: broadcast::Sender<MemoryChanged>,
    ready_tx: std::sync::mpsc::Sender<Result<()>>,
) {
    let mut store = match SqliteStore::open(&db_path, embedding_dim) {
        Ok(store) => {
            let _ = ready_tx.send(Ok(()));
            Some(store)
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
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.insert_memory(&note, embedding.as_deref())
                });
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if changed {
                    publish_change(
                        &events,
                        MemoryChanged {
                            id,
                            namespace,
                            kind: ChangeKind::Created,
                        },
                    );
                }
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::Update {
                namespace,
                id,
                updates,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    match s.get_memory(&id) {
                        Ok(Some(note)) if note.namespace == namespace => {
                            s.update_memory(&id, &updates)
                        }
                        Ok(Some(_)) | Ok(None) => Err(Error::NotFound(id.clone())),
                        Err(e) => Err(e),
                    }
                });
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if changed {
                    publish_change(
                        &events,
                        MemoryChanged {
                            id,
                            namespace,
                            kind: ChangeKind::Updated,
                        },
                    );
                }
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::Archive {
                namespace,
                id,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    match s.get_memory(&id) {
                        Ok(Some(note)) if note.namespace == namespace => s.archive_memory(&id),
                        Ok(Some(_)) | Ok(None) => Err(Error::NotFound(id.clone())),
                        Err(e) => Err(e),
                    }
                });
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if changed {
                    publish_change(
                        &events,
                        MemoryChanged {
                            id,
                            namespace,
                            kind: ChangeKind::Archived,
                        },
                    );
                }
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::AddLink { link, reply } => {
                let report =
                    run_store_op(&mut store, &db_path, embedding_dim, |s| s.add_link(&link));
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::RecordAccess { id, reply } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.record_access(&id)
                });
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::RecordAccesses { ids, reply } => {
                // No MemoryChanged event: access tracking is observability-only.
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.record_accesses(&ids)
                });
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::SetLinkStrength {
                source,
                target,
                link_type,
                strength,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.set_link_strength(&source, &target, link_type, strength)
                });
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::DeleteLink {
                source,
                target,
                link_type,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.delete_link(&source, &target, link_type)
                });
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::Supersede {
                namespace,
                old,
                new,
                reply,
            } => {
                let report = run_store_op(&mut store, &db_path, embedding_dim, |s| {
                    s.supersede(&old, &new)
                });
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                // supersede archives `old`; mirror the Archive arm so subscribers
                // observe the consolidation as an Archived event for `old`.
                if changed {
                    publish_change(
                        &events,
                        MemoryChanged {
                            id: old,
                            namespace,
                            kind: ChangeKind::Archived,
                        },
                    );
                }
                if !writer_usable {
                    break;
                }
            }
            #[cfg(test)]
            WriteCommand::PanicForTest { reply } => {
                let report =
                    run_store_op(&mut store, &db_path, embedding_dim, panic_for_test_store_op);
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::Shutdown { reply } => {
                // Fold the WAL back into the main file before the connection is
                // dropped, so the on-disk DB is a clean single file. Best-effort:
                // a checkpoint failure is logged but must not block shutdown.
                if let Some(active) = store.as_ref() {
                    if let Err(e) = active.checkpoint_truncate() {
                        tracing::warn!(error = %e, "WAL checkpoint on shutdown failed");
                    }
                }
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

    async fn add_link(&self, link: rb_types::MemoryLink) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::AddLink {
            link: Box::new(link),
            reply,
        };
        self.send_write(cmd, rx).await
    }

    async fn record_access(&self, id: MemoryId) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::RecordAccess { id, reply };
        self.send_write(cmd, rx).await
    }

    async fn record_accesses(&self, ids: Vec<MemoryId>) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::RecordAccesses { ids, reply };
        self.send_write(cmd, rx).await
    }

    async fn get_many(&self, ns: Namespace, ids: Vec<MemoryId>) -> Result<Vec<MemoryNote>> {
        self.with_read(move |store| store.get_many(&ns, &ids)).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use rb_engine::MemoryBackend;
    use rb_types::MemoryType;

    const DIM: usize = 8;

    fn note(ns: &Namespace, body: &str) -> MemoryNote {
        MemoryNote::new(ns.clone(), body.to_string(), MemoryType::Insight, 5)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_add_link_record_access_and_get_many() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("p2".to_string());

        let a = note(&ns, "source note");
        let b = note(&ns, "target note");
        let (aid, bid) = (a.id.clone(), b.id.clone());
        handle.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        // add_link goes through the writer and is visible via get (links loaded).
        handle
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.6,
                reason: "similar".to_string(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        let got = handle.get(ns.clone(), aid.clone()).await.unwrap().unwrap();
        assert_eq!(got.links.len(), 1);
        assert_eq!(got.links[0].target_id, bid);

        // record_access goes through the writer and bumps the count.
        handle.record_access(aid.clone()).await.unwrap();
        let after = handle.get(ns.clone(), aid.clone()).await.unwrap().unwrap();
        assert_eq!(after.access_count, 1);

        // get_many returns ns-scoped notes in request order via the read pool.
        let many = handle
            .get_many(ns, vec![bid.clone(), aid.clone()])
            .await
            .unwrap();
        let ids: Vec<rb_types::MemoryId> = many.iter().map(|n| n.id.clone()).collect();
        assert_eq!(ids, vec![bid, aid]);

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_checkpoints_so_data_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");

        let ns = Namespace::Project("ckpt-shutdown".to_string());
        let id;
        {
            let handle = StoreHandle::start(db.clone(), DIM, 2).unwrap();
            let n = note(&ns, "survive the shutdown checkpoint");
            id = n.id.clone();
            handle.write(n, Some(vec![0.2f32; DIM])).await.unwrap();
            // Graceful shutdown runs PRAGMA wal_checkpoint(TRUNCATE) in the writer
            // Shutdown arm, then joins the writer thread.
            handle.shutdown().await;
        }

        // Reopen a brand-new handle on the same file; the row must be present.
        let reopened = StoreHandle::start(db, DIM, 1).unwrap();
        let got = reopened.get(ns, id).await.unwrap();
        assert!(
            got.is_some(),
            "row must persist across a checkpointed shutdown"
        );
        reopened.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn writer_reopens_after_caught_store_panic() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();

        let (reply, rx) = oneshot::channel();
        handle
            .writer_tx
            .send(WriteCommand::PanicForTest { reply })
            .await
            .unwrap();

        let err = rx.await.unwrap().unwrap_err();
        assert!(
            matches!(err, Error::Storage(_)),
            "caught writer panic must be returned as storage error, got {err:?}"
        );

        let ns = Namespace::Project("writer-recovery".to_string());
        let n = note(&ns, "write after caught writer panic");
        let id = n.id.clone();
        handle.write(n, Some(vec![0.1f32; DIM])).await.unwrap();

        let got = handle.get(ns, id).await.unwrap();
        assert!(
            got.is_some(),
            "writer must reopen its store and accept later writes after a caught panic"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn caught_writer_panic_isolates_and_does_not_lose_later_writes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("panic-isolation".to_string());

        // 1. A successful write before the panic.
        let before = note(&ns, "written before the panic");
        let before_id = before.id.clone();
        handle.write(before, Some(vec![0.1f32; DIM])).await.unwrap();

        // 2. Induce a caught writer panic via the test-only command.
        let (reply, rx) = oneshot::channel();
        handle
            .writer_tx
            .send(WriteCommand::PanicForTest { reply })
            .await
            .unwrap();
        let err = rx.await.unwrap().unwrap_err();
        assert!(
            matches!(err, Error::Storage(_)),
            "caught panic must surface as a storage error, got {err:?}"
        );

        // 3. A successful write AFTER the panic (writer reopened its connection).
        let after = note(&ns, "written after the panic");
        let after_id = after.id.clone();
        handle.write(after, Some(vec![0.2f32; DIM])).await.unwrap();

        // 4. Both real writes are present; the panic left no partial/corrupt row.
        assert!(
            handle.get(ns.clone(), before_id).await.unwrap().is_some(),
            "pre-panic write must survive"
        );
        assert!(
            handle.get(ns.clone(), after_id).await.unwrap().is_some(),
            "post-panic write must commit on the reopened connection"
        );
        let listed = handle.list(ns, None, 50).await.unwrap();
        assert_eq!(
            listed.len(),
            2,
            "exactly the two real writes exist; the panic added nothing"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_without_subscriber_increments_dropped_event_counter() {
        let before = dropped_broadcast_count();

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        // Deliberately do NOT subscribe: the broadcast send will return Err
        // (no receivers), which must be counted, not silently dropped.
        let ns = Namespace::Project("no-subscriber".to_string());
        let n = note(&ns, "nobody is listening");
        handle.write(n, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.shutdown().await;

        assert!(
            dropped_broadcast_count() > before,
            "a broadcast with no receivers must increment the dropped counter \
             (before={before}, after={})",
            dropped_broadcast_count()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_link_decay_read_write_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("decay-handle".to_string());

        let a = note(&ns, "source for decay");
        let b = note(&ns, "target for decay");
        let (aid, bid) = (a.id.clone(), b.id.clone());
        handle.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        handle
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.9,
                reason: "seed".to_string(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        // Read candidates via the pool.
        let rows = handle.links_for_decay(10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].strength - 0.9).abs() < f32::EPSILON);

        // Set strength via the single writer.
        handle
            .set_link_strength(
                aid.clone(),
                bid.clone(),
                rb_types::LinkType::References,
                0.3,
            )
            .await
            .unwrap();
        let rows = handle.links_for_decay(10).await.unwrap();
        assert!((rows[0].strength - 0.3).abs() < f32::EPSILON);

        // Delete via the single writer.
        handle
            .delete_link(aid, bid, rb_types::LinkType::References)
            .await
            .unwrap();
        let rows = handle.links_for_decay(10).await.unwrap();
        assert!(rows.is_empty(), "link removed");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_near_duplicates_is_namespace_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns_a = Namespace::Project("a".to_string());
        let ns_b = Namespace::Project("b".to_string());

        // Anchor + near-identical twin in A.
        let mut anchor = note(&ns_a, "anchor");
        anchor.id = rb_types::MemoryId::new();
        let anchor_id = anchor.id.clone();
        handle
            .write(
                anchor,
                Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            )
            .await
            .unwrap();

        let twin = note(&ns_a, "twin");
        let twin_id = twin.id.clone();
        handle
            .write(twin, Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
            .await
            .unwrap();

        // A near-identical memory in namespace B that must never be returned.
        let foreign = note(&ns_b, "foreign");
        let foreign_id = foreign.id.clone();
        handle
            .write(
                foreign,
                Some(vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            )
            .await
            .unwrap();

        let dups = handle
            .near_duplicates(ns_a.clone(), anchor_id.clone(), 0.95, 10)
            .await
            .unwrap();
        let ids: Vec<rb_types::MemoryId> = dups.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(
            ids,
            vec![twin_id],
            "only the same-namespace twin is returned"
        );
        assert!(!ids.contains(&anchor_id), "anchor excluded (self)");
        assert!(!ids.contains(&foreign_id), "ns-B memory never crosses over");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_archives_old_and_sets_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old fact");
        let new = note(&ns, "new fact");
        let (old_id, new_id) = (old.id.clone(), new.id.clone());
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(new, Some(vec![0.2f32; DIM])).await.unwrap();

        handle
            .supersede(ns.clone(), old_id.clone(), new_id.clone())
            .await
            .unwrap();

        let got_old = handle
            .get(ns.clone(), old_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got_old.superseded_by.as_ref(), Some(&new_id));
        assert!(got_old.archived_at.is_some(), "old must be archived");
        let got_new = handle
            .get(ns.clone(), new_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(got_new.superseded_by.is_none());
        assert!(got_new.archived_at.is_none());

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_missing_new_target_errors() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old only");
        let old_id = old.id.clone();
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();

        let err = handle
            .supersede(ns.clone(), old_id.clone(), rb_types::MemoryId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Storage(_)), "got {err:?}");
        let got_old = handle
            .get(ns.clone(), old_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(got_old.superseded_by.is_none(), "rolled back: no pointer");
        assert!(got_old.archived_at.is_none(), "rolled back: not archived");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_publishes_archived_event_for_old() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("merge".to_string());

        let old = note(&ns, "old fact");
        let new = note(&ns, "new fact");
        let (old_id, new_id) = (old.id.clone(), new.id.clone());
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(new, Some(vec![0.2f32; DIM])).await.unwrap();

        let mut rx = handle.subscribe();
        handle
            .supersede(ns.clone(), old_id.clone(), new_id.clone())
            .await
            .unwrap();

        let evt = rx.recv().await.unwrap();
        assert_eq!(evt.id, old_id, "Archived event must target the old memory");
        assert_eq!(evt.namespace, ns);
        assert_eq!(
            evt.kind,
            crate::change::ChangeKind::Archived,
            "supersede must publish an Archived event for the absorbed memory"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn candidates_for_consolidation_lists_active_non_superseded() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("scan".to_string());

        let keep = note(&ns, "active");
        let archived = note(&ns, "archived");
        let new = note(&ns, "survivor");
        let (keep_id, archived_id, new_id) = (keep.id.clone(), archived.id.clone(), new.id.clone());
        handle.write(keep, Some(vec![0.1f32; DIM])).await.unwrap();
        handle
            .write(archived, Some(vec![0.2f32; DIM]))
            .await
            .unwrap();
        handle.write(new, Some(vec![0.3f32; DIM])).await.unwrap();

        // Supersede `archived` into `new`: it becomes archived + superseded and
        // must drop out of the candidate enumeration.
        handle
            .supersede(ns.clone(), archived_id.clone(), new_id.clone())
            .await
            .unwrap();

        let cands = handle.candidates_for_consolidation(100).await.unwrap();
        let ids: Vec<rb_types::MemoryId> = cands.iter().map(|c| c.id.clone()).collect();
        assert!(ids.contains(&keep_id), "active memory present");
        assert!(ids.contains(&new_id), "survivor present");
        assert!(
            !ids.contains(&archived_id),
            "archived/superseded memory must be excluded"
        );
        // Every returned candidate carries its namespace for per-ns grouping.
        assert!(cands.iter().all(|c| c.namespace == ns));

        handle.shutdown().await;
    }
}
