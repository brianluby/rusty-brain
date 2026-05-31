//! `rb_embed`: pluggable embedding providers for rusty-brain.
//!
//! The `EmbeddingProvider` trait, a Voyage remote impl, and an offline
//! deterministic provider for tests and no-API-key fallback. Concrete types
//! are added in subsequent tasks.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
