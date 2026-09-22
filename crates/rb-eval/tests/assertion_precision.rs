//! Instrument tests: verify scoring/reporting, not that production passes the gate.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use rb_eval::assertion_precision::assert_exact_ids;
use rb_types::MemoryId;
use std::process::Command;

fn id(suffix: u8) -> MemoryId {
    format!("12180000-0000-4000-8000-{suffix:012}")
        .parse()
        .unwrap()
}

#[test]
fn exact_assertion_rejects_any_extra_including_unknown_ids() {
    let expected = [id(1)];
    for extra in [id(2), id(99)] {
        let verdict = assert_exact_ids(&expected, &[id(1), extra.clone()]);
        assert!(!verdict.passed);
        assert_eq!(verdict.extra_ids, [extra.to_string()]);
        assert!(verdict.missing_ids.is_empty());
    }
}

#[test]
fn exact_assertion_handles_missing_empty_order_and_duplicates() {
    assert!(assert_exact_ids(&[], &[]).passed);
    assert!(assert_exact_ids(&[id(1), id(2)], &[id(2), id(1)]).passed);
    let missing = assert_exact_ids(&[id(1), id(2)], &[id(1)]);
    assert!(!missing.passed);
    assert_eq!(missing.missing_ids, [id(2).to_string()]);
    assert!(!assert_exact_ids(&[], &[id(1)]).passed);
    assert!(!assert_exact_ids(&[id(1)], &[]).passed);
    let duplicate = assert_exact_ids(&[id(1)], &[id(1), id(1)]);
    assert!(!duplicate.passed);
    assert_eq!(duplicate.duplicate_ids, [id(1).to_string()]);
}

#[tokio::test]
async fn real_engine_report_is_repeatable() {
    let first = rb_eval::assertion_precision::run_committed().await.unwrap();
    let second = rb_eval::assertion_precision::run_committed().await.unwrap();
    assert_eq!(first, second);
    assert_eq!(first.fixture_sha256.len(), 64);
    assert_eq!(first.required_cases, 3);
    assert_eq!(first.passed_required_cases, 3);
    assert!(first.precision_gate_passed);
    assert!(!first.all_cases_passed);
}

#[test]
fn committed_labels_are_nonvacuous_and_in_scope() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/assertion_precision.json")).unwrap();
    let mut classes = std::collections::BTreeSet::new();
    assert_eq!(fixture["schema_version"], 2);
    let mut case_ids = std::collections::BTreeSet::new();
    let mut required_case_ids = std::collections::BTreeSet::new();
    for case in fixture["cases"].as_array().unwrap() {
        assert!(case_ids.insert(case["id"].as_str().unwrap()));
        classes.insert(case["failure_class"].as_str().unwrap());
        if case["gate_required"].as_bool().unwrap() {
            required_case_ids.insert(case["id"].as_str().unwrap());
        }
        let memories = case["memories"].as_array().unwrap();
        let ids: std::collections::BTreeSet<_> =
            memories.iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), memories.len());
        let queries = case["queries"].as_array().unwrap();
        assert!(!queries.is_empty());
        for query in queries {
            let limit = query["limit"].as_u64().unwrap();
            assert!((1..=5).contains(&limit));
            let expected = query["expected_ids"].as_array().unwrap();
            assert!(expected.len() <= limit as usize);
            let expected_set: std::collections::BTreeSet<_> =
                expected.iter().map(|x| x.as_str().unwrap()).collect();
            assert_eq!(expected_set.len(), expected.len());
            for id in expected_set {
                let memory = memories
                    .iter()
                    .find(|m| m["id"] == id)
                    .expect("expected ID must actually be seeded");
                assert_eq!(memory["namespace"], case["namespace"]);
                assert!(case["supersedes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|p| p[0] != id));
            }
        }
    }
    assert_eq!(
        required_case_ids,
        ["supersede-chain", "five-slot-budget", "namespace-selection"]
            .into_iter()
            .collect()
    );
    assert_eq!(
        classes,
        [
            "hard_negative",
            "supersede_resurfacing",
            "budget_eviction",
            "namespace_leak",
            "session_noise"
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn offline_runner_reports_exact_sets_and_exits_for_the_gate() {
    let output = Command::new(env!("CARGO_BIN_EXE_assertion-precision"))
        .output()
        .unwrap();
    let error_context = format!(
        "runner must emit JSON, not silently skip; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect(&error_context);
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["judge_used"], false);
    assert!(report["no_judge_statement"]
        .as_str()
        .unwrap()
        .contains("No LLM"));
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(report["required_cases"], 3);
    assert_eq!(report["passed_required_cases"], 3);
    assert_eq!(report["precision_gate_passed"], true);
    assert_eq!(report["all_cases_passed"], false);
    assert_eq!(cases.len(), 6);
    let mut passed = 0;
    for case in cases {
        let queries = case["queries"].as_array().unwrap();
        assert!(!queries.is_empty());
        let mut case_pass = true;
        for query in queries {
            let expected: std::collections::BTreeSet<_> = query["expected_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect();
            let actual: std::collections::BTreeSet<_> = query["returned_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect();
            let exact = expected == actual
                && actual.len() == query["returned_ids"].as_array().unwrap().len();
            let evidence = query["returned_evidence"]
                .as_array()
                .expect("return actual production scores and channel attribution");
            assert_eq!(
                evidence.len(),
                query["returned_ids"].as_array().unwrap().len()
            );
            for (row, id) in evidence
                .iter()
                .zip(query["returned_ids"].as_array().unwrap())
            {
                assert_eq!(&row["id"], id);
                assert!(row["score"].as_f64().unwrap().is_finite());
                assert!(["fts", "vector", "graph"]
                    .iter()
                    .any(|channel| row["channels"][channel] == true));
            }
            assert_eq!(query["passed"], exact);
            case_pass &= exact;
        }
        assert_eq!(case["passed"], case_pass);
        passed += usize::from(case_pass);
    }
    assert_eq!(report["passed_cases"], passed);
    assert_eq!(report["total_cases"], cases.len());
    assert_eq!(
        report["score"].as_f64().unwrap(),
        passed as f64 / cases.len() as f64
    );
    let gate_passed = report["precision_gate_passed"].as_bool().unwrap();
    assert_eq!(output.status.code(), Some(if gate_passed { 0 } else { 1 }));
}
