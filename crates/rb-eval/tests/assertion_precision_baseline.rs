//! Direct production exact-ID probes authored before the evaluator. Required
//! budget/supersede/namespace cases are permanent regressions; unresolved
//! capability investigations remain opt-in.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use rb_embed::DeterministicProvider;
use rb_engine::{MemoryBackend, MemoryEngine, Provenance, RememberInput};
use rb_eval::backend::{SqliteBackend, EVAL_DIM};
use rb_types::{LinkType, MemoryId, MemoryLink, MemoryType, Namespace, RecallFilter};
use serde_json::Value;
use std::collections::BTreeSet;

async fn assert_class(class: &str) {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/assertion_precision.json")).unwrap();
    let now = chrono::DateTime::parse_from_rfc3339(fixture["clock"].as_str().unwrap())
        .unwrap()
        .to_utc();
    let mut failures = Vec::new();
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["failure_class"] == class)
    {
        let ns = Namespace::parse_db_string(case["namespace"].as_str().unwrap()).unwrap();
        let engine = MemoryEngine::new(
            SqliteBackend::in_memory(EVAL_DIM).unwrap(),
            DeterministicProvider::new(EVAL_DIM),
            ns,
        )
        .with_fixed_now(now);
        for row in case["memories"].as_array().unwrap() {
            let (mut note, vector) = engine
                .compose_note(RememberInput {
                    content: row["content"].as_str().unwrap().to_string(),
                    context: None,
                    memory_type: MemoryType::Insight,
                    importance: row["importance"].as_u64().unwrap().try_into().unwrap(),
                    keywords: Vec::new(),
                    tags: Vec::new(),
                    related_files: Vec::new(),
                    confidence: Some(1.0),
                    provenance: Provenance::default(),
                    anchors: Vec::new(),
                })
                .await
                .unwrap();
            note.id = row["id"].as_str().unwrap().parse().unwrap();
            note.namespace =
                Namespace::parse_db_string(row["namespace"].as_str().unwrap()).unwrap();
            note.session_id = row["session_id"].as_str().map(str::to_string);
            engine.backend().write(note, vector).await.unwrap();
        }
        for pair in case["links"].as_array().unwrap() {
            engine
                .backend()
                .add_link(MemoryLink {
                    source_id: pair[0].as_str().unwrap().parse().unwrap(),
                    target_id: pair[1].as_str().unwrap().parse().unwrap(),
                    link_type: LinkType::References,
                    strength: 1.0,
                    reason: "authored precision fixture graph edge".into(),
                    created_at: now,
                })
                .await
                .unwrap();
        }
        for pair in case["supersedes"].as_array().unwrap() {
            let old: MemoryId = pair[0].as_str().unwrap().parse().unwrap();
            let new: MemoryId = pair[1].as_str().unwrap().parse().unwrap();
            engine.backend().supersede_for_eval(&old, &new).unwrap();
        }
        for query in case["queries"].as_array().unwrap() {
            let actual = engine
                .recall(
                    query["query"].as_str().unwrap(),
                    query["limit"].as_u64().unwrap().try_into().unwrap(),
                    &RecallFilter::default(),
                )
                .await
                .unwrap();
            let returned: BTreeSet<String> =
                actual.iter().map(|r| r.memory.id.to_string()).collect();
            let expected: BTreeSet<String> = query["expected_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_string())
                .collect();
            let pass = returned == expected && actual.len() == returned.len();
            println!(
                "{}",
                serde_json::json!({"case":case["id"],"class":class,"query":query["query"],"expected_ids":expected,"returned_ids":returned,"passed":pass})
            );
            if !pass {
                failures.push(case["id"].clone());
            }
        }
    }
    assert!(
        failures.is_empty(),
        "exact ID-set capability failures: {failures:?}"
    );
}

#[tokio::test]
#[ignore = "capability gate; run explicitly, not an instrument correctness test"]
async fn hard_negatives() {
    assert_class("hard_negative").await;
}
#[tokio::test]
async fn supersede_resurfacing() {
    assert_class("supersede_resurfacing").await;
}
#[tokio::test]
async fn budget_eviction() {
    assert_class("budget_eviction").await;
}
#[tokio::test]
async fn namespace_leak() {
    assert_class("namespace_leak").await;
}
#[tokio::test]
#[ignore = "capability gate; run explicitly, not an instrument correctness test"]
async fn session_noise() {
    assert_class("session_noise").await;
}
