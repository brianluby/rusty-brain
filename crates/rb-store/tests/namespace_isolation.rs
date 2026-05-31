//! Namespace isolation gate (storage layer).
//!
//! Memories in different namespaces share one DB; scoped queries must never
//! leak rows across namespaces. This is the storage-layer guarantee that the
//! daemon's server-side isolation (spec §8) builds on.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;

use rb_store::{SqliteStore, Store};
use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace};

const DIM: usize = 4;

fn insert(
    store: &SqliteStore,
    ns: &Namespace,
    content: &str,
    keyword: &str,
    emb: [f32; DIM],
) -> MemoryId {
    let mut note = MemoryNote::new(ns.clone(), content.to_string(), MemoryType::Insight, 5);
    note.summary = content.to_string();
    note.keywords = vec![keyword.to_string()];
    note.tags = vec![keyword.to_string()];
    let id = note.id.clone();
    store.insert_memory(&note, Some(&emb)).unwrap();
    id
}

#[test]
fn scoped_queries_never_cross_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("isolation.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();

    let ns_a = Namespace::Project("a".to_string());
    let ns_b = Namespace::Project("b".to_string());

    // Same shared token "deployment" in both namespaces to force any leak to show.
    let a1 = insert(
        &store,
        &ns_a,
        "alpha deployment rollback plan",
        "deployment",
        [1.0, 0.0, 0.0, 0.0],
    );
    let a2 = insert(
        &store,
        &ns_a,
        "alpha config deployment notes",
        "deployment",
        [0.9, 0.1, 0.0, 0.0],
    );
    let b1 = insert(
        &store,
        &ns_b,
        "beta deployment incident review",
        "deployment",
        [0.0, 1.0, 0.0, 0.0],
    );
    let b2 = insert(
        &store,
        &ns_b,
        "beta deployment runbook",
        "deployment",
        [0.0, 0.9, 0.1, 0.0],
    );

    let a_ids: HashSet<MemoryId> = [a1.clone(), a2.clone()].into_iter().collect();
    let b_ids: HashSet<MemoryId> = [b1.clone(), b2.clone()].into_iter().collect();

    // --- list scoped to "a" returns only a-rows.
    let list_a: HashSet<MemoryId> = store
        .list(&ns_a, None, 50)
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(
        list_a, a_ids,
        "list(a) must return exactly the a-namespace rows"
    );
    assert!(
        list_a.is_disjoint(&b_ids),
        "list(a) must not contain any b-namespace rows"
    );

    // --- list scoped to "b" returns only b-rows.
    let list_b: HashSet<MemoryId> = store
        .list(&ns_b, None, 50)
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(
        list_b, b_ids,
        "list(b) must return exactly the b-namespace rows"
    );
    assert!(
        list_b.is_disjoint(&a_ids),
        "list(b) must not contain any a-namespace rows"
    );

    // --- keyword_search scoped to "a" for the shared token returns only a-rows.
    let kw_a: HashSet<MemoryId> = store
        .keyword_search(&ns_a, "deployment", 50)
        .unwrap()
        .into_iter()
        .collect();
    assert!(
        !kw_a.is_empty(),
        "keyword_search(a, 'deployment') must match a-rows"
    );
    assert!(
        kw_a.is_subset(&a_ids),
        "keyword_search(a) must only return a-namespace ids"
    );
    assert!(
        kw_a.is_disjoint(&b_ids),
        "keyword_search(a) must never return b-namespace ids"
    );

    // --- keyword_search scoped to "b" for the shared token returns only b-rows.
    let kw_b: HashSet<MemoryId> = store
        .keyword_search(&ns_b, "deployment", 50)
        .unwrap()
        .into_iter()
        .collect();
    assert!(
        !kw_b.is_empty(),
        "keyword_search(b, 'deployment') must match b-rows"
    );
    assert!(
        kw_b.is_subset(&b_ids),
        "keyword_search(b) must only return b-namespace ids"
    );
    assert!(
        kw_b.is_disjoint(&a_ids),
        "keyword_search(b) must never return a-namespace ids"
    );

    // --- vector_search scoped to "a": the query vector is closest to b1
    // ([0,1,0,0]) GLOBALLY, so an unscoped KNN would surface a b-row first.
    // Scoping to "a" must still return only a-namespace ids.
    let query: [f32; DIM] = [0.0, 1.0, 0.0, 0.0];
    let vec_a: HashSet<MemoryId> = store
        .vector_search(&ns_a, &query, 50)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(!vec_a.is_empty(), "vector_search(a) must match a-rows");
    assert!(
        vec_a.is_subset(&a_ids),
        "vector_search(a) must only return a-namespace ids"
    );
    assert!(
        vec_a.is_disjoint(&b_ids),
        "vector_search(a) must never return b-namespace ids"
    );

    // --- vector_search scoped to "b" returns only b-rows.
    let vec_b: HashSet<MemoryId> = store
        .vector_search(&ns_b, &query, 50)
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(!vec_b.is_empty(), "vector_search(b) must match b-rows");
    assert!(
        vec_b.is_subset(&b_ids),
        "vector_search(b) must only return b-namespace ids"
    );
    assert!(
        vec_b.is_disjoint(&a_ids),
        "vector_search(b) must never return a-namespace ids"
    );
}

#[test]
fn distinct_project_namespaces_do_not_share_rows() {
    // Guards against namespace db-string collisions: "project:a" != "project:b".
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("isolation2.db");
    let store = SqliteStore::open(&db_path, DIM).unwrap();

    let ns_a = Namespace::Project("a".to_string());
    let ns_b = Namespace::Project("b".to_string());

    let only_a = insert(
        &store,
        &ns_a,
        "unique alpha marker token",
        "alphaonly",
        [1.0, 0.0, 0.0, 0.0],
    );

    // Searching b for a token that exists only in a must return nothing.
    let leaked = store.keyword_search(&ns_b, "alphaonly", 50).unwrap();
    assert!(leaked.is_empty(), "b must not see a's unique token");

    // And a still finds it.
    let found = store.keyword_search(&ns_a, "alphaonly", 50).unwrap();
    assert!(found.contains(&only_a), "a must find its own unique token");
}
