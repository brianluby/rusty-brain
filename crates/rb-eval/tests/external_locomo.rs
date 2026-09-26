//! Offline smoke for the LoCoMo adapter and the memoizing provider.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::{DeterministicProvider, EmbedKind, EmbeddingProvider};
use rb_eval::backend::EVAL_DIM;
use rb_eval::external::core::{Memo, Profile};
use rb_eval::external::locomo::{run_sample, session_documents, Sample};
use std::sync::Arc;

fn samples() -> Vec<Sample> {
    serde_json::from_str(include_str!("../fixtures/external/locomo_smoke.json")).unwrap()
}

#[test]
fn session_documents_use_mempalace_raw_format() {
    let docs = session_documents(&samples()[0]).unwrap();
    assert_eq!(docs.len(), 3);
    assert_eq!(docs[0].0, "session_1");
    assert_eq!(
        docs[0].1,
        "Ana said, \"I adopted a greyhound named Pixel.\"\nBo said, \"Lovely!\""
    );
}

#[tokio::test]
async fn top_k_above_candidate_count_is_refused() {
    let provider = Arc::new(DeterministicProvider::new(EVAL_DIM));
    let err = run_sample(&provider, &samples()[0], Profile::Parity, 10).await;
    assert!(err.is_err(), "top-k 10 over 3 sessions bypasses retrieval");
}

#[tokio::test]
async fn memoized_runs_match_unmemoized_runs() {
    let plain = Arc::new(DeterministicProvider::new(EVAL_DIM));
    let memo = Arc::new(Memo::new(DeterministicProvider::new(EVAL_DIM)));
    for profile in [Profile::Parity, Profile::Native] {
        let a = run_sample(&plain, &samples()[0], profile, 2).await.unwrap();
        let b = run_sample(&memo, &samples()[0], profile, 2).await.unwrap();
        assert_eq!(a.len(), 3);
        for (x, y) in a.iter().zip(&b) {
            let ids = |r: &rb_eval::external::locomo::QaRecord| {
                r.ranked
                    .iter()
                    .map(|h| (h.session_id.clone(), h.score.to_bits()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(ids(x), ids(y));
        }
        assert!(a[2].evidence_empty && (a[2].evidence_recall - 1.0).abs() < 1e-12);
    }
    let texts = vec!["same".to_string(), "same".to_string()];
    let v = memo.embed(&texts, EmbedKind::Document).await.unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0], v[1]);
}
