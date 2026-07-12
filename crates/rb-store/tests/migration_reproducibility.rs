//! Migration reproducibility gate (anti-ghost-migration guard).
//!
//! Builds a FRESH temp-file database via `SqliteStore::open` (committed
//! migrations only + the dynamic vector table) and exercises EVERY query
//! path on the `Store` trait. If any column or table the code references is
//! missing, one of these calls fails and so does this test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_store::{SqliteStore, Store};
use rb_types::{LinkType, MemoryId, MemoryLink, MemoryNote, MemoryType, MemoryUpdates, Namespace};

const DIM: usize = 4;

fn note(ns: &Namespace, content: &str, ty: MemoryType, importance: u8) -> MemoryNote {
    MemoryNote::new(ns.clone(), content.to_string(), ty, importance)
}

#[test]
fn fresh_db_exercises_every_query_path() {
    // Fresh, isolated file-backed DB built only from committed migrations.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("repro.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();

    let ns = Namespace::Project("repro".to_string());

    // --- insert_memory: memory + FTS row (via trigger) + vector + (no links yet)
    let mut a = note(
        &ns,
        "rusqlite WAL mode enables concurrent readers",
        MemoryType::Insight,
        7,
    );
    a.summary = "wal enables concurrent readers".to_string();
    a.keywords = vec!["wal".to_string(), "sqlite".to_string()];
    a.tags = vec!["db".to_string()];
    let emb_a: [f32; DIM] = [1.0, 0.0, 0.0, 0.0];
    store.insert_memory(&a, Some(&emb_a)).unwrap();

    let mut b = note(
        &ns,
        "sqlite-vec provides brute-force KNN search",
        MemoryType::Reference,
        5,
    );
    b.summary = "sqlite-vec knn".to_string();
    b.keywords = vec!["sqlite".to_string(), "vector".to_string()];
    b.tags = vec!["db".to_string(), "search".to_string()];
    // Typed code anchors (migration 009): populated so the anchor
    // insert/load/filter paths are all exercised by this gate.
    b.anchors = vec![
        rb_types::MemoryAnchor::parse_file_spec("src/store/core.rs:10-20").unwrap(),
        rb_types::MemoryAnchor::new(rb_types::AnchorKind::Commit, "abc123def").unwrap(),
        rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "SqliteStore::open").unwrap(),
    ];
    let emb_b: [f32; DIM] = [0.0, 1.0, 0.0, 0.0];
    store.insert_memory(&b, Some(&emb_b)).unwrap();

    // --- get_memory: decode explicit columns + links
    let got = store.get_memory(&a.id).unwrap();
    assert!(got.is_some(), "inserted memory must be retrievable");
    let got = got.unwrap();
    assert_eq!(got.id, a.id);
    assert_eq!(got.content, a.content);
    assert_eq!(got.summary, a.summary);
    assert_eq!(got.keywords, a.keywords);
    assert_eq!(got.tags, a.tags);
    assert_eq!(got.memory_type, MemoryType::Insight);
    assert_eq!(got.importance, 7);
    assert!(got.archived_at.is_none(), "fresh memory is active");

    // get on a missing id returns Ok(None), not an error.
    let missing = MemoryId::new();
    assert!(store.get_memory(&missing).unwrap().is_none());

    // --- keyword_search: FTS5, scoped to ns, active only
    let kw = store.keyword_search(&ns, "concurrent", 10).unwrap();
    assert!(
        kw.contains(&a.id),
        "FTS must match content token 'concurrent'"
    );
    assert!(!kw.contains(&b.id), "b does not contain 'concurrent'");

    // FTS over a summary/keyword column too.
    let kw2 = store.keyword_search(&ns, "knn", 10).unwrap();
    assert!(
        kw2.contains(&b.id),
        "FTS must match summary/keyword token 'knn'"
    );

    // --- vector_search: sqlite-vec vec0 KNN; closest to emb_a is a.
    let hits = store.vector_search(&ns, &emb_a, 2).unwrap();
    assert!(!hits.is_empty(), "vector search returns candidates");
    assert_eq!(hits[0].0, a.id, "nearest neighbour of emb_a must be a");
    // distance is finite and non-negative.
    assert!(hits[0].1.is_finite() && hits[0].1 >= 0.0);

    // --- add_link + graph_neighbors: recursive CTE over memory_links
    let link = MemoryLink {
        source_id: a.id.clone(),
        target_id: b.id.clone(),
        link_type: LinkType::References,
        strength: 0.9,
        reason: "a cites b".to_string(),
        created_at: chrono::Utc::now(),
    };
    store.add_link(&link).unwrap();

    let neighbors = store.graph_neighbors(&a.id, 1).unwrap();
    assert!(
        neighbors.contains(&(b.id.clone(), 1)),
        "graph_neighbors must reach b from a at hop distance 1"
    );

    // --- anchors (migration 009): round-trip through get + the filter path
    let got_b = store.get_memory(&b.id).unwrap().unwrap();
    assert_eq!(got_b.anchors, b.anchors, "anchors must round-trip");
    assert!(
        got.anchors.is_empty(),
        "an anchor-less row loads an empty list (no backfill)"
    );
    let anchored = store
        .list_filtered(
            &ns,
            &rb_types::RecallFilter {
                anchors: vec![rb_types::AnchorFilter {
                    kind: rb_types::AnchorKind::File,
                    value: "src/store/core.rs".to_string(),
                }],
                ..Default::default()
            },
            10,
        )
        .unwrap();
    assert_eq!(
        anchored.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        vec![b.id.clone()],
        "anchor-filtered list must reach the anchored row"
    );

    // --- list: active only, ORDER BY created_at DESC
    let listed = store.list(&ns, None, 10).unwrap();
    let listed_ids: Vec<_> = listed.iter().map(|m| m.id.clone()).collect();
    assert!(listed_ids.contains(&a.id) && listed_ids.contains(&b.id));

    // min_importance filter excludes the importance-5 note.
    let important = store.list(&ns, Some(6), 10).unwrap();
    let important_ids: Vec<_> = important.iter().map(|m| m.id.clone()).collect();
    assert!(important_ids.contains(&a.id));
    assert!(
        !important_ids.contains(&b.id),
        "importance 5 < 6 must be excluded"
    );

    // --- update_memory: bumps updated_at, keeps FTS in sync
    let updates = MemoryUpdates {
        content: Some("updated: WAL plus sqlite-vec in one transaction".to_string()),
        summary: Some("updated summary".to_string()),
        importance: Some(9),
        tags: Some(vec!["db".to_string(), "updated".to_string()]),
        context: Some("ctx".to_string()),
        confidence: Some(0.8),
    };
    store.update_memory(&a.id, &updates).unwrap();
    let after = store.get_memory(&a.id).unwrap().unwrap();
    assert_eq!(
        after.content,
        "updated: WAL plus sqlite-vec in one transaction"
    );
    assert_eq!(after.importance, 9);
    assert!(
        after.updated_at >= after.created_at,
        "updated_at must be bumped"
    );
    // FTS reflects the NEW content: searching a new token finds a.
    let kw_after = store.keyword_search(&ns, "transaction", 10).unwrap();
    assert!(kw_after.contains(&a.id), "FTS must reflect updated content");
    // FTS desync guard: the OLD token must be removed from a's row, not merely
    // shadowed by a new one. An external-content FTS5 update that inserts the
    // new row without deleting the stale one would still satisfy the assertion
    // above but FAIL here.
    let kw_stale = store.keyword_search(&ns, "concurrent", 10).unwrap();
    assert!(
        !kw_stale.contains(&a.id),
        "stale FTS token 'concurrent' must be removed when a's content is updated"
    );

    // --- record_feedback: migration 008 memory_feedback table + confidence nudge
    // (a missing table/column here fails the whole gate, which is the point).
    let conf_before = store.get_memory(&a.id).unwrap().unwrap().confidence;
    let conf_after = store
        .record_feedback(&a.id, rb_types::FeedbackKind::Wrong, Some("alice"))
        .unwrap();
    assert!(
        conf_after < conf_before,
        "wrong feedback lowers confidence ({conf_before} -> {conf_after})"
    );
    assert!(
        (store.get_memory(&a.id).unwrap().unwrap().confidence - conf_after).abs() < 1e-6,
        "the row reflects the nudged confidence"
    );

    // --- archive_memory: soft delete; dropped from active list + keyword search
    store.archive_memory(&b.id).unwrap();
    let active_after = store.list(&ns, None, 10).unwrap();
    let active_ids: Vec<_> = active_after.iter().map(|m| m.id.clone()).collect();
    assert!(
        !active_ids.contains(&b.id),
        "archived memory absent from active list"
    );
    let kw_archived = store.keyword_search(&ns, "knn", 10).unwrap();
    assert!(
        !kw_archived.contains(&b.id),
        "archived memory absent from keyword search"
    );
    // But still fetchable directly with archived_at set.
    let b_archived = store.get_memory(&b.id).unwrap().unwrap();
    assert!(
        b_archived.archived_at.is_some(),
        "archived_at column must be set"
    );
}

