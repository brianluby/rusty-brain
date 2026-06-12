//! `rb_embed`: embedding providers for rusty-brain.
//!
//! Defines the `EmbeddingProvider` trait, the remote `VoyageProvider`,
//! a public offline `DeterministicProvider` used as a no-API-key fallback
//! and in tests, and (under the `local` feature) the `LocalProvider` for
//! offline ONNX embeddings via fastembed.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod deterministic;
#[cfg(feature = "local")]
mod local;
mod provider;
mod voyage;

pub use deterministic::DeterministicProvider;
#[cfg(feature = "local")]
pub use local::LocalProvider;
pub use provider::{EmbedKind, EmbeddingProvider};
pub use voyage::VoyageProvider;

#[cfg(all(test, feature = "local"))]
mod local_export_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use crate::{EmbeddingProvider, LocalProvider};

    #[test]
    fn local_provider_is_publicly_re_exported() {
        // Constructing via the crate-root path proves the `pub use` wiring.
        let p = LocalProvider::without_model("all-MiniLM-L6-v2", 384);
        assert_eq!(p.dim(), 384);
        assert_eq!(p.model_id(), "all-MiniLM-L6-v2");
    }
}
