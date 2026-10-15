//! #76: actual SQLite + engine eligibility-before-cutoff regressions. Hand-authored
//! vectors control order; these prove retrieval mechanics, not semantic-model quality.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::{EmbedKind, EmbeddingProvider};
use rb_engine::{MemoryBackend, MemoryEngine};
use rb_eval::backend::SqliteBackend;
use rb_types::{
    AnchorFilter, AnchorKind, LinkType, MemoryAnchor, MemoryId, MemoryLink, MemoryNote,
    MemoryState, MemoryType, Namespace, RecallFilter, TrustClass, WriteChannel,
};

struct QueryVector {
    fail: bool,
}
#[async_trait::async_trait]
impl EmbeddingProvider for QueryVector {
    fn model_id(&self) -> &str {
        "controlled-query-vector"
    }
    fn dim(&self) -> usize {
        2
    }
    async fn embed(&self, texts: &[String], _kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        if self.fail {
            return Err(rb_types::Error::Embedding("offline test outage".into()));
        }
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
}
fn ns() -> Namespace {
    Namespace::Project("filtered-recall-regression".into())
}
fn note(content: &str) -> MemoryNote {
    MemoryNote::new(ns(), content.into(), MemoryType::Insight, 8)
}
fn engine(fail: bool) -> MemoryEngine<SqliteBackend, QueryVector> {
    MemoryEngine::new(
        SqliteBackend::in_memory(2).unwrap(),
        QueryVector { fail },
        ns(),
    )
}
async fn write(backend: &SqliteBackend, note: MemoryNote, vector: Option<Vec<f32>>) -> MemoryId {
    let id = note.id.clone();
    backend.write(note, vector).await.unwrap();
    id
}
async fn contradiction(backend: &SqliteBackend, a: &MemoryId, b: &MemoryId) {
    backend
        .add_link(MemoryLink {
            source_id: a.clone(),
            target_id: b.clone(),
            link_type: LinkType::Contradicts,
            strength: 1.0,
            reason: "fixture disagreement".into(),
            created_at: chrono::Utc::now(),
        })
        .await
        .unwrap();
}
fn target_filter() -> RecallFilter {
    RecallFilter {
        types: vec![MemoryType::ArchitectureDecision],
        tags: vec!["eligible".into(), "production".into()],
        min_importance: Some(8),
        max_importance: Some(9),
        min_confidence: Some(0.8),
        max_confidence: Some(0.95),
        sources: vec!["cli".into()],
        trust_classes: vec![TrustClass::HumanConfirmed],
        anchors: vec![AnchorFilter {
            kind: AnchorKind::File,
            value: "./src/worker.rs".into(),
        }],
        ..Default::default()
    }
}
fn target() -> MemoryNote {
    let mut n = note(&format!("zebra rollout {}", "background ".repeat(80)));
    n.memory_type = MemoryType::ArchitectureDecision;
    n.tags = vec!["eligible".into(), "production".into()];
    n.origin_source = Some("cli".into());
    n.trust_class = TrustClass::HumanConfirmed;
    n.confidence = 0.9;
    n.anchors = vec![MemoryAnchor::parse_file_spec("src/worker.rs:3-9").unwrap()];
    n
}

#[tokio::test]
async fn filtered_match_beyond_both_channel_cutoffs_survives_and_keyword_degrades() {
    for fail in [false, true] {
        let engine = engine(fail);
        let backend = engine.backend();
        for i in 0..64 {
            write(
                backend,
                note(&format!("zebra zebra zebra rollout rollout {i}")),
                Some(vec![1.0, 0.0]),
            )
            .await;
        }
        let expected = write(backend, target(), Some(vec![0.9, 0.1])).await;
        let unfiltered = RecallFilter::default();
        assert!(!backend
            .keyword(ns(), "zebra rollout".into(), 4, unfiltered.clone())
            .await
            .unwrap()
            .contains(&expected));
        assert!(!backend
            .vector(ns(), vec![1.0, 0.0], 4, unfiltered)
            .await
            .unwrap()
            .iter()
            .any(|(id, _)| id == &expected));
        let filter = target_filter();
        assert_eq!(
            backend
                .keyword(ns(), "zebra rollout".into(), 4, filter.clone())
                .await
                .unwrap(),
            vec![expected.clone()]
        );
        assert_eq!(
            backend
                .vector(ns(), vec![1.0, 0.0], 4, filter.clone())
                .await
                .unwrap()
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>(),
            vec![expected.clone()]
        );
        let result = engine
            .recall_with_status("zebra rollout", 1, &filter)
            .await
            .unwrap();
        assert_eq!(result.degraded, fail);
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].memory.id, expected);
        assert!(result.results[0].channels.fts);
        assert_eq!(result.results[0].channels.vector, !fail);
    }
}

