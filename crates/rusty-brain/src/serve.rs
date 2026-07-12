//! `serve` subcommand: bind the daemon and run until Ctrl-C.
//!
//! This is the process that makes config reach auto-started daemons (C1):
//! every knob is taken from [`rb_config::EffectiveConfig`] (env > user
//! `config.toml` > default), which `serve` resolves from disk itself — so a
//! file-set knob needs no env forwarding at spawn. Only secrets
//! (`VOYAGE_API_KEY`, `RB_ENRICH_API_KEY`) are read from env directly.

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

/// Whether the local backend is requested by the resolved configuration:
/// `backend` equals `local` (case-insensitive) or a local model is named.
/// Values arrive pre-trimmed/non-empty from [`rb_config::EffectiveConfig`]
/// (env > config file).
fn local_requested(backend: Option<&str>, local_model: Option<&str>) -> bool {
    backend.is_some_and(|b| b.eq_ignore_ascii_case("local")) || local_model.is_some()
}

/// Run the daemon until `shutdown` resolves, with every knob taken from the
/// resolved `effective` config (env > user config.toml > default — C1).
/// The embedding provider precedence stays `local > voyage > deterministic`:
/// `embed_backend`/`local_model` select local, the env-only `VOYAGE_API_KEY`
/// secret selects Voyage. `accept_model_change` (the `--accept-model-change`
/// flag, OR-ed with `RB_ACCEPT_MODEL_CHANGE` for auto-start — deliberately
/// NOT a config-file knob: consent must be per-change) opts in to an
/// embedding-model swap; without it a swap fails closed at bind.
pub async fn run_serve(
    effective: rb_config::EffectiveConfig,
    read_pool_size: usize,
    jobs_config_path: Option<PathBuf>,
    accept_model_change: bool,
    http_flag: Option<String>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    let jobs_config = rb_daemon::JobsConfig::load(jobs_config_path.as_deref())?;
    let api_key = std::env::var("VOYAGE_API_KEY").ok();
    let kind = select_provider_kind(
        api_key,
        local_requested(
            effective.embed_backend.as_deref(),
            effective.local_model.as_deref(),
        ),
    );
    let accept = accept_model_change || accept_model_change_from_env();
    let http = resolve_http_listener(http_flag, effective.http)?;
    let config = DaemonConfig {
        socket_path: effective.socket_path,
        db_path: effective.db_path,
        read_pool_size,
        jobs_config,
        // Retention PRD: the resolved (validated, fail-closed) [retention]
        // policy; None = unconfigured, the daemon stays inert.
        retention_policy: effective.retention,
        request_idle_timeout: effective
            .idle_timeout_secs
            .map(std::time::Duration::from_secs),
        enrich: match (effective.enrich_base_url, effective.enrich_model) {
            (Some(base_url), Some(model)) => Some(rb_daemon::EnrichEndpoint { base_url, model }),
            _ => None,
        },
        // Pre-normalized by EffectiveConfig to "linear"/"rrf" (unknown values
        // already warned and dropped there); anything else is the default.
        fusion_mode: match effective.fusion_mode.as_deref() {
            Some("rrf") => rb_daemon::FusionMode::Rrf,
            _ => rb_daemon::FusionMode::Linear,
        },
        // Opt-in loopback HTTP listener (HTTP PRD): flag > config file, both
        // through the same loopback-only fail-closed validator. `None` =
        // zero footprint.
        http,
    };
    run_with_kind(kind, config, effective.local_model, accept, shutdown).await
}

/// Resolve the opt-in HTTP listener: `--http [bind]` flag > `[http]` config
/// file section > off. The flag's bind value goes through the SAME
/// loopback-only validator as the config file
/// ([`rb_config::validate_http_bind`]) and fails closed on anything that is
/// not a literal loopback ip:port — the daemon is never started with a
/// non-loopback listener (HTTP PRD HTTP-2; docs/THREAT_MODEL.md).
fn resolve_http_listener(
    http_flag: Option<String>,
    file_config: Option<rb_config::HttpConfig>,
) -> Result<Option<rb_daemon::HttpListenerConfig>> {
    let bind = match http_flag {
        Some(raw) => Some(rb_config::validate_http_bind(&raw)?),
        None => file_config.map(|h| h.bind),
    };
    Ok(bind.map(|bind| rb_daemon::HttpListenerConfig {
        bind,
        ..Default::default()
    }))
}

