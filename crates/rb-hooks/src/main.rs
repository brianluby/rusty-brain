//! `rusty-brain-hooks` — the per-event capture hook binary.
//!
//! FAIL-OPEN CONTRACT: this binary NEVER blocks, NEVER returns non-zero, and
//! NEVER lets an error reach the agent. It reads one event JSON on stdin,
//! captures memories / injects context best-effort, prints the CLI-specific
//! `{"continue":true,...}` to stdout, and always exits 0. Any panic or error
//! anywhere degrades to a literal `{"continue":true}`.

mod cache;
mod capture;
mod cli;
mod dispatch;
mod io;
mod scratch;
mod transcript;

use std::time::Duration;

use rb_agents::agent_for;
use rb_agents::detect_namespace;
use rb_agents::{AutoStart, DaemonClient};
use rb_agents::{HookEvent, HookResult};

use crate::cli::Args;
use crate::scratch::Scratch;

/// Overall wall-clock budget for the connect+capture phase. On expiry the
/// harness abandons the daemon work and still prints a fail-open response.
const OVERALL_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-connect budget for reaching the daemon.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

fn main() {
    // Optional tracing to stderr only when RUSTY_BRAIN_LOG is set (no stderr by
    // default so we never pollute the agent's hook channel).
    if std::env::var_os("RUSTY_BRAIN_LOG").is_some() {
        init_tracing();
    }

    let result = std::panic::catch_unwind(run);
    let rendered = match result {
        Ok(value) => value,
        Err(_) => serde_json::json!({ "continue": true }),
    };
    io::write_stdout(&rendered);
    std::process::exit(0);
}

/// Install a minimal stderr tracing subscriber when `RUSTY_BRAIN_LOG` is set.
///
/// Only called when the env var is present, so logging stays OFF by default and
/// the hook never pollutes the agent's channel. The level comes from
/// `RUSTY_BRAIN_LOG` (an `EnvFilter` directive, e.g. `debug` or
/// `rb_hooks=trace`), defaulting to `warn`. Fail-open: any init error (e.g. a
/// subscriber already set) is ignored; never panics.
fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_env("RUSTY_BRAIN_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .try_init();
}

/// The real body. Returns the JSON value to print. Any internal error is mapped
/// to a fail-open render so `main` can print it unconditionally.
fn run() -> serde_json::Value {
    // Parse args; on failure, last-resort literal continue.
    let args = match Args::parse_from(std::env::args()) {
        Ok(args) => args,
        Err(e) => {
            tracing::warn!("arg parse failed (fail-open): {e}");
            return serde_json::json!({ "continue": true });
        }
    };

    let cli = agent_for(args.agent);

    // Read + parse stdin (fail-open: Null on empty/invalid).
    let raw = io::read_stdin_json();
    let ctx = cli.parse_input(&raw);

    // Namespace detection runs OFF the async runtime (it shells out to git and
    // reads files). detect_namespace never panics; degrades to Global.
    let namespace = detect_namespace(&ctx.cwd);

    // Resolve daemon paths through rb-config (one truth with daemon + CLI). In
    // a degenerate environment (no HOME/XDG at all) the socket is unresolvable:
    // fail open with no daemon work rather than guessing a divergent path.
    let socket = match socket_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("socket path unresolvable (fail-open): {e}");
            return cli.render_output(&continue_result());
        }
    };

    // Only SessionStart may auto-start the daemon. Other events never spawn.
    // An unresolvable db path only disables auto-start; connecting to an
    // already-running daemon at `socket` still works.
    let auto_start = match &ctx.event {
        HookEvent::SessionStart { .. } => match db_path() {
            Ok(db) => Some(AutoStart {
                self_exe: daemon_bin(),
                db,
            }),
            Err(e) => {
                tracing::warn!("db path unresolvable; auto-start disabled (fail-open): {e}");
                None
            }
        },
        _ => None,
    };

    // Per-session capture scratch (W3.1): keyed by session id, namespace-isolated.
    // Absent when the event carries no session id — capture then no-ops. The
    // scratch is the sole capture-dedup mechanism (it coalesces exact repeats);
    // no separate dedup window straddles its per-session reset boundary.
    let scratch = ctx
        .session_id
        .as_deref()
        .map(|sid| Scratch::for_session(&namespace, sid));

    // Build a runtime; if that fails, fail open.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::warn!("runtime build failed (fail-open): {e}");
            return cli.render_output(&continue_result());
        }
    };

    // Provenance identity (W0.5): every hook write declares source=hook plus
    // the driving agent CLI and the event's session id; the daemon stamps these
    // onto the stored memories (user/host fall back to daemon-side whoami).
    let identity = rb_proto::ClientIdentity {
        agent: Some(cli.id().as_str().to_string()),
        session_id: ctx.session_id.clone(),
        source: Some("hook".to_string()),
        ..Default::default()
    };

    let result = runtime.block_on(async {
        // Overall timeout guards the whole connect+capture phase.
        match tokio::time::timeout(
            OVERALL_TIMEOUT,
            capture_phase(
                &socket,
                &namespace,
                auto_start,
                identity,
                scratch.as_ref(),
                &ctx,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("overall timeout (fail-open)");
                continue_result()
            }
        }
    });

    cli.render_output(&result)
}