#[tokio::test]
async fn direct_channels_agree_with_full_list_predicates_before_limit() {
    let engine = engine(false);
    let b = engine.backend();
    let good = target();
    let mut variants = Vec::new();
    for dimension in 0..15 {
        let mut n = target();
        match dimension {
            0 => n.memory_type = MemoryType::BugFix,
            1 => {
                n.tags.pop();
            }
            2 => n.importance = 7,
            3 => n.importance = 10,
            4 => n.confidence = 0.7,
            5 => n.origin_source = Some("hook".into()),
            6 => n.trust_class = TrustClass::AgentAttested,
            7 => n.anchors.clear(),
            8 => n.created_at -= chrono::Duration::days(30),
            9 => n.created_at += chrono::Duration::days(30),
            10 => n.origin_channel = Some(WriteChannel::Http),
            11 => n.confidence = 0.0,
            12 => n.namespace = Namespace::Project("foreign".into()),
            13 => n.origin_source = None,
            14 => n.confidence = 1.0,
            _ => unreachable!(),
        }
        variants.push(n);
    }
    let expected = write(b, good, Some(vec![0.9, 0.1])).await;
    for n in variants {
        write(b, n, Some(vec![1.0, 0.0])).await;
    }
    // Also include an eligible note without a vector: native vec0 TEXT-PK IN
    // must not receive its missing ID, nor let it hide the vector-backed match.
    let no_vector = write(b, target(), None).await;
    let mut filter = target_filter();
    filter.since = Some(chrono::Utc::now() - chrono::Duration::days(1));
    filter.until = Some(chrono::Utc::now() + chrono::Duration::days(1));
    let keyword = b
        .keyword(ns(), "zebra rollout".into(), 100, filter.clone())
        .await
        .unwrap();
    let listed = b.list(ns(), filter.clone(), 100).await.unwrap();
    let mut listed_ids: Vec<_> = listed
        .into_iter()
        .filter(|n| !n.is_quarantined() && !n.below_recall_confidence_floor())
        .map(|n| n.id)
        .collect();
    let mut keyword_ids = keyword.clone();
    listed_ids.sort_by_key(ToString::to_string);
    keyword_ids.sort_by_key(ToString::to_string);
    assert_eq!(keyword_ids, listed_ids);
    assert_eq!(keyword.len(), 2);
    assert!(keyword.contains(&expected) && keyword.contains(&no_vector));
    let vector = b.vector(ns(), vec![1.0, 0.0], 1, filter).await.unwrap();
    assert_eq!(vector.len(), 1);
    assert_eq!(vector[0].0, expected);
}

