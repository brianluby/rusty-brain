//! `rb_engine`: single-request memory orchestration (policy only).
//!
//! Generic over a `MemoryBackend` (store access) and an
//! `rb_embed::EmbeddingProvider`. P1 enrichment is heuristic only; LLM
//! enrichment and semantic link generation are deferred to P2.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod backend;
mod enrich;

pub use backend::MemoryBackend;
