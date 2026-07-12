//! `rb_config`: the single source of truth for cross-crate configuration
//! contracts — default socket/db paths, config-bearing env-var names, the
//! user config file (`~/.config/rusty-brain/config.toml`, see [`file`]), and
//! the auto-start env allowlist. Every binary (daemon, CLI, hooks) resolves
//! through this crate so they can never disagree on where the daemon lives.
//!
//! Knob precedence everywhere: CLI flag > env var > user config file >
//! built-in default ([`EffectiveConfig::resolve`]). The env allowlist for
//! auto-started daemon children is now **secrets + identity + XDG/HOME only**
//! ([`FORWARD_ENV`]); daemon knobs reach auto-started daemons through the
//! config file, which each process re-reads from disk itself (the C1/F20-class
//! fix). The pre-C1 knob env vars stay supported via [`LEGACY_KNOB_ENV`] —
//! frozen, never extended.

pub mod file;
pub mod namespace;

use std::path::{Path, PathBuf};

pub use file::{
    config_file_path, load_file_config, parse_file_config, EmbedFileConfig, EnrichFileConfig,
    FileConfig, LoadedFileConfig, SearchFileConfig,
};
use rb_types::{Error, Result};

/// Env var that overrides the daemon socket path.
pub const SOCKET_ENV: &str = "RUSTY_BRAIN_SOCKET";
/// Env var that overrides namespace detection entirely (same precedence as the
/// `--namespace` CLI flag: explicit always wins).
pub const NAMESPACE_ENV: &str = "RUSTY_BRAIN_NAMESPACE";
/// Env var that overrides the database path.
pub const DB_ENV: &str = "RUSTY_BRAIN_DB";
/// Env var that points at the evolution-jobs TOML config.
pub const JOBS_CONFIG_ENV: &str = "RB_JOBS_CONFIG";
/// Env var that opts in to an embedding-model swap (equivalent to the
/// `--accept-model-change` serve flag; used by auto-start, where no flag can
/// be passed). Truthy = non-empty and not `0`/`false`.
pub const ACCEPT_MODEL_CHANGE_ENV: &str = "RB_ACCEPT_MODEL_CHANGE";
/// Env var that overrides the daemon's per-connection request idle timeout in
/// whole seconds (default 60). Parse failures fall back to the default; mainly
/// for tests, which need the idle/reconnect path to run in seconds.
pub const IDLE_TIMEOUT_ENV: &str = "RUSTY_BRAIN_IDLE_TIMEOUT_SECS";
/// Env var that selects the embedding backend (`local` forces the local ONNX
/// provider). Config-file equivalent: `embed.backend`.
pub const EMBED_BACKEND_ENV: &str = "RB_EMBED_BACKEND";
/// Env var naming the local embedding model (implies the local backend).
/// Config-file equivalent: `embed.local_model`.
pub const LOCAL_MODEL_ENV: &str = "RB_LOCAL_MODEL";
/// Env var for the opt-in LLM-enrichment base URL. Config-file equivalent:
/// `enrich.base_url`.
pub const ENRICH_BASE_URL_ENV: &str = "RB_ENRICH_BASE_URL";
/// Env var for the opt-in LLM-enrichment model. Config-file equivalent:
/// `enrich.model`.
pub const ENRICH_MODEL_ENV: &str = "RB_ENRICH_MODEL";
/// Env var selecting the recall fusion strategy (`linear` or `rrf`, W2.2).
/// Config-file equivalent: `search.fusion`. NOT in [`FORWARD_ENV`] (C1: knobs
/// reach auto-started daemons via the config file, never the env allowlist).
pub const FUSION_MODE_ENV: &str = "RB_FUSION_MODE";

/// The exact set of parent env vars an auto-start daemon child may inherit.
/// Everything else is cleared before spawn (no parent-env leak into a
/// long-lived detached process).
///
/// C1 contract: this list is **secrets + provenance identity + path/config
/// resolution only** and must never grow a daemon knob again. Knobs live in
/// the user config file (see [`file`]), which the child re-reads from disk
/// itself — that is how config reaches an auto-started daemon without env
/// forwarding (the F20-class fix). The pre-C1 knob vars remain forwarded for
/// compatibility via [`LEGACY_KNOB_ENV`], which is frozen.
pub const FORWARD_ENV: &[&str] = &[
    // Secrets (env-only by design; the config file never holds credentials).
    "VOYAGE_API_KEY",
    "RB_ENRICH_API_KEY",
    // Provenance fallback for clients that declare no identity (W0.5):
    // `current_user()` reads USER/LOGNAME, so an env_clear'd auto-started
    // daemon would otherwise stamp no origin user.
    "USER",
    "LOGNAME",
    // Path + config-file resolution inside the child: the child re-reads
    // `config.toml` itself, so HOME / XDG_CONFIG_HOME must survive the spawn.
    "HOME",
    "PATH",
    "XDG_RUNTIME_DIR",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
];

