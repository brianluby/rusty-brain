//! Socket / database path resolution with env-var overrides for tests.

use std::path::PathBuf;

/// Env var that overrides the daemon socket path.
pub const SOCKET_ENV: &str = "RUSTY_BRAIN_SOCKET";
/// Env var that overrides the database path.
pub const DB_ENV: &str = "RUSTY_BRAIN_DB";

/// Resolve the socket path: explicit override wins, else the daemon default.
pub fn resolve_socket_path(override_value: Option<String>) -> rb_types::Result<PathBuf> {
    match override_value.filter(|p| !p.trim().is_empty()) {
        Some(p) => Ok(PathBuf::from(p)),
        None => rb_daemon::default_socket_path(),
    }
}

/// Resolve the database path: explicit override wins, else the daemon default.
pub fn resolve_db_path(override_value: Option<String>) -> rb_types::Result<PathBuf> {
    match override_value.filter(|p| !p.trim().is_empty()) {
        Some(p) => Ok(PathBuf::from(p)),
        None => rb_daemon::default_db_path(),
    }
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
}