/// Connect (best-effort, only when the event needs the store) and dispatch the
/// event to its capture flow.
async fn capture_phase(
    socket: &std::path::Path,
    namespace: &rb_types::Namespace,
    auto_start: Option<AutoStart>,
    identity: rb_proto::ClientIdentity,
    scratch: Option<&Scratch>,
    ctx: &rb_agents::HookContext,
) -> HookResult {
    // W3.1: PostToolUse (scratch append) and Stop (no-op) never touch the
    // daemon, so they skip the connect cost entirely; only SessionStart /
    // SessionCheckpoint / SessionEnd / PreCompact read or write the store.
    let mut client = if event_needs_daemon(&ctx.event) {
        DaemonClient::connect(
            socket,
            namespace.clone(),
            CONNECT_TIMEOUT,
            auto_start,
            Some(identity),
        )
        .await
    } else {
        None
    };
    dispatch::dispatch(client.as_mut(), scratch, ctx).await
}

/// True for events that read or write the store (and so need a daemon
/// connection): SessionStart (inject), UserPromptSubmit (recall + inject),
/// SessionCheckpoint/SessionEnd (fold + store), PreCompact (store). Local-only
/// events — PostToolUse (scratch append), Stop (no-op), Other — skip the
/// connect.
fn event_needs_daemon(event: &HookEvent) -> bool {
    matches!(
        event,
        HookEvent::SessionStart { .. }
            | HookEvent::UserPromptSubmit { .. }
            | HookEvent::SessionCheckpoint { .. }
            | HookEvent::SessionEnd { .. }
            | HookEvent::PreCompact { .. }
    )
}

fn continue_result() -> HookResult {
    HookResult::default()
}

/// Resolve the daemon socket path: `RUSTY_BRAIN_SOCKET` override, else the
/// user `config.toml`, else the rb-config default (C1). Delegates so hooks,
/// CLI, and daemon share ONE resolution rule and can never resolve different
/// sockets (F03/F12/F49). A malformed config file is an `Err` here, which the
/// caller fails open on (no daemon work) — never a guessed divergent path.
fn socket_path() -> rb_types::Result<std::path::PathBuf> {
    rb_config::resolve_socket_path()
}

/// Resolve the daemon db path: `RUSTY_BRAIN_DB` override, else the user
/// `config.toml`, else the rb-config default. Same single-rule delegation as
/// [`socket_path`].
fn db_path() -> rb_types::Result<std::path::PathBuf> {
    rb_config::resolve_db_path()
}

