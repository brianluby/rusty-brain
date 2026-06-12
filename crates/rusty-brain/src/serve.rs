//! `serve` subcommand: bind the daemon and run until Ctrl-C.

use rb_daemon::{Daemon, DaemonConfig, SharedEmbedder};
use rb_embed::{DeterministicProvider, EmbeddingProvider, VoyageProvider};
use rb_types::Result;
use std::path::PathBuf;

/// Default embedding dimension for the offline provider and Voyage's default model.
pub const DEFAULT_DIM: usize = 512;

/// Which embedding provider `serve` will use.
///
/// `Local` is always present in the enum (so selection logic is uniform), but
/// it can only be *constructed and run* when the crate is built with the
/// `local` feature. Selecting `Local` without the feature is a fail-closed
/// `Error::Embedding` in `run_with_kind` — never a silent fallback.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ProviderKind {
    Local,
    Voyage,
    Deterministic,
}

/// Pure selection with precedence `local > voyage > deterministic`.
/// `local_requested` comes from the environment (see [`run_serve`]); Voyage is
/// chosen iff a non-empty API key is present, otherwise Deterministic.
pub fn select_provider_kind(api_key: Option<String>, local_requested: bool) -> ProviderKind {
    if local_requested {
        return ProviderKind::Local;
    }
    match api_key {
        Some(k) if !k.trim().is_empty() => ProviderKind::Voyage,
        _ => ProviderKind::Deterministic,
    }
}

/// Read the `RB_ACCEPT_MODEL_CHANGE` opt-in from the environment (the
/// auto-start path, where no flag can be passed).
fn accept_model_change_from_env() -> bool {
    env_truthy(std::env::var(rb_config::ACCEPT_MODEL_CHANGE_ENV).ok())
}

