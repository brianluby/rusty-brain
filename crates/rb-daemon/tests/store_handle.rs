//! Integration tests for the StoreHandle concurrency core: writer thread,
//! read pool, change broadcast, and the async MemoryBackend impl.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rb_daemon::{ChangeKind, StoreHandle};
use rb_engine::MemoryBackend;
use rb_types::{Error, MemoryNote, MemoryType, MemoryUpdates, Namespace};

const DIM: usize = 8;

fn note(ns: &Namespace, body: &str) -> MemoryNote {
    let mut n = MemoryNote::new(ns.clone(), body.to_string(), MemoryType::Insight, 5);
    n.summary = body.chars().take(40).collect();
    n.keywords = vec!["memory".to_string()];
    n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_then_read_round_trips_through_pool() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 4).unwrap();

    let ns = Namespace::Project("a".to_string());
    let n = note(&ns, "always one db one transaction");
    let id = n.id.clone();
    let emb = vec![0.1f32; DIM];

    handle.write(n.clone(), Some(emb)).await.unwrap();

    let got = handle.get(ns.clone(), id.clone()).await.unwrap();
    assert!(got.is_some(), "written memory must be retrievable");
    assert_eq!(got.unwrap().content, "always one db one transaction");

    let listed = handle.list(ns.clone(), None, 50).await.unwrap();
    assert_eq!(listed.len(), 1, "list returns the one written memory");

    let kw = handle
        .keyword(ns.clone(), "memory".to_string(), 50)
        .await
        .unwrap();
    assert_eq!(kw, vec![id.clone()], "keyword search finds it by keyword");

    let vec_hits = handle.vector(ns, vec![0.1f32; DIM], 5).await.unwrap();
    assert_eq!(
        vec_hits.len(),
        1,
        "vector search returns the one embedded memory"
    );
    assert_eq!(vec_hits[0].0, id);

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn write_publishes_change_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 2).unwrap();
    let mut rx = handle.subscribe();

    let ns = Namespace::Project("a".to_string());
    let n = note(&ns, "broadcast me");
    let id = n.id.clone();
    handle.write(n, Some(vec![0.2f32; DIM])).await.unwrap();

    let evt = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("a change event must arrive within 2s")
        .expect("broadcast channel must not be closed");
    assert_eq!(evt.id, id);
    assert_eq!(evt.namespace, ns);
    assert_eq!(evt.kind, ChangeKind::Created);

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_completes_with_live_handle_clone() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 2).unwrap();
    let retained = handle.clone();

    tokio::time::timeout(std::time::Duration::from_secs(2), handle.shutdown())
        .await
        .expect("shutdown must not wait for retained StoreHandle clones");

    let ns = Namespace::Project("a".to_string());
    let err = retained
        .write(note(&ns, "write after shutdown"), Some(vec![0.5f32; DIM]))
        .await
        .expect_err("retained clones must reject writes after shutdown");
    assert!(
        matches!(err, Error::Storage(_)),
        "write after shutdown should fail as storage unavailable, got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_and_archive_emit_correct_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 2).unwrap();
    let mut rx = handle.subscribe();

    let ns = Namespace::Project("a".to_string());
    let n = note(&ns, "evolve me");
    let id = n.id.clone();
    handle.write(n, Some(vec![0.3f32; DIM])).await.unwrap();
    assert_eq!(rx.recv().await.unwrap().kind, ChangeKind::Created);

    let updates = MemoryUpdates {
        importance: Some(9),
        ..Default::default()
    };
    handle
        .update(ns.clone(), id.clone(), updates)
        .await
        .unwrap();
    assert_eq!(rx.recv().await.unwrap().kind, ChangeKind::Updated);

    handle.archive(ns.clone(), id.clone()).await.unwrap();
    assert_eq!(rx.recv().await.unwrap().kind, ChangeKind::Archived);

    let got = handle.get(ns, id).await.unwrap().unwrap();
    assert_eq!(got.importance, 9, "update persisted");
    assert!(got.archived_at.is_some(), "archive persisted (soft delete)");

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn many_concurrent_writers_lose_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 4).unwrap();
    let ns = Namespace::Project("a".to_string());

    const N: usize = 200;
    let mut tasks = Vec::with_capacity(N);
    for i in 0..N {
        let h = handle.clone();
        let ns = ns.clone();
        tasks.push(tokio::spawn(async move {
            let n = note(&ns, &format!("concurrent note {i}"));
            h.write(n, Some(vec![i as f32; DIM])).await
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }

    let listed = handle.list(ns, None, N + 10).await.unwrap();
    assert_eq!(listed.len(), N, "no writes lost under concurrency");

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn namespace_id_methods_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, 2).unwrap();

    let ns_a = Namespace::Project("a".to_string());
    let ns_b = Namespace::Project("b".to_string());
    let n = note(&ns_a, "do not leak across namespaces");
    let id = n.id.clone();
    handle.write(n, Some(vec![0.4f32; DIM])).await.unwrap();

    assert!(
        handle
            .get(ns_b.clone(), id.clone())
            .await
            .unwrap()
            .is_none(),
        "get must filter an id hit to the requested namespace"
    );

    let wrong_update = handle
        .update(
            ns_b.clone(),
            id.clone(),
            MemoryUpdates {
                importance: Some(10),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(wrong_update, Err(Error::NotFound(_))));

    let wrong_archive = handle.archive(ns_b, id.clone()).await;
    assert!(matches!(wrong_archive, Err(Error::NotFound(_))));

    let got = handle.get(ns_a, id).await.unwrap().unwrap();
    assert_eq!(got.importance, 5);
    assert!(got.archived_at.is_none());

    handle.shutdown().await;
}

/// A panicking read closure must not permanently shrink the pool:
/// the RAII guard returns the connection in Drop so subsequent reads
/// still succeed, and we can run pool_size + 1 sequential reads afterward.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicking_read_closure_returns_connection_via_raii() {
    const POOL_SIZE: usize = 2;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rb.db");
    let handle = StoreHandle::start(db, DIM, POOL_SIZE).unwrap();

    // Seed one memory so subsequent reads have something to return.
    let ns = Namespace::Project("a".to_string());
    let n = note(&ns, "raii test memory");
    handle.write(n, Some(vec![0.1f32; DIM])).await.unwrap();

    // Run a closure that panics inside spawn_blocking; the pool guard must
    // catch the unwind (via JoinError mapping) and return the connection.
    let err = handle.with_read_panicking_for_test().await;
    assert!(
        err.is_ok(),
        "panicking read closure must surface as Storage error, got {err:?}"
    );

    // Now run pool_size + 1 sequential reads — if the connection leaked we
    // would eventually block or error when the pool is exhausted.
    for _ in 0..(POOL_SIZE + 1) {
        let listed = handle.list(ns.clone(), None, 10).await.unwrap();
        assert!(
            !listed.is_empty(),
            "pool must not be permanently shrunk after panic"
        );
    }

    handle.shutdown().await;
}
