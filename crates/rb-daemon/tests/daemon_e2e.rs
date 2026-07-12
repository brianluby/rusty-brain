//! End-to-end daemon tests over a real Unix socket with the offline
//! DeterministicProvider (no network).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::Duration;

use rb_daemon::{Daemon, DaemonConfig, SharedEmbedder};
use rb_embed::DeterministicProvider;
use rb_proto::{Client, ClientIdentity};
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
        Self::start_with_embedder(
            pool_size,
            SharedEmbedder::new(DeterministicProvider::new(DIM)),
        )
        .await
    }

    async fn start_with_embedder(pool_size: usize, embedder: SharedEmbedder) -> RunningDaemon {
        let dir = tempdir();
        let socket = dir.path().join("runtime").join("sock");
        let db = dir.path().join("memory.db");
        let cfg = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db,
            read_pool_size: pool_size,
            jobs_config: rb_daemon::JobsConfig::default(),
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_engine::FusionMode::Linear,
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
            // Non-default on purpose: proves confidence round-trips through
            // wire + store instead of riding the serde default (1.0).
            Some(0.4),
        )
        .await
        .unwrap();

    let got = client.get(id.clone()).await.unwrap();
    assert!(got.is_some());
    let note = got.unwrap();
    assert_eq!(note.content, "rusty-brain uses one db and one transaction");
    assert_eq!(note.namespace, ns, "stored under the handshake namespace");
    assert!(
        (note.confidence - 0.4).abs() < f32::EPSILON,
        "confidence must round-trip through wire + store, got {}",
        note.confidence
    );

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