/// Pure core for env-flag parsing: truthy = present, non-empty, and not
/// `0`/`false` (case-insensitive). Injected value so tests never mutate
/// process-global env.
fn env_truthy(value: Option<String>) -> bool {
    value
        .map(|v| {
            let v = v.trim();
            !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

/// Read whether the local backend was requested via the environment.
/// True when `RB_EMBED_BACKEND=local` (case-insensitive) or `RB_LOCAL_MODEL`
/// is set to a non-empty value.
fn local_requested_from_env() -> bool {
    let backend = std::env::var("RB_EMBED_BACKEND")
        .ok()
        .map(|v| v.trim().eq_ignore_ascii_case("local"))
        .unwrap_or(false);
    let model_set = std::env::var("RB_LOCAL_MODEL")
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    backend || model_set
}

/// Run the daemon at the given paths until `shutdown` resolves.
/// Picks the embedding provider from the environment (`RB_EMBED_BACKEND` /
/// `RB_LOCAL_MODEL` for local, `VOYAGE_API_KEY` for Voyage).
/// `accept_model_change` (the `--accept-model-change` flag, OR-ed with
/// `RB_ACCEPT_MODEL_CHANGE` for auto-start) opts in to an embedding-model
/// swap; without it a swap fails closed at bind.
pub async fn run_serve(
    socket_path: PathBuf,
    db_path: PathBuf,
    read_pool_size: usize,
    jobs_config_path: Option<PathBuf>,
    accept_model_change: bool,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let jobs_config = rb_daemon::JobsConfig::load(jobs_config_path.as_deref())?;
    let api_key = std::env::var("VOYAGE_API_KEY").ok();
    let kind = select_provider_kind(api_key, local_requested_from_env());
    let accept = accept_model_change || accept_model_change_from_env();
    run_with_kind(
        kind,
        socket_path,
        db_path,
        read_pool_size,
        jobs_config,
        accept,
        shutdown,
    )
    .await
}

/// Construct the concrete provider for `kind` and run the daemon to shutdown.
/// Selecting `Local` without the `local` feature is a fail-closed error.
async fn run_with_kind(
    kind: ProviderKind,
    socket_path: PathBuf,
    db_path: PathBuf,
    read_pool_size: usize,
    jobs_config: rb_daemon::JobsConfig,
    accept_model_change: bool,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    match kind {
        ProviderKind::Local => {
            #[cfg(feature = "local")]
            {
                let model_name = std::env::var("RB_LOCAL_MODEL").unwrap_or_default();
                tracing::info!(
                    "using local ONNX embeddings via fastembed (model '{}'); \
                     weights download at runtime on first use",
                    if model_name.trim().is_empty() {
                        "all-MiniLM-L6-v2"
                    } else {
                        model_name.as_str()
                    }
                );
                let embedder = rb_embed::LocalProvider::load(&model_name)?;
                run_with_embedder(
                    socket_path,
                    db_path,
                    read_pool_size,
                    jobs_config,
                    embedder,
                    accept_model_change,
                    shutdown,
                )
                .await
            }
            #[cfg(not(feature = "local"))]
            {
                let _ = (
                    socket_path,
                    db_path,
                    read_pool_size,
                    jobs_config,
                    accept_model_change,
                    shutdown,
                );
                Err(rb_types::Error::Embedding(
                    "local embedding backend requested but this binary was built \
                     without the `local` feature; rebuild with `--features local`"
                        .to_string(),
                ))
            }
        }
        ProviderKind::Voyage => {
            let embedder = VoyageProvider::from_env()?;
            run_with_embedder(
                socket_path,
                db_path,
                read_pool_size,
                jobs_config,
                embedder,
                accept_model_change,
                shutdown,
            )
            .await
        }
        ProviderKind::Deterministic => {
            tracing::warn!(
                "VOYAGE_API_KEY not set; using offline DeterministicProvider \
                 (dim {DEFAULT_DIM}). Recall quality is reduced and embeddings \
                 are not portable to a real model."
            );
            let embedder = DeterministicProvider::new(DEFAULT_DIM);
            run_with_embedder(
                socket_path,
                db_path,
                read_pool_size,
                jobs_config,
                embedder,
                accept_model_change,
                shutdown,
            )
            .await
        }
    }
}

/// Bind a daemon for a concrete embedder and run it to shutdown.
async fn run_with_embedder<P>(
    socket_path: PathBuf,
    db_path: PathBuf,
    read_pool_size: usize,
    jobs_config: rb_daemon::JobsConfig,
    embedder: P,
    accept_model_change: bool,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()>
where
    P: EmbeddingProvider + 'static,
{
    // Explicit opt-in to an embedding-model swap, BEFORE bind so the daemon's
    // model-verified opens then succeed. A no-op when nothing changed.
    if accept_model_change {
        let changed =
            rb_daemon::accept_model_change(&db_path, embedder.dim(), embedder.model_id())?;
        if changed {
            tracing::info!(
                model = embedder.model_id(),
                "accepted embedding model change; corpus marked for re-embed \
                 (run `rusty-brain reembed` until changed=0)"
            );
        }
    }
    let config = DaemonConfig {
        socket_path,
        db_path,
        read_pool_size,
        jobs_config,
    };
    let daemon = Daemon::bind(config, SharedEmbedder::new(embedder)).await?;
    daemon.run(shutdown).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn env_truthy_accepts_set_values_and_rejects_off_values() {
        for v in ["1", "true", "TRUE", "yes"] {
            assert!(env_truthy(Some(v.to_string())), "{v:?} must opt in");
        }
        for v in ["", "  ", "0", "false", "FALSE"] {
            assert!(!env_truthy(Some(v.to_string())), "{v:?} must not opt in");
        }
        assert!(!env_truthy(None), "absent var must not opt in");
    }

    #[test]
    fn selects_voyage_when_key_present_and_local_not_requested() {
        let sel = select_provider_kind(Some("vk-123".to_string()), false);
        assert_eq!(sel, ProviderKind::Voyage);
    }

    #[test]
    fn selects_deterministic_when_key_absent_and_local_not_requested() {
        let sel = select_provider_kind(None, false);
        assert_eq!(sel, ProviderKind::Deterministic);
    }

    #[test]
    fn selects_deterministic_when_key_empty_and_local_not_requested() {
        let sel = select_provider_kind(Some(String::new()), false);
        assert_eq!(sel, ProviderKind::Deterministic);
    }

    #[test]
    fn selects_deterministic_when_key_is_whitespace_and_local_not_requested() {
        let sel = select_provider_kind(Some("   ".to_string()), false);
        assert_eq!(sel, ProviderKind::Deterministic);
    }

    #[test]
    fn local_requested_takes_precedence_over_voyage() {
        // Precedence is local > voyage > deterministic: even with a key,
        // an explicit local request wins.
        let sel = select_provider_kind(Some("vk-123".to_string()), true);
        assert_eq!(sel, ProviderKind::Local);
    }

    #[test]
    fn local_requested_takes_precedence_over_deterministic() {
        let sel = select_provider_kind(None, true);
        assert_eq!(sel, ProviderKind::Local);
    }

    #[cfg(not(feature = "local"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn local_selected_without_feature_is_an_embedding_error() {
        // When `local` is requested but the crate was built WITHOUT the
        // feature, run_serve must fail closed with Error::Embedding rather
        // than silently falling back to another provider.
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("rb.sock");
        let db = dir.path().join("rb.sqlite");
        let err = run_with_kind(
            ProviderKind::Local,
            socket,
            db,
            4,
            rb_daemon::JobsConfig::default(),
            false,
            std::future::ready(()),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Embedding(_)),
            "expected Error::Embedding when local feature is absent, got {err:?}"
        );
    }
}
