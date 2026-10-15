//! Bounded #85 coding evidence probes; retrieval coverage is not answer quality.
use crate::assertion_precision::{assert_exact_ids, IdSetAssertion, ReturnedEvidence};
use crate::backend::SqliteBackend;
use crate::replay::{text_sha256, EmbeddingFixture, ReplayProvider};
use rb_embed::{EmbedKind, EmbeddingProvider};
use rb_engine::{MemoryBackend, MemoryEngine, Provenance, RememberInput};
use rb_types::{
    LinkType, MemoryHistory, MemoryId, MemoryLink, MemoryType, Namespace, RecallFilter,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

const SOURCE: &str = include_str!("../fixtures/coding_memory.json");
pub const VECTOR_PATH: &str = "fixtures/embeddings/coding-memory-all-MiniLM-L6-v2.json";

#[derive(Deserialize)]
struct Fixture {
    schema_version: u32,
    clock: chrono::DateTime<chrono::Utc>,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    id: String,
    category: String,
    rationale: String,
    memories: Vec<Belief>,
    #[serde(default)]
    links: Vec<Link>,
    #[serde(default)]
    supersedes: Vec<[MemoryId; 2]>,
    #[serde(default)]
    archives: Vec<MemoryId>,
    queries: Vec<Query>,
}
#[derive(Deserialize)]
struct Belief {
    id: MemoryId,
    content: String,
    #[serde(default = "indexed")]
    indexed: bool,
}
fn indexed() -> bool {
    true
}
#[derive(Deserialize)]
struct Link {
    source: MemoryId,
    target: MemoryId,
    kind: LinkType,
}
#[derive(Deserialize)]
struct Query {
    query: String,
    limit: usize,
    expected_ids: Vec<MemoryId>,
    forbidden_ids: Vec<MemoryId>,
    #[serde(default)]
    expect_abstention: bool,
    #[serde(default)]
    history_target: Option<MemoryId>,
    #[serde(default)]
    expected_current_ids: Vec<MemoryId>,
    #[serde(default)]
    expected_archived_ids: Vec<MemoryId>,
    #[serde(default)]
    expected_contested_ids: Vec<MemoryId>,
    #[serde(default)]
    graph_only: Option<GraphOnly>,
}
#[derive(Deserialize)]
struct GraphOnly {
    seed: MemoryId,
    target: MemoryId,
    hops: u8,
}

/// Production direct candidate sets and independently checked authored topology.
#[derive(Debug, Serialize)]
pub struct CandidateEvidence {
    pub keyword_ids: Vec<MemoryId>,
    pub vector_ids: Vec<MemoryId>,
    pub graph_only_target: Option<MemoryId>,
    pub authored_hops: Option<u8>,
    pub topology_passed: bool,
}
/// Exact retrieval verdict; failures are retained even for unsupported capabilities.
#[derive(Debug, Serialize)]
pub struct QueryReport {
    pub query: String,
    pub surface: &'static str,
    pub limit: usize,
    pub assertion: IdSetAssertion,
    pub forbidden_ids: Vec<MemoryId>,
    pub forbidden_returned_ids: Vec<MemoryId>,
    pub expect_abstention: bool,
    pub expected_current_ids: Vec<MemoryId>,
    pub expected_archived_ids: Vec<MemoryId>,
    pub expected_contested_ids: Vec<MemoryId>,
    pub candidates: Option<CandidateEvidence>,
    pub returned_evidence: Vec<ReturnedEvidence>,
    pub history: Option<MemoryHistory>,
    pub abstain_reason: Option<rb_types::AbstainReason>,
    pub failures: Vec<String>,
    pub passed: bool,
}
#[derive(Debug, Serialize)]
pub struct CaseReport {
    pub id: String,
    pub category: String,
    pub rationale: String,
    pub queries: Vec<QueryReport>,
    pub passed: bool,
}
#[derive(Debug, Serialize, Default)]
pub struct CategoryReport {
    pub total_cases: usize,
    pub passed_cases: usize,
}
/// New development-only semantic baseline, separate from frozen semantic gates.
#[derive(Debug, Serialize)]
pub struct CodingReport {
    pub schema_version: u32,
    pub fixture_sha256: String,
    pub vectors_sha256: Option<String>,
    pub model_id: String,
    pub answer_quality: &'static str,
    pub limits: Vec<&'static str>,
    pub categories: BTreeMap<String, CategoryReport>,
    pub cases: Vec<CaseReport>,
}

fn fixture() -> anyhow::Result<Fixture> {
    let fixture: Fixture = serde_json::from_str(SOURCE)?;
    anyhow::ensure!(
        fixture.schema_version == 1,
        "unsupported coding fixture schema"
    );
    anyhow::ensure!(
        fixture.cases.len() == 6,
        "coding fixture must retain all six cases"
    );
    let mut case_ids = HashSet::new();
    let mut memory_ids = HashSet::new();
    for case in &fixture.cases {
        anyhow::ensure!(case_ids.insert(&case.id), "duplicate coding case ID");
        let ids: HashSet<_> = case.memories.iter().map(|m| &m.id).collect();
        anyhow::ensure!(ids.len() == case.memories.len(), "duplicate belief ID");
        for id in &ids {
            anyhow::ensure!(memory_ids.insert(*id), "belief ID reused across cases");
        }
        for link in &case.links {
            anyhow::ensure!(
                ids.contains(&link.source) && ids.contains(&link.target),
                "unknown link endpoint"
            );
        }
        for pair in &case.supersedes {
            anyhow::ensure!(
                pair[0] != pair[1] && pair.iter().all(|id| ids.contains(id)),
                "invalid supersede pair"
            );
        }
        anyhow::ensure!(
            case.archives.iter().all(|id| ids.contains(id)),
            "unknown archived ID"
        );
        anyhow::ensure!(!case.queries.is_empty(), "empty coding case");
        for query in &case.queries {
            anyhow::ensure!(
                (1..=5).contains(&query.limit),
                "coding query exceeds bounded output budget"
            );
            for labels in [
                &query.expected_ids,
                &query.forbidden_ids,
                &query.expected_current_ids,
                &query.expected_archived_ids,
                &query.expected_contested_ids,
            ] {
                anyhow::ensure!(
                    labels.iter().all(|id| ids.contains(id)),
                    "unknown evidence label"
                );
                anyhow::ensure!(
                    labels.iter().collect::<HashSet<_>>().len() == labels.len(),
                    "duplicate evidence label"
                );
            }
            anyhow::ensure!(
                query
                    .expected_ids
                    .iter()
                    .all(|id| !query.forbidden_ids.contains(id)),
                "expected evidence is forbidden"
            );
            anyhow::ensure!(
                query
                    .expected_current_ids
                    .iter()
                    .chain(&query.expected_archived_ids)
                    .chain(&query.expected_contested_ids)
                    .all(|id| query.expected_ids.contains(id)),
                "state label is not expected evidence"
            );
            anyhow::ensure!(
                query
                    .expected_current_ids
                    .iter()
                    .all(|id| !query.expected_archived_ids.contains(id)),
                "current evidence is archived"
            );
            anyhow::ensure!(
                !query.expect_abstention || query.expected_ids.is_empty(),
                "abstention expects evidence"
            );
            if let Some(target) = &query.history_target {
                anyhow::ensure!(ids.contains(target), "unknown history target");
                anyhow::ensure!(
                    !query.expect_abstention
                        && query.graph_only.is_none()
                        && query.expected_contested_ids.is_empty(),
                    "invalid history-only expectations"
                );
            }
            if let Some(graph) = &query.graph_only {
                anyhow::ensure!(
                    ids.contains(&graph.seed)
                        && ids.contains(&graph.target)
                        && (1..=2).contains(&graph.hops),
                    "invalid graph label"
                );
            }
        }
    }
    Ok(fixture)
}

fn input(belief: &Belief) -> RememberInput {
    RememberInput {
        content: belief.content.clone(),
        context: None,
        memory_type: MemoryType::Insight,
        importance: 7,
        keywords: Vec::new(),
        tags: Vec::new(),
        related_files: Vec::new(),
        confidence: Some(1.0),
        provenance: Provenance::default(),
        anchors: Vec::new(),
        trust_class: None,
    }
}

/// Record only #85's new document/query inputs; no frozen corpus or holdout is read.
pub async fn record_inputs<P: EmbeddingProvider>(
    engine: &MemoryEngine<SqliteBackend, P>,
) -> anyhow::Result<()> {
    for case in fixture()?.cases {
        for belief in case.memories {
            let (_, vector) = engine.compose_note(input(&belief)).await?;
            anyhow::ensure!(vector.is_some(), "provider omitted coding document vector");
        }
        for query in case.queries {
            if query.history_target.is_none() {
                engine
                    .embedder()
                    .embed(&[query.query], EmbedKind::Query)
                    .await?;
            }
        }
    }
    Ok(())
}

/// Require real model replay with exact input kinds; missing vectors never degrade silently.
pub async fn run_replay(raw: &str) -> anyhow::Result<CodingReport> {
    let vectors: EmbeddingFixture = serde_json::from_str(raw)?;
    anyhow::ensure!(
        vectors.model_id == "all-MiniLM-L6-v2" && vectors.dim == 384,
        "coding baseline requires the recorded local all-MiniLM-L6-v2 model"
    );
    let mut report =
        run_with(|| ReplayProvider::from_fixture_strict(&vectors).map_err(anyhow::Error::msg))
            .await?;
    report.vectors_sha256 = Some(text_sha256(raw));
    Ok(report)
}

/// Existing production engine/store path; alternate providers are instrument tests only.
pub async fn run_with<P: EmbeddingProvider>(
    mut provider: impl FnMut() -> anyhow::Result<P>,
) -> anyhow::Result<CodingReport> {
    let fixture = fixture()?;
    let mut cases = Vec::new();
    let mut model_id = String::new();
    for case in fixture.cases {
        let embedder = provider()?;
        model_id = embedder.model_id().to_string();
        let ns = Namespace::Project(format!("rb-eval-coding-{}", case.id));
        let engine = MemoryEngine::new(
            SqliteBackend::in_memory(embedder.dim())?,
            embedder,
            ns.clone(),
        )
        .with_fixed_now(fixture.clock);
        for belief in &case.memories {
            let (mut note, vector) = engine.compose_note(input(belief)).await?;
            anyhow::ensure!(vector.is_some(), "provider omitted coding document vector");
            note.id = belief.id.clone();
            // Controlled graph fixtures deliberately omit the stored vector, not recording.
            engine
                .backend()
                .write(note, if belief.indexed { vector } else { None })
                .await?;
        }
        for link in &case.links {
            engine
                .backend()
                .add_link(MemoryLink {
                    source_id: link.source.clone(),
                    target_id: link.target.clone(),
                    link_type: link.kind,
                    strength: 1.0,
                    reason: "authored coding evidence fixture".into(),
                    created_at: fixture.clock,
                })
                .await?;
        }
        for [old, new] in &case.supersedes {
            engine.backend().supersede_for_eval(old, new)?;
        }
        for id in &case.archives {
            engine.backend().archive_for_eval(id)?;
        }
        let mut queries = Vec::new();
        for query in case.queries {
            let mut failures = Vec::new();
            let mut history = None;
            let mut candidates = None;
            let mut returned_evidence = Vec::new();
            let mut abstain_reason = None;
            let surface;
            let returned_ids;
            if let Some(target) = &query.history_target {
                surface = "SqliteStore::memory_history";
                let timeline = engine.backend().history_for_eval(&ns, target)?;
                returned_ids = timeline
                    .chain
                    .iter()
                    .map(|e| e.id.clone())
                    .collect::<Vec<_>>();
                let current = timeline
                    .chain
                    .iter()
                    .filter(|e| e.current)
                    .map(|e| e.id.clone())
                    .collect::<Vec<_>>();
                let archived = timeline
                    .chain
                    .iter()
                    .filter(|e| e.archived)
                    .map(|e| e.id.clone())
                    .collect::<Vec<_>>();
                if !assert_exact_ids(&query.expected_current_ids, &current).passed {
                    failures.push("history_current_flags".into());
                }
                if !assert_exact_ids(&query.expected_archived_ids, &archived).passed {
                    failures.push("history_archived_flags".into());
                }
                if timeline.truncated {
                    failures.push("history_truncated".into());
                }
                history = Some(timeline);
            } else {
                surface = "MemoryEngine::recall_with_status";
                let filter = RecallFilter::default();
                let bound = query.limit.saturating_mul(4).max(query.limit);
                let keyword_ids = engine
                    .backend()
                    .keyword(ns.clone(), query.query.clone(), bound, filter.clone())
                    .await?;
                let vector = engine
                    .embedder()
                    .embed(std::slice::from_ref(&query.query), EmbedKind::Query)
                    .await?;
                anyhow::ensure!(vector.len() == 1, "provider omitted coding query vector");
                let vector_ids = engine
                    .backend()
                    .vector(
                        ns.clone(),
                        vector.into_iter().next().expect("one checked vector"),
                        bound,
                        filter,
                    )
                    .await?
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>();
                let mut topology_passed = true;
                if let Some(graph) = &query.graph_only {
                    let neighbors = engine
                        .backend()
                        .graph(ns.clone(), graph.seed.clone(), graph.hops)
                        .await?;
                    let shorter = engine
                        .backend()
                        .graph(ns.clone(), graph.seed.clone(), graph.hops - 1)
                        .await?;
                    topology_passed = keyword_ids.first() == Some(&graph.seed)
                        && !keyword_ids.contains(&graph.target)
                        && !vector_ids.contains(&graph.target)
                        && neighbors.contains(&(graph.target.clone(), graph.hops))
                        && !shorter.iter().any(|(id, _)| id == &graph.target);
                    if !topology_passed {
                        failures.push("graph_topology_or_direct_leakage".into());
                    }
                }
                candidates = Some(CandidateEvidence {
                    keyword_ids,
                    vector_ids,
                    topology_passed,
                    graph_only_target: query.graph_only.as_ref().map(|g| g.target.clone()),
                    authored_hops: query.graph_only.as_ref().map(|g| g.hops),
                });
                let outcome = engine
                    .recall_with_status(&query.query, query.limit, &RecallFilter::default())
                    .await?;
                anyhow::ensure!(
                    !outcome.degraded,
                    "coding semantic baseline degraded in {}",
                    case.id
                );
                returned_ids = outcome
                    .results
                    .iter()
                    .map(|r| r.memory.id.clone())
                    .collect::<Vec<_>>();
                let contested = outcome
                    .results
                    .iter()
                    .filter(|r| r.memory.contested)
                    .map(|r| r.memory.id.clone())
                    .collect::<Vec<_>>();
                if !assert_exact_ids(&query.expected_contested_ids, &contested).passed {
                    failures.push("contested_annotation".into());
                }
                if query.expect_abstention
                    && (outcome.abstained.is_none() || !returned_ids.is_empty())
                {
                    failures.push("abstention_contract".into());
                }
                abstain_reason = outcome.abstained;
                returned_evidence = outcome
                    .results
                    .into_iter()
                    .map(|r| ReturnedEvidence {
                        id: r.memory.id,
                        score: r.score,
                        channels: r.channels,
                    })
                    .collect();
                if let Some(graph) = &query.graph_only {
                    if graph.hops == 1
                        && returned_evidence
                            .iter()
                            .find(|r| r.id == graph.target)
                            .is_none_or(|r| {
                                !r.channels.graph || r.channels.fts || r.channels.vector
                            })
                    {
                        failures.push("graph_only_channel_annotation".into());
                    }
                }
            }
            let assertion = assert_exact_ids(&query.expected_ids, &returned_ids);
            if !assertion.missing_ids.is_empty() {
                failures.push("missing_evidence".into());
            }
            if !assertion.extra_ids.is_empty() {
                failures.push("extra_evidence".into());
            }
            if !assertion.duplicate_ids.is_empty() {
                failures.push("duplicate_evidence".into());
            }
            let forbidden_returned_ids = returned_ids
                .iter()
                .filter(|id| query.forbidden_ids.contains(id))
                .cloned()
                .collect::<Vec<_>>();
            if !forbidden_returned_ids.is_empty() {
                failures.push("forbidden_evidence".into());
            }
            queries.push(QueryReport {
                query: query.query,
                surface,
                limit: query.limit,
                assertion,
                forbidden_ids: query.forbidden_ids,
                forbidden_returned_ids,
                expect_abstention: query.expect_abstention,
                expected_current_ids: query.expected_current_ids,
                expected_archived_ids: query.expected_archived_ids,
                expected_contested_ids: query.expected_contested_ids,
                candidates,
                returned_evidence,
                history,
                abstain_reason,
                passed: failures.is_empty(),
                failures,
            });
        }
        cases.push(CaseReport {
            id: case.id,
            category: case.category,
            rationale: case.rationale,
            passed: queries.iter().all(|q| q.passed),
            queries,
        });
    }
    let mut categories: BTreeMap<String, CategoryReport> = BTreeMap::new();
    for case in &cases {
        let category = categories.entry(case.category.clone()).or_default();
        category.total_cases += 1;
        category.passed_cases += usize::from(case.passed);
    }
    Ok(CodingReport { schema_version: 1, fixture_sha256: text_sha256(SOURCE), vectors_sha256: None,
        model_id, answer_quality: "not_evaluated: no generated answer or model judge",
        limits: vec!["Six authored development cases; no held-out tuning, external benchmark or production-scale claim.",
            "Controlled graph-only cases deliberately omit stored vectors; not natural semantic cutoff evidence.",
            "Two-hop evidence remains unsupported by production recall and its missing-ID assertion is not weakened.",
            "History is the retained supersede timeline, not point-in-time reconstruction.",
            "No hooks, daemon transport, answer generation, or background consolidation is exercised."],
        categories, cases })
}