/// Construct the concrete provider for `kind` and run the daemon to shutdown.
/// Selecting `Local` without the `local` feature is a fail-closed error.
async fn run_with_kind(
    kind: ProviderKind,
    config: DaemonConfig,
    local_model: Option<String>,
    accept_model_change: bool,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    match kind {
        ProviderKind::Local => {
            #[cfg(feature = "local")]
            {
                let model_name = local_model.unwrap_or_default();
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
                run_with_embedder(config, embedder, accept_model_change, shutdown).await
            }
            #[cfg(not(feature = "local"))]
            {
                let _ = (config, local_model, accept_model_change, shutdown);
                Err(rb_types::Error::Embedding(
                    "local embedding backend requested but this binary was built \
                     without the `local` feature; rebuild with `--features local`"
                        .to_string(),
                ))
            }
        }
        ProviderKind::Voyage => {
            let embedder = VoyageProvider::from_env()?;
            run_with_embedder(config, embedder, accept_model_change, shutdown).await
        }
        ProviderKind::Deterministic => {
            tracing::warn!(
                "VOYAGE_API_KEY not set; using offline DeterministicProvider \
                 (dim {DEFAULT_DIM}). Recall quality is reduced and embeddings \
                 are not portable to a real model."
            );
            let embedder = DeterministicProvider::new(DEFAULT_DIM);
            run_with_embedder(config, embedder, accept_model_change, shutdown).await
        }
    }
}

/// Bind a daemon for a concrete embedder and run it to shutdown.
async fn run_with_embedder<P>(
    config: DaemonConfig,
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
            rb_daemon::accept_model_change(&config.db_path, embedder.dim(), embedder.model_id())?;
        if changed {
            tracing::info!(
                model = embedder.model_id(),
                "accepted embedding model change; corpus marked for re-embed \
                 (run `rusty-brain reembed` until changed=0)"
            );
        }
    }
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

    // C1: local selection is driven by the RESOLVED config (env > file), not
    // by direct env reads — these are pure over the resolved values.
    #[test]
    fn local_requested_matches_backend_local_case_insensitively() {
        assert!(local_requested(Some("local"), None));
        assert!(local_requested(Some("LOCAL"), None));
        assert!(!local_requested(Some("voyage"), None));
        assert!(!local_requested(None, None));
    }

    #[test]
    fn local_requested_when_a_local_model_is_named() {
        assert!(local_requested(None, Some("all-MiniLM-L6-v2")));
        assert!(
            local_requested(Some("voyage"), Some("m")),
            "model implies local"
        );
    }

    // HTTP PRD HTTP-1/HTTP-2: the `--http` flag and the `[http]` config file
    // section resolve into the daemon listener config with flag > file
    // precedence, through the SAME loopback-only validator (fail closed).
    #[test]
    fn http_config_is_off_when_neither_flag_nor_file_enable_it() {
        assert!(resolve_http_listener(None, None).unwrap().is_none());
    }

    #[test]
    fn http_flag_enables_the_listener_with_a_validated_bind() {
        let cfg = resolve_http_listener(Some("127.0.0.1:7777".to_string()), None)
            .unwrap()
            .expect("flag enables http");
        assert_eq!(cfg.bind.to_string(), "127.0.0.1:7777");
    }

    #[test]
    fn http_flag_overrides_the_file_bind() {
        let file = rb_config::HttpConfig {
            bind: "127.0.0.1:1111".parse().unwrap(),
        };
        let cfg = resolve_http_listener(Some("127.0.0.1:2222".to_string()), Some(file))
            .unwrap()
            .expect("http enabled");
        assert_eq!(cfg.bind.port(), 2222, "flag wins over file");
    }

    #[test]
    fn file_config_enables_the_listener_without_a_flag() {
        let file = rb_config::HttpConfig {
            bind: "127.0.0.1:3333".parse().unwrap(),
        };
        let cfg = resolve_http_listener(None, Some(file))
            .unwrap()
            .expect("http enabled");
        assert_eq!(cfg.bind.port(), 3333);
    }

    // SECURITY: a non-loopback flag value fails closed at resolution — the
    // daemon is never started with it.
    #[test]
    fn non_loopback_http_flag_fails_closed() {
        for bad in ["0.0.0.0:80", "192.168.1.4:9999", "example.com:80"] {
            let err = resolve_http_listener(Some(bad.to_string()), None).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("loopback") || msg.contains("ip:port"),
                "{bad}: {msg}"
            );
        }
    }

    #[cfg(not(feature = "local"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn local_selected_without_feature_is_an_embedding_error() {
        // When `local` is requested but the crate was built WITHOUT the
        // feature, run_serve must fail closed with Error::Embedding rather
        // than silently falling back to another provider.
        let dir = tempfile::tempdir().unwrap();
        let config = DaemonConfig {
            socket_path: dir.path().join("rb.sock"),
            db_path: dir.path().join("rb.sqlite"),
            read_pool_size: 4,
            jobs_config: rb_daemon::JobsConfig::default(),
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_daemon::FusionMode::Linear,
            http: None,
        };
        let err = run_with_kind(
            ProviderKind::Local,
            config,
            None,
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