#[test]
fn fresh_db_has_embedding_input_version_column_and_meta_seed() {
    // The 003 migration column + meta seed must apply on a fresh DB and round-trip.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("eiv.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();
    let ns = Namespace::Project("eiv".to_string());

    // A note stamped with the current composite version persists and reads back.
    let mut a = note(&ns, "composite stamped note", MemoryType::Insight, 5);
    a.embedding_model = "deterministic".to_string();
    a.embedding_input_version = "v2-composite".to_string();
    let emb: [f32; DIM] = [1.0, 0.0, 0.0, 0.0];
    store.insert_memory(&a, Some(&emb)).unwrap();
    let got = store.get_memory(&a.id).unwrap().unwrap();
    assert_eq!(got.embedding_input_version, "v2-composite");

    // A row written WITHOUT a stamp (e.g. an old code path) lands with the SQL
    // default backfill 'v1-content-only' — proving the column default works.
    let mut b = note(&ns, "unstamped row", MemoryType::Reference, 5);
    b.embedding_model = "deterministic".to_string();
    b.embedding_input_version = String::new(); // explicit empty stamp
    let emb_b: [f32; DIM] = [0.0, 1.0, 0.0, 0.0];
    store.insert_memory(&b, Some(&emb_b)).unwrap();
    let got_b = store.get_memory(&b.id).unwrap().unwrap();
    // We inserted an explicit empty string, so it round-trips as empty (the SQL
    // DEFAULT only applies when the column is omitted from the INSERT).
    assert_eq!(got_b.embedding_input_version, "");

    // The 003 migration also SEEDS meta.embedding_input_version = 'v2-composite'
    // (INSERT OR IGNORE). Assert it explicitly — the column round-trip above does
    // not exercise the meta seed, so a broken seed would otherwise pass this gate.
    assert_eq!(
        store
            .meta_value("embedding_input_version")
            .unwrap()
            .as_deref(),
        Some("v2-composite"),
        "migration 003 must seed meta.embedding_input_version"
    );
}

