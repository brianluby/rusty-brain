//! `rb_engine`: single-request memory orchestration (policy only).
//!
//! Generic over a [`MemoryBackend`] (store access) and an
//! [`rb_embed::EmbeddingProvider`]. Semantic link generation is handled by the
//! built-in `SimilarityLinker`. LLM enrichment is opt-in and OpenAI-compatible
//! (activated via `RB_ENRICH_BASE_URL` + `RB_ENRICH_MODEL` in `rb-enrich`).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod backend;
mod embed_input;
mod engine;
mod enrich;
mod enricher;
mod linker;
#[cfg(test)]
mod test_support;

pub use backend::MemoryBackend;
pub use embed_input::{embedding_input, EMBEDDING_INPUT_VERSION};
pub use engine::{MemoryEngine, Provenance, RecallOutcome, RememberInput};
pub use enricher::{Enricher, Enrichment};
pub use linker::{Linker, SimilarityLinker};
// Re-exported so the daemon can plumb the configured fusion strategy (W2.2)
// into `MemoryEngine::with_fusion_mode` without a direct rb-search dependency.
pub use rb_search::FusionMode;