#[tokio::test]
async fn contested_tri_state_selects_eligible_seed_and_archived_has_no_vectors() {
    let engine = engine(true);
    let b = engine.backend();
    for _ in 0..12 {
        write(b, note("zebra zebra zebra"), Some(vec![1.0, 0.0])).await;
    }
    let eligible = write(b, note("zebra rollout"), Some(vec![0.9, 0.1])).await;
    let neighbor = write(b, note("graph neighbor evidence"), None).await;
    let far = write(b, note("conflicting instruction"), None).await;
    contradiction(b, &eligible, &far).await;
    contradiction(b, &neighbor, &far).await;
    b.add_link(MemoryLink {
        source_id: eligible.clone(),
        target_id: neighbor.clone(),
        link_type: LinkType::References,
        strength: 1.0,
        reason: "fixture graph evidence".into(),
        created_at: chrono::Utc::now(),
    })
    .await
    .unwrap();
    let only_contested = RecallFilter {
        contested: Some(true),
        ..Default::default()
    };
    let hits = engine.recall("zebra", 3, &only_contested).await.unwrap();
    assert!(hits
        .iter()
        .any(|r| r.memory.id == neighbor && r.channels.graph));
    assert!(hits.iter().all(|r| r.memory.contested));
    for contested in [None, Some(false), Some(true)] {
        let filter = RecallFilter {
            contested,
            ..Default::default()
        };
        let ids = b
            .keyword(ns(), "zebra".into(), 100, filter.clone())
            .await
            .unwrap();
        assert_eq!(ids.contains(&eligible), contested != Some(false));
        let vectors = b.vector(ns(), vec![1.0, 0.0], 100, filter).await.unwrap();
        assert_eq!(
            vectors.iter().any(|(id, _)| id == &eligible),
            contested != Some(false)
        );
    }
    b.archive_for_eval(&eligible).unwrap();
    let archived = RecallFilter {
        state: MemoryState::Archived,
        contested: Some(false),
        ..Default::default()
    };
    let hits = engine.recall("zebra rollout", 1, &archived).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory.id, eligible);
    assert!(!hits[0].channels.vector && !hits[0].memory.contested);
    assert!(b
        .vector(ns(), vec![1.0, 0.0], 1, archived)
        .await
        .unwrap()
        .is_empty());
    let contested_archive = RecallFilter {
        state: MemoryState::Archived,
        contested: Some(true),
        ..Default::default()
    };
    assert!(engine
        .recall("zebra", 1, &contested_archive)
        .await
        .unwrap()
        .is_empty());
    let all = RecallFilter {
        state: MemoryState::All,
        ..Default::default()
    };
    assert!(b
        .keyword(ns(), "zebra".into(), 100, all.clone())
        .await
        .unwrap()
        .contains(&eligible));
    assert!(!b
        .vector(ns(), vec![1.0, 0.0], 100, all)
        .await
        .unwrap()
        .iter()
        .any(|(id, _)| id == &eligible));
}

#[tokio::test]
async fn filtered_empty_reasons_probe_only_otherwise_eligible_same_state_candidates() {
    use rb_types::AbstainReason::{FilterExcluded, NoCandidates};
    let engine = engine(false);
    let b = engine.backend();
    let mut blocked = note("zebra quarantine");
    blocked.origin_channel = Some(WriteChannel::Http);
    write(b, blocked, Some(vec![1.0, 0.0])).await;
    let mut exhausted = note("zebra exhausted");
    exhausted.confidence = 0.0;
    write(b, exhausted, Some(vec![1.0, 0.0])).await;
    let filter = RecallFilter {
        types: vec![MemoryType::BugFix],
        ..Default::default()
    };
    assert_eq!(
        engine
            .recall_with_status("zebra", 1, &filter)
            .await
            .unwrap()
            .abstained,
        Some(NoCandidates)
    );
    let id = write(b, note("zebra eligible"), Some(vec![1.0, 0.0])).await;
    assert_eq!(
        engine
            .recall_with_status("zebra", 1, &filter)
            .await
            .unwrap()
            .abstained,
        Some(FilterExcluded)
    );
    b.archive_for_eval(&id).unwrap();
    assert_eq!(
        engine
            .recall_with_status("zebra", 1, &filter)
            .await
            .unwrap()
            .abstained,
        Some(NoCandidates)
    );
    let archived_filter = RecallFilter {
        state: MemoryState::Archived,
        ..filter
    };
    assert_eq!(
        engine
            .recall_with_status("zebra", 1, &archived_filter)
            .await
            .unwrap()
            .abstained,
        Some(FilterExcluded)
    );
    let state_only = RecallFilter {
        state: MemoryState::Archived,
        ..Default::default()
    };
    assert_eq!(
        engine.recall("zebra", 1, &state_only).await.unwrap()[0]
            .memory
            .id,
        id
    );
    assert!(engine
        .recall(
            "zebra",
            1,
            &RecallFilter {
                state: MemoryState::Archived,
                contested: Some(true),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .is_empty());
}
