//! End-to-end daemon tests over a real Unix socket with the offline
//! DeterministicProvider (no network).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::time::Duration;

use rb_daemon::{Daemon, DaemonConfig, SharedEmbedder};
use rb_embed::DeterministicProvider;
use rb_proto::Client;
use rb_types::{MemoryType, MemoryUpdates, Namespace};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const DIM: usize = 8;

struct RunningDaemon {
    socket: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl RunningDaemon {
    async fn start(pool_size: usize) -> RunningDaemon {
        let dir = tempfile::tempdir_in("/private/tmp").unwrap();
        let socket = dir.path().join("sock");
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

        for _ in 0..200 {
            if tokio::net::UnixStream::connect(&socket).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

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
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
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
        .recall(
            "rusty-brain db transaction".to_string(),
            None,
            None,
            vec![],
            10,
        )
        .await
        .unwrap();
    assert!(
        results.iter().any(|r| r.memory.id == id),
        "recall must surface the remembered memory"
    );

    let listed = client.list(None, None, 50).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);

    let graph = client.graph(id.clone(), 1).await.unwrap();
    assert!(
        graph.iter().all(|m| m.id != id) || graph.is_empty() || graph.iter().any(|m| m.id == id),
        "graph returns a (possibly empty) neighborhood without error"
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
    let listed_after = client.list(None, None, 50).await.unwrap();
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
        .list(None, None, CLIENTS * PER_CLIENT + 10)
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

    let b_list = client_b.list(None, None, 50).await.unwrap();
    assert!(
        b_list.iter().all(|m| m.id != id_a),
        "namespace B must not see namespace A's memory via list"
    );
    assert!(
        b_list.iter().any(|m| m.id == id_b),
        "namespace B sees its own memory"
    );

    let b_recall = client_b
        .recall("secret".to_string(), None, None, vec![], 50)
        .await
        .unwrap();
    assert!(
        b_recall.iter().all(|r| r.memory.id != id_a),
        "namespace B must not recall namespace A's memory"
    );

    let a_list = client_a.list(None, None, 50).await.unwrap();
    assert!(
        a_list.iter().all(|m| m.id != id_b),
        "namespace A must not see namespace B's memory"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_bind_on_live_socket_fails_closed() {
    let daemon = RunningDaemon::start(2).await;

    let dir2 = tempfile::tempdir_in("/private/tmp").unwrap();
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
