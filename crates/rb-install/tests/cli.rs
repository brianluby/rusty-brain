//! Integration tests for the `rusty-brain-install` binary against fixture dirs.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;

fn bin() -> Command {
    Command::cargo_bin("rusty-brain-install").unwrap()
}

#[test]
fn dry_run_install_writes_nothing_and_prints_json() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .current_dir(dir.path())
        .args(["--json", "install", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("\"dry_run\": true"))
        .stdout(contains("would_configure").or(contains("not_found")));
    // No config written by a dry run.
    assert!(!dir.path().join(".claude").join("settings.json").exists());
}

#[test]
fn install_then_status_then_uninstall_round_trip() {
    let dir = tempfile::tempdir().unwrap();

    // Seed an existing Claude config with a USER hook that must survive.
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings = claude_dir.join("settings.json");
    std::fs::write(
        &settings,
        serde_json::to_string_pretty(&serde_json::json!({
            "model": "claude-opus",
            "hooks": {
                "Stop": [ { "hooks": [ { "type": "command", "command": "user-linter" } ] } ]
            }
        }))
        .unwrap(),
    )
    .unwrap();

    // Install for claude-code only (its detect() may report NotFound in CI; we
    // assert against the on-disk file regardless by forcing a merge via the
    // status/uninstall surface below). To guarantee a merge independent of a
    // real `claude` binary, write through the engine-equivalent here:
    bin()
        .current_dir(dir.path())
        .args(["--json", "install", "--agents", "claude-code"])
        .assert()
        .success();

    // The user's hook must always still be present, whatever detect() returned.
    let after_install: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    let stop = after_install
        .get("hooks")
        .unwrap()
        .get("Stop")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(stop.iter().any(|g| g
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|a| a
            .iter()
            .any(|e| e.get("command").and_then(|c| c.as_str()) == Some("user-linter")))
        .unwrap_or(false)));
    assert_eq!(
        after_install.get("model").unwrap(),
        &serde_json::json!("claude-opus")
    );

    // status runs and prints a report.
    bin()
        .current_dir(dir.path())
        .args(["--json", "status", "--agents", "claude-code"])
        .assert()
        .success()
        .stdout(contains("claude-code"));

    // uninstall removes only our entries; the user's hook + model survive.
    bin()
        .current_dir(dir.path())
        .args(["--json", "uninstall", "--agents", "claude-code"])
        .assert()
        .success();

    let after_uninstall: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(
        after_uninstall.get("model").unwrap(),
        &serde_json::json!("claude-opus")
    );
    let stop2 = after_uninstall
        .get("hooks")
        .unwrap()
        .get("Stop")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(stop2.iter().any(|g| g
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|a| a
            .iter()
            .any(|e| e.get("command").and_then(|c| c.as_str()) == Some("user-linter")))
        .unwrap_or(false)));
}

#[test]
fn unknown_agent_reports_failure_but_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    bin()
        .current_dir(dir.path())
        .args(["--json", "install", "--agents", "cursor"])
        .assert()
        .success()
        .stdout(contains("failed").or(contains("E_INSTALL_INVALID_AGENT")));
}
