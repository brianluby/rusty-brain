//! `rb-eval` — offline, deterministic retrieval-quality regression harness.
//!
//! This is a **dev/measurement crate**, deliberately excluded from the
//! `rusty-brain` binary's dependency closure. It ingests a committed fixture
//! corpus through `rb-engine` with [`rb_embed::DeterministicProvider`], runs
//! golden queries through `engine.recall`, computes ranking metrics, and asserts
//! each metric meets a committed baseline.
//!
//! ## What this measures (and what it does not)
//!
//! With deterministic (non-semantic) vectors, this harness guards **ranking
//! determinism, regression detection, and relative-ordering invariants** — i.e.
//! "did a change reorder results versus the committed baseline?". It does **not**
//! measure absolute semantic quality; that is the job of the optional, manual
//! `#[ignore]` real-model mode (Voyage / `local`). See `README.md`.
//!
//! ## Evolution jobs are OFF in every eval run (W1.9)
//!
//! `rb-eval` deliberately does not depend on `rb-daemon`, where the background
//! evolution jobs (importance recalibration, link decay, consolidation) live,
//! so no eval run can mutate importance/links/duplicates behind retrieval's
//! back — baselines measure retrieval alone. The harness test
//! `eval_runs_with_evolution_jobs_off` pins this behaviorally.
//!
//! As a test-only crate it opts out of the workspace's
//! `unwrap_used`/`expect_used` denials at the crate root; no panics leak into any
//! shipped crate because nothing shipped depends on `rb-eval`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod backend;
pub mod corpus;
pub mod metrics;
pub mod replay;
pub mod runner;
pub mod semantic_gate;

pub use corpus::{load_committed_corpus, load_committed_holdout_queries, Corpus, CorpusError};
pub use replay::{EmbeddingFixture, RecordedVector, RecordingProvider, ReplayProvider};
pub use runner::{
    build_engine_with, build_engine_with_at, check_against_baselines, compare_modes,
    compare_modes_committed, run_corpus, run_corpus_detailed, run_corpus_detailed_mode,
    run_corpus_with, run_eval, run_eval_mode, Baselines, ChannelContribution, DetailedRun,
    EvalReport, ModeComparison, QueryChannelHits, QueryDetail,
};
pub use semantic_gate::{
    check_semantic_floors, load_semantic_gate, QualityFloors, SemanticGateInputs,
    SemanticGateManifest,
};

// Re-export so harness callers can name the fusion mode without depending on
// `rb-search` directly.
pub use rb_search::FusionMode;
