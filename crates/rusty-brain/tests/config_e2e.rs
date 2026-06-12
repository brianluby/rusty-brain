//! C1 end-to-end: `~/.config/rusty-brain/config.toml` (here: under an
//! isolated `XDG_CONFIG_HOME`) drives the real binaries.
//!
//! Two F20-class proofs against the actual auto-start path:
//!  1. socket/db paths set ONLY in the config file round-trip a
//!     remember→recall through the CLI and its auto-started daemon;
//!  2. an `idle_timeout_secs` set ONLY in the config file is honored by the
//!     AUTO-STARTED daemon — no env forwarding of the knob anywhere (the
//!     spawner env_clears; only XDG_CONFIG_HOME survives, and the daemon
//!     re-reads the file itself). The distinguishing observation: a 1s idle
//!     timeout closes an established connection that the 60s default would
//!     have kept alive.
//!
//! Plus the fail-closed contract: a malformed config file aborts the CLI with
//! a message naming the file.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::cargo::cargo_bin;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Write `text` as the config file under `confdir` (an XDG_CONFIG_HOME root).
fn write_config(confdir: &Path, text: &str) {
    let dir = confdir.join("rusty-brain");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), text).unwrap();
}

/// Best-effort SIGKILL of the daemon that owns `socket` (via its pidfile),
/// so tests do not leave orphan daemons behind.
fn kill_daemon(socket: &Path) {
    let pidfile = socket.with_extension("pid");
    if let Ok(pid) = std::fs::read_to_string(&pidfile) {
        let _ = Command::new("kill").args(["-9", pid.trim()]).status();
    }
}

/// A `rusty-brain` invocation with config-file-only daemon knobs: an isolated
/// XDG_CONFIG_HOME and every relevant override env var removed.
fn cli(confdir: &Path) -> Command {
    let mut cmd = Command::new(cargo_bin("rusty-brain"));
    cmd.env("XDG_CONFIG_HOME", confdir)
        .env_remove("RUSTY_BRAIN_SOCKET")
        .env_remove("RUSTY_BRAIN_DB")
        .env_remove("RUSTY_BRAIN_IDLE_TIMEOUT_SECS")
        .env_remove("RB_EMBED_BACKEND")
        .env_remove("RB_LOCAL_MODEL")
        .env_remove("VOYAGE_API_KEY") // force offline DeterministicProvider
        .env("RUST_LOG", "warn");
    cmd
}

// (1) Config-file socket/db agreement: the CLI resolves paths from the file,
// auto-starts a daemon, and a second CLI process recalls through the same
// file-resolved socket. No RUSTY_BRAIN_SOCKET/DB env anywhere.
#[test]
fn config_file_paths_drive_cli_and_auto_started_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let confdir = dir.path().join("xdg");
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    write_config(
        &confdir,
        &format!(
            "socket_path = \"{}\"\ndb_path = \"{}\"\nidle_timeout_secs = 30\n",
            socket.display(),
            db.display()
        ),
    );

    let remember = cli(&confdir)
        .args(["remember", "config file is the single source of truth"])
        .args(["--type", "architecture_decision", "--importance", "9"])
        .output()
        .expect("run remember");
    assert!(
        remember.status.success(),
        "remember failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&remember.stdout),
        String::from_utf8_lossy(&remember.stderr)
    );

    let recall = cli(&confdir)
        .args(["recall", "single source of truth", "--limit", "10"])
        .output()
        .expect("run recall");
    let stdout = String::from_utf8_lossy(&recall.stdout);
    let ok = recall.status.success() && stdout.contains("config file is the single source");

    // Both processes and the auto-started daemon agreed on the file paths:
    // the daemon bound the file socket and created the file db.
    let socket_exists = socket.exists();
    let db_exists = db.exists();
    kill_daemon(&socket);
    assert!(
        ok,
        "recall through the config-file socket failed: status={:?} stdout={stdout}",
        recall.status
    );
    assert!(
        socket_exists,
        "daemon must bind the config-file socket path"
    );
    assert!(db_exists, "daemon must create the config-file db path");
}

// (2) The F20-class retirement proof: idle_timeout_secs reaches the
// AUTO-STARTED daemon from the config file alone. The spawner env_clears the
// child and forwards no idle knob (it is not set in the parent); the daemon
// re-reads config.toml via the forwarded XDG_CONFIG_HOME. Under the 60s
// default the post-idle ping would succeed, so its failure is the proof that
// the 1s file value took effect.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_timeout_from_config_file_reaches_auto_started_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let confdir = dir.path().join("xdg");
    let socket = dir.path().join("runtime").join("rb.sock");
    let db = dir.path().join("rb.db");
    write_config(
        &confdir,
        &format!(
            "socket_path = \"{}\"\ndb_path = \"{}\"\nidle_timeout_secs = 1\n",
            socket.display(),
            db.display()
        ),
    );

    // Auto-start the daemon through a normal CLI command.
    let remember = cli(&confdir)
        .args(["remember", "idle timeout comes from the config file"])
        .output()
        .expect("run remember");
    assert!(
        remember.status.success(),
        "remember failed: stderr={:?}",
        String::from_utf8_lossy(&remember.stderr)
    );

    // Establish a raw protocol connection and let it idle past 1s.
    let result = async {
        let mut client =
            rb_proto::Client::connect(&socket, rb_types::Namespace::Project("config-e2e".into()))
                .await?;
        client.ping().await?;
        tokio::time::sleep(Duration::from_millis(2500)).await;
        client.ping().await
    }
    .await;
    kill_daemon(&socket);

    assert!(
        result.is_err(),
        "the auto-started daemon must close an idle connection after the \
         config-file 1s timeout (a success here means the file knob never \
         reached it and the 60s default applied): {result:?}"
    );
}

// (3) Fail closed: malformed TOML aborts any CLI command with a message that
// names the config file — never a silent fall-through to default paths.
#[test]
fn malformed_config_file_fails_the_cli_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let confdir = dir.path().join("xdg");
    write_config(&confdir, "socket_path = [broken");

    let out = cli(&confdir)
        .args(["recall", "anything"])
        .output()
        .expect("run recall");
    assert!(
        !out.status.success(),
        "a malformed config file must fail the command"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("config.toml"),
        "the error must name the config file; stderr: {stderr}"
    );
}
