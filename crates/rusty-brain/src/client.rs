//! Client connection with daemon auto-start and bounded backoff retry.

use rb_proto::Client;
use rb_types::{Error, Namespace, Result};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Generic connect-with-retry: try `connect`; on the first failure run `spawn`
/// once only when `should_spawn` accepts that first error, then keep retrying up
/// to `max_attempts`, sleeping `backoff` between attempts. Returns the last
/// error if all attempts fail.
pub async fn connect_with_retry<C, Fut, T, S, P>(
    mut connect: C,
    spawn: S,
    should_spawn: P,
    max_attempts: usize,
    backoff: Duration,
) -> Result<T>
where
    C: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
    S: FnOnce() -> Result<()>,
    P: Fn(&Error) -> bool,
{
    let mut spawn = Some(spawn);
    let mut last_err: Option<Error> = None;
    let attempts = max_attempts.max(1);
    for attempt in 0..attempts {
        match connect().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt == 0 && !should_spawn(&e) {
                    return Err(e);
                }
                last_err = Some(e);
                if attempt == 0 {
                    if let Some(s) = spawn.take() {
                        s()?;
                    }
                }
                if backoff > Duration::ZERO && attempt + 1 < attempts {
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| Error::Io("connect failed".into())))
}

/// Connect to the daemon at `socket_path` for `namespace`, auto-starting a
/// detached `rusty-brain serve` child if the socket is not yet accepting.
pub async fn connect_or_start(
    socket_path: &Path,
    db_path: &Path,
    namespace: Namespace,
    self_exe: PathBuf,
) -> Result<Client> {
    let sock = socket_path.to_path_buf();
    let ns = namespace.clone();
    let connect = || {
        let sock = sock.clone();
        let ns = ns.clone();
        async move { Client::connect(&sock, ns).await }
    };
    let spawn_sock = socket_path.to_path_buf();
    let spawn_db = db_path.to_path_buf();
    let spawn = move || spawn_daemon(&self_exe, &spawn_sock, &spawn_db);
    connect_with_retry(
        connect,
        spawn,
        should_auto_start,
        50,
        Duration::from_millis(100),
    )
    .await
}

/// Spawn `rusty-brain serve` as a detached child, passing the resolved
/// `RUSTY_BRAIN_SOCKET`/`RUSTY_BRAIN_DB` so child + client agree on paths.
fn spawn_daemon(self_exe: &Path, socket_path: &Path, db_path: &Path) -> Result<()> {
    let mut cmd = daemon_command(self_exe, socket_path, db_path);
    cmd.spawn()
        .map(|_child| ())
        .map_err(|e| Error::Io(e.to_string()))
}

