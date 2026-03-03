//! Shared integration test utilities for the CLI.

use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

use rusty_brain_core::mind::Mind;
use types::{MindConfig, ObservationType};

/// A test observation to store via `Mind::remember`.
pub struct TestObs {
    pub obs_type: ObservationType,
    pub tool_name: String,
    pub summary: String,
    pub content: Option<String>,
}

/// Create a temporary mind with the given observations.
///
/// Returns `(TempDir, PathBuf)` — caller must hold the `TempDir` guard so the
/// temp directory is not deleted prematurely. The `PathBuf` points to the
/// `.mv2` file to pass via `--memory-path`.
pub fn setup_test_mind(observations: &[TestObs]) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("test.mv2");

    let config = MindConfig {
        memory_path: path.clone(),
        ..MindConfig::default()
    };
    let mind = Mind::open(config).expect("failed to open mind");

    for obs in observations {
        mind.remember(
            obs.obs_type,
            &obs.tool_name,
            &obs.summary,
            obs.content.as_deref(),
            None,
        )
        .expect("failed to remember observation");
    }

    (dir, path)
}

/// Build a `Command` for the `rusty-brain` CLI binary.
#[allow(dead_code)]
pub fn cli_cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rusty-brain"))
}

/// Run a CLI command with the given args and memory path.
/// Returns (exit_status, stdout, stderr).
#[allow(dead_code)]
pub fn run_cli(memory_path: &PathBuf, args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let output = cli_cmd()
        .arg("--memory-path")
        .arg(memory_path)
        .args(args)
        .output()
        .expect("failed to execute CLI");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    (output.status, stdout, stderr)
}
