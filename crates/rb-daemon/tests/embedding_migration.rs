//! Cross-dimension acceptance, bounded re-embedding, and recall through a live daemon.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};

use rb_daemon::{Daemon, DaemonConfig, JobsConfig, SharedEmbedder};
use rb_embed::{DeterministicProvider, EmbedKind, EmbeddingProvider};
use rb_proto::Client;
use rb_store::{SqliteStore, Store};
use rb_types::{MemoryNote, MemoryType, Namespace};

const OLD_DIM: usize = 512;
const NEW_DIM: usize = 384;

struct RetryDocumentProvider {
    deterministic: DeterministicProvider,
    fail_document_once: AtomicBool,
}

#[async_trait::async_trait]
impl EmbeddingProvider for RetryDocumentProvider {
    fn model_id(&self) -> &str {
        "migration-retry-fixture"
    }

    fn dim(&self) -> usize {
        NEW_DIM
    }

    async fn embed(&self, texts: &[String], kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        if matches!(kind, EmbedKind::Document)
            && self.fail_document_once.swap(false, Ordering::SeqCst)
        {
            return Err(rb_types::Error::Embedding(
                "injected first re-embed failure".to_string(),
            ));
        }
        self.deterministic.embed(texts, kind).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepts_512_to_384_retries_skips_and_recalls_new_vector() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("memory.db");
    let socket = dir.path().join("runtime").join("daemon.sock");
    let namespace = Namespace::Project("dimension-migration".to_string());
    let old = SqliteStore::open_with_model(&database, OLD_DIM, "old-voyage-fixture").unwrap();
    let mut memory = MemoryNote::new(
        namespace.clone(),
        "A cobalt lantern identifies the accepted cross-dimension migration.".to_string(),
        MemoryType::ArchitectureDecision,
        7,
    );
    let id = memory.id.clone();
    memory.embedding_model = "old-voyage-fixture".to_string();
    memory.embedding_input_version = "v2-composite".to_string();
    old.insert_memory(&memory, Some(&vec![0.125f32; OLD_DIM]))
        .unwrap();
    drop(old);

    let daemon = Daemon::bind_accepting_model_change(
        DaemonConfig {
            socket_path: socket.clone(),
            db_path: database.clone(),
            read_pool_size: 1,
            jobs_config: JobsConfig::default(),
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_engine::FusionMode::Linear,
            abstain_threshold: Some(0.0),
            write_gate: rb_types::WriteGateConfig::default(),
            http: None,
        },
        SharedEmbedder::new(RetryDocumentProvider {
            deterministic: DeterministicProvider::new(NEW_DIM),
            fail_document_once: AtomicBool::new(true),
        }),
    )
    .await
    .unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        daemon
            .run(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    let mut client = Client::connect(&socket, namespace.clone()).await.unwrap();
    assert_eq!(
        client.reembed(Some(10)).await.unwrap(),
        (1, 0, 1),
        "a failed embed is skipped, not treated as convergence"
    );
    assert_eq!(
        client
            .get(id.clone())
            .await
            .unwrap()
            .unwrap()
            .embedding_input_version,
        "",
        "the skipped row retains its stale stamp"
    );
    assert_eq!(
        client.reembed(Some(10)).await.unwrap(),
        (1, 1, 0),
        "the next bounded pass retries and converges the active row"
    );
    assert_eq!(client.reembed(Some(10)).await.unwrap(), (0, 0, 0));

    let recalled = client
        .recall(
            "A cobalt lantern identifies the accepted cross-dimension migration.".to_string(),
            None,
            vec![],
            10,
        )
        .await
        .unwrap();
    assert!(
        recalled
            .iter()
            .any(|result| result.memory.id == id && result.channels.vector),
        "the migrated corpus recalls the memory through its new-dimensional vector"
    );
    drop(client);

    shutdown_tx.send(()).unwrap();
    serving.await.unwrap();

    let reopened =
        SqliteStore::open_with_model(&database, NEW_DIM, "migration-retry-fixture").unwrap();
    let stored = reopened.get_memory(&id).unwrap().unwrap();
    assert_eq!(stored.embedding_model, "migration-retry-fixture");
    assert_eq!(stored.embedding_input_version, "v2-composite");
}
