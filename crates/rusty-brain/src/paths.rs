//! Socket / database path resolution with env-var overrides for tests.

use std::path::PathBuf;

// Env-var names are owned by rb-config (shared with the daemon and hooks);
// re-exported so existing `crate::paths::*` consumers keep working.
pub use rb_config::{DB_ENV, JOBS_CONFIG_ENV, SOCKET_ENV};

/// Resolve the socket path: explicit override wins, else the shared default.
pub fn resolve_socket_path(override_value: Option<String>) -> rb_types::Result<PathBuf> {
    match override_value.filter(|p| !p.trim().is_empty()) {
        Some(p) => Ok(PathBuf::from(p)),
        None => rb_config::default_socket_path(),
    }
}

/// Resolve the database path: explicit override wins, else the shared default.
pub fn resolve_db_path(override_value: Option<String>) -> rb_types::Result<PathBuf> {
    match override_value.filter(|p| !p.trim().is_empty()) {
        Some(p) => Ok(PathBuf::from(p)),
        None => rb_config::default_db_path(),
    }
}

/// Resolve the jobs-config path: explicit override wins, else the env value,
/// else `None` (meaning: load the all-disabled default). Blank strings are
/// treated as absent.
pub fn resolve_jobs_config_path(
    override_value: Option<String>,
    env_value: Option<String>,
) -> Option<PathBuf> {
    override_value
        .filter(|p| !p.trim().is_empty())
        .or_else(|| env_value.filter(|p| !p.trim().is_empty()))
        .map(PathBuf::from)
}

/// Read the socket path from the environment (override) or fall back to default.
pub fn socket_path_from_env() -> rb_types::Result<PathBuf> {
    resolve_socket_path(std::env::var(SOCKET_ENV).ok())
}

/// Read the db path from the environment (override) or fall back to default.
pub fn db_path_from_env() -> rb_types::Result<PathBuf> {
    resolve_db_path(std::env::var(DB_ENV).ok())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn socket_path_prefers_override() {
        let got = resolve_socket_path(Some("/tmp/rb-test.sock".to_string())).unwrap();
        assert_eq!(got, PathBuf::from("/tmp/rb-test.sock"));
    }

    #[test]
    fn socket_path_falls_back_to_default_when_no_override() {
        let got = resolve_socket_path(None).unwrap();
        assert_eq!(got.file_name().unwrap(), "sock");
    }

    #[test]
    fn socket_path_falls_back_to_default_when_override_is_empty() {
        let got = resolve_socket_path(Some("  ".to_string())).unwrap();
        assert_eq!(got.file_name().unwrap(), "sock");
    }

    #[test]
    fn db_path_prefers_override() {
        let got = resolve_db_path(Some("/tmp/rb-test.db".to_string())).unwrap();
        assert_eq!(got, PathBuf::from("/tmp/rb-test.db"));
    }

    #[test]
    fn db_path_falls_back_to_default_when_no_override() {
        let got = resolve_db_path(None).unwrap();
        assert_eq!(got.extension().unwrap(), "db");
    }

    #[test]
    fn db_path_falls_back_to_default_when_override_is_empty() {
        let got = resolve_db_path(Some("".to_string())).unwrap();
        assert_eq!(got.extension().unwrap(), "db");
    }

    #[test]
    fn jobs_config_prefers_override_then_env_then_none() {
        // Explicit override wins.
        assert_eq!(
            resolve_jobs_config_path(Some("/tmp/a.toml".to_string()), None),
            Some(PathBuf::from("/tmp/a.toml"))
        );
        // Env used when no override.
        assert_eq!(
            resolve_jobs_config_path(None, Some("/tmp/b.toml".to_string())),
            Some(PathBuf::from("/tmp/b.toml"))
        );
        // Neither -> None (all jobs disabled by default).
        assert_eq!(resolve_jobs_config_path(None, None), None);
        // Blank override falls through to env.
        assert_eq!(
            resolve_jobs_config_path(Some("  ".to_string()), Some("/tmp/c.toml".to_string())),
            Some(PathBuf::from("/tmp/c.toml"))
        );
    }
}
