//! The user-level config file: `~/.config/rusty-brain/config.toml` (C1/W0.2).
//!
//! Daemon knobs belong HERE, not in the auto-start env allowlist: every binary
//! (foreground daemon, auto-started daemon, CLI, hooks) re-reads this file from
//! disk itself, so a knob set here reaches an auto-started daemon with NO env
//! forwarding at all. That retires the F20 bug *class* — "config reaching a
//! foreground daemon but not auto-started ones" — instead of patching
//! instances by growing [`crate::FORWARD_ENV`].
//!
//! Precedence (per knob): CLI flag > env var > this file > built-in default.
//! Env vars keep working for compatibility (see [`crate::LEGACY_KNOB_ENV`]),
//! but new knobs land in this file only.
//!
//! Two hard boundaries, by design:
//!
//! - **Secrets are environment-only.** `VOYAGE_API_KEY` / `RB_ENRICH_API_KEY`
//!   have no config-file equivalent; credentials never live in a plaintext
//!   config file. They stay on [`crate::FORWARD_ENV`].
//! - **The repo-committed `.rusty-brain.toml` is NAMESPACE-IDENTITY-ONLY and
//!   is never read here.** It is untrusted repo content: cloning a repository
//!   must never be able to redirect sockets, databases, or embedding backends
//!   (see [`crate::namespace`]). Only the user-owned file under
//!   `XDG_CONFIG_HOME` is a configuration source.
//!
//! Failure policy: a *missing* file is fine (all defaults); *malformed* TOML or
//! a wrong-typed value fails closed with a message naming the file path;
//! *unknown keys* warn and are ignored (forward compatibility).

use std::path::{Path, PathBuf};

use rb_types::{Error, Result};
use serde::Deserialize;

/// File name of the user config file, under `<config dir>/rusty-brain/`.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The parsed user config file. Every field is optional: an absent key means
/// "use the env var / built-in default". See the module docs for precedence.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct FileConfig {
    /// Daemon Unix-socket path (env equivalent: `RUSTY_BRAIN_SOCKET`).
    #[serde(default)]
    pub socket_path: Option<PathBuf>,
    /// SQLite database path (env equivalent: `RUSTY_BRAIN_DB`).
    #[serde(default)]
    pub db_path: Option<PathBuf>,
    /// Per-connection request idle timeout in whole seconds (env equivalent:
    /// `RUSTY_BRAIN_IDLE_TIMEOUT_SECS`). Zero is invalid and ignored.
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    /// Evolution-jobs TOML path (env equivalent: `RB_JOBS_CONFIG`).
    #[serde(default)]
    pub jobs_config: Option<PathBuf>,
    /// `[embed]` section: embedding provider selection.
    #[serde(default)]
    pub embed: EmbedFileConfig,
    /// `[enrich]` section: opt-in LLM enrichment endpoint (no API key here —
    /// secrets are env-only).
    #[serde(default)]
    pub enrich: EnrichFileConfig,
    /// `[search]` section: recall ranking knobs.
    #[serde(default)]
    pub search: SearchFileConfig,
    /// `[retention]` section: the declarative forgetting policy (retention
    /// PRD RET-1). `None` when the section is absent — retention is opt-in,
    /// and "no section" must stay distinguishable from "disabled policy".
    #[serde(default)]
    pub retention: Option<RetentionFileConfig>,
    /// `[http]` section: the opt-in loopback HTTP listener (HTTP PRD
    /// HTTP-1/HTTP-2). `None` when the section is absent — the listener is
    /// off by default at every layer.
    #[serde(default)]
    pub http: Option<HttpFileConfig>,
}

/// `[embed]` section of the config file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct EmbedFileConfig {
    /// Embedding backend selector; `"local"` forces the local ONNX provider
    /// (env equivalent: `RB_EMBED_BACKEND`).
    #[serde(default)]
    pub backend: Option<String>,
    /// Local model name; setting it implies the local backend (env
    /// equivalent: `RB_LOCAL_MODEL`).
    #[serde(default)]
    pub local_model: Option<String>,
}

