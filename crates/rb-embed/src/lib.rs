//! `rb_embed`: embedding providers for rusty-brain.
//!
//! Defines the `EmbeddingProvider` trait, the remote `VoyageProvider`
//! (added in a later task), and a public offline `DeterministicProvider`
//! used as a no-API-key fallback and in tests.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod deterministic;
mod provider;

pub use deterministic::DeterministicProvider;
pub use provider::EmbeddingProvider;
