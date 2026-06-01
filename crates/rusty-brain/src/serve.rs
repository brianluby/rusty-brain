//! `serve` subcommand: bind the daemon and run until Ctrl-C.

use rb_daemon::{Daemon, DaemonConfig, SharedEmbedder};
use rb_embed::{DeterministicProvider, EmbeddingProvider, VoyageProvider};
use rb_types::Result;
use std::path::PathBuf;

/// Default embedding dimension for the offline provider and Voyage's default model.
pub const DEFAULT_DIM: usize = 512;

/// Which embedding provider `serve` will use.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ProviderKind {
    Voyage,
    Deterministic,
}

/// Pure selection: Voyage iff a non-empty API key is present, else Deterministic.
pub fn select_provider_kind(api_key: Option<String>) -> ProviderKind {
    match api_key {
        Some(k) if !k.trim().is_empty() => ProviderKind::Voyage,
        _ => ProviderKind::Deterministic,
    }
}

/// Run the daemon at the given paths until `shutdown` resolves.
/// Picks the embedding provider from the environment (`VOYAGE_API_KEY`).
pub async fn run_serve(
    socket_path: PathBuf,
    db_path: PathBuf,
    read_pool_size: usize,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let api_key = std::env::var("VOYAGE_API_KEY").ok();
    match select_provider_kind(api_key) {
        ProviderKind::Voyage => {
            let embedder = VoyageProvider::from_env()?;
            run_with_embedder(socket_path, db_path, read_pool_size, embedder, shutdown).await
        }
        ProviderKind::Deterministic => {
            tracing::warn!(
                "VOYAGE_API_KEY not set; using offline DeterministicProvider \
                 (dim {DEFAULT_DIM}). Recall quality is reduced and embeddings \
                 are not portable to a real model."
            );
            let embedder = DeterministicProvider::new(DEFAULT_DIM);
            run_with_embedder(socket_path, db_path, read_pool_size, embedder, shutdown).await
        }
    }
}

/// Bind a daemon for a concrete embedder and run it to shutdown.
async fn run_with_embedder<P>(
    socket_path: PathBuf,
    db_path: PathBuf,
    read_pool_size: usize,
    embedder: P,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()>
where
    P: EmbeddingProvider + 'static,
{
    let config = DaemonConfig {
        socket_path,
        db_path,
        read_pool_size,
    };
    let daemon = Daemon::bind(config, SharedEmbedder::new(embedder)).await?;
    daemon.run(shutdown).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn selects_voyage_when_key_present() {
        let sel = select_provider_kind(Some("vk-123".to_string()));
        assert_eq!(sel, ProviderKind::Voyage);
    }

    #[test]
    fn selects_deterministic_when_key_absent() {
        let sel = select_provider_kind(None);
        assert_eq!(sel, ProviderKind::Deterministic);
    }

    #[test]
    fn selects_deterministic_when_key_empty() {
        let sel = select_provider_kind(Some(String::new()));
        assert_eq!(sel, ProviderKind::Deterministic);
    }

    #[test]
    fn selects_deterministic_when_key_is_whitespace() {
        let sel = select_provider_kind(Some("   ".to_string()));
        assert_eq!(sel, ProviderKind::Deterministic);
    }
}
