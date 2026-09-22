//! Assertion-grade, no-judge retrieval precision over fixed local beliefs.
//! Uses the production engine and SQLite bridge; never reconstructs retrieval.
use crate::backend::{SqliteBackend, EVAL_DIM};
use rb_embed::DeterministicProvider;
use rb_engine::{MemoryBackend, MemoryEngine, Provenance, RememberInput};
use rb_types::{LinkType, MemoryId, MemoryLink, MemoryType, Namespace, RecallFilter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const FIXTURE: &str = include_str!("../fixtures/assertion_precision.json");
const REQUIRED_GATE_CASES: [&str; 3] = [
    "supersede-chain",
    "five-slot-budget",
    "namespace-selection",
];

#[derive(Deserialize)]
struct Fixture {
    schema_version: u32,
    clock: chrono::DateTime<chrono::Utc>,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    id: String,
    failure_class: String,
    gate_required: bool,
    rationale: String,
    namespace: String,
    memories: Vec<Belief>,
    supersedes: Vec<[MemoryId; 2]>,
    links: Vec<[MemoryId; 2]>,
    queries: Vec<Query>,
}
#[derive(Deserialize)]
struct Belief {
    id: MemoryId,
    content: String,
    importance: u8,
    namespace: String,
    session_id: Option<String>,
}
#[derive(Deserialize)]
struct Query {
    query: String,
    limit: usize,
    expected_ids: Vec<MemoryId>,
}

/// Unmodified production score and contributing channels for a returned row.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ReturnedEvidence {
    pub id: MemoryId,
    pub score: f32,
    pub channels: rb_types::ChannelHits,
}

/// Exact set comparison, with no fixture-key mapping that could hide unknown IDs.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct IdSetAssertion {
    pub expected_ids: Vec<String>,
    pub returned_ids: Vec<String>,
    pub missing_ids: Vec<String>,
    pub extra_ids: Vec<String>,
    pub duplicate_ids: Vec<String>,
    pub passed: bool,
}

/// Compare exact ID sets. Order is irrelevant; duplicate output is an error.
/// Pure instrument checks can inject extra IDs here without faking capability results.
pub fn assert_exact_ids(expected_ids: &[MemoryId], returned_ids: &[MemoryId]) -> IdSetAssertion {
    let expected: BTreeSet<String> = expected_ids.iter().map(ToString::to_string).collect();
    let returned_ids: Vec<String> = returned_ids.iter().map(ToString::to_string).collect();
    let returned: BTreeSet<String> = returned_ids.iter().cloned().collect();
    let mut seen = BTreeSet::new();
    let duplicate_ids: BTreeSet<String> = returned_ids
        .iter()
        .filter(|id| !seen.insert((*id).clone()))
        .cloned()
        .collect();
    IdSetAssertion {
        passed: expected == returned && duplicate_ids.is_empty(),
        expected_ids: expected.iter().cloned().collect(),
        returned_ids,
        missing_ids: expected.difference(&returned).cloned().collect(),
        extra_ids: returned.difference(&expected).cloned().collect(),
        duplicate_ids: duplicate_ids.into_iter().collect(),
    }
}

/// Exact evidence-set verdict. Returned IDs preserve production rank order.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct QueryReport {
    pub query: String,
    pub limit: usize,
    #[serde(flatten)]
    pub assertion: IdSetAssertion,
    pub returned_evidence: Vec<ReturnedEvidence>,
}
/// A case passes only if every sequential query passes (no averaging away errors).
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CaseReport {
    pub id: String,
    pub failure_class: String,
    pub gate_required: bool,
    pub rationale: String,
    pub queries: Vec<QueryReport>,
    pub passed: bool,
}
/// Capability report, distinct from tests of the measurement instrument.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct PrecisionReport {
    pub schema_version: u32,
    pub judge_used: bool,
    pub no_judge_statement: String,
    pub fixture_sha256: String,
    pub retrieval_path: String,
    pub embedding_provider: String,
    pub limits: Vec<String>,
    pub cases: Vec<CaseReport>,
    pub passed_cases: usize,
    pub total_cases: usize,
    /// Required cases are fixed by `REQUIRED_GATE_CASES`, not inferred from results.
    pub passed_required_cases: usize,
    pub required_cases: usize,
    pub all_cases_passed: bool,
    /// Fraction of fully passing cases, not average hit recall or prose similarity.
    pub score: f64,
    /// True only when the non-empty fixed required set passes exactly.
    pub precision_gate_passed: bool,
}