// W2.2: the trust machinery is reachable end-to-end over the wire — an agent
// can create a contradicts link and lower confidence, and reads then surface
// `contested` and the new prior.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn link_and_confidence_update_round_trip_surfaces_contested() {
    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("trust".to_string());
    let mut client = Client::connect(&daemon.socket, ns).await.unwrap();

    let a = client
        .remember(
            "deploys happen on fridays".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();
    let b = client
        .remember(
            "deploys are frozen on fridays".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();

    client
        .link(
            a.clone(),
            b.clone(),
            rb_types::LinkType::Contradicts,
            Some("policy reversed".to_string()),
        )
        .await
        .unwrap();

    let got_a = client.get(a.clone()).await.unwrap().unwrap();
    let got_b = client.get(b.clone()).await.unwrap().unwrap();
    assert!(got_a.contested, "link source must surface contested");
    assert!(got_b.contested, "link target must surface contested");

    // A duplicate edge is a clean validation-class error, not a Storage one.
    let err = client
        .link(a.clone(), b.clone(), rb_types::LinkType::Contradicts, None)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("already exists"),
        "duplicate link must be a guidance-bearing rejection: {err}"
    );

    // Confidence is settable through the wire update path.
    client
        .update(
            a.clone(),
            MemoryUpdates {
                confidence: Some(0.2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let after = client.get(a).await.unwrap().unwrap();
    assert!(
        (after.confidence - 0.2).abs() < f32::EPSILON,
        "confidence update must round-trip, got {}",
        after.confidence
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
                        Some(1.0),
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
            Some(1.0),
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
            Some(1.0),
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
        jobs_config: rb_daemon::JobsConfig::default(),
        retention_policy: None,
        request_idle_timeout: None,
        enrich: None,
        fusion_mode: rb_engine::FusionMode::Linear,
        http: None,
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
        jobs_config: rb_daemon::JobsConfig::default(),
        retention_policy: None,
        request_idle_timeout: None,
        enrich: None,
        fusion_mode: rb_engine::FusionMode::Linear,
        http: None,
    };
    let embedder = SharedEmbedder::new(DeterministicProvider::new(DIM));
    let daemon = Daemon::bind(cfg, embedder).await.unwrap();

    let dir2 = tempdir();
    let cfg2 = DaemonConfig {
        socket_path: socket,
        db_path: dir2.path().join("memory.db"),
        read_pool_size: 2,
        jobs_config: rb_daemon::JobsConfig::default(),
        retention_policy: None,
        request_idle_timeout: None,
        enrich: None,
        fusion_mode: rb_engine::FusionMode::Linear,
        http: None,
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
                Some(1.0),
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
            Some(1.0),
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

// W2.4 / Phase 2 gate: retroactive `scrub` over the wire redacts a secret
// that reached the store unredacted (the daemon's remember path does not
// redact — only capture-time hooks do), so a row predating a rule is cleaned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scrub_over_the_wire_redacts_an_unredacted_stored_secret() {
    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("scrub-wire".to_string());
    let mut client = Client::connect(&daemon.socket, ns).await.unwrap();

    // The daemon's Remember path stores verbatim (no capture-time redaction),
    // so this lands a plaintext secret in the store — exactly the pre-rule
    // row scrub exists to clean. The fake AWS key is BUILT from split literals
    // so no committed byte forms a scanner-matchable token (push protection).
    let secret = format!("AKIA{}", "A".repeat(16));
    let id = client
        .remember(
            format!("rotate the key {secret} before friday"),
            None,
            MemoryType::Insight,
            6,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();
    let before = client.get(id.clone()).await.unwrap().unwrap();
    assert!(before.content.contains(&secret));

    // Same-uid test client is admin, so the gate admits the op.
    let outcome = client.scrub_with_checkpoint().await.unwrap();
    assert!(outcome.scanned >= 1);
    assert_eq!(outcome.redacted, 1);
    assert_eq!(
        outcome.reembed_pending, 1,
        "content changed -> needs reembed"
    );
    assert!(
        !outcome.wal_checkpoint.unwrap().busy,
        "wire response must carry the completed at-rest checkpoint"
    );

    let after = client.get(id).await.unwrap().unwrap();
    assert!(
        !after.content.contains(&secret),
        "scrub must remove the stored secret: {}",
        after.content
    );
    assert!(after.content.contains("[REDACTED:aws-key]"));

    daemon.stop().await;
}

// W2.7 / Phase 2 gate: a killed-and-reconnected subscriber replays missed
// events from its cursor instead of going silently empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnected_subscriber_replays_missed_events_from_its_cursor() {
    use rb_proto::SubscribeItem;
    use rb_types::ChangeKind;

    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("replay".to_string());

    async fn next_change(sub: &mut Client) -> rb_types::MemoryChanged {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), sub.recv_change())
                .await
                .expect("subscribe stream timed out")
                .unwrap()
            {
                SubscribeItem::Change(evt) => break evt,
                SubscribeItem::Lagged(_) => continue,
            }
        }
    }

    async fn remember(w: &mut Client, body: &str) -> rb_types::MemoryId {
        w.remember(
            body.to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap()
    }

    let mut writer = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    // Live subscriber sees the first write and records its cursor.
    let mut sub = Client::connect(&daemon.socket, ns.clone()).await.unwrap();
    sub.subscribe().await.unwrap();
    let before = remember(&mut writer, "before the outage").await;
    let evt = next_change(&mut sub).await;
    assert_eq!(evt.id, before);
    let cursor = evt.seq.expect("live events carry the oplog seq (W2.7)");

    // Subscriber dies; two writes land while it is away.
    drop(sub);
    let missed_one = remember(&mut writer, "missed while away 1").await;
    let missed_two = remember(&mut writer, "missed while away 2").await;

    // Reconnect from the cursor: BOTH missed changes replay from the oplog,
    // in commit order, before live streaming resumes.
    let mut sub = Client::connect(&daemon.socket, ns.clone()).await.unwrap();
    sub.subscribe_since(Some(cursor)).await.unwrap();
    let replay_one = next_change(&mut sub).await;
    let replay_two = next_change(&mut sub).await;
    assert_eq!(replay_one.id, missed_one, "first missed change replays");
    assert_eq!(replay_one.kind, ChangeKind::Created);
    assert_eq!(replay_two.id, missed_two, "second missed change replays");
    assert!(
        replay_one.seq.unwrap() > cursor && replay_two.seq.unwrap() > replay_one.seq.unwrap(),
        "replay is strictly after the cursor, in commit order"
    );

    // And the stream is LIVE after the replay: a new write arrives exactly
    // once (the replay watermark must not duplicate it).
    let live = remember(&mut writer, "after the reconnect").await;
    let evt = next_change(&mut sub).await;
    assert_eq!(evt.id, live, "live streaming resumes after replay");
    assert!(evt.seq.unwrap() > replay_two.seq.unwrap());

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribe_streams_only_own_namespace_changes() {
    use rb_proto::SubscribeItem;
    use rb_types::ChangeKind;

    let daemon = RunningDaemon::start(4).await;
    let ns_a = Namespace::Project("a".to_string());
    let ns_b = Namespace::Project("b".to_string());

    // Subscriber on namespace A.
    let mut sub = Client::connect(&daemon.socket, ns_a.clone()).await.unwrap();
    sub.subscribe().await.unwrap();

    // Writer on namespace A: a Created event must reach the subscriber.
    let mut writer_a = Client::connect(&daemon.socket, ns_a.clone()).await.unwrap();
    let id_a = writer_a
        .remember(
            "a memory".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();

    // The subscriber receives the A change (skip any Lagged notices).
    let got = loop {
        match tokio::time::timeout(Duration::from_secs(5), sub.recv_change())
            .await
            .expect("subscribe stream timed out waiting for the A change")
            .unwrap()
        {
            SubscribeItem::Change(evt) => break evt,
            SubscribeItem::Lagged(_) => continue,
        }
    };
    assert_eq!(
        got.id, id_a,
        "subscriber must receive its namespace's change"
    );
    assert_eq!(got.namespace, ns_a);
    assert_eq!(got.kind, ChangeKind::Created);

    // Writer on namespace B: this change must NOT be delivered to the A subscriber.
    let mut writer_b = Client::connect(&daemon.socket, ns_b).await.unwrap();
    writer_b
        .remember(
            "b memory".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();

    // Do a second A write so there IS a frame to read; the B write must have been
    // filtered out server-side, so the very next Change is the second A event.
    let id_a2 = writer_a
        .remember(
            "a memory 2".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();

    let next = loop {
        match tokio::time::timeout(Duration::from_secs(5), sub.recv_change())
            .await
            .expect("subscribe stream timed out waiting for the second A change")
            .unwrap()
        {
            SubscribeItem::Change(evt) => break evt,
            SubscribeItem::Lagged(_) => continue,
        }
    };
    assert_eq!(
        next.id, id_a2,
        "the B-namespace change must be filtered out; next frame is the 2nd A change"
    );
    assert_eq!(next.namespace, ns_a, "no cross-namespace leakage");

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_job_link_decay_round_trips_over_the_wire() {
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("evolve-e2e".to_string());
    let mut client = Client::connect(&daemon.socket, ns).await.unwrap();

    let (scanned, changed, skipped) = client.run_job(rb_types::JobKind::LinkDecay).await.unwrap();
    // Empty store: nothing to scan, but the wire op resolves to JobRan.
    assert_eq!((scanned, changed, skipped), (0, 0, 0));

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_job_importance_recalibration_flows_through_client() {
    use rb_types::JobKind;

    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("recal-e2e".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    // Seed one memory in this namespace via the typed remember helper.
    let id = client
        .remember(
            "importance recalibration target".to_string(),
            Some("evolution".to_string()),
            MemoryType::Insight,
            4,
            vec!["evolution".to_string()],
            vec!["jobs".to_string()],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();

    // Trigger the cross-namespace importance job over the wire (Part R wrapper).
    let (scanned, changed, skipped) = client
        .run_job(JobKind::ImportanceRecalibration)
        .await
        .unwrap();
    assert_eq!(
        scanned, 1,
        "the one seeded row must be scanned by the importance job"
    );
    // A freshly-remembered, never-accessed row recalibrates to its base:
    // delta is 0 => unchanged => skipped, not changed.
    assert_eq!(changed, 0, "never-accessed row is unchanged");
    assert_eq!(skipped, 1, "never-accessed row is skipped");

    // The seeded memory still resolves and kept its base importance.
    let got = client
        .get(id)
        .await
        .unwrap()
        .expect("seeded memory present");
    assert_eq!(got.importance, 4, "never-accessed memory keeps its base");

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reembed_over_the_wire_is_stamp_skip_and_idempotent() {
    // Memories remembered through the daemon are written at the CURRENT stamp
    // (composite embedding + EMBEDDING_INPUT_VERSION), so a reembed pass scans
    // none of them and writes nothing — proving the stamp-skip + idempotency
    // contract end-to-end over the wire.
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("reembed-e2e".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    for i in 0..3 {
        client
            .remember(
                format!("reembed target {i}"),
                None,
                MemoryType::Insight,
                5,
                vec![format!("kw{i}")],
                vec!["reembed".to_string()],
                vec![],
                Some(1.0),
            )
            .await
            .unwrap();
    }

    // All rows are already current => scanned/changed/skipped all zero.
    let (scanned, changed, skipped) = client.reembed(None).await.unwrap();
    assert_eq!(
        (scanned, changed, skipped),
        (0, 0, 0),
        "freshly-remembered rows are already at the current stamp"
    );

    // A second pass is likewise a no-op (idempotent).
    let (s2, c2, k2) = client.reembed(Some(100)).await.unwrap();
    assert_eq!((s2, c2, k2), (0, 0, 0), "reembed is idempotent");

    daemon.stop().await;
}

/// Document embeds succeed (so `remember` works); Query embeds fail — the
/// W1.6d embedding-API-outage shape, injected per test run.
struct QueryFailingProvider;

#[async_trait::async_trait]
impl rb_embed::EmbeddingProvider for QueryFailingProvider {
    fn model_id(&self) -> &str {
        "query-failing"
    }

    fn dim(&self) -> usize {
        DIM
    }

    async fn embed(
        &self,
        texts: &[String],
        kind: rb_embed::EmbedKind,
    ) -> rb_types::Result<Vec<Vec<f32>>> {
        match kind {
            rb_embed::EmbedKind::Document => Ok(vec![vec![1.0; DIM]; texts.len()]),
            rb_embed::EmbedKind::Query => Err(Error::Embedding(
                "embedding API down (injected)".to_string(),
            )),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recall_degrades_on_embedder_outage_and_flags_the_wire_response() {
    // W1.6d / F19 end-to-end: with the embedder down at recall time, the
    // daemon still serves keyword+graph results AND the wire Response carries
    // the additive `degraded` flag.
    let daemon =
        RunningDaemon::start_with_embedder(2, SharedEmbedder::new(QueryFailingProvider)).await;
    let ns = Namespace::Project("degraded-recall".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    let id = client
        .remember(
            "daemon survives embedding outages".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();

    // Typed client path: recall succeeds (no hard failure) and the keyword
    // channel serves the stored memory.
    let results = client
        .recall("embedding outages".to_string(), None, vec![], 10)
        .await
        .unwrap();
    assert!(
        results.iter().any(|r| r.memory.id == id),
        "keyword channel must still serve the memory while the embedder is down"
    );

    // Raw frame path: the degraded flag rides the wire Response.
    let stream = tokio::net::UnixStream::connect(&daemon.socket)
        .await
        .unwrap();
    let mut framed = rb_proto::bounded_framed(stream);
    rb_proto::write_frame(
        &mut framed,
        &rb_proto::Handshake {
            contract_version: rb_proto::CONTRACT_VERSION,
            namespace: ns.clone(),
            identity: None,
        },
    )
    .await
    .unwrap();
    let ack: rb_proto::HandshakeAck = rb_proto::read_frame(&mut framed).await.unwrap();
    assert!(ack.ok, "handshake must succeed: {:?}", ack.message);

    rb_proto::write_frame(
        &mut framed,
        &rb_proto::Request::Recall {
            query: "embedding outages".to_string(),
            memory_type: None,
            tags: vec![],
            limit: 10,
            filter: rb_types::RecallFilter::default(),
        },
    )
    .await
    .unwrap();
    let resp: rb_proto::Response = rb_proto::read_frame(&mut framed).await.unwrap();
    match resp {
        rb_proto::Response::Recalled { results, degraded } => {
            assert!(degraded, "embedder outage must flag the wire response");
            assert!(
                results.iter().any(|r| r.memory.id == id),
                "degraded recall still returns the keyword hit"
            );
        }
        other => unreachable!("expected Recalled, got {other:?}"),
    }

    daemon.stop().await;
}

// PRD 2026-07-02 search-filter parity e2e: every new filter dimension (and a
// composition) flows client -> proto -> daemon -> engine -> store over a real
// socket, and a pre-filter frame (no `filter` key) still decodes and behaves
// exactly as before — additive, no CONTRACT_VERSION bump.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recall_and_list_filters_flow_over_the_wire() {
    use rb_types::{MemoryState, RecallFilter};

    let daemon = RunningDaemon::start(4).await;
    let ns = Namespace::Project("filter-parity".to_string());

    // Two writers with distinct declared sources so the provenance filter has
    // something to distinguish.
    let mut cli_client = Client::connect_with_identity(
        &daemon.socket,
        ns.clone(),
        Some(ClientIdentity {
            source: Some("cli".to_string()),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    let mut mcp_client = Client::connect_with_identity(
        &daemon.socket,
        ns.clone(),
        Some(ClientIdentity {
            source: Some("mcp".to_string()),
            ..Default::default()
        }),
    )
    .await
    .unwrap();

    let cutoff = chrono::Utc::now() - chrono::Duration::hours(1);

    async fn remember_with(
        client: &mut Client,
        body: &str,
        importance: u8,
        confidence: f32,
    ) -> rb_types::MemoryId {
        client
            .remember(
                body.to_string(),
                None,
                MemoryType::Insight,
                importance,
                vec![],
                vec![],
                vec![],
                Some(confidence),
            )
            .await
            .unwrap()
    }
    let low_conf = remember_with(&mut cli_client, "filterable low confidence", 3, 0.2).await;
    let high_conf = remember_with(&mut cli_client, "filterable high confidence", 8, 0.9).await;
    let from_mcp = remember_with(&mut mcp_client, "filterable from mcp", 8, 0.9).await;

    // Confidence range (recall).
    let (results, _) = cli_client
        .recall_filtered_with_status(
            "filterable".to_string(),
            RecallFilter {
                min_confidence: Some(0.5),
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    let got: std::collections::HashSet<_> = results.iter().map(|r| r.memory.id.clone()).collect();
    assert!(got.contains(&high_conf) && got.contains(&from_mcp) && !got.contains(&low_conf));

    // Source (list).
    let listed = cli_client
        .list_filtered(
            RecallFilter {
                sources: vec!["mcp".to_string()],
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
        vec![from_mcp.clone()]
    );

    // Date window: everything here was created after `cutoff`; an until-bound
    // BEFORE the writes excludes everything, a since-bound includes all three.
    let listed = cli_client
        .list_filtered(
            RecallFilter {
                until: Some(cutoff),
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    assert!(listed.is_empty(), "until-before-writes must exclude all");
    let listed = cli_client
        .list_filtered(
            RecallFilter {
                since: Some(cutoff),
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 3, "since-before-writes must include all");

    // Composition: source + min_importance + min_confidence.
    let (results, _) = cli_client
        .recall_filtered_with_status(
            "filterable".to_string(),
            RecallFilter {
                sources: vec!["cli".to_string()],
                min_importance: Some(7),
                min_confidence: Some(0.5),
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(
        results
            .iter()
            .map(|r| r.memory.id.clone())
            .collect::<Vec<_>>(),
        vec![high_conf.clone()]
    );

    // Contested (tri-state) over the wire.
    cli_client
        .link(
            high_conf.clone(),
            from_mcp.clone(),
            rb_types::LinkType::Contradicts,
            Some("parity test".to_string()),
        )
        .await
        .unwrap();
    let contested = cli_client
        .list_filtered(
            RecallFilter {
                contested: Some(true),
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    let got: std::collections::HashSet<_> = contested.iter().map(|n| n.id.clone()).collect();
    assert_eq!(
        got,
        [high_conf.clone(), from_mcp.clone()].into_iter().collect()
    );
    assert!(contested.iter().all(|n| n.contested));
    let uncontested = cli_client
        .list_filtered(
            RecallFilter {
                contested: Some(false),
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(
        uncontested.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
        vec![low_conf.clone()]
    );

    // Archived state: archive one, then reach it ONLY via the state filter —
    // through list AND through recall's keyword channel.
    cli_client.delete(low_conf.clone()).await.unwrap();
    let active_default = cli_client.list(None, 10).await.unwrap();
    assert!(active_default.iter().all(|n| n.id != low_conf));
    let archived_listed = cli_client
        .list_filtered(
            RecallFilter {
                state: MemoryState::Archived,
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(
        archived_listed
            .iter()
            .map(|n| n.id.clone())
            .collect::<Vec<_>>(),
        vec![low_conf.clone()]
    );
    let (archived_recall, _) = cli_client
        .recall_filtered_with_status(
            "filterable".to_string(),
            RecallFilter {
                state: MemoryState::Archived,
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(
        archived_recall
            .iter()
            .map(|r| r.memory.id.clone())
            .collect::<Vec<_>>(),
        vec![low_conf.clone()]
    );

    // Typed code anchors over a real socket (PRD 2026-07-02): the daemon
    // advertises the `anchors` capability at handshake, an anchored remember
    // persists its anchors, and recall/list scope by them — present under the
    // anchored file, absent under a different one (the PRD acceptance
    // criterion), composing with the metadata dimensions.
    assert!(
        cli_client.supports_anchors(),
        "a current daemon must advertise anchor support"
    );
    let anchors = vec![
        rb_types::MemoryAnchor::parse_file_spec("src/server.rs:10-40").unwrap(),
        rb_types::MemoryAnchor::new(rb_types::AnchorKind::Symbol, "Server::run").unwrap(),
    ];
    let anchored_id = cli_client
        .remember_anchored(
            "filterable anchored decision".to_string(),
            None,
            MemoryType::Insight,
            8,
            vec![],
            vec![],
            vec![],
            Some(0.9),
            anchors.clone(),
            None,
        )
        .await
        .unwrap();
    let file_filter = |path: &str| RecallFilter {
        anchors: vec![rb_types::AnchorFilter {
            kind: rb_types::AnchorKind::File,
            value: path.to_string(),
        }],
        ..Default::default()
    };
    let (results, _) = cli_client
        .recall_filtered_with_status("filterable".to_string(), file_filter("src/server.rs"), 10)
        .await
        .unwrap();
    assert_eq!(
        results
            .iter()
            .map(|r| r.memory.id.clone())
            .collect::<Vec<_>>(),
        vec![anchored_id.clone()],
        "recall scoped to the anchored file returns ONLY the anchored memory"
    );
    assert_eq!(
        results[0].memory.anchors, anchors,
        "anchors ride the result payload"
    );
    let (absent, _) = cli_client
        .recall_filtered_with_status("filterable".to_string(), file_filter("src/other.rs"), 10)
        .await
        .unwrap();
    assert!(absent.is_empty(), "a different file matches nothing");
    // List: anchor + metadata composition over the wire.
    let listed = cli_client
        .list_filtered(
            RecallFilter {
                min_importance: Some(7),
                anchors: vec![rb_types::AnchorFilter {
                    kind: rb_types::AnchorKind::Symbol,
                    value: "Server::run".to_string(),
                }],
                ..Default::default()
            },
            10,
        )
        .await
        .unwrap();
    assert_eq!(
        listed.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
        vec![anchored_id.clone()]
    );
    // An empty anchor-filter value is a wire-visible validation error.
    let err = cli_client
        .list_filtered(file_filter("   "), 10)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::InvalidArgument(_)),
        "empty anchor filter value must fail fast, got {err:?}"
    );

    // Old-client compatibility: a raw pre-filter frame (NO `filter` key at
    // all) must decode on the new daemon and behave exactly as before.
    let stream = tokio::net::UnixStream::connect(&daemon.socket)
        .await
        .unwrap();
    let mut framed = rb_proto::bounded_framed(stream);
    rb_proto::write_frame(
        &mut framed,
        &rb_proto::Handshake {
            contract_version: rb_proto::CONTRACT_VERSION,
            namespace: ns.clone(),
            identity: None,
        },
    )
    .await
    .unwrap();
    let ack: rb_proto::HandshakeAck = rb_proto::read_frame(&mut framed).await.unwrap();
    assert!(ack.ok, "handshake must succeed: {:?}", ack.message);
    rb_proto::write_frame(
        &mut framed,
        &serde_json::json!({ "op": "List", "min_importance": null, "limit": 10 }),
    )
    .await
    .unwrap();
    let resp: rb_proto::Response = rb_proto::read_frame(&mut framed).await.unwrap();
    match resp {
        rb_proto::Response::Listed { memories } => {
            let got: std::collections::HashSet<_> = memories.iter().map(|n| n.id.clone()).collect();
            assert_eq!(
                got,
                [high_conf, from_mcp, anchored_id].into_iter().collect(),
                "an old frame must keep the pre-filter default (active, unfiltered)"
            );
        }
        other => unreachable!("expected Listed, got {other:?}"),
    }

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn namespace_rename_round_trips_over_the_wire() {
    // W0.3 carryover e2e: populate two namespaces (vectors + FTS via the
    // engine's remember path), rename one over the wire, and prove recall —
    // including the vec0 KNN partition — follows the rows to the new
    // namespace while the old one is empty and the bystander is untouched.
    let daemon = RunningDaemon::start(4).await;

    let ns_old = Namespace::Project("scratch-dir".to_string());
    let ns_new = Namespace::Project("rusty-brain".to_string());
    let ns_other = Namespace::Project("bystander".to_string());

    let mut client_old = Client::connect(&daemon.socket, ns_old.clone())
        .await
        .unwrap();
    let id_a = client_old
        .remember(
            "the single writer owns the sqlite connection".to_string(),
            None,
            MemoryType::ArchitectureDecision,
            8,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();
    let id_b = client_old
        .remember(
            "vec0 partitions knn candidates by namespace".to_string(),
            None,
            MemoryType::Insight,
            6,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();

    let mut client_other = Client::connect(&daemon.socket, ns_other.clone())
        .await
        .unwrap();
    let id_other = client_other
        .remember(
            "bystander memory stays put".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(1.0),
        )
        .await
        .unwrap();

    // Rename over the wire. The handshake namespace does not scope the admin
    // op; any connection may issue it (gating arrives with W2.6).
    let (moved, vectors) = client_old
        .rename_namespace(ns_old.clone(), ns_new.clone(), false)
        .await
        .unwrap();
    assert_eq!(moved, 2, "both memories re-scoped");
    assert_eq!(vectors, 2, "both vectors re-keyed");

    // A client scoped to the NEW namespace recalls the rows — the vector
    // channel runs a KNN under the new partition key (DeterministicProvider
    // embeds query and corpus alike, so a hit requires the vec0 row).
    let mut client_new = Client::connect(&daemon.socket, ns_new.clone())
        .await
        .unwrap();
    let results = client_new
        .recall(
            "single writer sqlite connection".to_string(),
            None,
            vec![],
            10,
        )
        .await
        .unwrap();
    assert!(
        results.iter().any(|r| r.memory.id == id_a),
        "recall in the new namespace must surface the renamed memory"
    );
    let listed: Vec<_> = client_new
        .list(None, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert!(listed.contains(&id_a) && listed.contains(&id_b));

    // The old namespace is empty for list AND recall.
    let mut client_old_again = Client::connect(&daemon.socket, ns_old.clone())
        .await
        .unwrap();
    assert!(client_old_again.list(None, 50).await.unwrap().is_empty());
    assert!(client_old_again
        .recall(
            "single writer sqlite connection".to_string(),
            None,
            vec![],
            10,
        )
        .await
        .unwrap()
        .is_empty());

    // The bystander namespace is untouched.
    let other_listed = client_other.list(None, 50).await.unwrap();
    assert_eq!(other_listed.len(), 1);
    assert_eq!(other_listed[0].id, id_other);

    // Collision policy over the wire: renaming the bystander INTO the now
    // populated namespace refuses without --merge (validation-class, message
    // carries the remediation), then succeeds with merge and reports counts.
    let err = client_other
        .rename_namespace(ns_other.clone(), ns_new.clone(), false)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::InvalidArgument(_)),
        "collision must surface as InvalidArgument over the wire, got {err:?}"
    );
    assert!(err.to_string().contains("--merge"), "{err}");

    let (moved, vectors) = client_other
        .rename_namespace(ns_other, ns_new, true)
        .await
        .unwrap();
    assert_eq!(moved, 1);
    assert_eq!(vectors, 1);
    let merged: Vec<_> = client_new
        .list(None, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(merged.len(), 3, "merge appended the bystander row");
    assert!(merged.contains(&id_other));

    daemon.stop().await;
}

// W3.1 update-as-supersede: `remember_superseding` stores a replacement AND
// atomically archives the prior memory in one wire call (the path the `Update`
// and `Link` rejections point at). This is what the SessionEnd capture flow
// uses to keep ONE live summary per session as it is re-summarized.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remember_superseding_archives_old_and_keeps_new() {
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("supersede-wire".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    let old = client
        .remember(
            "first draft of the session summary".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();

    let new = client
        .remember_superseding(
            "revised session summary with the final decision".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            None,
            old.clone(),
        )
        .await
        .unwrap();
    assert_ne!(new, old, "the replacement is a distinct memory");

    // The old memory is archived and points at the replacement.
    let archived = client.get(old.clone()).await.unwrap().unwrap();
    assert_eq!(
        archived.superseded_by,
        Some(new.clone()),
        "old points at the replacement"
    );
    assert!(archived.archived_at.is_some(), "old is archived");

    // The replacement is the only live row.
    let live: Vec<_> = client
        .list(None, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(live, vec![new], "only the replacement remains live");

    daemon.stop().await;
}

// #501 caller compatibility: the supersede fold after `remember` is
// best-effort — when the old row was ALREADY resolved (here: superseded by an
// earlier fold whose id a fail-open hook lost), the store guard refuses the
// re-supersede, the daemon logs it, and the NEW memory is still stored. The
// established lineage pointer is never rewritten.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remember_superseding_an_already_resolved_row_keeps_new_and_lineage() {
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("supersede-stale-wire".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    let old = client
        .remember(
            "first draft of the session summary".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();
    let first = client
        .remember_superseding(
            "second draft of the session summary".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            None,
            old.clone(),
        )
        .await
        .unwrap();

    // A stale client re-supersedes the SAME old id: the remember must still
    // succeed (best-effort fold), and old must still point at `first`.
    let second = client
        .remember_superseding(
            "third draft written against a stale view".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            None,
            old.clone(),
        )
        .await
        .unwrap();
    assert_ne!(second, first);

    let old_row = client.get(old.clone()).await.unwrap().unwrap();
    assert_eq!(
        old_row.superseded_by,
        Some(first.clone()),
        "the established lineage pointer is never rewritten"
    );
    let second_row = client.get(second.clone()).await.unwrap().unwrap();
    assert!(
        second_row.archived_at.is_none(),
        "the new memory is stored live despite the refused fold"
    );

    daemon.stop().await;
}

// Vikunja #502 (scorecard safety-gate MIE investigation): the exact
// `fresh-test-runner` supersede chain from the memory scorecard, pinned at the
// wire level with the same DeterministicProvider the CI scorecard daemon falls
// back to (no VOYAGE_API_KEY). Both injection inputs — `Context` (what the
// SessionStart digest renders) and `Recall` (what the UserPromptSubmit
// injection renders) — must surface the chain TIP and must NEVER surface the
// archived superseded row, including for an adversarial query that is the
// stale value itself. This pins candidate mechanisms (a) archived-value leak
// and (b) recall miss OUT of the retrieval layer: the 2026-07-12 N=5 MIEs
// (runs 3 and 4) were mechanism (c) — the model ignored a correct injection
// (see docs/eval/2026-07-12-w35-scorecard-n5-run.md).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn superseded_decision_never_reaches_context_or_recall_and_tip_always_does() {
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("rb-sc-fresh-test-runner".to_string());
    let mut client = Client::connect(&daemon.socket, ns.clone()).await.unwrap();

    // plant_explicit semantics from scripts/memory-scorecard.sh: two insight
    // facts at importance 5, the second stored as a superseding update.
    let old = client
        .remember(
            "Decision: run the test suite with `cargo test`.".to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            None,
        )
        .await
        .unwrap();
    let tip = client
        .remember_superseding(
            "Update: we moved the test suite to `cargo nextest run` for \
             parallelism and isolation. Use nextest, not plain `cargo test`, \
             in CI and locally."
                .to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            None,
            old.clone(),
        )
        .await
        .unwrap();

    // Context (the SessionStart digest input): the tip is the one recent row;
    // the archived value appears in NEITHER half.
    let (recent, important, total) = client.context().await.unwrap();
    assert_eq!(total, 1, "only the tip is in the recent window");
    assert!(
        recent.iter().any(|m| m.id == tip),
        "the supersede-chain tip must reach the digest"
    );
    assert!(
        recent.iter().chain(important.iter()).all(|m| m.id != old),
        "the archived superseded row must never reach the digest"
    );

    // Recall (the UserPromptSubmit injection input): the scenario's exact
    // work prompt, paraphrases, and the stale value itself as an adversarial
    // query. The tip must surface every time; the archived row never.
    for query in [
        "I need to run the project's tests. What exact command should I use?",
        "run the project's tests",
        "test command",
        "cargo test",
    ] {
        let results = client
            .recall(query.to_string(), None, vec![], 5)
            .await
            .unwrap();
        assert!(
            results.iter().any(|r| r.memory.id == tip),
            "the tip must surface for query {query:?}"
        );
        assert!(
            results.iter().all(|r| r.memory.id != old),
            "the archived superseded value must never surface for query {query:?}"
        );
    }

    daemon.stop().await;
}

// W3.1 write-time near-dup suppression: two hook captures with identical
// content embed to the same vector under the DeterministicProvider, so storing
// the second (a cosine-1.0 near-dup) absorbs the first via supersede — automatic
// capture can never pile up redundant rows between consolidation runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hook_writes_collapse_near_duplicates_at_write_time() {
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("hook-neardup".to_string());
    let hook = Some(ClientIdentity {
        source: Some("hook".to_string()),
        ..Default::default()
    });
    let mut client = Client::connect_with_identity(&daemon.socket, ns.clone(), hook)
        .await
        .unwrap();

    let content = "Session touched src/store.rs; ran cargo test; all green".to_string();
    let first = client
        .remember(
            content.clone(),
            None,
            MemoryType::Reference,
            4,
            vec![],
            vec!["hook".to_string()],
            vec![],
            Some(0.7),
        )
        .await
        .unwrap();
    let second = client
        .remember(
            content,
            None,
            MemoryType::Reference,
            4,
            vec![],
            vec!["hook".to_string()],
            vec![],
            Some(0.7),
        )
        .await
        .unwrap();
    assert_ne!(first, second);

    // The earlier hook capture was absorbed into the newer one.
    let archived = client.get(first.clone()).await.unwrap().unwrap();
    assert_eq!(
        archived.superseded_by,
        Some(second.clone()),
        "first absorbed into second"
    );
    assert!(archived.archived_at.is_some());

    // Only the newest capture remains live.
    let live: Vec<_> = client
        .list(None, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(live, vec![second], "only the newest hook capture is live");

    daemon.stop().await;
}

// W3.1 write-time near-dup suppression is STRICTLY gated to hook-source on both
// the new write and each candidate: a non-hook (user/cli/mcp) memory is never
// collapsed, even when it is a genuine near-dup of a hook capture.
//
// The three memories share BYTE-IDENTICAL composite embeddings (same content,
// tags, keywords, context — only the connection's origin_source differs), so
// every one is a cosine-1.0 near-dup candidate of every other under the
// DeterministicProvider. That makes the source gate the ONLY thing that can
// spare the cli row — and the hook control proves the candidate IS surfaced and
// would be collapsed but for its source. (A prior version differed in tags, so
// the cli row was never even a candidate and the gate was never exercised.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn near_dup_suppression_never_touches_non_hook_memories() {
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("neardup-safety".to_string());
    const TEXT: &str = "Deploys are frozen on Fridays — team policy";

    let connect = |source: &'static str| {
        Client::connect_with_identity(
            &daemon.socket,
            ns.clone(),
            Some(ClientIdentity {
                source: Some(source.to_string()),
                ..Default::default()
            }),
        )
    };

    // Identical-composite store helper (no tags/keywords/context so the composite
    // is exactly the content — the ONLY varying axis is the connection's source).
    async fn store(client: &mut Client, text: &str) -> rb_types::MemoryId {
        client
            .remember(
                text.to_string(),
                None,
                MemoryType::Insight,
                5,
                vec![],
                vec![],
                vec![],
                None,
            )
            .await
            .unwrap()
    }

    // A user-authored (cli) memory — exactly the kind suppression must protect.
    let mut cli = connect("cli").await.unwrap();
    let user_mem = store(&mut cli, TEXT).await;

    // First HOOK capture (identical composite): suppression surfaces the cli row
    // as a near-dup but its source gate spares it, and there is no prior hook row.
    let mut hook = connect("hook").await.unwrap();
    let hook_first = store(&mut hook, TEXT).await;

    // Second HOOK capture (identical composite): suppression now surfaces BOTH
    // the cli row and the first hook row. The first hook row IS collapsed (proving
    // the candidate is reachable and would be superseded); the cli row is spared.
    let hook_second = store(&mut hook, TEXT).await;

    // CONTROL: an identical-composite HOOK row IS absorbed into the newest write.
    let hook_first_after = cli.get(hook_first.clone()).await.unwrap().unwrap();
    assert_eq!(
        hook_first_after.superseded_by,
        Some(hook_second.clone()),
        "an identical hook row must be collapsed (candidate is reachable)"
    );
    assert!(hook_first_after.archived_at.is_some());

    // GATE: the non-hook memory — a near-dup under the SAME conditions — is NEVER
    // touched. The only difference from the collapsed hook row is its source.
    let user_after = cli.get(user_mem.clone()).await.unwrap().unwrap();
    assert!(
        user_after.archived_at.is_none(),
        "non-hook memory must never be suppressed, even as a cosine-1.0 near-dup"
    );
    assert_eq!(user_after.superseded_by, None);

    // Live set: the cli row and the newest hook row; the older hook row is gone.
    let live: std::collections::HashSet<_> = hook
        .list(None, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert!(
        live.contains(&user_mem) && live.contains(&hook_second),
        "the cli row and newest hook row are live: {live:?}"
    );
    assert!(
        !live.contains(&hook_first),
        "the collapsed hook row is no longer live"
    );

    daemon.stop().await;
}

/// W5a.4 protocol-evolution fixture: an N-1 client handshake. The daemon's
/// CURRENT policy window is exact-match (no breaking bump is in flight, so no
/// N/N-1 dual-support window is open) — an N-1 handshake must be REJECTED
/// GRACEFULLY: a HandshakeAck { ok: false } that names both versions, then a
/// closed connection. If a future bump opens a dual-support window, this test
/// is the seam to flip: it pins the version-skew behavior either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n_minus_one_handshake_is_rejected_gracefully() {
    let n_minus_one = rb_proto::CONTRACT_VERSION
        .checked_sub(1)
        .expect("N-1 fixture requires CONTRACT_VERSION >= 1");
    let daemon = RunningDaemon::start(2).await;

    let stream = tokio::net::UnixStream::connect(&daemon.socket)
        .await
        .unwrap();
    let mut framed = rb_proto::bounded_framed(stream);
    rb_proto::write_frame(
        &mut framed,
        &rb_proto::Handshake {
            contract_version: n_minus_one,
            namespace: Namespace::Project("skew".to_string()),
            identity: None,
        },
    )
    .await
    .unwrap();

    let ack: rb_proto::HandshakeAck = rb_proto::read_frame(&mut framed).await.unwrap();
    assert!(!ack.ok, "N-1 handshake must be refused, got ack: {ack:?}");
    assert_eq!(
        ack.contract_version,
        rb_proto::CONTRACT_VERSION,
        "the nack advertises the server's version so the client can report skew"
    );
    let message = ack.message.expect("nack carries a diagnostic message");
    assert!(
        message.contains(&rb_proto::CONTRACT_VERSION.to_string())
            && message.contains(&n_minus_one.to_string()),
        "diagnostic names both versions: {message}"
    );

    // After the nack the daemon closes the connection: no further frame ever
    // arrives (read yields EOF/error rather than hanging).
    let eof = tokio::time::timeout(
        Duration::from_secs(5),
        rb_proto::read_frame::<_, rb_proto::HandshakeAck>(&mut framed),
    )
    .await
    .expect("connection must be closed promptly after a version nack");
    assert!(eof.is_err(), "no frames after a version nack, got: {eof:?}");

    daemon.stop().await;
}

// Retention PRD verification e2e: seed aged + high-importance + protected +
// contested memories, then drive dry-run / apply / hard over a real socket
// and assert eligibility honors every guard. The same-uid client passes the
// hard-execute admin gate (the scrub precedent); the guardrail memories
// SURVIVE every pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retention_forget_flow_over_the_wire_respects_guards() {
    use rb_engine::MemoryBackend as _;

    let dir = tempdir();
    let socket = dir.path().join("runtime").join("sock");
    let db = dir.path().join("memory.db");
    let ns = Namespace::Project("retention-e2e".to_string());

    // Seed BEFORE the daemon starts: `remember` stamps created_at = now, so
    // aged rows must be written directly through a store handle.
    let seed = |content: &str, importance: u8, days: i64, tags: Vec<String>| {
        let mut m = rb_types::MemoryNote::new(
            ns.clone(),
            content.to_string(),
            MemoryType::Insight,
            importance,
        );
        m.created_at = chrono::Utc::now() - chrono::Duration::days(days);
        m.tags = tags;
        m
    };
    let old_low = seed("stale scratch detail", 2, 60, vec![]);
    let old_high = seed("author-vital decision", 9, 60, vec![]);
    let old_protected = seed(
        "protected by tag",
        2,
        60,
        vec!["architecture_decision".to_string()],
    );
    let contested_a = seed("contested claim", 2, 60, vec![]);
    let contested_b = seed("contradicting claim", 2, 60, vec![]);
    let young_low = seed("fresh scratch", 2, 0, vec![]);
    let (old_low_id, old_high_id, old_protected_id) = (
        old_low.id.clone(),
        old_high.id.clone(),
        old_protected.id.clone(),
    );
    let (contested_a_id, contested_b_id, young_id) = (
        contested_a.id.clone(),
        contested_b.id.clone(),
        young_low.id.clone(),
    );
    {
        let handle = rb_daemon::StoreHandle::start_with_model(
            db.clone(),
            DIM,
            "deterministic".to_string(),
            1,
        )
        .unwrap();
        for m in [
            old_low,
            old_high,
            old_protected,
            contested_a,
            contested_b,
            young_low,
        ] {
            handle.write(m, Some(vec![0.1f32; DIM])).await.unwrap();
        }
        handle
            .add_link(rb_types::MemoryLink {
                source_id: contested_a_id.clone(),
                target_id: contested_b_id.clone(),
                link_type: rb_types::LinkType::Contradicts,
                strength: 1.0,
                reason: "e2e contradiction".to_string(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        handle.shutdown().await;
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
        http: None,
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

    let mut client = Client::connect(&socket, ns.clone()).await.unwrap();
    let mut policy = rb_types::RetentionPolicy {
        enabled: false,
        max_age_days: Some(30),
        protected_tags: vec!["architecture_decision".to_string()],
        ..rb_types::RetentionPolicy::default()
    };

    // Dry-run works while DISABLED (read-only preview) and honors every
    // guard: exactly the old, low-importance, unprotected, uncontested row.
    let plan = client
        .forget_plan(policy.clone(), rb_types::ForgetMode::Apply)
        .await
        .unwrap();
    assert_eq!(plan.total_eligible, 1);
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(plan.candidates[0].id, old_low_id);

    // Execute while disabled: refused, nothing changed.
    let err = client
        .forget_execute(policy.clone(), rb_types::ForgetMode::Apply)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("enabled"),
        "disabled policy must refuse execution: {err}"
    );
    assert!(client
        .get(old_low_id.clone())
        .await
        .unwrap()
        .unwrap()
        .archived_at
        .is_none());

    // Enabled apply: archives the planned set — and ONLY it.
    policy.enabled = true;
    let outcome = client
        .forget_execute(policy.clone(), rb_types::ForgetMode::Apply)
        .await
        .unwrap();
    assert_eq!((outcome.archived, outcome.purged), (1, 0));
    assert!(client
        .get(old_low_id.clone())
        .await
        .unwrap()
        .unwrap()
        .archived_at
        .is_some());

    // Hard plan: the archived row is now the sole purge candidate.
    let plan = client
        .forget_plan(policy.clone(), rb_types::ForgetMode::Hard)
        .await
        .unwrap();
    assert_eq!(plan.candidates.len(), 1);
    assert_eq!(plan.candidates[0].id, old_low_id);
    assert!(plan.candidates[0].archived);

    // Hard execute (same-uid peer passes the admin gate, like scrub): purged
    // from the DB — get returns None.
    let outcome = client
        .forget_execute(policy.clone(), rb_types::ForgetMode::Hard)
        .await
        .unwrap();
    assert_eq!((outcome.archived, outcome.purged), (0, 1));
    assert!(client.get(old_low_id.clone()).await.unwrap().is_none());

    // Every guardrail memory SURVIVED all three passes, active and intact.
    for (id, label) in [
        (&old_high_id, "importance >= floor"),
        (&old_protected_id, "protected tag"),
        (&contested_a_id, "contested (local)"),
        (&contested_b_id, "contested (far)"),
        (&young_id, "younger than max_age_days"),
    ] {
        let got = client.get(id.clone()).await.unwrap();
        assert!(
            got.is_some_and(|m| m.archived_at.is_none()),
            "{label} memory must survive the sweeps"
        );
    }

    // RET-4 visibility: the sweep is recorded even though this daemon has no
    // [retention] policy of its own (eligible gauge stays None).
    let (stats, _, _) = client.stats(None).await.unwrap();
    assert!(stats.last_forget_at.is_some(), "last-forget recorded");
    assert_eq!(stats.retention_eligible, None, "no daemon-side policy");

    drop(client);
    let _ = tx.send(());
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("daemon shutdown")
        .expect("daemon task ok");
}

// ---------------------------------------------------------------------------
// Guided review (PRD 2026-07-02 contradiction/dedup review)
// ---------------------------------------------------------------------------

/// Store one CLI-sourced memory with explicit confidence and NO tags/keywords
/// (so the composite embedding is exactly the content — identical contents
/// become cosine-1.0 near-dups under the DeterministicProvider, distinct
/// contents stay far apart).
async fn store_plain(client: &mut Client, text: &str, confidence: f32) -> rb_types::MemoryId {
    client
        .remember(
            text.to_string(),
            None,
            MemoryType::Insight,
            5,
            vec![],
            vec![],
            vec![],
            Some(confidence),
        )
        .await
        .unwrap()
}

async fn cli_client(socket: &std::path::Path, ns: &Namespace) -> rb_types::Result<Client> {
    Client::connect_with_identity(
        socket,
        ns.clone(),
        Some(ClientIdentity {
            source: Some("cli".to_string()),
            ..Default::default()
        }),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_queue_surfaces_seeded_items_in_priority_order() {
    // PRD acceptance 1: `review` surfaces a seeded contradiction and a
    // near-dup pair in priority order; dry-run makes zero writes.
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("review-queue".to_string());
    let mut client = cli_client(&daemon.socket, &ns).await.unwrap();

    // Tier 3 first so insertion order cannot fake priority order.
    let low = store_plain(&mut client, "half remembered hunch about the cache", 0.2).await;
    // Tier 2: identical contents => cosine-1.0 twins (write-time suppression
    // is hook-gated, so both CLI writes stay live).
    let dup_a = store_plain(&mut client, "release trains depart every Tuesday", 1.0).await;
    let dup_b = store_plain(&mut client, "release trains depart every Tuesday", 1.0).await;
    // Tier 1: an explicit contradiction.
    let con_a = store_plain(
        &mut client,
        "the daemon owns exactly one writer thread",
        1.0,
    )
    .await;
    let con_b = store_plain(
        &mut client,
        "every connection writes straight to sqlite",
        1.0,
    )
    .await;
    client
        .link(
            con_a.clone(),
            con_b.clone(),
            rb_types::LinkType::Contradicts,
            Some("seeded".to_string()),
        )
        .await
        .unwrap();

    let plan = client.review_plan(None, None, None, None).await.unwrap();
    let reasons: Vec<rb_types::ReviewReason> = plan.items.iter().map(|i| i.reason).collect();
    assert_eq!(
        reasons,
        vec![
            rb_types::ReviewReason::Contradiction,
            rb_types::ReviewReason::NearDuplicate,
            rb_types::ReviewReason::LowConfidence,
        ],
        "priority order: contradictions, dups, low-confidence — got {plan:?}"
    );
    let contra_ids: Vec<&rb_types::MemoryId> =
        plan.items[0].members.iter().map(|m| &m.id).collect();
    assert!(contra_ids.contains(&&con_a) && contra_ids.contains(&&con_b));
    let dup_ids: Vec<&rb_types::MemoryId> = plan.items[1].members.iter().map(|m| &m.id).collect();
    assert!(dup_ids.contains(&&dup_a) && dup_ids.contains(&&dup_b));
    assert!(
        plan.items[1].similarity.unwrap() >= 0.95,
        "near-dup similarity rides the item"
    );
    assert_eq!(plan.items[2].members[0].id, low);
    assert_eq!(
        plan.items[2].members[0].origin_source.as_deref(),
        Some("cli"),
        "members carry provenance for the human decision"
    );
    assert!(plan.policy.is_none() && plan.planned.is_empty());

    // With a policy, the dry-run plans EXACTLY the policy's actions (the
    // shared-plan determinism the apply pass executes).
    let planned = client
        .review_plan(
            Some(rb_types::ReviewPolicy::AutoMergeDups),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(planned.policy, Some(rb_types::ReviewPolicy::AutoMergeDups));
    assert_eq!(planned.planned.len(), 1, "one dup pair to merge");
    assert_eq!(planned.planned[0].action, rb_types::ReviewAction::Merge);
    assert_eq!(planned.planned[0].key, planned.items[1].key);

    // Dry-runs made zero writes: everything is still live and unchanged.
    for id in [&low, &dup_a, &dup_b, &con_a, &con_b] {
        let m = client.get((*id).clone()).await.unwrap().unwrap();
        assert!(m.archived_at.is_none(), "{id} was mutated by a dry-run");
    }

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_apply_auto_merge_dups_merges_pair_and_feeds_history() {
    // PRD acceptance 2: `--apply --policy auto-merge-dups` merges a dup pair
    // into one superseding memory (originals archived), recording provenance
    // and feeding the history timeline.
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("review-apply".to_string());
    let mut client = cli_client(&daemon.socket, &ns).await.unwrap();

    let dup_a = store_plain(&mut client, "retro notes live in the shared drive", 0.9).await;
    let dup_b = store_plain(&mut client, "retro notes live in the shared drive", 0.6).await;
    let con_a = store_plain(&mut client, "use tabs in this repository", 1.0).await;
    let con_b = store_plain(&mut client, "use spaces in this repository", 1.0).await;
    client
        .link(
            con_a.clone(),
            con_b.clone(),
            rb_types::LinkType::Contradicts,
            None,
        )
        .await
        .unwrap();

    let outcome = client
        .review_apply(rb_types::ReviewPolicy::AutoMergeDups, None, None, None)
        .await
        .unwrap();
    assert_eq!(outcome.policy, Some(rb_types::ReviewPolicy::AutoMergeDups));
    assert_eq!(outcome.merged, 1, "one dup pair merged: {outcome:?}");
    assert_eq!(
        outcome.skipped, 1,
        "the contradiction is out of policy scope"
    );
    assert_eq!(outcome.total_items, 2);
    assert_eq!(outcome.failure, None);

    // Both originals archived and superseded by ONE combined memory.
    let a = client.get(dup_a.clone()).await.unwrap().unwrap();
    let b = client.get(dup_b.clone()).await.unwrap().unwrap();
    assert!(a.archived_at.is_some() && b.archived_at.is_some());
    let merged_id = a.superseded_by.clone().expect("a points at the merge");
    assert_eq!(b.superseded_by, Some(merged_id.clone()), "one survivor");
    let merged = client.get(merged_id.clone()).await.unwrap().unwrap();
    assert!(merged.archived_at.is_none(), "the combined memory is live");
    assert!(
        merged
            .content
            .contains("retro notes live in the shared drive"),
        "combined content carries the originals"
    );
    assert!(
        (merged.confidence - 0.9).abs() < 1e-6,
        "merge keeps the strongest confidence, got {}",
        merged.confidence
    );
    assert_eq!(
        merged.origin_source.as_deref(),
        Some("cli"),
        "REV-4: the resolution records the standard provenance"
    );

    // The contradiction pair is untouched.
    for id in [&con_a, &con_b] {
        let m = client.get((*id).clone()).await.unwrap().unwrap();
        assert!(m.archived_at.is_none());
    }

    // History (PRD 5 integration): the original's timeline walks forward to
    // the merged memory.
    let history = client.history(dup_a.clone(), None).await.unwrap();
    let chain_ids: Vec<&rb_types::MemoryId> = history.chain.iter().map(|e| &e.id).collect();
    assert!(
        chain_ids.contains(&&merged_id),
        "the merge shows up in the supersede chain: {history:?}"
    );

    // The queue converges: re-running review no longer surfaces the dup pair.
    let plan = client.review_plan(None, None, None, None).await.unwrap();
    assert!(
        plan.items
            .iter()
            .all(|i| i.reason != rb_types::ReviewReason::NearDuplicate),
        "merged pair must not resurface: {plan:?}"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_resolve_actions_apply_atomically_over_the_wire() {
    // PRD acceptance 3/4: interactive per-item actions apply atomically;
    // snoozed items do not reappear.
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("review-resolve".to_string());
    let mut client = cli_client(&daemon.socket, &ns).await.unwrap();

    let con_a = store_plain(&mut client, "deploy on fridays is fine", 0.8).await;
    let con_b = store_plain(&mut client, "never deploy on fridays", 0.8).await;
    client
        .link(
            con_a.clone(),
            con_b.clone(),
            rb_types::LinkType::Contradicts,
            None,
        )
        .await
        .unwrap();
    let low = store_plain(&mut client, "possibly the cache is per host", 0.2).await;

    // keep --bump nudges every member up by the documented delta.
    let resolution = client
        .resolve_review_item(
            rb_types::ReviewReason::Contradiction,
            vec![con_a.clone(), con_b.clone()],
            rb_types::ReviewAction::Keep { bump: true },
            None,
        )
        .await
        .unwrap();
    assert_eq!(resolution.action, "keep");
    assert_eq!(resolution.confidence.len(), 2);
    for mc in &resolution.confidence {
        assert!(
            (mc.confidence - 0.85).abs() < 1e-6,
            "0.8 + 0.05 bump, got {}",
            mc.confidence
        );
    }
    let bumped = client.get(con_a.clone()).await.unwrap().unwrap();
    assert!((bumped.confidence - 0.85).abs() < 1e-6, "bump persisted");

    // demote lowers the target's confidence by the documented step.
    let resolution = client
        .resolve_review_item(
            rb_types::ReviewReason::LowConfidence,
            vec![low.clone()],
            rb_types::ReviewAction::Demote { id: low.clone() },
            None,
        )
        .await
        .unwrap();
    assert_eq!(resolution.action, "demote");
    let demoted = client.get(low.clone()).await.unwrap().unwrap();
    assert!(
        (demoted.confidence - 0.05).abs() < 1e-6,
        "0.2 - 0.15 step, got {}",
        demoted.confidence
    );

    // archive soft-deletes the chosen loser and leaves the winner live.
    let resolution = client
        .resolve_review_item(
            rb_types::ReviewReason::Contradiction,
            vec![con_a.clone(), con_b.clone()],
            rb_types::ReviewAction::Archive { id: con_b.clone() },
            None,
        )
        .await
        .unwrap();
    assert_eq!(resolution.action, "archive");
    let loser = client.get(con_b.clone()).await.unwrap().unwrap();
    assert!(loser.archived_at.is_some(), "loser archived");
    let winner = client.get(con_a.clone()).await.unwrap().unwrap();
    assert!(winner.archived_at.is_none(), "winner untouched");

    // snooze hides the remaining low-confidence item from the queue.
    let before = client.review_plan(None, None, None, None).await.unwrap();
    assert!(
        before
            .items
            .iter()
            .any(|i| i.reason == rb_types::ReviewReason::LowConfidence),
        "low-confidence item present before snooze: {before:?}"
    );
    let resolution = client
        .resolve_review_item(
            rb_types::ReviewReason::LowConfidence,
            vec![low.clone()],
            rb_types::ReviewAction::Snooze { days: 3 },
            None,
        )
        .await
        .unwrap();
    assert_eq!(resolution.action, "snooze");
    let until = resolution.snoozed_until.expect("expiry returned");
    assert!(until > chrono::Utc::now().timestamp());
    let after = client.review_plan(None, None, None, None).await.unwrap();
    assert!(
        after
            .items
            .iter()
            .all(|i| i.reason != rb_types::ReviewReason::LowConfidence),
        "snoozed item must not reappear: {after:?}"
    );
    assert_eq!(after.totals.snoozed, 1);

    // Fail-closed validation: malformed resolutions never mutate.
    let err = client
        .resolve_review_item(
            rb_types::ReviewReason::NearDuplicate,
            vec![con_a.clone()],
            rb_types::ReviewAction::Merge,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
    let outsider = store_plain(&mut client, "an unrelated bystander", 1.0).await;
    let err = client
        .resolve_review_item(
            rb_types::ReviewReason::Contradiction,
            vec![con_a.clone(), con_b.clone()],
            rb_types::ReviewAction::Archive { id: outsider },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");

    // Namespace isolation: a foreign-namespace member fails closed (NotFound
    // at the daemon, surfaced as a wire error the client maps to
    // Error::Storage — the update/delete isolation convention) and nothing
    // is written.
    let other = Namespace::Project("review-resolve-other".to_string());
    let mut foreign_client = cli_client(&daemon.socket, &other).await.unwrap();
    let foreign = store_plain(&mut foreign_client, "foreign namespace memory", 1.0).await;
    let err = client
        .resolve_review_item(
            rb_types::ReviewReason::LowConfidence,
            vec![foreign.clone()],
            rb_types::ReviewAction::Archive {
                id: foreign.clone(),
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Storage(msg) if msg.contains("not found")),
        "foreign-namespace resolve must fail closed, got {err:?}"
    );
    let untouched = foreign_client.get(foreign).await.unwrap().unwrap();
    assert!(untouched.archived_at.is_none());

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_execute_requires_policy_and_absent_dry_run_previews() {
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("review-gate".to_string());
    let mut client = cli_client(&daemon.socket, &ns).await.unwrap();
    store_plain(&mut client, "just one memory", 0.2).await;

    // Raw frame path (an old or hand-rolled client): executing without a
    // policy is refused — REV-3 requires an explicit policy filter.
    let stream = tokio::net::UnixStream::connect(&daemon.socket)
        .await
        .unwrap();
    let mut framed = rb_proto::bounded_framed(stream);
    rb_proto::write_frame(
        &mut framed,
        &rb_proto::Handshake {
            contract_version: rb_proto::CONTRACT_VERSION,
            namespace: ns.clone(),
            identity: None,
        },
    )
    .await
    .unwrap();
    let ack: rb_proto::HandshakeAck = rb_proto::read_frame(&mut framed).await.unwrap();
    assert!(ack.ok);

    let raw: serde_json::Value = serde_json::json!({"op": "Review", "dry_run": false});
    rb_proto::write_frame(&mut framed, &raw).await.unwrap();
    let resp: rb_proto::Response = rb_proto::read_frame(&mut framed).await.unwrap();
    match resp {
        rb_proto::Response::Error { kind, message } => {
            assert_eq!(kind, "invalid_argument", "{message}");
            assert!(message.contains("policy"), "{message}");
        }
        other => panic!("execute without a policy must be refused, got {other:?}"),
    }

    // A frame with NO dry_run key at all must PREVIEW (the serde default),
    // never execute.
    let raw: serde_json::Value = serde_json::json!({"op": "Review"});
    rb_proto::write_frame(&mut framed, &raw).await.unwrap();
    let resp: rb_proto::Response = rb_proto::read_frame(&mut framed).await.unwrap();
    match resp {
        rb_proto::Response::ReviewPlanned { plan } => {
            assert_eq!(plan.totals.low_confidence, 1);
        }
        other => panic!("an absent dry_run must preview, got {other:?}"),
    }

    daemon.stop().await;
}

/// Delegates to the DeterministicProvider until the test ARMS a bounded
/// document-embed budget; once the budget is spent every further document
/// embed fails — injecting a mid-batch failure between two planned merges.
struct ArmableFailingProvider {
    inner: DeterministicProvider,
    budget: std::sync::Arc<std::sync::atomic::AtomicI64>,
}

#[async_trait::async_trait]
impl rb_embed::EmbeddingProvider for ArmableFailingProvider {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn dim(&self) -> usize {
        DIM
    }

    async fn embed(
        &self,
        texts: &[String],
        kind: rb_embed::EmbedKind,
    ) -> rb_types::Result<Vec<Vec<f32>>> {
        if kind == rb_embed::EmbedKind::Document
            && self
                .budget
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
                <= 0
        {
            return Err(Error::Embedding(
                "document embed outage (injected)".to_string(),
            ));
        }
        self.inner.embed(texts, kind).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_apply_reports_partial_outcome_on_mid_batch_failure() {
    // The ForgetOutcome shape (PR #60): completed per-item work is never
    // disowned by a mid-batch failure — the first merge stays committed, the
    // failure is reported, and the pass is re-runnable.
    let budget = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(i64::MAX));
    let embedder = SharedEmbedder::new(ArmableFailingProvider {
        inner: DeterministicProvider::new(DIM),
        budget: std::sync::Arc::clone(&budget),
    });
    let daemon = RunningDaemon::start_with_embedder(2, embedder).await;
    let ns = Namespace::Project("review-partial".to_string());
    let mut client = cli_client(&daemon.socket, &ns).await.unwrap();

    // Two independent dup pairs => two planned merges, in deterministic
    // (oldest-anchor-first) order.
    let a1 = store_plain(&mut client, "standups happen at nine thirty", 1.0).await;
    let a2 = store_plain(&mut client, "standups happen at nine thirty", 1.0).await;
    let b1 = store_plain(&mut client, "the staging box lives in the closet", 1.0).await;
    let b2 = store_plain(&mut client, "the staging box lives in the closet", 1.0).await;

    // Arm the outage: exactly ONE more document embed may succeed (the first
    // merge's combined memory); the second merge's embed fails.
    budget.store(1, std::sync::atomic::Ordering::SeqCst);

    let outcome = client
        .review_apply(rb_types::ReviewPolicy::AutoMergeDups, None, None, None)
        .await
        .unwrap();
    assert_eq!(outcome.merged, 1, "the first merge committed: {outcome:?}");
    let failure = outcome
        .failure
        .expect("the second merge's failure is reported");
    assert!(failure.contains("injected"), "{failure}");

    // Exactly ONE pair merged (which one is timing-dependent: seeds written
    // within the same unix second tie on created_at, so the anchor order
    // falls to the random-id tiebreak), and the other pair is fully intact.
    let mut merged_pairs = 0;
    for (x, y) in [(&a1, &a2), (&b1, &b2)] {
        let first = client.get((*x).clone()).await.unwrap().unwrap();
        let second = client.get((*y).clone()).await.unwrap().unwrap();
        if first.superseded_by.is_some() {
            merged_pairs += 1;
            assert!(
                first.archived_at.is_some() && second.archived_at.is_some(),
                "a merged pair archives both originals"
            );
            assert_eq!(
                second.superseded_by, first.superseded_by,
                "both originals point at the one combined memory"
            );
        } else {
            assert!(
                first.archived_at.is_none()
                    && second.archived_at.is_none()
                    && second.superseded_by.is_none(),
                "the failed merge left its pair untouched"
            );
        }
    }
    assert_eq!(merged_pairs, 1, "completed merge stays committed");

    // Disarm: the pass is re-runnable and converges.
    budget.store(i64::MAX, std::sync::atomic::Ordering::SeqCst);
    let outcome = client
        .review_apply(rb_types::ReviewPolicy::AutoMergeDups, None, None, None)
        .await
        .unwrap();
    assert_eq!(outcome.merged, 1, "re-run completes the remaining pair");
    assert_eq!(outcome.failure, None);

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_merges_of_the_same_pair_resolve_exactly_once() {
    // PR #63 CRITICAL: two concurrent resolutions of the same pair (e.g. an
    // interactive session racing a policy sweep) must never split the chain.
    // The whole merge is ONE writer transaction, so the loser re-validates
    // inside its own tx, finds the members claimed, and fails with the
    // distinct stale-plan error while the winner's chain stays single.
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("review-race".to_string());
    let mut seeder = cli_client(&daemon.socket, &ns).await.unwrap();
    let a = store_plain(&mut seeder, "the same fact twice", 1.0).await;
    let b = store_plain(&mut seeder, "the same fact twice", 1.0).await;

    let mut c1 = cli_client(&daemon.socket, &ns).await.unwrap();
    let mut c2 = cli_client(&daemon.socket, &ns).await.unwrap();
    let ids = vec![a.clone(), b.clone()];
    let (r1, r2) = tokio::join!(
        c1.resolve_review_item(
            rb_types::ReviewReason::NearDuplicate,
            ids.clone(),
            rb_types::ReviewAction::Merge,
            None,
        ),
        c2.resolve_review_item(
            rb_types::ReviewReason::NearDuplicate,
            ids.clone(),
            rb_types::ReviewAction::Merge,
            None,
        ),
    );
    let (ok, err) = match (r1, r2) {
        (Ok(ok), Err(err)) | (Err(err), Ok(ok)) => (ok, err),
        (Ok(x), Ok(y)) => panic!("BOTH merges succeeded — split chain: {x:?} / {y:?}"),
        (Err(x), Err(y)) => panic!("both merges failed: {x:?} / {y:?}"),
    };
    assert!(
        matches!(err, Error::StalePlan(_)),
        "the loser gets the distinct stale-plan error, got {err:?}"
    );
    let merged_id = ok.merged_into.expect("winner reports the combined id");

    // Single, consistent chain: both originals point at the ONE combined
    // memory; exactly one combined row is live.
    let a_row = seeder.get(a).await.unwrap().unwrap();
    let b_row = seeder.get(b).await.unwrap().unwrap();
    assert_eq!(a_row.superseded_by, Some(merged_id.clone()));
    assert_eq!(b_row.superseded_by, Some(merged_id.clone()));
    assert!(a_row.archived_at.is_some() && b_row.archived_at.is_some());
    let live: Vec<_> = seeder.list(None, 50).await.unwrap();
    assert_eq!(
        live.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
        vec![merged_id],
        "exactly one live memory: the winner's combined row"
    );

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolve_revalidates_the_planned_relationship() {
    // PR #63 HIGH (TOCTOU): a Resolve must prove the (reason, ids) claim at
    // resolve time — the wire must not accept Merge for two arbitrary active
    // ids, a broken contradiction, or a pair someone already resolved.
    let daemon = RunningDaemon::start(2).await;
    let ns = Namespace::Project("review-stale".to_string());
    let mut client = cli_client(&daemon.socket, &ns).await.unwrap();

    // Merge of two DISTINCT (non-near-dup) actives: stale plan, no writes.
    let x = store_plain(&mut client, "topic one entirely", 1.0).await;
    let y = store_plain(&mut client, "a different topic altogether", 1.0).await;
    let err = client
        .resolve_review_item(
            rb_types::ReviewReason::NearDuplicate,
            vec![x.clone(), y.clone()],
            rb_types::ReviewAction::Merge,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::StalePlan(_)), "{err:?}");
    for id in [&x, &y] {
        let m = client.get((*id).clone()).await.unwrap().unwrap();
        assert!(m.archived_at.is_none() && m.superseded_by.is_none());
    }

    // Merge after a member was resolved (archived): stale plan.
    let d1 = store_plain(&mut client, "duplicated remark", 1.0).await;
    let d2 = store_plain(&mut client, "duplicated remark", 1.0).await;
    client.delete(d2.clone()).await.unwrap();
    let err = client
        .resolve_review_item(
            rb_types::ReviewReason::NearDuplicate,
            vec![d1.clone(), d2.clone()],
            rb_types::ReviewAction::Merge,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::StalePlan(_)), "{err:?}");

    // A contradiction action after the pair stopped contradicting (one side
    // archived): stale plan, nothing mutated.
    let c1 = store_plain(&mut client, "always squash merge", 0.8).await;
    let c2 = store_plain(&mut client, "never squash merge", 0.8).await;
    client
        .link(
            c1.clone(),
            c2.clone(),
            rb_types::LinkType::Contradicts,
            None,
        )
        .await
        .unwrap();
    client.delete(c2.clone()).await.unwrap();
    let err = client
        .resolve_review_item(
            rb_types::ReviewReason::Contradiction,
            vec![c1.clone(), c2.clone()],
            rb_types::ReviewAction::Keep { bump: true },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::StalePlan(_)), "{err:?}");
    let untouched = client.get(c1.clone()).await.unwrap().unwrap();
    assert!(
        (untouched.confidence - 0.8).abs() < 1e-6,
        "the stale keep-bump must not nudge confidence"
    );

    // Merge on a Contradiction item is rejected outright (reason-restricted).
    let e1 = store_plain(&mut client, "tabs are correct", 1.0).await;
    let e2 = store_plain(&mut client, "spaces are correct", 1.0).await;
    client
        .link(
            e1.clone(),
            e2.clone(),
            rb_types::LinkType::Contradicts,
            None,
        )
        .await
        .unwrap();
    let err = client
        .resolve_review_item(
            rb_types::ReviewReason::Contradiction,
            vec![e1, e2],
            rb_types::ReviewAction::Merge,
            None,
        )
        .await
        .unwrap_err();
    match &err {
        Error::InvalidArgument(msg) => {
            assert!(msg.contains("near-duplicate"), "{msg}");
        }
        other => panic!("merge on a contradiction must be invalid, got {other:?}"),
    }

    daemon.stop().await;
}
