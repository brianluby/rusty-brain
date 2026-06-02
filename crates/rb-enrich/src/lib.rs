//! `rb_enrich`: opt-in LLM enrichment and semantic linking for rusty-brain.
//!
//! The default path is the offline, deterministic [`HeuristicEnricher`]. The
//! opt-in [`OpenAiCompatEnricher`] and [`OpenAiCompatLinker`] use the OpenAI
//! chat-completions protocol, compatible with OpenAI, Azure OpenAI, Ollama,
//! llama.cpp, and any other OpenAI-compatible server. They are NEVER required;
//! absence of `RB_ENRICH_BASE_URL` + `RB_ENRICH_MODEL` degrades to the
//! heuristic path. No live network is touched by the test suite (wiremock only).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod heuristic;
mod linker;
mod openai_compat;

pub use heuristic::HeuristicEnricher;
pub use linker::OpenAiCompatLinker;
pub use openai_compat::OpenAiCompatEnricher;