/// Execute every committed case in a fresh real SQLite store. Sequential queries
/// within one case share the store. Only the clock and fixture identities are pinned;
/// fusion weights, score floor, metadata filter and candidate assembly stay default.
pub async fn run_committed() -> anyhow::Result<PrecisionReport> {
    let fixture: Fixture = serde_json::from_str(FIXTURE)?;
    anyhow::ensure!(
        fixture.schema_version == 2,
        "unsupported assertion fixture schema {}",
        fixture.schema_version
    );
    let case_ids: BTreeSet<&str> = fixture
        .cases
        .iter()
        .map(|case| case.id.as_str())
        .collect();
    anyhow::ensure!(
        case_ids.len() == fixture.cases.len(),
        "assertion fixture case IDs must be unique"
    );
    let required_cases: BTreeSet<&str> = fixture
        .cases
        .iter()
        .filter(|case| case.gate_required)
        .map(|case| case.id.as_str())
        .collect();
    let expected_required_cases: BTreeSet<&str> =
        REQUIRED_GATE_CASES.iter().copied().collect();
    anyhow::ensure!(
        required_cases == expected_required_cases,
        "required assertion cases changed: expected {expected_required_cases:?}, got {required_cases:?}"
    );
    let mut cases = Vec::new();
    for case in fixture.cases {
        let engine = MemoryEngine::new(
            SqliteBackend::in_memory(EVAL_DIM)?,
            DeterministicProvider::new(EVAL_DIM),
            Namespace::parse_db_string(&case.namespace)?,
        )
        .with_fixed_now(fixture.clock);
        for belief in case.memories {
            let (mut note, vector) = engine
                .compose_note(RememberInput {
                    content: belief.content,
                    context: None,
                    memory_type: MemoryType::Insight,
                    importance: belief.importance,
                    keywords: Vec::new(),
                    tags: Vec::new(),
                    related_files: Vec::new(),
                    confidence: Some(1.0),
                    provenance: Provenance::default(),
                    anchors: Vec::new(),
                })
                .await?;
            // Compose uses real heuristic enrichment + production document embeddings.
            // Override identity and selection scope before inserting through SqliteStore.
            // This deliberately omits remember's implicit similarity-link generation:
            // graph topology is authored below, not chosen by nonsemantic vectors.
            note.id = belief.id;
            note.namespace = Namespace::parse_db_string(&belief.namespace)?;
            note.session_id = belief.session_id;
            engine.backend().write(note, vector).await?;
        }
        for [source_id, target_id] in case.links {
            engine
                .backend()
                .add_link(MemoryLink {
                    source_id,
                    target_id,
                    link_type: LinkType::References,
                    strength: 1.0,
                    reason: "authored precision fixture graph edge".into(),
                    created_at: fixture.clock,
                })
                .await?;
        }
        for [old, new] in case.supersedes {
            engine.backend().supersede_for_eval(&old, &new)?;
        }
        let mut queries = Vec::new();
        for query in case.queries {
            let outcome = engine
                .recall_with_status(&query.query, query.limit, &RecallFilter::default())
                .await?;
            anyhow::ensure!(
                !outcome.degraded,
                "unexpected degraded recall in {}",
                case.id
            );
            let returned_ids: Vec<MemoryId> = outcome
                .results
                .iter()
                .map(|r| r.memory.id.clone())
                .collect();
            queries.push(QueryReport {
                query: query.query,
                limit: query.limit,
                assertion: assert_exact_ids(&query.expected_ids, &returned_ids),
                returned_evidence: outcome
                    .results
                    .into_iter()
                    .map(|r| ReturnedEvidence {
                        id: r.memory.id,
                        score: r.score,
                        channels: r.channels,
                    })
                    .collect(),
            });
        }
        let passed = !queries.is_empty() && queries.iter().all(|q| q.assertion.passed);
        cases.push(CaseReport {
            id: case.id,
            failure_class: case.failure_class,
            gate_required: case.gate_required,
            rationale: case.rationale,
            queries,
            passed,
        });
    }
    let passed_cases = cases.iter().filter(|case| case.passed).count();
    let total_cases = cases.len();
    let passed_required_cases = cases
        .iter()
        .filter(|case| case.gate_required && case.passed)
        .count();
    let required_cases = cases.iter().filter(|case| case.gate_required).count();
    Ok(PrecisionReport {
        schema_version: 2,
        judge_used: false,
        no_judge_statement: "No LLM, model judge, prose or file substring scoring, or external service is used. Labels are fixed locally authored memory ID sets; every missing, extra, or duplicate ID fails its query, and every query must pass for its case to pass.".into(),
        fixture_sha256: format!("{:x}", Sha256::digest(FIXTURE.as_bytes())),
        retrieval_path: "MemoryEngine::compose_note -> SqliteBackend/SqliteStore -> MemoryEngine::recall_with_status (FTS5 + sqlite-vec + graph)".into(),
        embedding_provider: format!("DeterministicProvider(dim={EVAL_DIM}); offline, nonsemantic"),
        limits: vec![
            "Authored finite corpus, not semantic generalization or agent task success.".into(),
            "No daemon transport, capture hooks, auto-link generation, or concurrency coverage.".into(),
            "Five-slot retrieval budget only; hook rendering and its 200-character projection are not exercised.".into(),
            "Namespace selection is not an authorization guarantee.".into(),
            "Non-required cases remain scored and visible as capability evidence; they cannot be averaged into or silently weaken the required gate.".into(),
        ],
        cases,
        passed_cases,
        total_cases,
        passed_required_cases,
        required_cases,
        all_cases_passed: total_cases > 0 && passed_cases == total_cases,
        score: if total_cases == 0 {
            0.0
        } else {
            passed_cases as f64 / total_cases as f64
        },
        precision_gate_passed: required_cases > 0 && passed_required_cases == required_cases,
    })
}
