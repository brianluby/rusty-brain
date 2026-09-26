//! Offline smoke for the LongMemEval adapter (Vikunja #60). Uses the
//! deterministic provider, so it checks plumbing, id round-trip and
//! isolation — never retrieval quality.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::DeterministicProvider;
use rb_eval::backend::EVAL_DIM;
use rb_eval::external::longmemeval::{run_question, session_documents, Entry, Profile};
use std::sync::Arc;

fn entries() -> Vec<Entry> {
    serde_json::from_str(include_str!("../fixtures/external/longmemeval_smoke.json")).unwrap()
}

#[test]
fn documents_are_user_turns_only_and_never_carry_session_ids() {
    for entry in entries() {
        for (id, doc) in session_documents(&entry) {
            assert!(!doc.contains("answer_") && !doc.contains(&id));
            assert!(
                !doc.contains("It grinds very evenly"),
                "assistant text excluded"
            );
        }
    }
    let e = &entries()[1];
    assert_eq!(
        session_documents(e).len(),
        3,
        "assistant-only session skipped"
    );
}

#[tokio::test]
async fn both_profiles_round_trip_gold_session_ids() {
    let provider = Arc::new(DeterministicProvider::new(EVAL_DIM));
    for profile in [Profile::Parity, Profile::Native] {
        for entry in entries() {
            let r = run_question(&provider, &entry, profile).await.unwrap();
            let known: std::collections::HashSet<_> = entry.haystack_session_ids.iter().collect();
            assert!(r.ranked.iter().all(|h| known.contains(&h.session_id)));
            assert_eq!(r.corpus_size, session_documents(&entry).len());
            if entry.answer_session_ids.is_empty() {
                assert_eq!(r.metrics.mrr, 0.0);
            }
        }
    }
    let native = run_question(&provider, &entries()[0], Profile::Native)
        .await
        .unwrap();
    assert_eq!(native.ranked[0].session_id, "answer_s1", "{native:?}");
}

#[tokio::test]
async fn questions_are_isolated_from_each_other() {
    let provider = Arc::new(DeterministicProvider::new(EVAL_DIM));
    let all = entries();
    for profile in [Profile::Parity, Profile::Native] {
        let first = run_question(&provider, &all[0], profile).await.unwrap();
        run_question(&provider, &all[1], profile).await.unwrap();
        let again = run_question(&provider, &all[0], profile).await.unwrap();
        let key = |r: &rb_eval::external::longmemeval::QuestionRecord| {
            r.ranked
                .iter()
                .map(|h| (h.session_id.clone(), h.score.to_bits()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            key(&first),
            key(&again),
            "{profile:?} leaked state across questions"
        );
    }
}
