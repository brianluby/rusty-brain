//! End-to-end recording-time `as_of` lists (Vikunja #102) over a real Unix
//! socket: the handshake capability, the as-of envelope, the daemon's
//! 1,000-row clamp with `truncated` + `next_after`, and the validation
//! errors — the wire half of the #88 assessment's test table.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use rb_daemon::{Daemon, DaemonConfig, SharedEmbedder};
use rb_embed::DeterministicProvider;
use rb_proto::{Client, Request, Response};
use rb_store::{SqliteStore, Store as _};
use rb_types::{
    AsOfPage, Error, MemoryId, MemoryNote, MemoryState, MemoryType, MetadataBasis, Namespace,
    RecallFilter, TemporalBasis, TimePrecision,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DIM: usize = 8;
/// 2026-07-01T00:00:00Z.
const JUL_1: i64 = 1_782_864_000;
const DAY: i64 = 86_400;

fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(secs, 0).unwrap()
}

fn ns() -> Namespace {
    Namespace::Project("as-of-e2e".into())
}

struct RunningDaemon {
    socket: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl RunningDaemon {
    /// Seed the database with controlled recording times BEFORE the daemon
    /// opens it, then start the daemon on it.
    async fn start_seeded(seed: impl FnOnce(&SqliteStore)) -> RunningDaemon {
        Self::start_seeded_with(seed, SharedEmbedder::new(DeterministicProvider::new(DIM))).await
    }

    async fn start_seeded_with(
        seed: impl FnOnce(&SqliteStore),
        embedder: SharedEmbedder,
    ) -> RunningDaemon {
        let dir = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let socket = dir.path().join("runtime").join("sock");
        let db = dir.path().join("memory.db");
        {
            let store = SqliteStore::open_with_model(&db, DIM, "deterministic").unwrap();
            seed(&store);
        }
        let cfg = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db,
            read_pool_size: 2,
            jobs_config: rb_daemon::JobsConfig::default(),
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_engine::FusionMode::Linear,
            abstain_threshold: Some(0.0),
            write_gate: rb_types::WriteGateConfig::default(),
            http: None,
        };
        let daemon = Daemon::bind(cfg, embedder).await.unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            daemon
                .run(async move {
                    let _ = rx.await;
                })
                .await
                .unwrap();
        });
        wait_for(&socket).await;
        RunningDaemon {
            socket,
            shutdown: Some(tx),
            task: Some(task),
            _dir: dir,
        }
    }

    async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("daemon task did not shut down within 5s")
                .expect("daemon task failed during shutdown");
        }
    }
}

async fn wait_for(socket: &Path) {
    for _ in 0..200 {
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("daemon socket not reachable at {}", socket.display());
}

fn plant(
    store: &SqliteStore,
    ns: &Namespace,
    summary: &str,
    created: i64,
    archived: Option<i64>,
    superseded_by: Option<&MemoryId>,
) -> MemoryId {
    let mut note = MemoryNote::new(
        ns.clone(),
        format!("content for {summary}"),
        MemoryType::ArchitectureDecision,
        6,
    );
    note.summary = summary.to_string();
    note.created_at = at(created);
    note.updated_at = at(archived.unwrap_or(created));
    note.archived_at = archived.map(at);
    note.superseded_by = superseded_by.cloned();
    let id = note.id.clone();
    store.insert_memory(&note, None).unwrap();
    id
}

fn as_of(t: i64) -> RecallFilter {
    RecallFilter {
        as_of: Some(at(t)),
        ..Default::default()
    }
}

fn is_invalid_argument(resp: &Response) -> bool {
    matches!(resp, Response::Error { kind, .. } if kind == "invalid_argument")
}

fn assert_recorded_envelope(page: &AsOfPage, t: i64) {
    assert_eq!(page.as_of, at(t));
    assert_eq!(page.temporal_basis, TemporalBasis::Recorded);
    assert_eq!(page.precision, TimePrecision::Second);
    assert_eq!(page.metadata, MetadataBasis::Current);
    assert!(
        page.notes
            .iter()
            .any(|n| n.contains("purged rows never appear")),
        "the purge limit is stated: {:?}",
        page.notes
    );
}

/// Tests 6 and 10 over the wire: the late-recorded event answers M at Sep 15
/// with `temporal_basis: "recorded"`, and the envelope says the metadata is
/// current. The raw JSON carries the documented keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_recorded_event_is_labelled_as_recorded_with_current_metadata() {
    let (jun_1, sep_15, oct_10) = (JUL_1 - 30 * DAY, JUL_1 + 76 * DAY, JUL_1 + 101 * DAY);
    let mut m_id = None;
    let daemon = RunningDaemon::start_seeded(|store| {
        let p = plant(
            store,
            &ns(),
            "migrated to Postgres on Sep 1",
            oct_10,
            None,
            None,
        );
        m_id = Some(plant(
            store,
            &ns(),
            "Production runs MySQL",
            jun_1,
            Some(oct_10),
            Some(&p),
        ));
    })
    .await;
    let m_id = m_id.unwrap();
    let mut client = Client::connect(&daemon.socket, ns()).await.unwrap();
    assert!(client.supports_as_of(), "the daemon advertises `as_of`");

    let (rows, page) = client.list_as_of(as_of(sep_15), 20).await.unwrap();
    assert_eq!(
        rows.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
        vec![m_id]
    );
    assert_recorded_envelope(&page, sep_15);
    assert!(!page.truncated);
    assert!(page.next_after.is_none());

    let raw = client
        .request(Request::List {
            min_importance: None,
            limit: 20,
            filter: as_of(sep_15),
        })
        .await
        .unwrap();
    let json = serde_json::to_value(&raw).unwrap();
    assert_eq!(json["as_of"]["temporal_basis"], "recorded");
    assert_eq!(json["as_of"]["precision"], "second");
    assert_eq!(json["as_of"]["metadata"], "current");
    assert_eq!(json["as_of"]["truncated"], false);

    // A plain list is byte-compatible: no as-of envelope at all.
    let plain = client
        .request(Request::List {
            min_importance: None,
            limit: 20,
            filter: RecallFilter::default(),
        })
        .await
        .unwrap();
    assert!(matches!(plain, Response::Listed { .. }));
    drop(client);
    daemon.stop().await;
}

