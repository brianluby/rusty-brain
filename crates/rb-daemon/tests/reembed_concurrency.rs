//! Real engine -> read pool -> single writer regression for stale reembed results.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_daemon::StoreHandle;
use rb_embed::{DeterministicProvider, EmbedKind, EmbeddingProvider};
use rb_engine::{embedding_input, MemoryBackend, MemoryEngine, EMBEDDING_INPUT_VERSION};
use rb_types::{MemoryNote, MemoryType, MemoryUpdates, Namespace};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

const DIM: usize = 8;

/// Pause only the first provider call, after the engine has captured its input.
/// Notifications (not sleeps) force the edit to commit before that call returns.
struct PausedProvider {
    started: Arc<Notify>,
    resume: Arc<Notify>,
    pause: AtomicBool,
}

#[async_trait::async_trait]
impl EmbeddingProvider for PausedProvider {
    fn model_id(&self) -> &str {
        "deterministic"
    }

    fn dim(&self) -> usize {
        DIM
    }

    async fn embed(&self, texts: &[String], kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        if self.pause.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            self.resume.notified().await;
        }
        DeterministicProvider::new(DIM).embed(texts, kind).await
    }
}

#[tokio::test]
async fn reembed_rejects_interleaved_tag_edit_and_next_pass_converges() {
    assert_interleaved_edit(MemoryUpdates {
        tags: Some(vec!["edited-tag".into()]),
        ..Default::default()
    })
    .await;
}

#[tokio::test]
async fn reembed_rejects_interleaved_content_edit_and_next_pass_converges() {
    assert_interleaved_edit(MemoryUpdates {
        content: Some("edited content".into()),
        ..Default::default()
    })
    .await;
}

#[tokio::test]
async fn reembed_rejects_interleaved_context_edit_and_next_pass_converges() {
    assert_interleaved_edit(MemoryUpdates {
        context: Some("edited context".into()),
        ..Default::default()
    })
    .await;
}

async fn assert_interleaved_edit(updates: MemoryUpdates) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let dir = tempfile::tempdir().unwrap();
        let handle = StoreHandle::start(dir.path().join("race.db"), DIM, 2).unwrap();
        let ns = Namespace::Project("reembed-race".into());
        let mut note = MemoryNote::new(
            ns.clone(),
            "unchanged content".into(),
            MemoryType::Insight,
            5,
        );
        note.embedding_model = "deterministic".into();
        // Already stale: a CAS on model/version would miss the subsequent edit,
        // which sets this SAME empty sentinel again.
        note.embedding_input_version = String::new();
        note.tags = vec!["old-tag".into()];
        let id = note.id.clone();
        let old_vector = vec![1.0; DIM];
        handle
            .write(note.clone(), Some(old_vector.clone()))
            .await
            .unwrap();

        let started = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let engine = MemoryEngine::new(
            handle.clone(),
            PausedProvider {
                started: started.clone(),
                resume: resume.clone(),
                pause: AtomicBool::new(true),
            },
            ns.clone(),
        );
        let edit = async {
            started.notified().await;
            handle
                .update(ns.clone(), id.clone(), updates)
                .await
                .unwrap();
            let edited = handle.get(ns.clone(), id.clone()).await.unwrap().unwrap();
            assert_eq!(edited.embedding_input_version, note.embedding_input_version);
            assert_eq!(edited.embedding_model, note.embedding_model);
            resume.notify_one();
            edited
        };
        let (first, edited) = tokio::join!(engine.reembed(10), edit);
        assert_eq!(
            first.unwrap(),
            (1, 0, 1),
            "obsolete embedding must be skipped, not certified"
        );
        let after = handle.get(ns.clone(), id.clone()).await.unwrap().unwrap();
        assert_eq!(after.tags, edited.tags);
        assert_eq!(after.content, edited.content);
        assert_eq!(after.context, edited.context);
        assert_eq!(
            after.updated_at, edited.updated_at,
            "maintenance must not bump the authorial timestamp"
        );
        assert_eq!(
            after.embedding_input_version, "",
            "the edit's stale marker must survive"
        );
        let hits = handle.vector(ns.clone(), old_vector, 1).await.unwrap();
        assert_eq!(hits[0].0, id);
        assert!(
            hits[0].1.abs() < 1e-6,
            "rejected write must leave the original vector intact"
        );
        let candidates = handle
            .memories_for_reembed("deterministic".into(), EMBEDDING_INPUT_VERSION.into(), 10)
            .await
            .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, id);

        assert_eq!(engine.reembed(10).await.unwrap(), (1, 1, 0));
        let repaired = handle.get(ns.clone(), id.clone()).await.unwrap().unwrap();
        assert_eq!(repaired.embedding_input_version, EMBEDDING_INPUT_VERSION);
        assert_eq!(repaired.updated_at, edited.updated_at);
        let expected = DeterministicProvider::new(DIM)
            .embed(&[embedding_input(&edited)], EmbedKind::Document)
            .await
            .unwrap()
            .remove(0);
        let hits = handle.vector(ns, expected, 1).await.unwrap();
        assert_eq!(hits[0].0, id);
        assert!(
            hits[0].1.abs() < 1e-6,
            "second pass must embed the edited input"
        );
        assert_eq!(engine.reembed(10).await.unwrap(), (0, 0, 0));
        handle.shutdown().await;
    })
    .await
    .expect("reembed/edit interleaving must not deadlock");
}
