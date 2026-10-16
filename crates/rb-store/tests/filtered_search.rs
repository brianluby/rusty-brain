//! #76: real vec0 native prefilter and unfiltered exceptional-path checks.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use rb_store::{SqliteStore, Store};
use rb_types::{MemoryNote, MemoryType, Namespace, RecallFilter, WriteChannel};

#[test]
fn default_knn_safety_fallback_reaches_past_the_internal_overfetch_window() {
    let store = SqliteStore::open_in_memory(2).unwrap();
    let ns = Namespace::Project("eligibility".into());
    for i in 0..96 {
        let mut n = MemoryNote::new(
            ns.clone(),
            format!("zebra rollout {i}"),
            MemoryType::Insight,
            8,
        );
        match i % 3 {
            0 => n.origin_channel = Some(WriteChannel::Http),
            1 => n.origin_source = Some("http".into()),
            _ => n.confidence = 0.0,
        }
        store.insert_memory(&n, Some(&[1.0, 0.0])).unwrap();
    }
    let mut eligible = MemoryNote::new(
        ns.clone(),
        "zebra rollout eligible".into(),
        MemoryType::Insight,
        8,
    );
    // An explicit non-HTTP channel overrides a legacy HTTP source, exactly as
    // MemoryNote::is_quarantined specifies.
    eligible.origin_source = Some("http".into());
    eligible.origin_channel = Some(WriteChannel::Cli);
    eligible.confidence = rb_types::RECALL_MIN_ELIGIBLE_CONFIDENCE;
    store.insert_memory(&eligible, Some(&[0.9, 0.1])).unwrap();
    let mut foreign = eligible.clone();
    foreign.id = rb_types::MemoryId::new();
    foreign.namespace = Namespace::Project("foreign".into());
    store.insert_memory(&foreign, Some(&[1.0, 0.0])).unwrap();
    assert_eq!(
        store.keyword_search(&ns, "zebra rollout", 1).unwrap(),
        vec![eligible.id.clone()]
    );
    let vector = store
        .vector_search_filtered(&ns, &[1.0, 0.0], 1, &RecallFilter::default())
        .unwrap();
    assert_eq!(vector.len(), 1);
    assert_eq!(vector[0].0, eligible.id);
    assert!(vector[0].1 > 0.0 && vector[0].1 < 0.1);
}

#[test]
fn filtered_vector_empty_sets_and_invalid_dimensions_are_not_silent_fallbacks() {
    let store = SqliteStore::open_in_memory(2).unwrap();
    let ns = Namespace::Project("eligibility".into());
    let filter = RecallFilter {
        tags: vec!["absent".into()],
        ..Default::default()
    };
    assert!(store
        .vector_search_filtered(&ns, &[1.0, 0.0], 4, &filter)
        .unwrap()
        .is_empty());
    assert!(matches!(
        store.vector_search_filtered(&ns, &[1.0], 4, &filter),
        Err(rb_types::Error::DimensionMismatch { .. })
    ));
    assert!(store
        .vector_search_filtered(&ns, &[1.0, 0.0], 0, &filter)
        .unwrap()
        .is_empty());
}

#[test]
fn context_eligibility_and_uuid_ties_resolve_before_the_sql_cutoff() {
    let store = SqliteStore::open_in_memory(2).unwrap();
    let ns = Namespace::Project("stable-context".into());
    let time = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .unwrap()
        .to_utc();
    let mut expected = Vec::new();
    for i in (0..60).rev() {
        let mut decision = MemoryNote::new(
            ns.clone(),
            format!("decision {i}"),
            MemoryType::ArchitectureDecision,
            8,
        );
        decision.id = format!("00000000-0000-4000-8000-{i:012}").parse().unwrap();
        decision.created_at = time;
        if i == 0 {
            decision.confidence = rb_types::RECALL_MIN_ELIGIBLE_CONFIDENCE;
        }
        if i < 50 {
            expected.push(decision.id.clone());
        }
        store.insert_memory(&decision, None).unwrap();
        let mut activity =
            MemoryNote::new(ns.clone(), format!("activity {i}"), MemoryType::Insight, 9);
        activity.created_at = time + chrono::Duration::days(1);
        store.insert_memory(&activity, None).unwrap();
        let mut poisoned =
            MemoryNote::new(ns.clone(), format!("poison {i}"), MemoryType::Constraint, 9);
        poisoned.origin_channel = Some(WriteChannel::Http);
        poisoned.created_at = activity.created_at;
        store.insert_memory(&poisoned, None).unwrap();
    }
    let mut exhausted = MemoryNote::new(
        ns.clone(),
        "confidence exhausted".into(),
        MemoryType::Constraint,
        10,
    );
    exhausted.confidence = 0.0;
    let exhausted_id = exhausted.id.clone();
    store.insert_memory(&exhausted, None).unwrap();
    let filter = RecallFilter {
        types: vec![
            MemoryType::Constraint,
            MemoryType::ArchitectureDecision,
            MemoryType::Preference,
        ],
        recall_eligible: true,
        ..Default::default()
    };
    expected.reverse();
    let selected = store.list_filtered(&ns, &filter, 50).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|note| note.id.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(store.list_filtered(&ns, &filter, 50).unwrap(), selected);
    let audit = store
        .list_filtered(&ns, &RecallFilter::default(), 200)
        .unwrap();
    assert!(audit.iter().any(MemoryNote::is_quarantined));
    assert!(audit.iter().any(|note| note.id == exhausted_id));
}
