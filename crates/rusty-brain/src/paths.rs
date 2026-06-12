//! Socket / database path resolution with env-var overrides for tests.

use std::path::PathBuf;

// Env-var names are owned by rb-config (shared with the daemon and hooks);
// re-exported so existing `crate::paths::*` consumers keep working.
pub use rb_config::{DB_ENV, JOBS_CONFIG_ENV, SOCKET_ENV};

// Env-override composition (one trim/non-empty rule for hooks, CLI, and
// daemon) is owned by rb-config; re-exported so `crate::paths::*` callers keep
// working and the CLI can never drift from the hooks' resolution.
pub use rb_config::{db_path_from_env, socket_path_from_env};

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::path::PathBuf;

    // The socket/db override-vs-default rule is pinned in rb-config's own tests
    // (including the whitespace-only case); only the jobs-config composition
    // lives here.

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