/// `[enrich]` section of the config file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct EnrichFileConfig {
    /// Chat-completions base URL (env equivalent: `RB_ENRICH_BASE_URL`).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Model name (env equivalent: `RB_ENRICH_MODEL`). Both `base_url` and
    /// `model` are required to activate LLM enrichment.
    #[serde(default)]
    pub model: Option<String>,
}

/// `[search]` section of the config file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct SearchFileConfig {
    /// Recall fusion strategy: `"linear"` (default) or `"rrf"` (W2.2; the
    /// default flip decision is deferred to W4.1 eval evidence). Env
    /// equivalent: `RB_FUSION_MODE`. Unknown values warn and are ignored.
    #[serde(default)]
    pub fusion: Option<String>,
}

/// `[retention]` section of the config file (retention PRD RET-1).
///
/// DELIBERATE strictness deviation: unlike every other section, unknown keys
/// here fail closed (`deny_unknown_fields`) instead of warn-and-ignore. This
/// section drives a policy that mutates memories — a typo'd rule that
/// silently no-ops is a data-loss footgun in reverse, and a typo that
/// silently drops a guard is the real footgun. Range/coherence validation
/// happens at resolve time via `rb_types::RetentionPolicy::validate` and
/// fails resolution (never warn-and-repair).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionFileConfig {
    /// Master opt-in; absent means `false` (forgetting stays off).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Forget horizon in days (archive on apply, purge on hard).
    #[serde(default)]
    pub max_age_days: Option<u32>,
    /// Archive horizon in days (soft stage before forget).
    #[serde(default)]
    pub archive_after_days: Option<u32>,
    /// Never forget at/above this importance (1..=10; default 6).
    #[serde(default)]
    pub importance_floor: Option<u8>,
    /// Tags that exempt a memory from forgetting entirely.
    #[serde(default)]
    pub protected_tags: Option<Vec<String>>,
    /// Per-pass sweep bound (default 500).
    #[serde(default)]
    pub batch_limit: Option<u32>,
}

/// `[http]` section of the config file (HTTP PRD HTTP-1/HTTP-2).
///
/// DELIBERATE strictness deviation (the `[retention]` precedent): unknown
/// keys fail closed (`deny_unknown_fields`) instead of warn-and-ignore. This
/// section opens a NETWORK LISTENER — a typo'd `bind` key that is silently
/// ignored would leave the daemon listening on the default address while the
/// user believes their value applied. Bind validation (literal loopback
/// `ip:port` only) happens at resolve time via
/// [`crate::validate_http_bind`] and fails resolution, never warn-and-repair.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpFileConfig {
    /// Master opt-in; absent means `false` (no listener). A `bind` value
    /// alone never enables the listener.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Literal loopback `ip:port` to bind (e.g. `"127.0.0.1:7777"`,
    /// `"[::1]:0"`). Absent means `127.0.0.1:0` (ephemeral port). Hostnames
    /// never parse — no DNS lookup can decide where the daemon listens.
    #[serde(default)]
    pub bind: Option<String>,
}

/// A loaded (or absent) config file plus any non-fatal warnings produced while
/// parsing it.
#[derive(Debug, Clone, Default)]
pub struct LoadedFileConfig {
    /// The parsed config; all-default when no file exists.
    pub config: FileConfig,
    /// Non-fatal findings (unknown keys, invalid-but-ignorable values).
    pub warnings: Vec<String>,
    /// The file that was actually read, `None` when absent.
    pub source: Option<PathBuf>,
}

/// Location of the user config file: `$XDG_CONFIG_HOME/rusty-brain/config.toml`
/// when `XDG_CONFIG_HOME` is set and non-empty, else
/// `~/.config/rusty-brain/config.toml` on every platform (the conventional CLI
/// location, also on macOS — matching git/gh/ripgrep rather than
/// `Library/Preferences`).
pub fn config_file_path() -> Result<PathBuf> {
    let base = match crate::nonempty_env_path("XDG_CONFIG_HOME") {
        Some(path) => path,
        None => crate::home_dir()?.join(".config"),
    };
    Ok(base.join("rusty-brain").join(CONFIG_FILE_NAME))
}