fn daemon_command(self_exe: &Path, socket_path: &Path, db_path: &Path) -> Command {
    let mut cmd = Command::new(self_exe);
    cmd.arg("serve");
    // Security: never inherit the parent environment into a long-lived detached
    // daemon. Clear it, then forward ONLY what the daemon needs.
    cmd.env_clear();
    cmd.env(crate::paths::SOCKET_ENV, socket_path);
    cmd.env(crate::paths::DB_ENV, db_path);
    // Forward each allowlisted var that is actually set in the parent.
    const FORWARD: &[&str] = &[
        "VOYAGE_API_KEY",
        "ANTHROPIC_API_KEY",
        "HOME",
        "PATH",
        "XDG_RUNTIME_DIR",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
    ];
    for key in FORWARD {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd
}

/// Classify whether a connect failure is one a freshly-spawned daemon would fix.
/// Only `NotFound` (socket file absent) and `ConnectionRefused` (no listener)
/// are transient-on-start; everything else (incl. `PermissionDenied`) is
/// permanent and must NOT trigger a spawn or further retries.
fn should_auto_start_kind(kind: Option<std::io::ErrorKind>) -> bool {
    matches!(
        kind,
        Some(std::io::ErrorKind::NotFound) | Some(std::io::ErrorKind::ConnectionRefused)
    )
}

fn should_auto_start(error: &Error) -> bool {
    should_auto_start_kind(error.io_kind())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn succeeds_on_first_try_without_spawning() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let sp = Arc::clone(&spawned);
        let spawn = move || {
            sp.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let at = Arc::clone(&attempts);
        let connect = move || {
            at.fetch_add(1, Ordering::SeqCst);
            async { Ok::<u32, rb_types::Error>(7) }
        };
        let v = connect_with_retry(connect, spawn, |_| true, 5, std::time::Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(v, 7);
        assert_eq!(
            spawned.load(Ordering::SeqCst),
            0,
            "no spawn when first connect works"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn spawns_then_retries_until_connected() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let sp = Arc::clone(&spawned);
        let spawn = move || {
            sp.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let at = Arc::clone(&attempts);
        let connect = move || {
            let n = at.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(rb_types::Error::from_io(&std::io::Error::from(
                        std::io::ErrorKind::NotFound,
                    )))
                } else {
                    Ok::<u32, rb_types::Error>(42)
                }
            }
        };
        let v = connect_with_retry(
            connect,
            spawn,
            should_auto_start,
            10,
            std::time::Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(v, 42);
        assert_eq!(
            spawned.load(Ordering::SeqCst),
            1,
            "spawned exactly once after first failure"
        );
        assert!(attempts.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let spawn = || Ok(());
        let connect = || async {
            Err::<u32, rb_types::Error>(rb_types::Error::from_io(&std::io::Error::from(
                std::io::ErrorKind::ConnectionRefused,
            )))
        };
        let err = connect_with_retry(
            connect,
            spawn,
            should_auto_start,
            3,
            std::time::Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(err.io_kind().is_some());
    }

    #[tokio::test]
    async fn does_not_spawn_for_non_startable_errors() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let sp = Arc::clone(&spawned);
        let spawn = move || {
            sp.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        let connect = || async {
            Err::<u32, rb_types::Error>(rb_types::Error::Storage(
                "contract version mismatch".into(),
            ))
        };
        let err = connect_with_retry(
            connect,
            spawn,
            should_auto_start,
            3,
            std::time::Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, rb_types::Error::Storage(_)));
        assert_eq!(spawned.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn auto_starts_only_for_notfound_and_connection_refused() {
        assert!(should_auto_start_kind(Some(std::io::ErrorKind::NotFound)));
        assert!(should_auto_start_kind(Some(
            std::io::ErrorKind::ConnectionRefused
        )));
    }

    #[test]
    fn does_not_auto_start_for_permanent_or_unknown_errors() {
        // Permission denied is permanent: spawning a child will not fix it.
        assert!(!should_auto_start_kind(Some(
            std::io::ErrorKind::PermissionDenied
        )));
        // A non-io error (no ErrorKind) is never auto-started.
        assert!(!should_auto_start_kind(None));
    }

    #[test]
    fn daemon_command_clears_env_and_forwards_only_allowlisted_vars() {
        // An allowlisted var the parent has set must be forwarded.
        std::env::set_var("VOYAGE_API_KEY", "voyage-key");
        // A parent var NOT on the allowlist must not reach the child.
        std::env::set_var("RB_TEST_SHOULD_NOT_LEAK", "secret");

        let socket = Path::new("/tmp/rb.sock");
        let db = Path::new("/tmp/rb.db");
        let cmd = daemon_command(Path::new("/bin/echo"), socket, db);

        let envs: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| (key.to_os_string(), value.to_os_string()))
            })
            .collect();

        // Forwarded: resolved paths (always set explicitly).
        assert_eq!(
            envs.get(std::ffi::OsStr::new(crate::paths::SOCKET_ENV)),
            Some(&socket.as_os_str().to_os_string())
        );
        assert_eq!(
            envs.get(std::ffi::OsStr::new(crate::paths::DB_ENV)),
            Some(&db.as_os_str().to_os_string())
        );
        // Forwarded: allowlisted API key present in the parent.
        assert_eq!(
            envs.get(std::ffi::OsStr::new("VOYAGE_API_KEY")),
            Some(&std::ffi::OsString::from("voyage-key"))
        );
        // NOT forwarded: an arbitrary parent var.
        assert!(
            !envs.contains_key(std::ffi::OsStr::new("RB_TEST_SHOULD_NOT_LEAK")),
            "non-allowlisted parent vars must be cleared"
        );

        std::env::remove_var("RB_TEST_SHOULD_NOT_LEAK");
        std::env::remove_var("VOYAGE_API_KEY");
    }
}
