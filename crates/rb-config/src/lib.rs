//! `rb_config`: the single source of truth for cross-crate configuration
//! contracts — default socket/db paths, config-bearing env-var names, and the
//! auto-start env allowlist. Every binary (daemon, CLI, hooks) resolves through
//! this crate so they can never disagree on where the daemon lives.

use std::path::PathBuf;

use rb_types::{Error, Result};

/// Env var that overrides the daemon socket path.
pub const SOCKET_ENV: &str = "RUSTY_BRAIN_SOCKET";
/// Env var that overrides the database path.
pub const DB_ENV: &str = "RUSTY_BRAIN_DB";
/// Env var that points at the evolution-jobs TOML config.
pub const JOBS_CONFIG_ENV: &str = "RB_JOBS_CONFIG";

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

    #[test]
    fn forward_env_never_forwards_socket_or_db_overrides() {
        // Spawners pass the RESOLVED socket/db explicitly; forwarding the raw
        // override vars too would let a stale parent value shadow them.
        assert!(!FORWARD_ENV.contains(&SOCKET_ENV));
        assert!(!FORWARD_ENV.contains(&DB_ENV));
    }
}