/// Pre-C1 daemon-knob env vars, kept working for compatibility (env still
/// wins over the config file). Spawners forward these only when explicitly
/// set in the parent, so existing env-based setups keep reaching auto-started
/// daemons.
///
/// FROZEN: never add to this list. A new daemon knob goes in the config file
/// (and optionally gets an env override read by the daemon process itself);
/// it must NOT need spawn-time forwarding — that requirement is exactly the
/// F20 bug class this crate retired. The freeze is pinned by
/// `legacy_knob_env_is_frozen`.
pub const LEGACY_KNOB_ENV: &[&str] = &[
    EMBED_BACKEND_ENV,
    LOCAL_MODEL_ENV,
    ENRICH_BASE_URL_ENV,
    ENRICH_MODEL_ENV,
    JOBS_CONFIG_ENV,
    ACCEPT_MODEL_CHANGE_ENV,
    IDLE_TIMEOUT_ENV,
];

/// Every env var a spawner forwards into an auto-started daemon child:
/// [`FORWARD_ENV`] (secrets + identity + XDG/HOME) plus the frozen
/// [`LEGACY_KNOB_ENV`] compatibility knobs. Both spawners (CLI and hooks)
/// iterate this so they can never disagree.
pub fn spawn_forward_env() -> impl Iterator<Item = &'static str> {
    FORWARD_ENV.iter().chain(LEGACY_KNOB_ENV.iter()).copied()
}

fn nonempty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> Result<PathBuf> {
    nonempty_env_path("HOME")
        .or_else(|| nonempty_env_path("USERPROFILE"))
        .ok_or_else(|| Error::Io("cannot determine a home directory".to_string()))
}