/// Test 13 over the wire: more members than the effective limit — a limit of
/// 20, and a request above the daemon's 1,000-row clamp — report `truncated`
/// with a `next_after` cursor, and following the cursor returns every member
/// exactly once, in `(created_at, memory_id)` order, across equal seconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn truncated_pages_follow_the_cursor_without_gaps_or_duplicates() {
    const MEMBERS: i64 = 1_050;
    let t = JUL_1 + DAY;
    let daemon = RunningDaemon::start_seeded(|store| {
        for i in 0..MEMBERS {
            // Seven distinct seconds: ties at every page boundary.
            plant(store, &ns(), &format!("row {i}"), JUL_1 + i % 7, None, None);
        }
        // Non-members at T: created after T, or archived before it.
        plant(store, &ns(), "later", t + 1, None, None);
        plant(store, &ns(), "gone", JUL_1, Some(JUL_1 + 3), None);
    })
    .await;
    let mut client = Client::connect(&daemon.socket, ns()).await.unwrap();

    for limit in [20usize, 5_000] {
        let mut filter = as_of(t);
        let mut seen: Vec<(i64, String)> = Vec::new();
        let mut pages = 0;
        loop {
            let (rows, page) = client.list_as_of(filter.clone(), limit).await.unwrap();
            assert_recorded_envelope(&page, t);
            pages += 1;
            assert!(rows.len() <= limit.min(1_000), "the daemon clamps to 1,000");
            seen.extend(
                rows.iter()
                    .map(|n| (n.created_at.timestamp(), n.id.to_string())),
            );
            if !page.truncated {
                assert!(page.next_after.is_none());
                break;
            }
            let cursor = page.next_after.expect("truncated pages carry a cursor");
            let last = rows.last().unwrap();
            assert_eq!(
                (cursor.created_at, &cursor.memory_id),
                (last.created_at, &last.id)
            );
            filter.as_of_after = Some(cursor);
        }
        assert_eq!(seen.len(), MEMBERS as usize, "limit {limit}: every member");
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, seen, "limit {limit}: ordered, no duplicates");
        let expected_pages = (MEMBERS as usize).div_ceil(limit.min(1_000));
        assert_eq!(pages, expected_pages, "limit {limit}");
    }
    drop(client);
    daemon.stop().await;
}