/// Read and parse the user config file. A missing file (or an unresolvable
/// home, i.e. no `HOME` at all) yields the all-default config; malformed
/// content fails closed with the file path in the message.
pub fn load_file_config() -> Result<LoadedFileConfig> {
    let path = match config_file_path() {
        Ok(path) => path,
        // No HOME/XDG at all: there is no config file to read. Path *defaults*
        // will fail later with their own clear errors where actually needed.
        Err(_) => return Ok(LoadedFileConfig::default()),
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (config, warnings) = parse_file_config(&text, &path)?;
            Ok(LoadedFileConfig {
                config,
                warnings,
                source: Some(path),
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LoadedFileConfig::default()),
        Err(e) => Err(Error::Io(format!(
            "cannot read config file {}: {e}",
            path.display()
        ))),
    }
}

/// Pure parsing core (injected text + path so tests never touch the real
/// config). Unknown keys warn and are ignored; malformed TOML and wrong-typed
/// values fail closed naming `source`.
pub fn parse_file_config(text: &str, source: &Path) -> Result<(FileConfig, Vec<String>)> {
    let table: toml::Table = text.parse().map_err(|e| {
        Error::InvalidArgument(format!("malformed config file {}: {e}", source.display()))
    })?;
    let mut warnings = Vec::new();
    warn_unknown_keys(&table, source, &mut warnings);
    let config: FileConfig = table.try_into().map_err(|e| {
        Error::InvalidArgument(format!(
            "invalid value in config file {}: {e}",
            source.display()
        ))
    })?;
    Ok((config, warnings))
}

/// Keys the file understands; anything else is warned about, not fatal
/// (forward compat: an old binary must keep working under a newer file).
fn warn_unknown_keys(table: &toml::Table, source: &Path, warnings: &mut Vec<String>) {
    const TOP_LEVEL: &[&str] = &[
        "socket_path",
        "db_path",
        "idle_timeout_secs",
        "jobs_config",
        "embed",
        "enrich",
        "search",
        "retention",
        "http",
    ];
    const EMBED: &[&str] = &["backend", "local_model"];
    const ENRICH: &[&str] = &["base_url", "model"];
    const SEARCH: &[&str] = &["fusion"];
    // No RETENTION or HTTP list here: `[retention]` and `[http]` unknown keys
    // FAIL CLOSED via `deny_unknown_fields` on their section structs (see
    // their doc comments), so the warn path never applies to them.

    for key in table.keys() {
        if !TOP_LEVEL.contains(&key.as_str()) {
            warnings.push(unknown_key_warning(key, source));
        }
    }
    for (section, known) in [("embed", EMBED), ("enrich", ENRICH), ("search", SEARCH)] {
        if let Some(toml::Value::Table(inner)) = table.get(section) {
            for key in inner.keys() {
                if !known.contains(&key.as_str()) {
                    warnings.push(unknown_key_warning(&format!("{section}.{key}"), source));
                }
            }
        }
    }
}

fn unknown_key_warning(key: &str, source: &Path) -> String {
    let hint = match key {
        // Consent must be explicit per model change; a persisted `true` would
        // silently accept every future swap (the lingering-env hazard the
        // store's model-verification tests pin down).
        "accept_model_change" => {
            " (not a config-file knob: consent must be per-change — use \
             `serve --accept-model-change` or RB_ACCEPT_MODEL_CHANGE)"
        }
        // Credentials never live in the plaintext config file.
        "voyage_api_key" | "api_key" | "enrich.api_key" | "embed.api_key" => {
            " (secrets are environment-only: VOYAGE_API_KEY / RB_ENRICH_API_KEY)"
        }
        // Namespace identity is per-repo, not per-user.
        "namespace" => {
            " (namespace identity belongs in the repo-committed .rusty-brain.toml, \
             not the user config)"
        }
        _ => "",
    };
    format!("ignoring unknown key `{key}` in {}{hint}", source.display())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn src() -> PathBuf {
        PathBuf::from("/test/config.toml")
    }

    #[test]
    fn full_config_parses_every_knob() {
        let text = r#"
            socket_path = "/tmp/rb.sock"
            db_path = "/tmp/rb.db"
            idle_timeout_secs = 30
            jobs_config = "/tmp/jobs.toml"

            [embed]
            backend = "local"
            local_model = "all-MiniLM-L6-v2"

            [enrich]
            base_url = "http://localhost:11434/v1"
            model = "llama3"
        "#;
        let (config, warnings) = parse_file_config(text, &src()).unwrap();
        assert!(warnings.is_empty(), "no warnings expected: {warnings:?}");
        assert_eq!(config.socket_path, Some(PathBuf::from("/tmp/rb.sock")));
        assert_eq!(config.db_path, Some(PathBuf::from("/tmp/rb.db")));
        assert_eq!(config.idle_timeout_secs, Some(30));
        assert_eq!(config.jobs_config, Some(PathBuf::from("/tmp/jobs.toml")));
        assert_eq!(config.embed.backend.as_deref(), Some("local"));
        assert_eq!(
            config.embed.local_model.as_deref(),
            Some("all-MiniLM-L6-v2")
        );
        assert_eq!(
            config.enrich.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(config.enrich.model.as_deref(), Some("llama3"));
    }

    #[test]
    fn empty_file_is_all_defaults() {
        let (config, warnings) = parse_file_config("", &src()).unwrap();
        assert_eq!(config, FileConfig::default());
        assert!(warnings.is_empty());
    }

    // Forward compat: unknown keys warn (named, with the file path) but never
    // fail — an old binary must keep working under a newer config file.
    #[test]
    fn unknown_keys_warn_and_are_ignored() {
        let text = r#"
            socket_path = "/tmp/rb.sock"
            future_knob = true

            [embed]
            backend = "local"
            quantization = "int8"

            [telemetry]
            enabled = true
        "#;
        let (config, warnings) = parse_file_config(text, &src()).unwrap();
        assert_eq!(config.socket_path, Some(PathBuf::from("/tmp/rb.sock")));
        assert_eq!(config.embed.backend.as_deref(), Some("local"));
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        for needle in ["future_knob", "embed.quantization", "telemetry"] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "warning for `{needle}` missing in {warnings:?}"
            );
        }
        for w in &warnings {
            assert!(
                w.contains("/test/config.toml"),
                "warnings must name the file: {w}"
            );
        }
    }

    // Secrets must never be config-file keys; the warning says where they go.
    #[test]
    fn secret_keys_warn_with_an_env_only_hint() {
        let text = "voyage_api_key = \"vk-123\"\n";
        let (_, warnings) = parse_file_config(text, &src()).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("environment-only"),
            "secret hint missing: {}",
            warnings[0]
        );
    }

    // Persisting model-change consent would silently accept every future swap.
    #[test]
    fn accept_model_change_is_rejected_as_a_file_knob() {
        let text = "accept_model_change = true\n";
        let (_, warnings) = parse_file_config(text, &src()).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("per-change"),
            "consent hint missing: {}",
            warnings[0]
        );
    }

    // Fail closed on malformed TOML, naming the file (never silently default —
    // a typo'd config must not silently route data to default paths).
    #[test]
    fn malformed_toml_fails_closed_naming_the_file() {
        let err = parse_file_config("socket_path = [unterminated", &src()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("/test/config.toml"), "{msg}");
        assert!(msg.contains("malformed"), "{msg}");
    }

    // Wrong-typed values are misconfiguration, not forward compat: fail closed.
    #[test]
    fn wrong_typed_value_fails_closed_naming_the_file() {
        let err = parse_file_config("idle_timeout_secs = \"soon\"", &src()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("/test/config.toml"), "{msg}");
    }

    // Retention PRD RET-1: the [retention] section parses every knob.
    #[test]
    fn retention_section_parses_every_knob() {
        let text = r#"
            [retention]
            enabled = true
            max_age_days = 365
            archive_after_days = 90
            importance_floor = 5
            protected_tags = ["architecture_decision", "postmortem"]
            batch_limit = 200
        "#;
        let (config, warnings) = parse_file_config(text, &src()).unwrap();
        assert!(warnings.is_empty(), "no warnings expected: {warnings:?}");
        let retention = config.retention.expect("[retention] section present");
        assert_eq!(retention.enabled, Some(true));
        assert_eq!(retention.max_age_days, Some(365));
        assert_eq!(retention.archive_after_days, Some(90));
        assert_eq!(retention.importance_floor, Some(5));
        assert_eq!(
            retention.protected_tags,
            Some(vec![
                "architecture_decision".to_string(),
                "postmortem".to_string()
            ])
        );
        assert_eq!(retention.batch_limit, Some(200));
    }

    #[test]
    fn absent_retention_section_parses_to_none() {
        let (config, _) = parse_file_config("", &src()).unwrap();
        assert!(config.retention.is_none());
    }

    // DELIBERATE deviation from the warn-and-ignore rule: [retention] mutates
    // memories, so a typo'd key must fail closed, not silently no-op (data
    // loss in reverse) or silently drop a guard (the real footgun).
    #[test]
    fn unknown_key_in_retention_fails_closed() {
        let text = r#"
            [retention]
            enabled = true
            max_age_dayz = 30
        "#;
        let err = parse_file_config(text, &src()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("/test/config.toml"), "{msg}");
        assert!(
            msg.contains("max_age_dayz"),
            "must name the typo'd key: {msg}"
        );
    }

    // Wrong-typed retention values fail closed like every wrong-typed value.
    #[test]
    fn wrong_typed_retention_value_fails_closed() {
        let text = r#"
            [retention]
            max_age_days = "a year"
        "#;
        let err = parse_file_config(text, &src()).unwrap_err();
        assert!(err.to_string().contains("/test/config.toml"), "{err}");
    }

    // HTTP PRD HTTP-1: the [http] section parses every knob.
    #[test]
    fn http_section_parses_every_knob() {
        let text = r#"
            [http]
            enabled = true
            bind = "127.0.0.1:7777"
        "#;
        let (config, warnings) = parse_file_config(text, &src()).unwrap();
        assert!(warnings.is_empty(), "no warnings expected: {warnings:?}");
        let http = config.http.expect("[http] section present");
        assert_eq!(http.enabled, Some(true));
        assert_eq!(http.bind.as_deref(), Some("127.0.0.1:7777"));
    }

    // Off by default at the file layer: no section means no listener.
    #[test]
    fn absent_http_section_parses_to_none() {
        let (config, _) = parse_file_config("", &src()).unwrap();
        assert!(config.http.is_none());
    }

    // DELIBERATE deviation from the warn-and-ignore rule (the [retention]
    // precedent): [http] opens a network listener, so a typo'd key must fail
    // closed — a silently ignored `bindd` would listen on the default address
    // while the user believes their value applied.
    #[test]
    fn unknown_key_in_http_fails_closed() {
        let text = r#"
            [http]
            enabled = true
            bindd = "127.0.0.1:7777"
        "#;
        let err = parse_file_config(text, &src()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("/test/config.toml"), "{msg}");
        assert!(msg.contains("bindd"), "must name the typo'd key: {msg}");
    }

    // Wrong-typed [http] values fail closed like every wrong-typed value.
    #[test]
    fn wrong_typed_http_value_fails_closed() {
        let text = r#"
            [http]
            enabled = "yes"
        "#;
        let err = parse_file_config(text, &src()).unwrap_err();
        assert!(err.to_string().contains("/test/config.toml"), "{err}");
    }

    // The user config location is derived from XDG/HOME exclusively — never
    // from the working directory, so a cloned repo's `.rusty-brain.toml` (or
    // any repo file) can never become a daemon-config source. Identity-only
    // handling of the repo file is pinned in `namespace.rs` tests.
    #[test]
    fn config_path_is_under_the_user_config_dir() {
        // Uses whatever XDG_CONFIG_HOME/HOME the process has; we only assert
        // the rusty-brain/config.toml tail, not the base (env-mutating
        // precedence tests live in lib.rs under ENV_LOCK).
        if let Ok(p) = config_file_path() {
            assert!(p.ends_with("rusty-brain/config.toml"), "{p:?}");
        }
    }
}