fn cache_base_dir() -> Result<PathBuf> {
    if let Some(path) = nonempty_env_path("XDG_CACHE_HOME") {
        return Ok(path);
    }

    #[cfg(target_os = "macos")]
    {
        Ok(home_dir()?.join("Library").join("Caches"))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(path) = nonempty_env_path("LOCALAPPDATA") {
            Ok(path)
        } else {
            Ok(home_dir()?.join("AppData").join("Local"))
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(home_dir()?.join(".cache"))
    }
}

fn data_base_dir() -> Result<PathBuf> {
    if let Some(path) = nonempty_env_path("XDG_DATA_HOME") {
        return Ok(path);
    }

    #[cfg(target_os = "macos")]
    {
        Ok(home_dir()?.join("Library").join("Application Support"))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(path) = nonempty_env_path("APPDATA") {
            Ok(path)
        } else {
            Ok(home_dir()?.join("AppData").join("Roaming"))
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(home_dir()?.join(".local").join("share"))
    }
}

/// Local mutable state (namespace pins). `~/.local/state` on Linux per XDG;
/// macOS has no state-dir convention, so it shares the data location.
pub(crate) fn state_base_dir() -> Result<PathBuf> {
    if let Some(path) = nonempty_env_path("XDG_STATE_HOME") {
        return Ok(path);
    }

    #[cfg(target_os = "macos")]
    {
        Ok(home_dir()?.join("Library").join("Application Support"))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(path) = nonempty_env_path("LOCALAPPDATA") {
            Ok(path)
        } else {
            Ok(home_dir()?.join("AppData").join("Local"))
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(home_dir()?.join(".local").join("state"))
    }
}

/// Default Unix-domain-socket path.
pub fn default_socket_path() -> Result<PathBuf> {
    if let Some(rt) = nonempty_env_path("XDG_RUNTIME_DIR") {
        return Ok(rt.join("rusty-brain").join("sock"));
    }

    Ok(cache_base_dir()?.join("rusty-brain").join("sock"))
}

/// Default database path.
pub fn default_db_path() -> Result<PathBuf> {
    Ok(data_base_dir()?.join("rusty-brain").join("memory.db"))
}

/// One env-override rule for every binary: a set var whose trimmed value is
/// non-empty wins; unset, empty, or whitespace-only falls back to the default.
/// Hooks, CLI, and daemon all resolve through this single rule so a malformed
/// override can never make them disagree (the F03/F12/F49 divergence class).
/// These are raw filesystem paths, so a non-UTF-8 value is honored as-is when
/// non-empty (the trim rule only applies where "whitespace" is well-defined,
/// i.e. valid UTF-8) — `std::env::var` would silently drop it.
fn env_override(name: &str) -> Option<PathBuf> {
    let raw = std::env::var_os(name)?;
    match raw.to_str() {
        Some(value) if value.trim().is_empty() => None,
        // Non-UTF-8 (`to_str() == None`) is necessarily non-empty: the empty
        // string is valid UTF-8, so it lands in the arm above.
        _ => Some(PathBuf::from(raw)),
    }
}

/// A config-file string knob: trimmed, empty treated as unset (same spirit as
/// [`env_override`] — `backend = ""` is not a selection).
fn file_string(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// A config-file path knob: whitespace-only treated as unset (TOML strings are
/// always UTF-8, so the trim rule is total here).
fn file_path(value: &Option<PathBuf>) -> Option<PathBuf> {
    value
        .as_deref()
        .filter(|p| p.to_str().is_none_or(|s| !s.trim().is_empty()))
        .map(Path::to_path_buf)
}

/// An env string knob: trimmed, empty/whitespace-only/unset treated as unset.
fn env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Resolve the socket path: `RUSTY_BRAIN_SOCKET` override, else the config
/// file, else the default. Fails closed on a malformed config file even when
/// the env override is set — a broken file is fixed, not silently bypassed.
pub fn resolve_socket_path() -> Result<PathBuf> {
    let loaded = file::load_file_config()?;
    socket_path_with(&loaded.config)
}

/// Resolve the database path: `RUSTY_BRAIN_DB` override, else the config file,
/// else the default. Same fail-closed rule as [`resolve_socket_path`].
pub fn resolve_db_path() -> Result<PathBuf> {
    let loaded = file::load_file_config()?;
    db_path_with(&loaded.config)
}

fn socket_path_with(config: &FileConfig) -> Result<PathBuf> {
    if let Some(p) = env_override(SOCKET_ENV) {
        return Ok(p);
    }
    if let Some(p) = file_path(&config.socket_path) {
        return Ok(p);
    }
    default_socket_path()
}

fn db_path_with(config: &FileConfig) -> Result<PathBuf> {
    if let Some(p) = env_override(DB_ENV) {
        return Ok(p);
    }
    if let Some(p) = file_path(&config.db_path) {
        return Ok(p);
    }
    default_db_path()
}

/// The idle timeout: env (must parse as whole seconds > 0; anything else
/// warns and is ignored) > config file (`0` warns and is ignored) > `None`
/// (the daemon's built-in 60s default).
fn resolve_idle_timeout_secs(file_value: Option<u64>, warnings: &mut Vec<String>) -> Option<u64> {
    if let Ok(raw) = std::env::var(IDLE_TIMEOUT_ENV) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            match trimmed.parse::<u64>() {
                Ok(secs) if secs > 0 => return Some(secs),
                _ => warnings.push(format!(
                    "ignoring invalid {IDLE_TIMEOUT_ENV}={trimmed:?} (want whole seconds > 0)"
                )),
            }
        }
    }
    match file_value {
        Some(0) => {
            warnings.push("ignoring idle_timeout_secs = 0 in config file (want > 0)".to_string());
            None
        }
        other => other,
    }
}

/// Normalize the fusion-mode knob (env > file): `linear`/`rrf`
/// (case-insensitive, trimmed) pass through; anything else warns and is
/// ignored per knob source, falling back to the next source then the engine
/// default — the idle-timeout warn-and-ignore precedent.
fn resolve_fusion_mode(
    env_value: Option<String>,
    file_value: Option<String>,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let mut normalize = |value: Option<String>, source: &str| -> Option<String> {
        let raw = value?;
        let v = raw.trim().to_ascii_lowercase();
        match v.as_str() {
            "linear" | "rrf" => Some(v),
            _ => {
                warnings.push(format!(
                    "ignoring invalid {source}={raw:?} (want \"linear\" or \"rrf\")"
                ));
                None
            }
        }
    };
    if let Some(v) = normalize(env_value, FUSION_MODE_ENV) {
        return Some(v);
    }
    normalize(file_value, "search.fusion in config file")
}

/// Resolve the `[retention]` section into a validated policy (retention PRD
/// RET-1). Config-file-only knob — no env equivalent, per the "new knobs land
/// in the file only" rule. An absent section is `None` (retention stays a
/// no-op). DELIBERATE deviation from the warn-and-ignore precedent: an
/// invalid policy FAILS resolution — a section that authorizes mutating
/// memories is never silently repaired or partially applied.
fn resolve_retention(
    file_value: &Option<file::RetentionFileConfig>,
) -> Result<Option<rb_types::RetentionPolicy>> {
    let Some(section) = file_value else {
        return Ok(None);
    };
    let defaults = rb_types::RetentionPolicy::default();
    let policy = rb_types::RetentionPolicy {
        enabled: section.enabled.unwrap_or(defaults.enabled),
        max_age_days: section.max_age_days,
        archive_after_days: section.archive_after_days,
        importance_floor: section
            .importance_floor
            .unwrap_or(defaults.importance_floor),
        // Trimmed so the resolved (and wire-carried) form is canonical; blank
        // entries still fail validation below. The store guard additionally
        // compares case-insensitively (PR #60 review).
        protected_tags: section
            .protected_tags
            .as_ref()
            .map(|tags| tags.iter().map(|t| t.trim().to_string()).collect())
            .unwrap_or(defaults.protected_tags),
        batch_limit: section.batch_limit.unwrap_or(defaults.batch_limit),
    };
    policy.validate()?;
    Ok(Some(policy))
}

/// The fully resolved per-process configuration: env var > user config file >
/// built-in default, per knob (CLI flags are applied above this by the
/// binaries). Resolved identically by the CLI, the hooks, and the daemon —
/// which re-reads the config file from disk itself, so a file-set knob reaches
/// an auto-started daemon with no env forwarding (C1, the F20-class fix).
///
/// Secrets (`VOYAGE_API_KEY`, `RB_ENRICH_API_KEY`) are deliberately absent:
/// they are env-only and never pass through this struct (it derives `Debug`).
#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    /// Daemon Unix-socket path.
    pub socket_path: PathBuf,
    /// SQLite database path.
    pub db_path: PathBuf,
    /// Embedding backend selector (`"local"` forces the local provider).
    pub embed_backend: Option<String>,
    /// Local embedding model name (implies the local backend).
    pub local_model: Option<String>,
    /// LLM-enrichment base URL (active only together with `enrich_model`).
    pub enrich_base_url: Option<String>,
    /// LLM-enrichment model name.
    pub enrich_model: Option<String>,
    /// Evolution-jobs TOML path.
    pub jobs_config: Option<PathBuf>,
    /// Per-connection request idle timeout in seconds; `None` = daemon default.
    pub idle_timeout_secs: Option<u64>,
    /// Recall fusion strategy, normalized to `"linear"` or `"rrf"` (W2.2);
    /// `None` = engine default (linear). Unknown values warn and are ignored
    /// (the idle-timeout precedent). The daemon parses this into its
    /// `FusionMode` — rb-config stays a leaf over rb-types.
    pub fusion_mode: Option<String>,
    /// Validated `[retention]` policy; `None` when the section is absent
    /// (forgetting stays a no-op — retention PRD RET-1). Fail-closed: an
    /// invalid section aborts resolution instead of warning.
    pub retention: Option<rb_types::RetentionPolicy>,
    /// Non-fatal findings (unknown config keys, ignored invalid values).
    /// Callers surface these (`tracing::warn!`); they never fail resolution.
    pub warnings: Vec<String>,
}

impl EffectiveConfig {
    /// Resolve every knob from the process env and the user config file.
    /// Fails closed on a malformed config file (message names the file) and
    /// when neither HOME nor XDG vars can ground the default paths.
    pub fn resolve() -> Result<Self> {
        let loaded = file::load_file_config()?;
        let mut warnings = loaded.warnings;
        let config = loaded.config;
        let idle_timeout_secs = resolve_idle_timeout_secs(config.idle_timeout_secs, &mut warnings);
        let fusion_mode = resolve_fusion_mode(
            env_string(FUSION_MODE_ENV),
            file_string(&config.search.fusion),
            &mut warnings,
        );
        Ok(Self {
            socket_path: socket_path_with(&config)?,
            db_path: db_path_with(&config)?,
            embed_backend: env_string(EMBED_BACKEND_ENV)
                .or_else(|| file_string(&config.embed.backend)),
            local_model: env_string(LOCAL_MODEL_ENV)
                .or_else(|| file_string(&config.embed.local_model)),
            enrich_base_url: env_string(ENRICH_BASE_URL_ENV)
                .or_else(|| file_string(&config.enrich.base_url)),
            enrich_model: env_string(ENRICH_MODEL_ENV)
                .or_else(|| file_string(&config.enrich.model)),
            jobs_config: env_override(JOBS_CONFIG_ENV).or_else(|| file_path(&config.jobs_config)),
            idle_timeout_secs,
            fusion_mode,
            retention: resolve_retention(&config.retention)?,
            warnings,
        })
    }
}

/// The current OS user, for provenance fallback (W0.5): `USER` then `LOGNAME`,
/// `None` in a degenerate environment. The daemon stamps this onto memories
/// written by clients that declared no identity (same-host UDS, so the
/// daemon's view of the user is the client's user).
pub fn current_user() -> Option<String> {
    ["USER", "LOGNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

/// The local hostname, for provenance fallback (W0.5). Unix: `gethostname(2)`;
/// elsewhere (and on any failure) the `HOSTNAME` env var; else `None`.
pub fn current_hostname() -> Option<String> {
    #[cfg(unix)]
    {
        // _SC_HOST_NAME_MAX is at most 255 on supported platforms; a fixed
        // 256-byte buffer (incl. NUL) is the conventional portable bound.
        let mut buf = [0u8; 256];
        // SAFETY: gethostname writes at most `len` bytes into the provided
        // buffer and NUL-terminates when it fits; the buffer outlives the call.
        #[allow(unsafe_code)]
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            if let Ok(name) = std::str::from_utf8(&buf[..end]) {
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    std::env::var("HOSTNAME").ok().filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: &std::path::Path) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }

        fn remove(name: &'static str) -> Self {
            let previous = std::env::var_os(name);
            std::env::remove_var(name);
            Self { name, previous }
        }
    }

    /// Point config-file resolution at an isolated tempdir (so a developer's
    /// real `~/.config/rusty-brain/config.toml` can never leak into tests) and
    /// clear every knob env var. Returns the guards plus the config dir.
    fn isolated_config_env() -> (Vec<EnvGuard>, tempfile::TempDir) {
        let confdir = tempfile::tempdir().unwrap();
        let mut guards = vec![EnvGuard::set("XDG_CONFIG_HOME", confdir.path())];
        for name in [
            SOCKET_ENV,
            DB_ENV,
            JOBS_CONFIG_ENV,
            IDLE_TIMEOUT_ENV,
            EMBED_BACKEND_ENV,
            LOCAL_MODEL_ENV,
            ENRICH_BASE_URL_ENV,
            ENRICH_MODEL_ENV,
            FUSION_MODE_ENV,
        ] {
            guards.push(EnvGuard::remove(name));
        }
        (guards, confdir)
    }

    /// Write `text` as the user config file under `confdir` (which is the
    /// XDG_CONFIG_HOME the guards point at).
    fn write_config(confdir: &tempfile::TempDir, text: &str) {
        let dir = confdir.path().join("rusty-brain");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), text).unwrap();
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    #[test]
    fn current_user_reads_user_env_first() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("USER", std::path::Path::new("provenance-test-user"));
        assert_eq!(current_user().as_deref(), Some("provenance-test-user"));
    }

    #[cfg(unix)]
    #[test]
    fn current_hostname_resolves_on_unix() {
        let name = current_hostname();
        assert!(
            name.as_deref().is_some_and(|n| !n.is_empty()),
            "gethostname must resolve on unix, got {name:?}"
        );
    }

    #[test]
    fn socket_path_is_under_a_rusty_brain_dir_named_sock() {
        let _guard = ENV_LOCK.lock().unwrap();
        let p = default_socket_path().unwrap();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("sock"));
        assert!(
            p.parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                == Some("rusty-brain"),
            "socket lives in a rusty-brain directory: {p:?}"
        );
    }

    #[test]
    fn db_path_ends_with_rusty_brain_db_file() {
        let p = default_db_path().unwrap();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("memory.db"));
    }

    #[test]
    fn xdg_runtime_dir_is_honored_for_socket() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set("XDG_RUNTIME_DIR", dir.path());
        let p = default_socket_path().unwrap();
        assert!(
            p.starts_with(dir.path()),
            "socket must live under XDG_RUNTIME_DIR when set: {p:?}"
        );
    }

    #[test]
    fn env_overrides_win_when_set_to_a_real_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, _confdir) = isolated_config_env();
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("override.sock");
        let db = dir.path().join("override.db");
        let _g1 = EnvGuard::set(SOCKET_ENV, &sock);
        let _g2 = EnvGuard::set(DB_ENV, &db);
        assert_eq!(resolve_socket_path().unwrap(), sock);
        assert_eq!(resolve_db_path().unwrap(), db);
    }

    // W0.2: a whitespace-only override is not a path. Every binary must fall
    // back to the default here, or hooks and CLI resolve different sockets.
    #[test]
    fn whitespace_only_env_overrides_fall_back_to_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, _confdir) = isolated_config_env();
        let _g1 = EnvGuard::set(SOCKET_ENV, std::path::Path::new("  "));
        let _g2 = EnvGuard::set(DB_ENV, std::path::Path::new("  "));
        assert_eq!(
            resolve_socket_path().unwrap(),
            default_socket_path().unwrap()
        );
        assert_eq!(resolve_db_path().unwrap(), default_db_path().unwrap());
    }

    // Overrides are raw filesystem paths: a non-UTF-8 value must be honored
    // verbatim, not silently dropped (std::env::var would error on it).
    #[cfg(unix)]
    #[test]
    fn non_utf8_env_override_is_honored_as_a_path() {
        use std::os::unix::ffi::OsStrExt as _;
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, _confdir) = isolated_config_env();
        let raw = std::ffi::OsStr::from_bytes(b"/tmp/rb-\xff-override.db");
        let _g = EnvGuard::set(DB_ENV, std::path::Path::new(raw));
        assert_eq!(resolve_db_path().unwrap(), PathBuf::from(raw));
    }

    // C1 precedence matrix, per knob: env var > config file > built-in
    // default. Asserted for the four agreement-test knobs (socket, db, embed
    // backend, idle timeout).
    #[test]
    fn precedence_is_env_then_file_then_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(
            &confdir,
            r#"
            socket_path = "/file/rb.sock"
            db_path = "/file/rb.db"
            idle_timeout_secs = 30

            [embed]
            backend = "local"
            "#,
        );

        // File over default (no env set).
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(effective.socket_path, PathBuf::from("/file/rb.sock"));
        assert_eq!(effective.db_path, PathBuf::from("/file/rb.db"));
        assert_eq!(effective.embed_backend.as_deref(), Some("local"));
        assert_eq!(effective.idle_timeout_secs, Some(30));

        // Env over file.
        let _e1 = EnvGuard::set(SOCKET_ENV, std::path::Path::new("/env/rb.sock"));
        let _e2 = EnvGuard::set(DB_ENV, std::path::Path::new("/env/rb.db"));
        let _e3 = EnvGuard::set(EMBED_BACKEND_ENV, std::path::Path::new("voyage"));
        let _e4 = EnvGuard::set(IDLE_TIMEOUT_ENV, std::path::Path::new("5"));
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(effective.socket_path, PathBuf::from("/env/rb.sock"));
        assert_eq!(effective.db_path, PathBuf::from("/env/rb.db"));
        assert_eq!(effective.embed_backend.as_deref(), Some("voyage"));
        assert_eq!(effective.idle_timeout_secs, Some(5));
    }

    #[test]
    fn defaults_apply_when_neither_env_nor_file_set_a_knob() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(&confdir, "");
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(effective.socket_path, default_socket_path().unwrap());
        assert_eq!(effective.db_path, default_db_path().unwrap());
        assert_eq!(effective.embed_backend, None);
        assert_eq!(effective.local_model, None);
        assert_eq!(effective.enrich_base_url, None);
        assert_eq!(effective.enrich_model, None);
        assert_eq!(effective.jobs_config, None);
        assert_eq!(effective.idle_timeout_secs, None);
        assert_eq!(effective.fusion_mode, None);
        assert!(effective.warnings.is_empty(), "{:?}", effective.warnings);
    }

    // Retention PRD RET-1: a [retention] section resolves into a validated
    // policy on EffectiveConfig (config-file-only knob — no env equivalent).
    #[test]
    fn retention_section_resolves_to_a_validated_policy() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(
            &confdir,
            r#"
            [retention]
            enabled = true
            max_age_days = 365
            archive_after_days = 90
            protected_tags = ["architecture_decision"]
            "#,
        );
        let effective = EffectiveConfig::resolve().unwrap();
        let policy = effective.retention.expect("policy resolved");
        assert!(policy.enabled);
        assert_eq!(policy.max_age_days, Some(365));
        assert_eq!(policy.archive_after_days, Some(90));
        // Unset knobs take the rb-types defaults.
        assert_eq!(policy.importance_floor, rb_types::DEFAULT_IMPORTANCE_FLOOR);
        assert_eq!(policy.batch_limit, rb_types::DEFAULT_BATCH_LIMIT);
        assert_eq!(
            policy.protected_tags,
            vec!["architecture_decision".to_string()]
        );
        assert!(effective.warnings.is_empty(), "{:?}", effective.warnings);
    }

    #[test]
    fn retention_protected_tags_are_trimmed_at_resolve() {
        // PR #60 review: stray whitespace in a protected tag must not survive
        // into the resolved policy (the store guard also normalizes — this
        // keeps the policy displayable and the wire form canonical).
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(
            &confdir,
            r#"
            [retention]
            max_age_days = 180
            protected_tags = ["  architecture_decision ", "postmortem"]
            "#,
        );
        let policy = EffectiveConfig::resolve().unwrap().retention.unwrap();
        assert_eq!(
            policy.protected_tags,
            vec![
                "architecture_decision".to_string(),
                "postmortem".to_string()
            ]
        );
    }

    #[test]
    fn absent_retention_section_resolves_to_none() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(&confdir, "");
        let effective = EffectiveConfig::resolve().unwrap();
        assert!(effective.retention.is_none());
    }

    // DELIBERATE deviation from warn-and-ignore (the fusion/idle-timeout
    // precedent): an out-of-range retention value fails resolution closed —
    // a policy that mutates memories is never silently repaired.
    #[test]
    fn out_of_range_retention_value_fails_resolution_closed() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(
            &confdir,
            r#"
            [retention]
            enabled = true
            max_age_days = 365
            importance_floor = 11
            "#,
        );
        let err = EffectiveConfig::resolve().unwrap_err();
        assert!(
            err.to_string().contains("importance_floor"),
            "must name the offending key: {err}"
        );
    }

    // enabled=true with no age rule is an incoherent policy: fail closed.
    #[test]
    fn enabled_retention_without_rules_fails_resolution_closed() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(
            &confdir,
            r#"
            [retention]
            enabled = true
            "#,
        );
        let err = EffectiveConfig::resolve().unwrap_err();
        assert!(err.to_string().contains("max_age_days"), "{err}");
    }

    // A disabled-but-present section still resolves (so dry-run can preview
    // a policy before the user enables it) and still validates its values.
    #[test]
    fn disabled_retention_section_resolves_and_validates() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(
            &confdir,
            r#"
            [retention]
            max_age_days = 180
            "#,
        );
        let effective = EffectiveConfig::resolve().unwrap();
        let policy = effective.retention.expect("policy resolved");
        assert!(!policy.enabled, "enabled defaults to false");
        assert_eq!(policy.max_age_days, Some(180));

        write_config(
            &confdir,
            r#"
            [retention]
            max_age_days = 0
            "#,
        );
        let err = EffectiveConfig::resolve().unwrap_err();
        assert!(err.to_string().contains("max_age_days"), "{err}");
    }

    // W2.2: the fusion-mode knob follows the same env > file > default
    // precedence, normalizes case, and warns-and-ignores unknown values.
    #[test]
    fn fusion_mode_precedence_normalization_and_invalid_value_warning() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(
            &confdir,
            r#"
            [search]
            fusion = "rrf"
            "#,
        );

        // File over default.
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(effective.fusion_mode.as_deref(), Some("rrf"));
        assert!(effective.warnings.is_empty(), "{:?}", effective.warnings);

        // Env over file, case-insensitive.
        {
            let _e = EnvGuard::set(FUSION_MODE_ENV, std::path::Path::new("Linear"));
            let effective = EffectiveConfig::resolve().unwrap();
            assert_eq!(effective.fusion_mode.as_deref(), Some("linear"));
        }

        // Invalid env value warns and falls back to the (valid) file value.
        {
            let _e = EnvGuard::set(FUSION_MODE_ENV, std::path::Path::new("cosine"));
            let effective = EffectiveConfig::resolve().unwrap();
            assert_eq!(effective.fusion_mode.as_deref(), Some("rrf"));
            assert!(
                effective
                    .warnings
                    .iter()
                    .any(|w| w.contains("RB_FUSION_MODE")),
                "invalid value must warn: {:?}",
                effective.warnings
            );
        }

        // Invalid file value (no env) warns and yields None (engine default).
        write_config(
            &confdir,
            r#"
            [search]
            fusion = "best"
            "#,
        );
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(effective.fusion_mode, None);
        assert!(
            effective
                .warnings
                .iter()
                .any(|w| w.contains("search.fusion")),
            "invalid file value must warn: {:?}",
            effective.warnings
        );
    }

    // C1 agreement: with the config file as the ONLY source, the path
    // resolvers used by the hooks (`resolve_socket_path`/`resolve_db_path`)
    // and the full resolution used by CLI + daemon (`EffectiveConfig`) yield
    // the identical values — one truth for every binary.
    #[test]
    fn hook_resolvers_agree_with_effective_config_from_file_only() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(
            &confdir,
            r#"
            socket_path = "/file-only/rb.sock"
            db_path = "/file-only/rb.db"
            "#,
        );
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(resolve_socket_path().unwrap(), effective.socket_path);
        assert_eq!(resolve_db_path().unwrap(), effective.db_path);
        assert_eq!(effective.socket_path, PathBuf::from("/file-only/rb.sock"));
        assert_eq!(effective.db_path, PathBuf::from("/file-only/rb.db"));
    }

    // The config file is found via XDG_CONFIG_HOME (and that var is on
    // FORWARD_ENV, so an auto-started daemon re-reads the same file).
    #[test]
    fn xdg_config_home_locates_the_config_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        assert_eq!(
            config_file_path().unwrap(),
            confdir.path().join("rusty-brain").join("config.toml")
        );
    }

    // Malformed TOML fails closed with the file path in the message — even
    // when env overrides are set. A broken config file is fixed, not silently
    // bypassed (half-env, half-broken-file resolution is the divergence trap).
    #[test]
    fn malformed_config_file_fails_closed_even_with_env_overrides() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(&confdir, "socket_path = [broken");
        let _g = EnvGuard::set(SOCKET_ENV, std::path::Path::new("/env/rb.sock"));
        let err = resolve_socket_path().unwrap_err();
        assert!(
            err.to_string().contains("config.toml"),
            "error must name the config file: {err}"
        );
        let err = EffectiveConfig::resolve().unwrap_err();
        assert!(err.to_string().contains("config.toml"), "{err}");
    }

    // Unknown keys warn (forward compat) but never fail resolution.
    #[test]
    fn unknown_config_keys_surface_as_warnings_not_errors() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(&confdir, "future_knob = 1\n");
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(effective.warnings.len(), 1, "{:?}", effective.warnings);
        assert!(effective.warnings[0].contains("future_knob"));
    }

    // An invalid idle override (env or file) warns and falls through instead
    // of producing an instantly-dying or never-dying connection.
    #[test]
    fn invalid_idle_timeout_values_warn_and_fall_through() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_guards, confdir) = isolated_config_env();
        write_config(&confdir, "idle_timeout_secs = 30\n");
        let _g = EnvGuard::set(IDLE_TIMEOUT_ENV, std::path::Path::new("soon"));
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(
            effective.idle_timeout_secs,
            Some(30),
            "garbage env must fall through to the file value"
        );
        assert!(
            effective.warnings.iter().any(|w| w.contains("soon")),
            "{:?}",
            effective.warnings
        );

        write_config(&confdir, "idle_timeout_secs = 0\n");
        let _g0 = EnvGuard::remove(IDLE_TIMEOUT_ENV);
        let effective = EffectiveConfig::resolve().unwrap();
        assert_eq!(effective.idle_timeout_secs, None, "0 is not a timeout");
        assert!(!effective.warnings.is_empty());
    }

    #[test]
    fn xdg_data_home_is_honored_for_db() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set("XDG_DATA_HOME", dir.path());
        let p = default_db_path().unwrap();
        assert!(
            p.starts_with(dir.path()),
            "db must live under XDG_DATA_HOME when set: {p:?}"
        );
    }

    // C1: the allowlist is secrets + provenance identity + path/config
    // resolution ONLY. Daemon knobs must never re-enter it — they reach
    // auto-started daemons through the config file the child reads itself.
    #[test]
    fn forward_env_is_secrets_identity_and_path_resolution_only() {
        assert_eq!(
            FORWARD_ENV,
            &[
                "VOYAGE_API_KEY",
                "RB_ENRICH_API_KEY",
                "USER",
                "LOGNAME",
                "HOME",
                "PATH",
                "XDG_RUNTIME_DIR",
                "XDG_DATA_HOME",
                "XDG_CONFIG_HOME",
            ],
            "FORWARD_ENV must stay secrets + identity + XDG/HOME; \
             a new daemon knob goes in config.toml, never here (F20 class)"
        );
        for knob in LEGACY_KNOB_ENV {
            assert!(
                !FORWARD_ENV.contains(knob),
                "knob {knob} must not be on FORWARD_ENV"
            );
        }
    }

    // C1 freeze: the compatibility knob list never grows. New knobs are
    // config-file knobs; if you are editing this test to add a var, you are
    // reintroducing the F20 bug class — stop and put it in FileConfig instead.
    #[test]
    fn legacy_knob_env_is_frozen() {
        assert_eq!(
            LEGACY_KNOB_ENV,
            &[
                "RB_EMBED_BACKEND",
                "RB_LOCAL_MODEL",
                "RB_ENRICH_BASE_URL",
                "RB_ENRICH_MODEL",
                "RB_JOBS_CONFIG",
                "RB_ACCEPT_MODEL_CHANGE",
                "RUSTY_BRAIN_IDLE_TIMEOUT_SECS",
            ]
        );
    }

    // Compat: every pre-C1 env knob still reaches an auto-started daemon when
    // explicitly set in the parent (env wins over file), via the spawn union.
    #[test]
    fn spawn_forward_env_is_the_union_of_allowlist_and_legacy_knobs() {
        let spawned: Vec<&str> = spawn_forward_env().collect();
        for var in FORWARD_ENV.iter().chain(LEGACY_KNOB_ENV) {
            assert!(spawned.contains(var), "spawn list must include {var}");
        }
        assert_eq!(spawned.len(), FORWARD_ENV.len() + LEGACY_KNOB_ENV.len());
    }

    // W0.5 provenance fallback: the daemon stamps `current_user()` (USER then
    // LOGNAME) on memories from identity-less clients; an env_clear'd
    // auto-started daemon must keep seeing those vars or the fallback is lost.
    #[test]
    fn forward_env_includes_the_provenance_user_fallback_vars() {
        for var in ["USER", "LOGNAME"] {
            assert!(FORWARD_ENV.contains(&var), "FORWARD_ENV must include {var}");
        }
    }

    #[test]
    fn forward_env_never_forwards_socket_or_db_overrides() {
        // Spawners pass the RESOLVED socket/db explicitly; forwarding the raw
        // override vars too would let a stale parent value shadow them.
        let spawned: Vec<&str> = spawn_forward_env().collect();
        assert!(!spawned.contains(&SOCKET_ENV));
        assert!(!spawned.contains(&DB_ENV));
    }
}
