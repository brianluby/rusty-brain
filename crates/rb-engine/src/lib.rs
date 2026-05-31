//! `rb_engine`: single-request orchestration for rusty-brain.
//!
//! Generic over a `MemoryBackend` trait and an `EmbeddingProvider`, so the
//! engine stays pure policy (embed plus rank) and testable without a real
//! store. Concrete types are added in subsequent tasks.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