/// Tests 7 and 8 over the wire: `as_of` with a non-default state, a
/// sub-second instant, or ranked recall is a validation error; active list
/// and recall without `as_of` are untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn as_of_validation_errors_and_recall_is_unchanged() {
    let daemon = RunningDaemon::start_seeded(|store| {
        plant(store, &ns(), "fusion uses RRF", JUL_1, None, None);
    })
    .await;
    let mut client = Client::connect(&daemon.socket, ns()).await.unwrap();

    for state in [MemoryState::Archived, MemoryState::All] {
        let filter = RecallFilter {
            state,
            ..as_of(JUL_1)
        };
        let err = client.list_as_of(filter.clone(), 5).await.unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(_)),
            "{state:?}: {err:?}"
        );
        // The daemon itself rejects it too (raw frame, no client check).
        let raw = client
            .request(Request::List {
                min_importance: None,
                limit: 5,
                filter,
            })
            .await
            .unwrap();
        assert!(is_invalid_argument(&raw), "{raw:?}");
    }
    let sub_second = RecallFilter {
        as_of: Some(at(JUL_1) + chrono::Duration::milliseconds(250)),
        ..Default::default()
    };
    let raw = client
        .request(Request::List {
            min_importance: None,
            limit: 5,
            filter: sub_second,
        })
        .await
        .unwrap();
    assert!(is_invalid_argument(&raw), "{raw:?}");

    let err = client
        .recall_filtered_with_status("fusion".into(), as_of(JUL_1), 5)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::InvalidArgument(m) if m.contains("list only")),
        "{err:?}"
    );
    let (results, _, _, _) = client
        .recall_filtered_with_status("fusion".into(), RecallFilter::default(), 5)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "active recall unchanged without as_of");
    drop(client);
    daemon.stop().await;
}

/// An embedder that takes longer than a second, so a write composed before
/// embedding would carry a `created_at` from a second it was not yet stored.
struct SlowEmbedder(DeterministicProvider);

#[async_trait::async_trait]
impl rb_embed::EmbeddingProvider for SlowEmbedder {
    fn model_id(&self) -> &str {
        self.0.model_id()
    }

    fn dim(&self) -> usize {
        self.0.dim()
    }

    async fn embed(
        &self,
        texts: &[String],
        kind: rb_embed::EmbedKind,
    ) -> rb_types::Result<Vec<Vec<f32>>> {
        if kind == rb_embed::EmbedKind::Document {
            tokio::time::sleep(Duration::from_millis(1_200)).await;
        }
        self.0.embed(texts, kind).await
    }
}

/// PR #109 review (Codex P1) over the wire: `remember` composes (and used
/// to stamp) a note before embedding, so a slow embedding made `as_of`
/// report it as present before it was stored, and a superseding write showed
/// both versions. Recording time is now the write transaction's: nothing is
/// a member before its write, and the supersede hands off in one instant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remember_is_recorded_at_write_time_not_compose_time() {
    let daemon = RunningDaemon::start_seeded_with(
        |_| {},
        SharedEmbedder::new(SlowEmbedder(DeterministicProvider::new(DIM))),
    )
    .await;
    let mut client = Client::connect(&daemon.socket, ns()).await.unwrap();

    let composed = chrono::Utc::now().timestamp();
    let old = client
        .remember(
            "the fusion decision is RRF".into(),
            None,
            MemoryType::ArchitectureDecision,
            6,
            vec![],
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();
    let old_row = client.get(old.clone()).await.unwrap().unwrap();
    assert!(
        old_row.created_at.timestamp() > composed,
        "stamped after the 1.2s embedding, not at compose time"
    );
    let (rows, _) = client.list_as_of(as_of(composed), 20).await.unwrap();
    assert!(rows.is_empty(), "not a member before it was written");

    let new = client
        .remember_superseding(
            "the fusion decision is Linear".into(),
            None,
            MemoryType::ArchitectureDecision,
            6,
            vec![],
            vec![],
            vec![],
            None,
            old.clone(),
        )
        .await
        .unwrap();
    let new_row = client.get(new.clone()).await.unwrap().unwrap();
    let old_row = client.get(old.clone()).await.unwrap().unwrap();
    let handoff = new_row.created_at.timestamp();
    assert_eq!(old_row.archived_at.map(|t| t.timestamp()), Some(handoff));
    for t in [
        old_row.created_at.timestamp(),
        handoff - 1,
        handoff,
        handoff + 1,
    ] {
        let (rows, _) = client.list_as_of(as_of(t), 20).await.unwrap();
        let ids: Vec<_> = rows.into_iter().map(|n| n.id).collect();
        let want = if t < handoff { &old } else { &new };
        assert_eq!(ids, vec![want.clone()], "exactly one version at {t}");
    }
    drop(client);
    daemon.stop().await;
}
