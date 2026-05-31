//! `rb_embed`: embedding providers for rusty-brain.
//!
//! Defines the `EmbeddingProvider` trait, the remote `VoyageProvider`,
//! and a public offline `DeterministicProvider` used as a no-API-key
//! fallback and in tests.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod deterministic;
mod provider;
mod voyage;

pub use deterministic::DeterministicProvider;
pub use provider::EmbeddingProvider;
pub use voyage::VoyageProvider;
