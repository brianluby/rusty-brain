//! `rusty-brain-hooks` — the per-event capture hook binary.
//!
//! FAIL-OPEN CONTRACT: this binary NEVER blocks, NEVER returns non-zero, and
//! NEVER lets an error reach the agent. It reads one event JSON on stdin,
//! captures memories / injects context best-effort, prints the CLI-specific
//! `{"continue":true,...}` to stdout, and always exits 0. Any panic or error
//! anywhere degrades to a literal `{"continue":true}`.

mod capture;
mod cli;
mod dedup;
mod dispatch;
mod io;

use std::time::Duration;

use rb_agents::agent_for;
use rb_agents::detect_namespace;
use rb_agents::{AutoStart, DaemonClient};
use rb_agents::{HookEvent, HookResult};

use crate::cli::Args;
use crate::dedup::DedupCache;

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

    // Only SessionStart may auto-start the daemon. Other events never spawn.
    let auto_start = match &ctx.event {
        HookEvent::SessionStart { .. } => Some(AutoStart {
            self_exe: daemon_bin(),
            db: db_path(),
        }),
        _ => None,
    };

    let dedup = DedupCache::for_namespace(&namespace);

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

    let result = runtime.block_on(async {
        // Overall timeout guards the whole connect+capture phase.
        match tokio::time::timeout(
            OVERALL_TIMEOUT,
            capture_phase(&namespace, auto_start, &dedup, &ctx),
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

/// Connect (best-effort) and dispatch the event to its capture flow.
async fn capture_phase(
    namespace: &rb_types::Namespace,
    auto_start: Option<AutoStart>,
    dedup: &DedupCache,
    ctx: &rb_agents::HookContext,
) -> HookResult {
    let socket = socket_path();
    let mut client =
        DaemonClient::connect(&socket, namespace.clone(), CONNECT_TIMEOUT, auto_start).await;
    dispatch::dispatch(client.as_mut(), dedup, ctx).await
}

fn continue_result() -> HookResult {
    HookResult {
        system_message: None,
        continue_execution: true,
    }
}

/// Resolve the daemon socket path from `RUSTY_BRAIN_SOCKET`, else a temp default.
fn socket_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("RUSTY_BRAIN_SOCKET") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    default_runtime_dir().join("rusty-brain").join("sock")
}

/// Resolve the daemon db path from `RUSTY_BRAIN_DB`, else a data-dir default.
fn db_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("RUSTY_BRAIN_DB") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    default_data_dir().join("rusty-brain").join("memory.db")
}

fn default_runtime_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !d.is_empty() {
            return std::path::PathBuf::from(d);
        }
    }
    std::env::temp_dir()
}

fn default_data_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        if !d.is_empty() {
            return std::path::PathBuf::from(d);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            return std::path::PathBuf::from(home).join(".local").join("share");
        }
    }
    std::env::temp_dir()
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
}
