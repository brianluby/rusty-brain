//! The single-writer store handle: one dedicated OS thread owns the write
//! connection (rusqlite is `!Sync`, so it must never be shared); a bounded
//! pool of read connections serves concurrent reads via `spawn_blocking`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rb_engine::MemoryBackend;
use rb_store::{RecalRow, SqliteStore, Store};
use rb_types::{Error, MemoryId, MemoryNote, MemoryUpdates, Namespace, Result};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Mutex, Semaphore};

use crate::change::{ChangeKind, MemoryChanged};

const BROADCAST_CAPACITY: usize = 256;
const WRITE_QUEUE_CAPACITY: usize = 256;

/// How often the background flusher drains buffered access bumps into one
/// batched writer op (W1.8). Read paths only ever touch the in-memory buffer;
/// the buffer is bounded in practice by the distinct ids recalled within one
/// interval (entries are tens of bytes each).
const ACCESS_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

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

/// Stamp the just-committed op's oplog seq onto a `MemoryChanged` and publish
/// it (W2.7). Single source for all five writer arms (fix #5/#7): the seq is
/// `MAX(seq)` on the writer's own connection immediately after the committed
/// oplog row, so on the single writer it is reliably THIS op's seq. The
/// read is effectively infallible here (the op just succeeded on this
/// connection); a real error would mean the writer is failing, so it is
/// logged rather than silently swallowed — a `None` seq would slip past the
/// replay-overlap dedup in `stream_changes` and be re-delivered.
fn publish_change_stamped(
    events: &broadcast::Sender<MemoryChanged>,
    store: Option<&SqliteStore>,
    id: MemoryId,
    namespace: Namespace,
    kind: ChangeKind,
) {
    let seq = match store.map(|s| s.last_oplog_seq()) {
        Some(Ok(seq)) => Some(seq),
        Some(Err(e)) => {
            tracing::warn!(error = %e, "could not read oplog seq for a change event; \
                 emitting without a replay cursor (a reconnecting subscriber may re-see this event)");
            None
        }
        None => None,
    };
    publish_change(
        events,
        MemoryChanged {
            id,
            namespace,
            kind,
            seq,
        },
    );
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
    /// Record a usefulness-feedback event for a memory and nudge its trust prior
    /// (W3.7). Namespace-verified like `Update` before the store writes the event
    /// row + confidence + oplog in one transaction. The typed reply carries the
    /// post-nudge `confidence` (the `RenameNamespace`/`Scrub` typed-reply
    /// precedent).
    RecordFeedback {
        namespace: Namespace,
        id: MemoryId,
        kind: rb_types::FeedbackKind,
        principal: Option<String>,
        reply: oneshot::Sender<Result<f32>>,
    },
    /// Apply a batch of BUFFERED access bumps (W1.8). Recall and `get` never
    /// enqueue a writer command for access tracking — they accumulate into
    /// [`AccessBuffer`]; this command carries the periodic/shutdown flush.
    FlushAccesses {
        bumps: Vec<rb_store::AccessBump>,
        reply: oneshot::Sender<Result<()>>,
    },
    SetLinkStrength {
        source: MemoryId,
        target: MemoryId,
        link_type: rb_types::LinkType,
        strength: f32,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Set one memory's EFFECTIVE importance without touching its
    /// `base_importance` author prior (W1.9). The importance-recalibration
    /// job's only write path; namespace-verified like `Update`.
    SetRecalibratedImportance {
        namespace: Namespace,
        id: MemoryId,
        importance: u8,
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
    /// Replace one memory's stored vector and stamp it to the current
    /// `(model, input_version)` (P5 Feature A re-embed write path). The engine
    /// recomputes the vector and passes it here; the store performs the only
    /// vector-UPDATE path under the single writer.
    Reembed {
        id: MemoryId,
        embedding: Vec<f32>,
        model: String,
        input_version: String,
        reply: oneshot::Sender<Result<()>>,
    },
    /// One-time namespace rename (W0.3 carryover): re-scope every memory row
    /// from `old` to `new` in ONE store transaction (memories + vec0 partition
    /// re-key + one oplog row). Cross-namespace admin op, so unlike
    /// Update/Archive it carries no handshake-namespace verification — the
    /// server validates both namespaces before enqueueing.
    RenameNamespace {
        old: Namespace,
        new: Namespace,
        merge: bool,
        reply: oneshot::Sender<Result<rb_store::NamespaceRenameOutcome>>,
    },
    /// Retroactively redact secrets from every stored memory (W2.4
    /// `rusty-brain scrub`). Cross-namespace admin op (peer-gated server-side
    /// like RenameNamespace); rewrites text + blanks embedding stamps, then
    /// the caller runs `reembed` to recompute vectors.
    Scrub {
        reply: oneshot::Sender<Result<rb_store::ScrubOutcome>>,
    },
    #[cfg(test)]
    PanicForTest {
        reply: oneshot::Sender<Result<()>>,
    },
    /// Test-only: run an op that opens a transaction and returns `Err` WITHOUT
    /// rolling back, exercising the W1.6b post-op `is_autocommit` poison check.
    #[cfg(test)]
    PoisonForTest {
        reply: oneshot::Sender<Result<()>>,
    },
    /// Test-only: exit the writer loop abnormally (death guard left armed),
    /// simulating an unrecoverable writer failure such as a failed reopen.
    #[cfg(test)]
    DieForTest {
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
    /// Becomes `true` when the writer thread dies ABNORMALLY (W1.6c). Graceful
    /// shutdown never flips it; see [`StoreHandle::writer_died`].
    writer_death: watch::Receiver<bool>,
    /// In-memory accumulator for access-tracking bumps (W1.8): recall/`get`
    /// buffer here instead of enqueueing writer ops; a background task flushes
    /// batches every [`ACCESS_FLUSH_INTERVAL`], and `shutdown` drains the rest.
    access_buf: Arc<AccessBuffer>,
    /// Count of write commands the writer thread has received (excluding
    /// `Shutdown`). Observability + the W1.8 zero-writer-ops-on-recall proof.
    writer_ops: Arc<std::sync::atomic::AtomicU64>,
}

/// Buffered access-tracking state shared by every [`StoreHandle`] clone.
struct AccessBuffer {
    /// id -> (accumulated count, latest access unix seconds). Drained whole on
    /// each flush; `tokio::sync::Mutex` so the async paths never block a
    /// runtime worker and there is no poisoning to reason about.
    pending: Mutex<std::collections::HashMap<MemoryId, PendingAccess>>,
    /// Set once by whichever handle buffers the first access; the flusher task
    /// is spawned lazily from an async context so construction never requires
    /// a Tokio runtime.
    flusher_started: AtomicBool,
}

#[derive(Clone, Copy)]
struct PendingAccess {
    count: u64,
    last_accessed_at: i64,
}

/// Drain ALL buffered bumps, returning them as store-level [`rb_store::AccessBump`]s.
async fn drain_access_buffer(buf: &AccessBuffer) -> Vec<rb_store::AccessBump> {
    let mut pending = buf.pending.lock().await;
    pending
        .drain()
        .map(|(id, p)| rb_store::AccessBump {
            id,
            count: p.count,
            last_accessed_at: p.last_accessed_at,
        })
        .collect()
}

struct ReadPool {
    permits: Arc<Semaphore>,
    stores: Arc<Mutex<Vec<SqliteStore>>>,
}

impl ReadPool {
    fn open(db_path: &Path, dim: usize, model: Option<&str>, size: usize) -> Result<Self> {
        let mut stores = Vec::with_capacity(size);
        for _ in 0..size {
            stores.push(open_store(db_path, dim, model)?);
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(size)),
            stores: Arc::new(Mutex::new(stores)),
        })
    }
}

/// Open one store connection, enforcing the embedding-model invariant whenever
/// a model identity is bound (the daemon path; tests may pass `None`).
fn open_store(db_path: &Path, dim: usize, model: Option<&str>) -> Result<SqliteStore> {
    match model {
        Some(model) => SqliteStore::open_with_model(db_path, dim, model),
        None => SqliteStore::open(db_path, dim),
    }
}

/// Explicit opt-in for an embedding-model swap (`--accept-model-change` /
/// `RB_ACCEPT_MODEL_CHANGE`): atomically adopt `new_model` and stale every
/// row's embedding stamp so the reembed machinery converges the corpus. Runs
/// BEFORE the daemon's model-verified opens; a missing DB is a no-op (a fresh
/// DB seeds the model at first open). Returns `true` when a swap occurred.
pub fn accept_model_change(db_path: &Path, embedding_dim: usize, new_model: &str) -> Result<bool> {
    if !db_path.exists() {
        return Ok(false);
    }
    let store = SqliteStore::open(db_path, embedding_dim)?;
    store.accept_model_change(new_model)
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
    /// Start the writer thread and open the read pool without binding an
    /// embedding-model identity (test seam; no model invariant enforced).
    pub fn start(db_path: PathBuf, embedding_dim: usize, read_pool_size: usize) -> Result<Self> {
        Self::start_inner(db_path, embedding_dim, None, read_pool_size)
    }

    /// Start the writer thread and open the read pool, enforcing the
    /// embedding-model invariant on every connection (the daemon path).
    pub fn start_with_model(
        db_path: PathBuf,
        embedding_dim: usize,
        embedding_model: String,
        read_pool_size: usize,
    ) -> Result<Self> {
        Self::start_inner(
            db_path,
            embedding_dim,
            Some(embedding_model),
            read_pool_size,
        )
    }

    fn start_inner(
        db_path: PathBuf,
        embedding_dim: usize,
        embedding_model: Option<String>,
        read_pool_size: usize,
    ) -> Result<Self> {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (writer_tx, writer_rx) = mpsc::channel::<WriteCommand>(WRITE_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        // Writer-death signal (W1.6c): the writer thread owns the sender via a
        // drop guard; the daemon races the receiver in `Server::run`'s select!.
        let (death_tx, death_rx) = watch::channel(false);

        // The WRITER opens first; the read pool only after its ready signal.
        // `SqliteStore::init` runs the one-shot vector-schema rebuild
        // (W1.1/W1.7) at open, and sequencing it on the single writer
        // connection makes the rebuild single-flight by construction — a
        // large-corpus rebuild cannot race N pool opens into busy_timeout
        // failures.
        let writer_events = events.clone();
        let writer_path = db_path.clone();
        let writer_model = embedding_model.clone();
        let writer_ops = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let writer_ops_counter = Arc::clone(&writer_ops);
        let writer_join = std::thread::Builder::new()
            .name("rb-writer".to_string())
            .spawn(move || {
                writer_loop(
                    writer_path,
                    embedding_dim,
                    writer_model,
                    writer_rx,
                    writer_events,
                    ready_tx,
                    death_tx,
                    writer_ops_counter,
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

        let pool = match ReadPool::open(
            &db_path,
            embedding_dim,
            embedding_model.as_deref(),
            read_pool_size.max(1),
        ) {
            Ok(pool) => Arc::new(pool),
            Err(e) => {
                // The writer is already running: drop its only sender so its
                // recv loop ends, then join — never leak a live writer thread
                // from a failed construction.
                drop(writer_tx);
                let _ = writer_join.join();
                return Err(e);
            }
        };

        Ok(Self {
            writer_tx,
            pool,
            events,
            writer_join: Arc::new(Mutex::new(Some(writer_join))),
            shutting_down: Arc::new(AtomicBool::new(false)),
            writer_death: death_rx,
            access_buf: Arc::new(AccessBuffer {
                pending: Mutex::new(std::collections::HashMap::new()),
                flusher_started: AtomicBool::new(false),
            }),
            writer_ops,
        })
    }

    /// Resolves when the writer thread has died ABNORMALLY — it exited its
    /// loop without a graceful shutdown (e.g. a failed reopen after a panic or
    /// after a poisoned connection). NEVER resolves on graceful shutdown.
    ///
    /// `Server::run` races this future in its `select!` so a dead writer shuts
    /// the daemon down instead of leaving a zombie that pongs `Ping` while
    /// every write fails (W1.6c / F17). The returned future is `'static`: it
    /// owns a clone of the watch receiver, so it can be pinned across the
    /// accept loop without borrowing the handle.
    pub fn writer_died(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = self.writer_death.clone();
        async move {
            loop {
                if *rx.borrow_and_update() {
                    return;
                }
                if rx.changed().await.is_err() {
                    // Sender dropped without ever signaling: the writer exited
                    // gracefully. Park forever — a graceful exit must not trip
                    // the daemon's writer-death arm.
                    std::future::pending::<()>().await;
                }
            }
        }
    }

    /// Test-only: make the writer thread exit abnormally (death guard armed),
    /// as if an unrecoverable failure stopped it.
    #[cfg(test)]
    pub(crate) async fn kill_writer_for_test(&self) {
        let (reply, rx) = oneshot::channel();
        let _ = self
            .writer_tx
            .send(WriteCommand::DieForTest { reply })
            .await;
        let _ = rx.await;
    }

    /// Subscribe to best-effort memory change notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<MemoryChanged> {
        self.events.subscribe()
    }

    /// Replay the durable oplog as change events for `namespace`, strictly
    /// after `after`, oldest first, capped at `limit` (W2.7
    /// replay-on-reconnect). Goes through the bounded read pool.
    pub async fn oplog_changes_since(
        &self,
        namespace: rb_types::Namespace,
        after: u64,
        limit: usize,
    ) -> Result<rb_store::OplogReplayPage> {
        self.with_read(move |store| store.oplog_changes_since(&namespace, after, limit))
            .await
    }

    /// Read up to `limit` active memories with the fields the importance job
    /// needs. Goes through the bounded read pool (never the writer). Used only by
    /// the cross-namespace maintenance jobs, which then issue any importance
    /// changes back through `set_recalibrated_importance` (the single writer,
    /// W1.9: effective importance only — never the author prior).
    pub async fn memories_for_recalibration(&self, limit: usize) -> Result<Vec<RecalRow>> {
        self.with_read(move |store| store.memories_for_recalibration(limit))
            .await
    }

    /// Namespace-scoped observability aggregate (doctor/stats PRD). Goes
    /// through the bounded read pool — the stats path issues zero writer ops
    /// (W1.8) by construction. `model`/`input_version` are the CURRENT
    /// embedding stamp (for the re-embed backlog count); `top_limit` bounds
    /// the top-recalled list.
    pub async fn namespace_stats(
        &self,
        namespace: Namespace,
        window_days: u32,
        model: String,
        input_version: String,
        top_limit: usize,
    ) -> Result<rb_types::MemoryStats> {
        self.with_read(move |store| {
            store.namespace_stats(&namespace, window_days, &model, &input_version, top_limit)
        })
        .await
    }

    /// Whether the writer thread is alive (i.e. has not died ABNORMALLY).
    /// Snapshot of the same watch [`StoreHandle::writer_died`] resolves on;
    /// surfaced on the stats/status path so a zombie-adjacent state is visible.
    pub fn writer_alive(&self) -> bool {
        !*self.writer_death.borrow()
    }

    /// Gracefully close the write queue and join the dedicated writer thread.
    pub async fn shutdown(self) {
        // Final access flush (W1.8): persist whatever the interval flusher has
        // not yet drained, BEFORE the shutdown flag closes the write path.
        // Best-effort — a dead writer must not block shutdown.
        if let Err(e) = self.flush_accesses().await {
            tracing::debug!(error = %e, "final access flush on shutdown failed; bumps dropped");
        }
        let StoreHandle {
            writer_tx,
            pool,
            events,
            writer_join,
            shutting_down,
            writer_death: _writer_death,
            access_buf: _access_buf,
            writer_ops: _writer_ops,
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

    /// Accumulate access bumps for `ids` into the in-memory buffer (W1.8).
    /// NEVER touches the writer thread: recall/`get` call this and return.
    /// Duplicate ids within one call bump once (mirrors the old
    /// `record_accesses` semantics where one UPDATE touched each row once).
    async fn buffer_accesses(&self, ids: Vec<MemoryId>) {
        if ids.is_empty() {
            return;
        }
        let now = chrono::Utc::now().timestamp();
        {
            let mut pending = self.access_buf.pending.lock().await;
            let mut seen: std::collections::HashSet<MemoryId> =
                std::collections::HashSet::with_capacity(ids.len());
            for id in ids {
                if !seen.insert(id.clone()) {
                    continue;
                }
                let entry = pending.entry(id).or_insert(PendingAccess {
                    count: 0,
                    last_accessed_at: now,
                });
                entry.count += 1;
                entry.last_accessed_at = now;
            }
        }
        self.ensure_access_flusher();
    }

    /// Spawn the interval flusher exactly once (lazily, from the first
    /// buffered access, so we are guaranteed to be inside a Tokio runtime).
    ///
    /// The task holds only WEAK references to the writer channel and the
    /// buffer: it can never keep the writer thread alive after every handle is
    /// gone, and it exits on its next tick once the handles drop or shutdown
    /// begins (`shutdown` performs its own final drain).
    fn ensure_access_flusher(&self) {
        if self.access_buf.flusher_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let buf = Arc::downgrade(&self.access_buf);
        let writer_tx = self.writer_tx.downgrade();
        let shutting_down = Arc::clone(&self.shutting_down);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(ACCESS_FLUSH_INTERVAL).await;
                if shutting_down.load(Ordering::SeqCst) {
                    return; // shutdown() drains the buffer itself
                }
                let Some(buf) = buf.upgrade() else {
                    return; // every StoreHandle dropped
                };
                let bumps = drain_access_buffer(&buf).await;
                drop(buf);
                if bumps.is_empty() {
                    continue;
                }
                let Some(tx) = writer_tx.upgrade() else {
                    return; // writer channel closed
                };
                let (reply, rx) = oneshot::channel();
                if tx
                    .send(WriteCommand::FlushAccesses { bumps, reply })
                    .await
                    .is_err()
                {
                    return; // writer gone; bumps dropped (best-effort)
                }
                drop(tx);
                match rx.await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::debug!(error = %e, "access flush failed; bumps dropped");
                    }
                    Err(_) => return, // writer dropped the reply: it is exiting
                }
            }
        });
    }

    /// Drain ALL buffered access bumps into one batched writer op, awaiting
    /// the result. Used by `shutdown` (final drain) and by tests that need
    /// deterministic visibility; the steady-state path is the interval flusher.
    #[doc(hidden)]
    pub async fn flush_accesses(&self) -> Result<()> {
        let bumps = drain_access_buffer(&self.access_buf).await;
        if bumps.is_empty() {
            return Ok(());
        }
        let (reply, rx) = oneshot::channel();
        self.send_write(WriteCommand::FlushAccesses { bumps, reply }, rx)
            .await
    }

    /// Count of write commands the writer thread has received so far
    /// (excluding `Shutdown`). Exposed for observability and for the W1.8
    /// proof that recall issues zero writer-thread ops.
    #[doc(hidden)]
    pub fn writer_ops_count(&self) -> u64 {
        self.writer_ops.load(std::sync::atomic::Ordering::SeqCst)
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

    /// Test-only helper: how many distinct memory ids currently have buffered
    /// (unflushed) access bumps. Proves recall BUFFERED instead of writing.
    ///
    /// This method is `pub` only to be reachable from `tests/`. Do NOT call
    /// it in production code.
    #[doc(hidden)]
    pub async fn pending_access_len_for_test(&self) -> usize {
        self.access_buf.pending.lock().await.len()
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

    /// Set one memory's EFFECTIVE importance through the single writer,
    /// leaving its `base_importance` author prior untouched (W1.9). The
    /// importance-recalibration job's only write path. Namespace-verified:
    /// a missing or cross-namespace id fails closed with `NotFound`.
    pub async fn set_recalibrated_importance(
        &self,
        namespace: Namespace,
        id: MemoryId,
        importance: u8,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::SetRecalibratedImportance {
            namespace,
            id,
            importance,
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

    /// One-time namespace rename through the single writer (W0.3 carryover):
    /// re-scope every memory from `old` to `new` in one store transaction.
    /// Refuses a non-empty target unless `merge` is set (validation-class
    /// error). No `MemoryChanged` events are published: the bulk admin op has
    /// no per-memory change identity, and subscribers are namespace-scoped
    /// snapshots of a pre-rename world — the oplog row is the durable record.
    pub async fn rename_namespace(
        &self,
        old: Namespace,
        new: Namespace,
        merge: bool,
    ) -> Result<rb_store::NamespaceRenameOutcome> {
        // Mirrors `send_write`, with a typed reply payload.
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(Error::Storage("writer thread unavailable".to_string()));
        }
        let (reply, rx) = oneshot::channel();
        self.writer_tx
            .send(WriteCommand::RenameNamespace {
                old,
                new,
                merge,
                reply,
            })
            .await
            .map_err(|_| Error::Storage("writer thread unavailable".to_string()))?;
        rx.await
            .map_err(|_| Error::Storage("writer dropped reply".to_string()))?
    }

    /// Retroactively redact secrets from every stored memory through the single
    /// writer (W2.4). Returns the scan/redact/reembed-pending counts; the
    /// caller then runs `reembed` to recompute vectors for the changed rows.
    pub async fn scrub(&self) -> Result<rb_store::ScrubOutcome> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(Error::Storage("writer thread unavailable".to_string()));
        }
        let (reply, rx) = oneshot::channel();
        self.writer_tx
            .send(WriteCommand::Scrub { reply })
            .await
            .map_err(|_| Error::Storage("writer thread unavailable".to_string()))?;
        rx.await
            .map_err(|_| Error::Storage("writer dropped reply".to_string()))?
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
    embedding_model: Option<&str>,
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
        StoreOpOutcome::Completed(result) => {
            // W1.6b poison check: a COMPLETED op that returned Err while
            // leaving the connection mid-transaction (e.g. a failed COMMIT
            // whose drop-rollback also failed) would make every later op die
            // with "cannot start a transaction within a transaction". Reuse
            // the panic path's drop+reopen machinery; the caller still gets
            // the op's own error.
            let poisoned = result.is_err() && store.as_ref().is_some_and(|s| !s.is_autocommit());
            if !poisoned {
                return StoreOpReport {
                    result,
                    writer_usable: true,
                };
            }
            tracing::error!(
                "writer op failed and left an open transaction; reopening writer connection"
            );
            drop(store.take());
            match open_store(db_path, embedding_dim, embedding_model) {
                Ok(reopened) => {
                    *store = Some(reopened);
                    StoreOpReport {
                        result,
                        writer_usable: true,
                    }
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        "failed to reopen writer connection after poisoned op; writer exiting"
                    );
                    StoreOpReport {
                        result,
                        writer_usable: false,
                    }
                }
            }
        }
        StoreOpOutcome::Panicked(msg) => {
            tracing::error!(
                panic = %msg,
                "writer-thread store op panicked; reopening writer connection"
            );

            // A panic may have skipped SQLite rollback code or left rusqlite's
            // FFI state suspect. Drop the old connection first so any open
            // transaction is closed before the replacement connection is opened.
            drop(store.take());

            match open_store(db_path, embedding_dim, embedding_model) {
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

/// Test-only op for the W1.6b poison check: open a transaction, then complete
/// with `Err` WITHOUT rolling back — the failed-COMMIT-then-failed-ROLLBACK
/// shape no public write path can produce now that every op is RAII-guarded.
#[cfg(test)]
fn poison_for_test_store_op(store: &SqliteStore) -> Result<()> {
    store.leave_transaction_open_for_test()?;
    Err(Error::Storage(
        "deliberate poisoned writer op for test".to_string(),
    ))
}

/// Arms the writer-death signal (W1.6c). Dropped ARMED — including on a panic
/// unwinding out of the writer loop — it flips the watch to `true`, which
/// resolves [`StoreHandle::writer_died`] and shuts the daemon down. The loop
/// disarms it only on the graceful exits (a `Shutdown` command, or every
/// handle dropping the command channel), so a writer that stops serving writes
/// for any other reason can never leave a zombie daemon behind.
struct WriterDeathGuard {
    tx: watch::Sender<bool>,
    graceful: bool,
}

impl WriterDeathGuard {
    fn disarm(&mut self) {
        self.graceful = true;
    }
}

impl Drop for WriterDeathGuard {
    fn drop(&mut self) {
        if !self.graceful {
            let _ = self.tx.send(true);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn writer_loop(
    db_path: PathBuf,
    embedding_dim: usize,
    embedding_model: Option<String>,
    mut rx: mpsc::Receiver<WriteCommand>,
    events: broadcast::Sender<MemoryChanged>,
    ready_tx: std::sync::mpsc::Sender<Result<()>>,
    death_tx: watch::Sender<bool>,
    ops: Arc<std::sync::atomic::AtomicU64>,
) {
    let mut death = WriterDeathGuard {
        tx: death_tx,
        graceful: false,
    };

    let mut store = match open_store(&db_path, embedding_dim, embedding_model.as_deref()) {
        Ok(store) => {
            let _ = ready_tx.send(Ok(()));
            Some(store)
        }
        Err(e) => {
            // The open failure is surfaced to `start_inner` via the ready
            // channel and no handle ever exists: a construction failure, not
            // a writer death.
            death.disarm();
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    loop {
        let Some(cmd) = rx.blocking_recv() else {
            // Every sender dropped (a failed construction's cleanup, or a
            // shutdown path that skipped the Shutdown command): deliberate
            // teardown, not a writer death.
            death.disarm();
            break;
        };
        // Count every write op received (Shutdown is lifecycle, not an op).
        // The W1.8 gate test reads this to prove recall enqueues nothing.
        if !matches!(cmd, WriteCommand::Shutdown { .. }) {
            ops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        match cmd {
            WriteCommand::Insert {
                note,
                embedding,
                reply,
            } => {
                let namespace = note.namespace.clone();
                let id = note.id.clone();
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| s.insert_memory(&note, embedding.as_deref()),
                );
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if changed {
                    publish_change_stamped(
                        &events,
                        store.as_ref(),
                        id,
                        namespace,
                        ChangeKind::Created,
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
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| match s.get_memory(&id) {
                        Ok(Some(note)) if note.namespace == namespace => {
                            s.update_memory(&id, &updates)
                        }
                        Ok(Some(_)) | Ok(None) => Err(Error::NotFound(id.clone())),
                        Err(e) => Err(e),
                    },
                );
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if changed {
                    publish_change_stamped(
                        &events,
                        store.as_ref(),
                        id,
                        namespace,
                        ChangeKind::Updated,
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
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| match s.get_memory(&id) {
                        Ok(Some(note)) if note.namespace == namespace => s.archive_memory(&id),
                        Ok(Some(_)) | Ok(None) => Err(Error::NotFound(id.clone())),
                        Err(e) => Err(e),
                    },
                );
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if changed {
                    publish_change_stamped(
                        &events,
                        store.as_ref(),
                        id,
                        namespace,
                        ChangeKind::Archived,
                    );
                }
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::AddLink { link, reply } => {
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| s.add_link(&link),
                );
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::RecordFeedback {
                namespace,
                id,
                kind,
                principal,
                reply,
            } => {
                // Capture the typed payload via the RenameNamespace/Scrub
                // pattern (run_store_op is Result<()>-only).
                let mut confidence = None;
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    // Verify the row lives in the caller's namespace AND is still
                    // active before mutating. The archived check is enforced HERE,
                    // at the single-writer serialization point, to close the TOCTOU
                    // the engine's pre-check (a pooled read) leaves open: a memory
                    // archived between that read and this command must not receive
                    // feedback. Fail closed (NotFound) on missing/cross-namespace/
                    // archived — feedback targets live, recallable memories.
                    |s| match s.get_memory(&id) {
                        Ok(Some(note))
                            if note.namespace == namespace && note.archived_at.is_none() =>
                        {
                            confidence =
                                Some(s.record_feedback(&id, kind, principal.as_deref())?);
                            Ok(())
                        }
                        Ok(Some(_)) | Ok(None) => Err(Error::NotFound(id.clone())),
                        Err(e) => Err(e),
                    },
                );
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let result = report.result.and_then(|()| {
                    confidence.ok_or_else(|| {
                        Error::Storage("feedback completed without an outcome".to_string())
                    })
                });
                let _ = reply.send(result);
                // Feedback nudges confidence (durable ranking state), so
                // subscribers observe it as an Updated change.
                if changed {
                    publish_change_stamped(
                        &events,
                        store.as_ref(),
                        id,
                        namespace,
                        ChangeKind::Updated,
                    );
                }
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::FlushAccesses { bumps, reply } => {
                // No MemoryChanged event: access tracking is observability-only.
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| s.record_access_bumps(&bumps),
                );
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
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| s.set_link_strength(&source, &target, link_type, strength),
                );
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::SetRecalibratedImportance {
                namespace,
                id,
                importance,
                reply,
            } => {
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    // Mirror the Update arm: verify the row lives in the
                    // caller's namespace before mutating, fail closed
                    // (NotFound) on a missing or cross-namespace id.
                    |s| match s.get_memory(&id) {
                        Ok(Some(note)) if note.namespace == namespace => {
                            s.set_recalibrated_importance(&id, importance)
                        }
                        Ok(Some(_)) | Ok(None) => Err(Error::NotFound(id.clone())),
                        Err(e) => Err(e),
                    },
                );
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if changed {
                    publish_change_stamped(
                        &events,
                        store.as_ref(),
                        id,
                        namespace,
                        ChangeKind::Updated,
                    );
                }
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
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| s.delete_link(&source, &target, link_type),
                );
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
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| {
                        // Mirror the Update/Archive arms: verify BOTH memories live in
                        // the caller's namespace before mutating, so the primitive can
                        // never merge across namespaces and the Archived event below is
                        // provably published under `old`'s real namespace. Fail closed
                        // (NotFound) on a missing or cross-namespace target.
                        match (s.get_memory(&old), s.get_memory(&new)) {
                            (Ok(Some(o)), Ok(Some(n)))
                                if o.namespace == namespace && n.namespace == namespace =>
                            {
                                s.supersede(&old, &new)
                            }
                            (Err(e), _) | (_, Err(e)) => Err(e),
                            _ => Err(Error::NotFound(old.clone())),
                        }
                    },
                );
                let changed = report.result.is_ok();
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                // supersede archives `old`; mirror the Archive arm so subscribers
                // observe the consolidation as an Archived event for `old`.
                if changed {
                    publish_change_stamped(
                        &events,
                        store.as_ref(),
                        old,
                        namespace,
                        ChangeKind::Archived,
                    );
                }
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::Reembed {
                id,
                embedding,
                model,
                input_version,
                reply,
            } => {
                // No MemoryChanged event: re-embed only refreshes the vector for
                // search quality; the note's user-visible content is unchanged.
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| s.update_vector(&id, &embedding, &model, &input_version),
                );
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::RenameNamespace {
                old,
                new,
                merge,
                reply,
            } => {
                // No MemoryChanged events for the bulk rename (see the
                // StoreHandle method doc); the store writes the durable
                // `namespace_rename` oplog row inside the same transaction.
                let mut outcome = None;
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| {
                        outcome = Some(s.rename_namespace(&old, &new, merge)?);
                        Ok(())
                    },
                );
                let writer_usable = report.writer_usable;
                let result = report.result.and_then(|()| {
                    outcome.ok_or_else(|| {
                        Error::Storage("namespace rename completed without an outcome".to_string())
                    })
                });
                let _ = reply.send(result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::Scrub { reply } => {
                // No MemoryChanged events: scrub rewrites text in place but
                // publishes no per-memory change (subscribers are not a
                // redaction audit channel); the store writes the durable
                // `scrub` oplog row inside the same transaction.
                let mut outcome = None;
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    |s| {
                        outcome = Some(s.scrub()?);
                        Ok(())
                    },
                );
                let writer_usable = report.writer_usable;
                let result = report.result.and_then(|()| {
                    outcome.ok_or_else(|| {
                        Error::Storage("scrub completed without an outcome".to_string())
                    })
                });
                let _ = reply.send(result);
                if !writer_usable {
                    break;
                }
            }
            #[cfg(test)]
            WriteCommand::PanicForTest { reply } => {
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    panic_for_test_store_op,
                );
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            #[cfg(test)]
            WriteCommand::PoisonForTest { reply } => {
                let report = run_store_op(
                    &mut store,
                    &db_path,
                    embedding_dim,
                    embedding_model.as_deref(),
                    poison_for_test_store_op,
                );
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            #[cfg(test)]
            WriteCommand::DieForTest { reply } => {
                // Exit WITHOUT disarming the death guard: the abnormal-exit
                // shape (e.g. a failed reopen) the daemon must react to.
                let _ = reply.send(Err(Error::Storage("writer killed for test".to_string())));
                break;
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
                death.disarm();
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

    async fn graph(&self, ns: Namespace, id: MemoryId, depth: u8) -> Result<Vec<(MemoryId, u8)>> {
        self.with_read(move |store| {
            let Some(anchor) = store.get_memory(&id)? else {
                return Ok(Vec::new());
            };
            if anchor.namespace != ns || anchor.archived_at.is_some() {
                return Ok(Vec::new());
            }

            // (id, hops) pairs with real minimum hop distances (W1.5). The
            // namespace/active filter drops pairs but preserves each survivor's
            // hop value — hops measure link-graph distance, not list position.
            let pairs = store.graph_neighbors(&id, depth)?;
            let mut filtered = Vec::with_capacity(pairs.len());
            for (graph_id, hops) in pairs {
                let Some(note) = store.get_memory(&graph_id)? else {
                    continue;
                };
                if note.namespace == ns && note.archived_at.is_none() {
                    filtered.push((graph_id, hops));
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

    async fn record_feedback(
        &self,
        ns: Namespace,
        id: MemoryId,
        kind: rb_types::FeedbackKind,
        principal: Option<String>,
    ) -> Result<f32> {
        // Bespoke send with a typed reply payload (the `rename_namespace`/`scrub`
        // precedent); `send_write` only carries `Result<()>`.
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(Error::Storage("writer thread unavailable".to_string()));
        }
        let (reply, rx) = oneshot::channel();
        self.writer_tx
            .send(WriteCommand::RecordFeedback {
                namespace: ns,
                id,
                kind,
                principal,
                reply,
            })
            .await
            .map_err(|_| Error::Storage("writer thread unavailable".to_string()))?;
        rx.await
            .map_err(|_| Error::Storage("writer dropped reply".to_string()))?
    }

    async fn record_access(&self, id: MemoryId) -> Result<()> {
        // W1.8: buffered, not written — recall/`get` issue ZERO writer ops.
        // The interval flusher (or shutdown) batches the bump to the writer.
        self.buffer_accesses(vec![id]).await;
        Ok(())
    }

    async fn record_accesses(&self, ids: Vec<MemoryId>) -> Result<()> {
        // W1.8: buffered, not written — recall/`get` issue ZERO writer ops.
        // The interval flusher (or shutdown) batches the bump to the writer.
        self.buffer_accesses(ids).await;
        Ok(())
    }

    async fn get_many(&self, ns: Namespace, ids: Vec<MemoryId>) -> Result<Vec<MemoryNote>> {
        self.with_read(move |store| store.get_many(&ns, &ids)).await
    }

    async fn active_contradicts(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> Result<std::collections::HashSet<MemoryId>> {
        self.with_read(move |store| store.active_contradicts(&ns, &ids))
            .await
    }

    async fn memories_for_reembed(
        &self,
        model: String,
        input_version: String,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        self.with_read(move |store| store.memories_for_reembed(&model, &input_version, limit))
            .await
    }

    async fn update_vector(
        &self,
        id: MemoryId,
        embedding: Vec<f32>,
        model: String,
        input_version: String,
    ) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::Reembed {
            id,
            embedding,
            model,
            input_version,
            reply,
        };
        self.send_write(cmd, rx).await
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
    async fn start_with_model_refuses_a_swapped_model_then_accept_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");

        // Seed the DB under one model identity (a real write so rows exist).
        let handle =
            StoreHandle::start_with_model(db.clone(), DIM, "deterministic".into(), 1).unwrap();
        let ns = Namespace::Project("model-swap".to_string());
        let mut seeded = note(&ns, "seeded under deterministic");
        seeded.embedding_input_version = "v2-composite".to_string();
        let id = seeded.id.clone();
        handle.write(seeded, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.shutdown().await;

        // A same-dim model swap must fail closed with the remediation hint.
        let err = StoreHandle::start_with_model(db.clone(), DIM, "voyage-3".into(), 1)
            .map(|_| ())
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("embedding model changed") && msg.contains("--accept-model-change"),
            "refusal must carry the hint: {msg}"
        );

        // Explicit opt-in: swap + stale, then the model-verified start succeeds.
        let changed = accept_model_change(&db, DIM, "voyage-3").unwrap();
        assert!(changed, "a real swap reports true");
        let handle = StoreHandle::start_with_model(db, DIM, "voyage-3".into(), 1).unwrap();
        let got = handle.get(ns, id).await.unwrap().unwrap();
        assert_eq!(
            got.embedding_input_version, "",
            "accepted swap stales the row to the reembed sentinel"
        );
        handle.shutdown().await;
    }

    #[test]
    fn accept_model_change_is_a_noop_for_a_missing_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("never-created.db");
        let changed = accept_model_change(&db, DIM, "voyage-3").unwrap();
        assert!(!changed, "nothing to accept on a fresh install");
        assert!(!db.exists(), "the opt-in path must not create the DB");
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

        // record_access buffers (W1.8); the flush applies the bump in one
        // batched writer op.
        handle.record_access(aid.clone()).await.unwrap();
        handle.flush_accesses().await.unwrap();
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
    async fn writer_recovers_after_an_op_leaves_an_open_transaction() {
        // W1.6b: a completed-with-Err op that strands the connection inside a
        // transaction (failed COMMIT + failed ROLLBACK shape) must trigger the
        // drop+reopen path, so subsequent writes commit on a clean connection
        // instead of dying with "cannot start a transaction within a
        // transaction".
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("txn-poison".to_string());

        // 1. A successful write before the poisoned op.
        let before = note(&ns, "written before the poison");
        let before_id = before.id.clone();
        handle.write(before, Some(vec![0.1f32; DIM])).await.unwrap();

        // 2. Inject the mid-transaction failure via the test-only command.
        let (reply, rx) = oneshot::channel();
        handle
            .writer_tx
            .send(WriteCommand::PoisonForTest { reply })
            .await
            .unwrap();
        let err = rx.await.unwrap().unwrap_err();
        assert!(
            matches!(err, Error::Storage(_)),
            "the poisoned op's own error reaches the caller, got {err:?}"
        );

        // 3. The next write commits: the writer detected !is_autocommit and
        //    reopened its connection rather than running inside the leftover
        //    transaction.
        let after = note(&ns, "written after the poison");
        let after_id = after.id.clone();
        handle.write(after, Some(vec![0.2f32; DIM])).await.unwrap();

        assert!(
            handle.get(ns.clone(), before_id).await.unwrap().is_some(),
            "pre-poison write must survive"
        );
        assert!(
            handle.get(ns.clone(), after_id).await.unwrap().is_some(),
            "post-poison write must commit on the reopened connection"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abnormal_writer_death_resolves_writer_died() {
        // W1.6c: an abnormal writer exit must resolve the death signal the
        // daemon races in its accept loop.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();

        let died = handle.writer_died();
        handle.kill_writer_for_test().await;

        tokio::time::timeout(std::time::Duration::from_secs(5), died)
            .await
            .expect("writer_died must resolve after an abnormal writer exit");

        // The dead writer rejects further writes (no silent acceptance).
        let ns = Namespace::Project("dead-writer".to_string());
        let err = handle
            .write(note(&ns, "after death"), Some(vec![0.1f32; DIM]))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Storage(_)), "got {err:?}");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graceful_shutdown_does_not_resolve_writer_died() {
        // The death signal is for ABNORMAL exits only: a clean shutdown must
        // never trip the daemon's writer-death arm.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();

        let died = handle.writer_died();
        handle.shutdown().await;

        let raced = tokio::time::timeout(std::time::Duration::from_millis(300), died).await;
        assert!(
            raced.is_err(),
            "writer_died must stay pending across a graceful shutdown"
        );
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
        // The namespace guard loads `new` first and fails closed when it does not
        // exist, before ever touching `old`.
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");
        let got_old = handle
            .get(ns.clone(), old_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(got_old.superseded_by.is_none(), "old untouched: no pointer");
        assert!(got_old.archived_at.is_none(), "old untouched: not archived");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_supersede_rejects_cross_namespace_target() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns_a = Namespace::Project("a".to_string());
        let ns_b = Namespace::Project("b".to_string());

        let old = note(&ns_a, "a-old");
        let new = note(&ns_b, "b-new");
        let (old_id, new_id) = (old.id.clone(), new.id.clone());
        handle.write(old, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(new, Some(vec![0.2f32; DIM])).await.unwrap();

        // Superseding an A memory into a B memory must fail closed: the namespace
        // guard refuses to merge across namespaces even though `new` exists.
        let err = handle
            .supersede(ns_a.clone(), old_id.clone(), new_id.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "got {err:?}");

        let got_old = handle
            .get(ns_a.clone(), old_id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(
            got_old.superseded_by.is_none(),
            "no cross-namespace pointer"
        );
        assert!(got_old.archived_at.is_none(), "old untouched: not archived");

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

        // A write's MemoryChanged event can be published just after its write reply
        // returns, so a late Created event for `old`/`new` may still land on this
        // freshly-subscribed receiver ahead of the supersede event. Drain until the
        // Archived event for the absorbed (old) memory rather than assuming it is
        // strictly first (de-flakes the subscribe/write-event race seen under CI
        // scheduling; the supersede call above guarantees the event is sent).
        let mut archived = None;
        for _ in 0..6 {
            match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
                Ok(Ok(evt))
                    if evt.kind == crate::change::ChangeKind::Archived && evt.id == old_id =>
                {
                    archived = Some(evt);
                    break;
                }
                Ok(Ok(_)) => continue, // a stray Created event from the prior writes
                _ => break,
            }
        }
        let evt = archived.expect("supersede must publish an Archived event for the old memory");
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_reembed_scan_and_update_vector_through_single_writer() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("reembed".to_string());

        // Write a row with a stale stamp (as if remembered pre-P5).
        let mut stale = note(&ns, "stale stamped row");
        stale.embedding_model = "old-model".to_string();
        stale.embedding_input_version = "v1-content-only".to_string();
        let id = stale.id.clone();
        handle.write(stale, Some(vec![0.1f32; DIM])).await.unwrap();

        // The scan finds it (cross-namespace read pool path).
        let cands = handle
            .memories_for_reembed("deterministic".to_string(), "v2-composite".to_string(), 100)
            .await
            .unwrap();
        assert!(cands.iter().any(|n| n.id == id), "stale row is a candidate");

        // Re-embed it through the single writer; the row is stamped current.
        handle
            .update_vector(
                id.clone(),
                vec![0.5f32; DIM],
                "deterministic".to_string(),
                "v2-composite".to_string(),
            )
            .await
            .unwrap();
        let after = handle.get(ns.clone(), id.clone()).await.unwrap().unwrap();
        assert_eq!(after.embedding_model, "deterministic");
        assert_eq!(after.embedding_input_version, "v2-composite");

        // Idempotent: a second scan finds nothing stale, so a re-run writes 0.
        let cands2 = handle
            .memories_for_reembed("deterministic".to_string(), "v2-composite".to_string(), 100)
            .await
            .unwrap();
        assert!(cands2.iter().all(|n| n.id != id), "row no longer stale");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_update_vector_rejects_wrong_dimension() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        let ns = Namespace::Project("dim".to_string());

        let n = note(&ns, "dim contract");
        let id = n.id.clone();
        handle.write(n, Some(vec![0.1f32; DIM])).await.unwrap();

        // A wrong-length vector fails closed (dim contract is unchanged).
        let err = handle
            .update_vector(
                id,
                vec![0.5f32; DIM + 3],
                "deterministic".to_string(),
                "v2-composite".to_string(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::DimensionMismatch { .. }),
            "got {err:?}"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_memories_for_recalibration_reads_access_fields() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("recal".to_string());

        let m = note(&ns, "accessed twice");
        let id = m.id.clone();
        handle.write(m, Some(vec![0.1f32; DIM])).await.unwrap();

        // Two accesses bump access_count to 2 and stamp last_accessed_at.
        // Buffered (W1.8): both accumulate in memory, one flush persists them.
        handle.record_access(id.clone()).await.unwrap();
        handle.record_access(id.clone()).await.unwrap();
        handle.flush_accesses().await.unwrap();

        let rows = handle.memories_for_recalibration(100).await.unwrap();
        let row = rows
            .iter()
            .find(|r| r.id == id)
            .expect("recal row must be present");
        assert_eq!(row.namespace, ns);
        assert_eq!(row.access_count, 2, "two record_access calls => count 2");
        assert!(
            row.last_accessed_at.is_some(),
            "last_accessed_at must be stamped after record_access"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_set_recalibrated_importance_is_namespace_scoped_and_keeps_prior() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("recal-write".to_string());

        let m = note(&ns, "anchored to its author prior");
        let id = m.id.clone();
        let base = m.importance;
        handle.write(m, Some(vec![0.1f32; DIM])).await.unwrap();

        // Cross-namespace write fails closed, exactly like Update.
        let err = handle
            .set_recalibrated_importance(Namespace::Global, id.clone(), 7)
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::NotFound(_)),
            "cross-namespace job write must be NotFound, got {err:?}"
        );

        // In-namespace write moves the EFFECTIVE importance only (W1.9).
        handle
            .set_recalibrated_importance(ns.clone(), id.clone(), 7)
            .await
            .unwrap();
        let after = handle
            .get(ns.clone(), id.clone())
            .await
            .unwrap()
            .expect("memory present");
        assert_eq!(after.importance, 7, "effective importance moved");
        let row = handle
            .memories_for_recalibration(10)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .expect("recal row present");
        assert_eq!(
            row.base_importance, base,
            "the author prior must survive the job write"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_record_feedback_is_namespace_scoped_and_nudges_confidence() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("feedback-write".to_string());

        let mut m = note(&ns, "a decision worth grading");
        m.confidence = 0.8;
        let id = m.id.clone();
        handle.write(m, Some(vec![0.1f32; DIM])).await.unwrap();

        // Cross-namespace feedback fails closed, exactly like Update.
        let err = handle
            .record_feedback(
                Namespace::Global,
                id.clone(),
                rb_types::FeedbackKind::Wrong,
                Some("alice".to_string()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::NotFound(_)),
            "cross-namespace feedback must be NotFound, got {err:?}"
        );

        // In-namespace `wrong` lowers confidence through the single writer and
        // the typed reply carries the post-nudge value.
        let after = handle
            .record_feedback(
                ns.clone(),
                id.clone(),
                rb_types::FeedbackKind::Wrong,
                Some("alice".to_string()),
            )
            .await
            .unwrap();
        assert!((after - 0.5).abs() < 1e-6, "0.8 - 0.30 = 0.50, got {after}");
        let stored = handle
            .get(ns.clone(), id.clone())
            .await
            .unwrap()
            .expect("memory present")
            .confidence;
        assert!((stored - 0.5).abs() < 1e-6, "the row reflects the nudge");

        // Once archived, the writer-level guard rejects feedback (NotFound),
        // closing the TOCTOU even when the engine's pre-check is bypassed.
        handle.archive(ns.clone(), id.clone()).await.unwrap();
        let err = handle
            .record_feedback(
                ns.clone(),
                id.clone(),
                rb_types::FeedbackKind::Helpful,
                None,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, Error::NotFound(_)),
            "feedback on an archived row must be NotFound at the writer, got {err:?}"
        );
        // The archived row's confidence is untouched by the rejected feedback.
        let unchanged = handle
            .get(ns, id)
            .await
            .unwrap()
            .expect("memory present")
            .confidence;
        assert!(
            (unchanged - 0.5).abs() < 1e-6,
            "rejected feedback left confidence intact"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recall_issues_zero_writer_ops_and_defers_access_bumps() {
        // W1.8 / Phase 1 gate: a full hybrid recall through the real engine
        // over the real StoreHandle must enqueue NOTHING on the writer thread.
        // Access bumps are buffered in memory and applied later as ONE batched
        // writer op.
        use rb_embed::DeterministicProvider;

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("zero-writer-recall".to_string());

        let a = note(&ns, "writer thread serializes mutations");
        let b = note(&ns, "writer thread accepts mutations");
        let (aid, _bid) = (a.id.clone(), b.id.clone());
        handle.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        let engine = rb_engine::MemoryEngine::new(
            handle.clone(),
            crate::SharedEmbedder::new(DeterministicProvider::new(DIM)),
            ns.clone(),
        );

        let ops_before = handle.writer_ops_count();
        let results = engine
            .recall("writer thread mutations", 5, None, &[])
            .await
            .unwrap();
        assert!(!results.is_empty(), "keyword overlap must return results");
        assert_eq!(
            handle.writer_ops_count(),
            ops_before,
            "recall must issue ZERO writer-thread ops"
        );

        // The bumps were buffered (one entry per returned id), not persisted.
        assert_eq!(
            handle.pending_access_len_for_test().await,
            results.len(),
            "every returned id buffers exactly one pending bump"
        );
        let unflushed = handle.get(ns.clone(), aid.clone()).await.unwrap().unwrap();
        assert_eq!(unflushed.access_count, 0, "bump not yet visible in the DB");

        // One flush persists ALL bumps in a single batched writer op.
        handle.flush_accesses().await.unwrap();
        let flushed = handle.get(ns.clone(), aid).await.unwrap().unwrap();
        assert_eq!(flushed.access_count, 1, "flush applied the buffered bump");
        assert!(flushed.last_accessed_at.is_some());
        assert_eq!(
            handle.writer_ops_count(),
            ops_before + 1,
            "the whole recall's access tracking costs exactly one writer op"
        );

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn namespace_stats_runs_on_the_read_pool_with_zero_writer_ops() {
        // W1.8 mirror for the stats path (doctor/stats PRD hard constraint):
        // the whole aggregation runs on the read pool and must enqueue NOTHING
        // on the writer thread — which also proves it triggers zero FTS writes
        // (every FTS write rides a writer op).
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("zero-writer-stats".to_string());

        let a = note(&ns, "stats aggregate over feedback");
        let b = note(&ns, "stats aggregate over accesses");
        let (aid, bid) = (a.id.clone(), b.id.clone());
        handle.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(b, Some(vec![0.2f32; DIM])).await.unwrap();
        handle
            .record_feedback(ns.clone(), aid, rb_types::FeedbackKind::Helpful, None)
            .await
            .unwrap();
        handle
            .record_feedback(ns.clone(), bid.clone(), rb_types::FeedbackKind::Wrong, None)
            .await
            .unwrap();
        handle
            .record_feedback(ns.clone(), bid, rb_types::FeedbackKind::Stale, None)
            .await
            .unwrap();

        let ops_before = handle.writer_ops_count();
        let stats = handle
            .namespace_stats(ns.clone(), 30, String::new(), String::new(), 5)
            .await
            .unwrap();
        assert_eq!(
            handle.writer_ops_count(),
            ops_before,
            "stats must issue ZERO writer-thread ops (W1.8)"
        );

        assert_eq!(stats.namespace, ns.as_db_string());
        assert_eq!(stats.live, 2);
        assert_eq!(stats.feedback.helpful, 1);
        assert_eq!(stats.feedback.wrong, 1);
        assert_eq!(stats.feedback.stale, 1);

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn writer_alive_reports_liveness_and_flips_on_death() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        assert!(handle.writer_alive(), "a fresh handle has a live writer");

        handle.kill_writer_for_test().await;
        // The death watch flips asynchronously with the thread exit; poll.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while handle.writer_alive() && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            !handle.writer_alive(),
            "an abnormally dead writer must report not-alive"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn background_flusher_persists_buffered_bumps_without_explicit_flush() {
        // The interval flusher (spawned lazily on the first buffered access)
        // must persist access stats on its own — eventual consistency, no
        // caller-driven flush.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        let ns = Namespace::Project("bg-flush".to_string());

        let n = note(&ns, "eventually counted");
        let id = n.id.clone();
        handle.write(n, Some(vec![0.1f32; DIM])).await.unwrap();

        handle.record_access(id.clone()).await.unwrap();

        // Poll until the background flush lands (interval is 2s; allow ample
        // slack for slow CI schedulers before declaring failure).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut seen = 0;
        while std::time::Instant::now() < deadline {
            seen = handle
                .get(ns.clone(), id.clone())
                .await
                .unwrap()
                .map(|n| n.access_count)
                .unwrap_or(0);
            if seen == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(seen, 1, "interval flusher must persist the buffered bump");

        handle.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_flushes_pending_access_bumps() {
        // Buffered bumps must survive a graceful shutdown: the final drain in
        // `shutdown` persists them before the write queue closes.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let ns = Namespace::Project("shutdown-flush".to_string());

        let id;
        {
            let handle = StoreHandle::start(db.clone(), DIM, 1).unwrap();
            let n = note(&ns, "counted across shutdown");
            id = n.id.clone();
            handle.write(n, Some(vec![0.1f32; DIM])).await.unwrap();
            // Two buffered accesses accumulate count=2 for the id.
            handle.record_access(id.clone()).await.unwrap();
            handle.record_access(id.clone()).await.unwrap();
            handle.shutdown().await;
        }

        let reopened = StoreHandle::start(db, DIM, 1).unwrap();
        let got = reopened.get(ns, id).await.unwrap().unwrap();
        assert_eq!(
            got.access_count, 2,
            "shutdown must flush accumulated bumps (count preserved, not 1)"
        );
        assert!(got.last_accessed_at.is_some());
        reopened.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn record_accesses_dedups_within_call_and_accumulates_across_calls() {
        // Mirrors the old single-UPDATE semantics: duplicates within ONE call
        // bump once; separate calls accumulate.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        let ns = Namespace::Project("dedup".to_string());

        let n = note(&ns, "dedup target");
        let id = n.id.clone();
        handle.write(n, Some(vec![0.1f32; DIM])).await.unwrap();

        handle
            .record_accesses(vec![id.clone(), id.clone()])
            .await
            .unwrap();
        handle.record_access(id.clone()).await.unwrap();
        handle.flush_accesses().await.unwrap();

        let got = handle.get(ns, id).await.unwrap().unwrap();
        assert_eq!(
            got.access_count, 2,
            "within-call duplicate bumps once; the second call adds one more"
        );

        handle.shutdown().await;
    }
}
