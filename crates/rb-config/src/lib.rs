//! `rb_config`: the single source of truth for cross-crate configuration
//! contracts — default socket/db paths, config-bearing env-var names, and the
//! auto-start env allowlist. Every binary (daemon, CLI, hooks) resolves through
//! this crate so they can never disagree on where the daemon lives.

pub mod namespace;

use std::path::PathBuf;

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

/// The exact set of parent env vars an auto-start daemon child may inherit.
/// Everything else is cleared before spawn (no parent-env leak into a
/// long-lived detached process). Keep this list minimal — adding a var widens
/// the leak surface and must fail the allowlist tests in every spawner.
pub const FORWARD_ENV: &[&str] = &[
    // Embedding provider selection + credentials.
    "VOYAGE_API_KEY",
    "RB_EMBED_BACKEND",
    "RB_LOCAL_MODEL",
    // Opt-in LLM enrichment.
    "RB_ENRICH_BASE_URL",
    "RB_ENRICH_MODEL",
    "RB_ENRICH_API_KEY",
    // Evolution-jobs config file.
    "RB_JOBS_CONFIG",
    // Explicit opt-in to an embedding-model swap (corpus re-embed).
    "RB_ACCEPT_MODEL_CHANGE",
    // Request idle timeout override; safe to forward (defaulted when unset).
    "RUSTY_BRAIN_IDLE_TIMEOUT_SECS",
    // Provenance fallback for clients that declare no identity (W0.5):
    // `current_user()` reads USER/LOGNAME, so an env_clear'd auto-started
    // daemon would otherwise stamp no origin user.
    "USER",
    "LOGNAME",
    // Path resolution inside the child.
    "HOME",
    "PATH",
    "XDG_RUNTIME_DIR",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
];

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

/// Resolve the socket path: `RUSTY_BRAIN_SOCKET` override, else the default.
pub fn socket_path_from_env() -> Result<PathBuf> {
    match env_override(SOCKET_ENV) {
        Some(p) => Ok(p),
        None => default_socket_path(),
    }
}

/// Resolve the database path: `RUSTY_BRAIN_DB` override, else the default.
pub fn db_path_from_env() -> Result<PathBuf> {
    match env_override(DB_ENV) {
        Some(p) => Ok(p),
        None => default_db_path(),
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
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("override.sock");
        let db = dir.path().join("override.db");
        let _g1 = EnvGuard::set(SOCKET_ENV, &sock);
        let _g2 = EnvGuard::set(DB_ENV, &db);
        assert_eq!(socket_path_from_env().unwrap(), sock);
        assert_eq!(db_path_from_env().unwrap(), db);
    }

    // W0.2: a whitespace-only override is not a path. Every binary must fall
    // back to the default here, or hooks and CLI resolve different sockets.
    #[test]
    fn whitespace_only_env_overrides_fall_back_to_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g1 = EnvGuard::set(SOCKET_ENV, std::path::Path::new("  "));
        let _g2 = EnvGuard::set(DB_ENV, std::path::Path::new("  "));
        assert_eq!(
            socket_path_from_env().unwrap(),
            default_socket_path().unwrap()
        );
        assert_eq!(db_path_from_env().unwrap(), default_db_path().unwrap());
    }

    // Overrides are raw filesystem paths: a non-UTF-8 value must be honored
    // verbatim, not silently dropped (std::env::var would error on it).
    #[cfg(unix)]
    #[test]
    fn non_utf8_env_override_is_honored_as_a_path() {
        use std::os::unix::ffi::OsStrExt as _;
        let _lock = ENV_LOCK.lock().unwrap();
        let raw = std::ffi::OsStr::from_bytes(b"/tmp/rb-\xff-override.db");
        let _g = EnvGuard::set(DB_ENV, std::path::Path::new(raw));
        assert_eq!(db_path_from_env().unwrap(), PathBuf::from(raw));
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

    // Regression for the F20 allowlist gap: an auto-started daemon must see the
    // embedding-backend selection, the local-model choice, and the jobs config —
    // otherwise it silently degrades to defaults the user never chose.
    #[test]
    fn forward_env_includes_the_previously_missing_config_vars() {
        for var in ["RB_EMBED_BACKEND", "RB_LOCAL_MODEL", "RB_JOBS_CONFIG"] {
            assert!(FORWARD_ENV.contains(&var), "FORWARD_ENV must include {var}");
        }
    }

    // W0.1: an auto-started daemon must honor the injected idle timeout, or the
    // idle/reconnect e2e cannot shrink the 60s default down to test scale.
    #[test]
    fn forward_env_includes_the_idle_timeout_override() {
        assert!(FORWARD_ENV.contains(&IDLE_TIMEOUT_ENV));
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
        assert!(!FORWARD_ENV.contains(&SOCKET_ENV));
        assert!(!FORWARD_ENV.contains(&DB_ENV));
    }
}
