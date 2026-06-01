//! Client connection with daemon auto-start and bounded backoff retry.

use rb_proto::Client;
use rb_types::{Error, Namespace, Result};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Generic connect-with-retry: try `connect`; on the first failure run `spawn`
/// once, then keep retrying up to `max_attempts`, sleeping `backoff` between
/// attempts. Returns the last error if all attempts fail.
pub async fn connect_with_retry<C, Fut, T, S>(
    mut connect: C,
    spawn: S,
    max_attempts: usize,
    backoff: Duration,
) -> Result<T>
where
    C: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
    S: FnOnce() -> Result<()>,
{
    let mut spawn = Some(spawn);
    let mut last_err: Option<Error> = None;
    for attempt in 0..max_attempts.max(1) {
        match connect().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_err = Some(e);
                if attempt == 0 {
                    if let Some(s) = spawn.take() {
                        s()?;
                    }
                }
                if backoff > Duration::ZERO {
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
    let spawn = move || spawn_daemon(&self_exe, &spawn_sock);
    connect_with_retry(connect, spawn, 50, Duration::from_millis(100)).await
}

/// Spawn `rusty-brain serve` as a detached child, passing the resolved
/// `RUSTY_BRAIN_SOCKET` so child + client agree on the socket path.
fn spawn_daemon(self_exe: &Path, socket_path: &Path) -> Result<()> {
    let mut cmd = std::process::Command::new(self_exe);
    cmd.arg("serve");
    cmd.env(crate::paths::SOCKET_ENV, socket_path);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    cmd.spawn()
        .map(|_child| ())
        .map_err(|e| Error::Io(e.to_string()))
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
        let v = connect_with_retry(connect, spawn, 5, std::time::Duration::ZERO)
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
                    Err(rb_types::Error::Io("no socket".into()))
                } else {
                    Ok::<u32, rb_types::Error>(42)
                }
            }
        };
        let v = connect_with_retry(connect, spawn, 10, std::time::Duration::ZERO)
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
        let connect = || async { Err::<u32, rb_types::Error>(rb_types::Error::Io("never".into())) };
        let err = connect_with_retry(connect, spawn, 3, std::time::Duration::ZERO)
            .await
            .unwrap_err();
        assert!(matches!(err, rb_types::Error::Io(_)));
    }
}