/// Resolve the `rusty-brain` DAEMON binary for auto-start.
///
/// The auto-start path runs `<bin> serve`, which MUST be the daemon binary, not
/// this hooks binary. `install.sh` places `rusty-brain` and `rusty-brain-hooks`
/// together in the same directory, so we resolve the daemon as a sibling of the
/// running hooks executable. If the sibling is missing (or `current_exe` is
/// unresolvable), fall back to the bare name `rusty-brain`, which the OS resolves
/// on `PATH`. Fully fail-open: if nothing is resolvable, auto-start simply fails
/// and `connect` degrades to `None` — never panics, never blocks.
fn daemon_bin() -> std::path::PathBuf {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(DAEMON_BIN_NAME)));
    match sibling {
        Some(p) if p.exists() => p,
        _ => std::path::PathBuf::from(DAEMON_BIN_NAME),
    }
}

/// Bare name of the daemon binary (no `.exe`: this hook path is unix-only in
/// practice, and the bare name still resolves on `PATH` everywhere).
const DAEMON_BIN_NAME: &str = "rusty-brain";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::sync::Mutex;

    /// Serializes env-mutating tests (process-global env; see rb-config tests).
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

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    /// Clear the `RUSTY_BRAIN_*` overrides so the default-resolution path runs.
    fn without_overrides() -> [EnvGuard; 2] {
        [
            EnvGuard::remove("RUSTY_BRAIN_SOCKET"),
            EnvGuard::remove("RUSTY_BRAIN_DB"),
        ]
    }

    /// Point config-file resolution (C1) at an isolated empty tempdir so a
    /// developer's real `~/.config/rusty-brain/config.toml` can never leak
    /// into these tests. Returns the guard plus the dir (kept alive).
    fn isolated_config() -> (EnvGuard, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (EnvGuard::set("XDG_CONFIG_HOME", dir.path()), dir)
    }

    // F03/F12/F49 regression: the hooks binary and the daemon MUST resolve the
    // same default socket/db paths, or capture writes to a daemon the CLI never
    // reads. rb_daemon re-exports rb-config, so these pin hooks == daemon.
    #[test]
    fn hook_default_paths_agree_with_daemon_when_xdg_vars_are_set() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _overrides = without_overrides();
        let (_conf_guard, _confdir) = isolated_config();
        let runtime = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let _g1 = EnvGuard::set("XDG_RUNTIME_DIR", runtime.path());
        let _g2 = EnvGuard::set("XDG_DATA_HOME", data.path());

        assert_eq!(
            socket_path().unwrap(),
            rb_daemon::default_socket_path().unwrap(),
            "hooks and daemon must agree on the socket path"
        );
        assert_eq!(
            db_path().unwrap(),
            rb_daemon::default_db_path().unwrap(),
            "hooks and daemon must agree on the db path"
        );
    }

    #[test]
    fn hook_default_paths_agree_with_daemon_when_xdg_vars_are_unset() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _overrides = without_overrides();
        let home = tempfile::tempdir().unwrap();
        let _g1 = EnvGuard::remove("XDG_RUNTIME_DIR");
        let _g2 = EnvGuard::remove("XDG_CACHE_HOME");
        let _g3 = EnvGuard::remove("XDG_DATA_HOME");
        // The config file is HOME-derived too here (no XDG_CONFIG_HOME): the
        // empty temp HOME has none, so defaults apply.
        let _g5 = EnvGuard::remove("XDG_CONFIG_HOME");
        let _g4 = EnvGuard::set("HOME", home.path());

        let socket = socket_path().unwrap();
        let db = db_path().unwrap();
        assert_eq!(
            socket,
            rb_daemon::default_socket_path().unwrap(),
            "hooks and daemon must agree on the socket path"
        );
        assert_eq!(
            db,
            rb_daemon::default_db_path().unwrap(),
            "hooks and daemon must agree on the db path"
        );
        // Both live under HOME-derived platform dirs, never a temp dir.
        assert!(
            socket.starts_with(home.path()),
            "socket under HOME: {socket:?}"
        );
        assert!(db.starts_with(home.path()), "db under HOME: {db:?}");
    }

    #[test]
    fn explicit_env_overrides_win_over_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_conf_guard, _confdir) = isolated_config();
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("override.sock");
        let db = dir.path().join("override.db");
        let _g1 = EnvGuard::set("RUSTY_BRAIN_SOCKET", &sock);
        let _g2 = EnvGuard::set("RUSTY_BRAIN_DB", &db);

        assert_eq!(socket_path().unwrap(), sock);
        assert_eq!(db_path().unwrap(), db);
    }

    // A whitespace-only override is not a path: hooks must fall back to the
    // daemon default exactly like the CLI does (both delegate to rb-config),
    // not treat " " as a real socket — the W0.2 divergence class.
    #[test]
    fn whitespace_only_overrides_agree_with_daemon_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        let (_conf_guard, _confdir) = isolated_config();
        let _g1 = EnvGuard::set("RUSTY_BRAIN_SOCKET", std::path::Path::new("  "));
        let _g2 = EnvGuard::set("RUSTY_BRAIN_DB", std::path::Path::new("  "));
        assert_eq!(
            socket_path().unwrap(),
            rb_daemon::default_socket_path().unwrap(),
            "whitespace socket override must fall back to the daemon default"
        );
        assert_eq!(
            db_path().unwrap(),
            rb_daemon::default_db_path().unwrap(),
            "whitespace db override must fall back to the daemon default"
        );
    }

    // C1 agreement (the F20-class fix at the resolver level): with the user
    // config.toml as the ONLY source — no RUSTY_BRAIN_* env at all — the hooks
    // resolve the exact paths the CLI and the daemon resolve through
    // `rb_config::EffectiveConfig`. The daemon re-reads the same file itself
    // at startup (XDG_CONFIG_HOME is on FORWARD_ENV), so no env forwarding of
    // the knob is involved anywhere.
    #[test]
    fn config_file_paths_agree_with_cli_and_daemon_resolution() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _overrides = without_overrides();
        let (_conf_guard, confdir) = isolated_config();
        let dir = confdir.path().join("rusty-brain");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "socket_path = \"/from-file/rb.sock\"\ndb_path = \"/from-file/rb.db\"\n",
        )
        .unwrap();

        let effective = rb_config::EffectiveConfig::resolve().unwrap();
        assert_eq!(
            socket_path().unwrap(),
            effective.socket_path,
            "hooks and CLI/daemon must resolve the identical config-file socket"
        );
        assert_eq!(
            db_path().unwrap(),
            effective.db_path,
            "hooks and CLI/daemon must resolve the identical config-file db"
        );
        assert_eq!(
            socket_path().unwrap(),
            std::path::PathBuf::from("/from-file/rb.sock")
        );
        assert_eq!(
            db_path().unwrap(),
            std::path::PathBuf::from("/from-file/rb.db")
        );
    }

    // Malformed config: hooks get an Err (and fail open, doing no daemon
    // work) rather than silently resolving a divergent default path.
    #[test]
    fn malformed_config_file_resolves_to_an_error_not_a_guessed_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _overrides = without_overrides();
        let (_conf_guard, confdir) = isolated_config();
        let dir = confdir.path().join("rusty-brain");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "socket_path = [broken").unwrap();

        let err = socket_path().unwrap_err();
        assert!(
            err.to_string().contains("config.toml"),
            "error must name the config file: {err}"
        );
    }

    #[test]
    fn daemon_bin_resolves_to_the_daemon_not_the_hooks_binary() {
        let path = daemon_bin();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("daemon binary path must have a utf8 file name");
        // It must point at the daemon binary, never at the hooks binary that runs
        // this code (`<self> serve` would be invalid for `rusty-brain-hooks`).
        assert_eq!(
            name, DAEMON_BIN_NAME,
            "auto-start must target the rusty-brain daemon, not the hooks binary"
        );
        assert_ne!(
            name, "rusty-brain-hooks",
            "auto-start must NOT target the hooks binary"
        );
    }

    #[test]
    fn checkpoint_needs_daemon_but_stop_does_not() {
        assert!(event_needs_daemon(&HookEvent::SessionCheckpoint {
            reason: Some("Stop".to_string())
        }));
        assert!(!event_needs_daemon(&HookEvent::Stop {
            last_assistant_message: Some("done".to_string()),
            stop_hook_active: false,
        }));
    }
}
