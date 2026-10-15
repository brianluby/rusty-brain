//! #85 topology/history/ID regressions; deterministic probes are not semantic proof.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use rb_embed::DeterministicProvider;
use rb_eval::coding_memory::{run_replay, run_with, VECTOR_PATH};
use rb_types::MemoryId;

fn id(number: u32) -> MemoryId {
    format!("12470000-0000-4000-8000-{number:012}")
        .parse()
        .unwrap()
}

/// One-hop-only evidence must really be absent from both direct channels.
#[tokio::test]
async fn graph_fixture_has_exact_topology_and_never_disguises_two_hop_as_pass() {
    let report = run_with(|| Ok(DeterministicProvider::new(32)))
        .await
        .unwrap();
    let one = report
        .cases
        .iter()
        .find(|c| c.id == "controlled-one-hop-incident")
        .unwrap();
    let query = &one.queries[0];
    assert!(one.passed, "{:?}", query.failures);
    let candidates = query.candidates.as_ref().unwrap();
    assert!(candidates.topology_passed);
    assert!(!candidates.keyword_ids.contains(&id(5)));
    assert!(!candidates.vector_ids.contains(&id(5)));
    let incident = query
        .returned_evidence
        .iter()
        .find(|r| r.id == id(5))
        .unwrap();
    assert!(incident.channels.graph && !incident.channels.fts && !incident.channels.vector);

    let two = report
        .cases
        .iter()
        .find(|c| c.id == "controlled-two-hop-incident")
        .unwrap();
    assert_eq!(two.category, "unsupported_two_hop");
    assert!(!two.passed);
    assert!(two.queries[0].candidates.as_ref().unwrap().topology_passed);
    assert_eq!(two.queries[0].assertion.missing_ids, [id(8).to_string()]);
    assert!(two.queries[0].assertion.extra_ids.is_empty());
    assert!(two.queries[0]
        .failures
        .iter()
        .any(|f| f == "missing_evidence"));
}

/// Current recall and explicit history have different, fixed evidence contracts.
#[tokio::test]
async fn current_and_prior_use_active_recall_and_explicit_history() {
    let report = run_with(|| Ok(DeterministicProvider::new(32)))
        .await
        .unwrap();
    let case = report
        .cases
        .iter()
        .find(|c| c.id == "current-and-prior-decision")
        .unwrap();
    assert!(case.passed, "{:?}", case.queries);
    assert_eq!(case.queries[0].assertion.returned_ids, [id(10).to_string()]);
    assert!(case.queries[0].forbidden_returned_ids.is_empty());
    let prior = &case.queries[1];
    assert!(prior.candidates.is_none() && prior.returned_evidence.is_empty());
    let history = prior.history.as_ref().unwrap();
    assert_eq!(
        history
            .chain
            .iter()
            .map(|e| e.id.clone())
            .collect::<Vec<_>>(),
        [id(9), id(10)]
    );
    assert!(history.chain[0].archived && !history.chain[0].current);
    assert!(!history.chain[1].archived && history.chain[1].current);
    assert!(!history.truncated);
}

/// Opposing near-identical facts remain distinct, contested, and not merged.
#[tokio::test]
async fn conflict_requires_both_exact_ids_and_forbids_obsolete_instruction() {
    let report = run_with(|| Ok(DeterministicProvider::new(32)))
        .await
        .unwrap();
    let case = report
        .cases
        .iter()
        .find(|c| c.id == "near-identical-conflicting-instructions")
        .unwrap();
    assert!(case.passed, "{:?}", case.queries[0]);
    assert!(case.queries[0].forbidden_returned_ids.is_empty());
    assert!(case.queries[0].failures.is_empty());
}

/// Missing semantic inputs are a hard failure, not keyword fallback or fake vectors.
#[tokio::test]
async fn empty_real_fixture_fails_closed() {
    let error = run_replay(r#"{"model_id":"all-MiniLM-L6-v2","dim":384,"vectors":[]}"#)
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<rb_types::Error>(),
        Some(rb_types::Error::Embedding(_))
    ));
}

/// Record only new #85 semantic inputs using the real local model, never Voyage.
#[cfg(feature = "record-local")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual local ONNX recording; isolated #85 inputs only"]
async fn record_coding_memory_local_vectors() {
    use rb_embed::EmbeddingProvider;
    use rb_engine::MemoryEngine;
    use rb_eval::{
        backend::{eval_namespace, SqliteBackend},
        RecordingProvider,
    };
    let local = rb_embed::LocalProvider::load("all-MiniLM-L6-v2").unwrap();
    let recorder = RecordingProvider::new(local);
    let engine = MemoryEngine::new(
        SqliteBackend::in_memory(recorder.dim()).unwrap(),
        recorder,
        eval_namespace(),
    );
    rb_eval::coding_memory::record_inputs(&engine)
        .await
        .unwrap();
    let fixture = engine.embedder().fixture("Real local all-MiniLM-L6-v2 #85-only development vectors; no frozen corpus or sealed holdout. Graph-only documents are recorded even though their stored vectors are deliberately omitted. Regenerate: cargo test --locked -p rb-eval --features record-local --test coding_memory record_coding_memory_local_vectors -- --ignored --exact --nocapture").unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(VECTOR_PATH);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&fixture).unwrap() + "\n",
    )
    .unwrap();
    println!(
        "Recorded {} real vectors into {}",
        fixture.vectors.len(),
        path.display()
    );
}

/// Offline semantic capability run is manual until the isolated recording exists.
#[tokio::test]
#[ignore = "manual categorized real replay; run after local recording"]
async fn replay_coding_memory_recorded_vectors() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(VECTOR_PATH);
    let report = run_replay(&std::fs::read_to_string(path).unwrap())
        .await
        .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    assert_eq!(report.model_id, "all-MiniLM-L6-v2");
    assert!(report.vectors_sha256.is_some());
    for case in &report.cases {
        for query in &case.queries {
            assert!(query.candidates.as_ref().is_none_or(|c| c.topology_passed));
            assert!(query.forbidden_returned_ids.is_empty());
        }
    }
    let history = report
        .cases
        .iter()
        .find(|c| c.id == "current-and-prior-decision")
        .unwrap();
    assert!(history.passed, "{history:?}");
    let conflict = report
        .cases
        .iter()
        .find(|c| c.id == "near-identical-conflicting-instructions")
        .unwrap();
    assert!(conflict.passed, "{conflict:?}");
    let graph = report
        .cases
        .iter()
        .find(|c| c.id == "controlled-one-hop-incident")
        .unwrap();
    assert!(graph.passed, "{graph:?}");
    let unsupported = report
        .cases
        .iter()
        .find(|c| c.id == "controlled-two-hop-incident")
        .unwrap();
    assert!(!unsupported.passed);
    assert_eq!(
        unsupported.queries[0].assertion.missing_ids,
        [id(8).to_string()]
    );
}
