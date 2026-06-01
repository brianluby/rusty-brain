//! `rb_enrich`: opt-in LLM enrichment and semantic linking for rusty-brain.
//!
//! The default path is the offline, deterministic [`HeuristicEnricher`]. The
//! opt-in [`AnthropicEnricher`] and [`AnthropicLinker`] talk to the Anthropic
//! API and are NEVER required; absence of `ANTHROPIC_API_KEY` degrades to the
//! heuristic path. No live network is touched by the test suite (wiremock only).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod heuristic;

pub use heuristic::HeuristicEnricher;