#[test]
fn reembed_scan_and_update_vector_round_trip() {
    use rb_store::Store as _;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("reembed.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();
    let ns = Namespace::Project("reembed".to_string());

    // A stale row (old model + version) and a current row.
    let mut stale = note(&ns, "stale row", MemoryType::Insight, 5);
    stale.embedding_model = "old-model".to_string();
    stale.embedding_input_version = "v1-content-only".to_string();
    let stale_emb: [f32; DIM] = [1.0, 0.0, 0.0, 0.0];
    store.insert_memory(&stale, Some(&stale_emb)).unwrap();

    let mut current = note(&ns, "current row", MemoryType::Insight, 5);
    current.embedding_model = "deterministic".to_string();
    current.embedding_input_version = "v2-composite".to_string();
    let cur_emb: [f32; DIM] = [0.0, 1.0, 0.0, 0.0];
    store.insert_memory(&current, Some(&cur_emb)).unwrap();

    // Only the stale row is a re-embed candidate.
    let cands = store
        .memories_for_reembed("deterministic", "v2-composite", 100)
        .unwrap();
    let ids: Vec<_> = cands.iter().map(|c| c.id.clone()).collect();
    assert!(ids.contains(&stale.id), "stale row is a candidate");
    assert!(!ids.contains(&current.id), "current row is not a candidate");

    // Re-embed the stale row: replaces vector + stamps to current.
    let new_emb: [f32; DIM] = [0.0, 0.0, 1.0, 0.0];
    store
        .update_vector(&stale.id, &new_emb, "deterministic", "v2-composite")
        .unwrap();
    let after = store.get_memory(&stale.id).unwrap().unwrap();
    assert_eq!(after.embedding_model, "deterministic");
    assert_eq!(after.embedding_input_version, "v2-composite");

    // Now it is no longer a candidate: a second scan finds nothing (idempotent).
    let cands2 = store
        .memories_for_reembed("deterministic", "v2-composite", 100)
        .unwrap();
    assert!(
        cands2.iter().all(|c| c.id != stale.id),
        "re-embedded row is no longer stale"
    );

    // The replacement vector is the nearest neighbor of new_emb.
    let hits = store.vector_search(&ns, &new_emb, 1).unwrap();
    assert_eq!(hits[0].0, stale.id, "updated vector is searchable");
}

#[test]
fn update_vector_rejects_missing_id_and_wrong_dim() {
    use rb_store::Store as _;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("reembed_err.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();

    // Missing id => NotFound.
    let missing = MemoryId::new();
    let emb: [f32; DIM] = [1.0, 0.0, 0.0, 0.0];
    let err = store
        .update_vector(&missing, &emb, "deterministic", "v2-composite")
        .unwrap_err();
    assert!(matches!(err, rb_types::Error::NotFound(_)), "got {err:?}");

    // Wrong dimension => DimensionMismatch (fail-closed), before any write.
    let ns = Namespace::Project("dim".to_string());
    let n = note(&ns, "dim guard", MemoryType::Insight, 5);
    store.insert_memory(&n, Some(&emb)).unwrap();
    let bad: [f32; 2] = [1.0, 2.0];
    let err = store
        .update_vector(&n.id, &bad, "deterministic", "v2-composite")
        .unwrap_err();
    assert!(
        matches!(err, rb_types::Error::DimensionMismatch { .. }),
        "got {err:?}"
    );
}
