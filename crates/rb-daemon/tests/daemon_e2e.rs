//! End-to-end daemon tests over a real Unix socket with the offline
//! DeterministicProvider (no network).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use rb_daemon::{Daemon, DaemonConfig, SharedEmbedder};
use rb_embed::DeterministicProvider;
use rb_proto::Client;
use rb_types::{Error, MemoryType, MemoryUpdates, Namespace};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DIM: usize = 8;

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir()).unwrap()
}

struct RunningDaemon {
    socket: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl RunningDaemon {
    async fn start(pool_size: usize) -> RunningDaemon {
        let dir = tempdir();
        let socket = dir.path().join("runtime").join("sock");
        let db = dir.path().join("memory.db");
        let cfg = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db,
            read_pool_size: pool_size,
        };
        let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
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

        let mut ready = false;
        for _ in 0..200 {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            ready,
            "daemon socket was not reachable within startup timeout at {}",
            socket.display()
        );

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_round_trip_through_client() {
    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("a".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    client.ping().await.unwrap();

    let id = client
        .remember(
            "rusty-brain uses one db and one transaction".to_string(),
            Some("architecture".to_string()),
            MemoryType::ArchitectureDecision,
            8,
            vec!["sqlite".to_string()],
            vec!["design".to_string()],
            vec!["src/store.rs".to_string()],
        )
        .await
        .unwrap();

    let got = client.get(id.clone()).await.unwrap();
    assert!(got.is_some());
    let note = got.unwrap();
    assert_eq!(note.content, "rusty-brain uses one db and one transaction");
    assert_eq!(note.namespace, ns, "stored under the handshake namespace");

    let results = client
        .recall("rusty-brain db transaction".to_string(), None, vec![], 10)
        .await
        .unwrap();
    assert!(
        results.iter().any(|r| r.memory.id == id),
        "recall must surface the remembered memory"
    );

    let listed = client.list(None, 50).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);

    let graph = client.graph(id.clone(), 1).await.unwrap();
    let unique: std::collections::HashSet<_> = graph.iter().map(|m| m.id.clone()).collect();
    assert_eq!(
        unique.len(),
        graph.len(),
        "graph results must not contain duplicate memories"
    );
    assert!(
        graph.iter().all(|m| m.namespace == ns),
        "graph results must stay inside the handshake namespace"
    );

    let updates = MemoryUpdates {
        importance: Some(10),
        tags: Some(vec!["design".to_string(), "core".to_string()]),
        ..Default::default()
    };
    client.update(id.clone(), updates).await.unwrap();
    let after = client.get(id.clone()).await.unwrap().unwrap();
    assert_eq!(after.importance, 10);

    let (recent, important, total) = client.context().await.unwrap();
    assert!(total >= 1, "context total counts the active memory");
    assert!(
        recent.iter().chain(important.iter()).any(|m| m.id == id),
        "context surfaces the high-importance memory"
    );

    client.delete(id.clone()).await.unwrap();
    let listed_after = client.list(None, 50).await.unwrap();
    assert!(
        listed_after.iter().all(|m| m.id != id),
        "archived memory is not listed"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn many_concurrent_clients_no_lost_writes_no_errors() {
    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("a".to_string());

    const CLIENTS: usize = 16;
    const PER_CLIENT: usize = 10;

    let mut tasks = Vec::with_capacity(CLIENTS);
    for c in 0..CLIENTS {
        let socket = daemon.socket.clone();
        let ns = ns.clone();
        tasks.push(tokio::spawn(async move {
            let mut client = Client::connect(&socket, ns).await.unwrap();
            for i in 0..PER_CLIENT {
                client
                    .remember(
                        format!("memory from client {c} item {i}"),
                        None,
                        MemoryType::Insight,
                        5,
                        vec!["concurrent".to_string()],
                        vec![],
                        vec![],
                    )
                    .await
                    .unwrap();
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    let mut verifier = Client::connect(&daemon.socket, ns).await.unwrap();
    let listed = verifier
        .list(None, CLIENTS * PER_CLIENT + 10)
        .await
        .unwrap();
    assert_eq!(
        listed.len(),
        CLIENTS * PER_CLIENT,
        "all {} writes must be present (no lost writes)",
        CLIENTS * PER_CLIENT
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn namespace_isolation_enforced_server_side() {
    let daemon = RunningDaemon::start(4).await;

    let ns_a = Namespace::Project("a".to_string());
    let mut client_a = Client::connect(&daemon.socket, ns_a).await.unwrap();
    let id_a = client_a
        .remember(
            "secret belonging to project a".to_string(),
            None,
            MemoryType::Insight,
            7,
            vec!["alpha".to_string()],
            vec![],
            vec![],
        )
        .await
        .unwrap();

    let ns_b = Namespace::Project("b".to_string());
    let mut client_b = Client::connect(&daemon.socket, ns_b).await.unwrap();
    let id_b = client_b
        .remember(
            "secret belonging to project b".to_string(),
            None,
            MemoryType::Insight,
            7,
            vec!["beta".to_string()],
            vec![],
            vec![],
        )
        .await
        .unwrap();

    let b_list = client_b.list(None, 50).await.unwrap();
    assert!(
        b_list.iter().all(|m| m.id != id_a),
        "namespace B must not see namespace A's memory via list"
    );
    assert!(
        b_list.iter().any(|m| m.id == id_b),
        "namespace B sees its own memory"
    );

    let b_recall = client_b
        .recall("secret".to_string(), None, vec![], 50)
        .await
        .unwrap();
    assert!(
        b_recall.iter().all(|r| r.memory.id != id_a),
        "namespace B must not recall namespace A's memory"
    );

    let a_list = client_a.list(None, 50).await.unwrap();
    assert!(
        a_list.iter().all(|m| m.id != id_b),
        "namespace A must not see namespace B's memory"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_handshake_namespace_is_rejected_before_ack() {
    let daemon = RunningDaemon::start(2).await;

    let err = Client::connect(&daemon.socket, Namespace::Project(String::new()))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("handshake rejected")
            || err.to_string().contains("invalid namespace"),
        "invalid namespace must fail closed during handshake: {err}"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_bind_on_live_socket_fails_closed() {
    let daemon = RunningDaemon::start(2).await;

    let dir2 = tempdir();
    let cfg2 = DaemonConfig {
        socket_path: daemon.socket.clone(),
        db_path: dir2.path().join("memory.db"),
        read_pool_size: 2,
    };
    let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
    let err = Daemon::bind(cfg2, embedder).await.unwrap_err();
    assert!(
        err.to_string().contains("already listening"),
        "second bind on a live socket must fail closed: {err}"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_bind_before_accept_loop_fails_closed() {
    let dir = tempdir();
    let socket = dir.path().join("runtime").join("sock");
    let cfg = DaemonConfig {
        socket_path: socket.clone(),
        db_path: dir.path().join("memory.db"),
        read_pool_size: 2,
    };
    let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
    let daemon = Daemon::bind(cfg, embedder).await.unwrap();

    let dir2 = tempdir();
    let cfg2 = DaemonConfig {
        socket_path: socket,
        db_path: dir2.path().join("memory.db"),
        read_pool_size: 2,
    };
    let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
    let err = Daemon::bind(cfg2, embedder).await.unwrap_err();
    assert!(
        err.to_string().contains("already listening"),
        "second bind before run must fail closed: {err}"
    );

    drop(daemon);
}

/// A Recall or List request with a very large limit must return at most
/// MAX_LIMIT rows and must not error. The server clamps the value before
/// passing it to the engine, so a caller cannot over-read the store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_limit_is_clamped_and_does_not_error() {
    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("clamp-test".to_string());
    let mut client = Client::connect(&daemon.socket, ns).await.unwrap();

    // Store a small number of memories (well below MAX_LIMIT).
    for i in 0..5 {
        client
            .remember(
                format!("memory {i}"),
                None,
                MemoryType::Insight,
                5,
                vec![],
                vec![],
                vec![],
            )
            .await
            .unwrap();
    }

    // A limit far exceeding MAX_LIMIT (1000) must succeed and return only
    // what is in the store (<=5), never causing a negative LIMIT or panic.
    let listed = client.list(None, usize::MAX).await.unwrap();
    assert!(
        listed.len() <= 5,
        "list with usize::MAX limit must return at most the stored count, got {}",
        listed.len()
    );

    let recalled = client
        .recall("memory".to_string(), None, vec![], usize::MAX)
        .await
        .unwrap();
    assert!(
        recalled.len() <= 5,
        "recall with usize::MAX limit must return at most the stored count, got {}",
        recalled.len()
    );

    daemon.stop().await;
}

/// A client handshaked under Project("b") must not see or mutate Project("a")
/// data over the wire, even by direct id. This is the wire-level namespace
/// isolation guarantee: get returns None, update/delete return NotFound-class
/// errors, and graph returns an empty neighborhood.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_namespace_isolation_cross_namespace_ops_fail_closed() {
    let daemon = RunningDaemon::start(4).await;

    // Project A: store one memory.
    let ns_a = Namespace::Project("a".to_string());
    let mut client_a = Client::connect(&daemon.socket, ns_a.clone()).await.unwrap();
    let id_a = client_a
        .remember(
            "project a secret".to_string(),
            None,
            MemoryType::Insight,
            7,
            vec![],
            vec![],
            vec![],
        )
        .await
        .unwrap();

    // Project B: connect under a different namespace.
    let ns_b = Namespace::Project("b".to_string());
    let mut client_b = Client::connect(&daemon.socket, ns_b).await.unwrap();

    // get(id_a) must return None — not visible across namespace boundary.
    let got = client_b.get(id_a.clone()).await.unwrap();
    assert!(
        got.is_none(),
        "get across namespace boundary must return None, not expose the memory"
    );

    // update(id_a, ..) must return an error (NotFound at the store layer, surfaced
    // as a wire Error response, which the client maps to Error::Storage or similar).
    let update_err = client_b
        .update(
            id_a.clone(),
            MemoryUpdates {
                importance: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(update_err, Error::Storage(_)),
        "update across namespace boundary must error, got {update_err:?}"
    );

    // delete(id_a) must return an error.
    let delete_err = client_b.delete(id_a.clone()).await.unwrap_err();
    assert!(
        matches!(delete_err, Error::Storage(_)),
        "delete across namespace boundary must error, got {delete_err:?}"
    );

    // graph(id_a, ..) must return an empty neighborhood (anchor not in namespace).
    let graph_result = client_b.graph(id_a.clone(), 2).await.unwrap();
    assert!(
        graph_result.is_empty(),
        "graph across namespace boundary must return empty, got {} nodes",
        graph_result.len()
    );

    // Confirm A's data is untouched: it can still get its own memory.
    let still_there = client_a.get(id_a).await.unwrap();
    assert!(
        still_there.is_some(),
        "Project A memory must remain visible to Project A after cross-namespace ops"
    );

    daemon.stop().await;
}
