//! Integration tests for the `rusty-brain-install` binary against fixture dirs.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;

fn bin() -> Command {
    Command::cargo_bin("rusty-brain-install").unwrap()
}

/// Create a fake `claude` executable inside `dir` so `ClaudeCodeInstaller::detect()`
/// resolves it on `PATH` (it scans `$PATH` for a file named `claude` via
/// `find_binary_on_path`). Without this, CI — where `claude` is absent — would
/// short-circuit to `NotFound` and SKIP the merge entirely, leaving the
/// round-trip assertions hollow. Returns the `PATH` string to hand to the child.
#[cfg(unix)]
fn fake_claude_path(dir: &std::path::Path) -> String {
    use std::os::unix::fs::PermissionsExt as _;
    let bin_path = dir.join("claude");
    std::fs::write(&bin_path, "#!/bin/sh\necho claude 1.0.0\n").unwrap();
    std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let existing = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), existing)
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

/// True if the settings JSON carries our injected sentinel hook block — an
/// entry that mentions both the `SENTINEL` marker key and the
/// `rusty-brain-hooks` command. Proves the merge actually ran.
#[cfg(unix)]
fn has_sentinel_block(settings: &serde_json::Value) -> bool {
    let text = settings.to_string();
    text.contains(rb_agents::install::SENTINEL) && text.contains("rusty-brain-hooks")
}

/// True if a `Stop` hook entry still carries the user's `user-linter` command.
#[cfg(unix)]
fn has_user_linter(settings: &serde_json::Value) -> bool {
    settings
        .get("hooks")
        .and_then(|h| h.get("Stop"))
        .and_then(|s| s.as_array())
        .map(|stop| {
            stop.iter().any(|g| {
                g.get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|a| {
                        a.iter().any(|e| {
                            e.get("command").and_then(|c| c.as_str()) == Some("user-linter")
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(unix)]
#[test]
fn install_then_status_then_uninstall_round_trip() {
    let dir = tempfile::tempdir().unwrap();

    // A fake `claude` on PATH so detect() succeeds and the engine actually runs
    // the merge — otherwise (e.g. in CI, where `claude` is absent) install would
    // short-circuit to NotFound and these assertions would pass without any
    // merge ever happening.
    let path_dir = tempfile::tempdir().unwrap();
    let test_path = fake_claude_path(path_dir.path());

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

    // Install for claude-code only. With the fake `claude` on PATH, detect()
    // succeeds and the merge truly runs against the on-disk settings.json.
    bin()
        .current_dir(dir.path())
        .env("PATH", &test_path)
        .args(["--json", "install", "--agents", "claude-code"])
        .assert()
        .success();

    let after_install: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    // The merge actually happened: our sentinel block is present...
    assert!(
        has_sentinel_block(&after_install),
        "settings.json must contain our sentinel hook block after install (proves the merge ran); got: {after_install}"
    );
    // ...and the user's pre-seeded hook + model survived the merge.
    assert!(
        has_user_linter(&after_install),
        "the user's Stop hook must survive install; got: {after_install}"
    );
    assert_eq!(
        after_install.get("model").unwrap(),
        &serde_json::json!("claude-opus")
    );

    // status runs and prints a report.
    bin()
        .current_dir(dir.path())
        .env("PATH", &test_path)
        .args(["--json", "status", "--agents", "claude-code"])
        .assert()
        .success()
        .stdout(contains("claude-code"));

    // uninstall removes only our entries; the user's hook + model survive.
    bin()
        .current_dir(dir.path())
        .env("PATH", &test_path)
        .args(["--json", "uninstall", "--agents", "claude-code"])
        .assert()
        .success();

    let after_uninstall: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    // Our sentinel block is gone...
    assert!(
        !has_sentinel_block(&after_uninstall),
        "our sentinel hook block must be removed after uninstall; got: {after_uninstall}"
    );
    // ...while the user's hook + model remain untouched.
    assert!(
        has_user_linter(&after_uninstall),
        "the user's Stop hook must survive uninstall; got: {after_uninstall}"
    );
    assert_eq!(
        after_uninstall.get("model").unwrap(),
        &serde_json::json!("claude-opus")
    );
}

#[cfg(unix)]
#[test]
fn default_global_operations_configure_only_supported_agents() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let path_dir = tempfile::tempdir().unwrap();
    fake_claude_path(path_dir.path());
    // Include OMP on PATH: implicit global selection must skip it even when
    // installed. Restrict PATH to the fixtures, not the host's real CLIs.
    for name in ["gemini", "codex", "omp"] {
        std::fs::copy(path_dir.path().join("claude"), path_dir.path().join(name)).unwrap();
    }

    for (operation, expected_status) in [
        ("install", "configured"),
        ("status", "present"),
        ("uninstall", "removed"),
        ("status", "absent"),
    ] {
        let output = bin()
            .current_dir(project.path())
            .env("HOME", home.path())
            .env("PATH", path_dir.path())
            .args(["--json", operation, "--global"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
        let agents = report["agents"].as_array().unwrap();
        let ids: Vec<_> = agents
            .iter()
            .map(|agent| agent["agent"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["claude-code", "gemini", "codex"]);
        for agent in agents {
            assert_eq!(agent["status"], expected_status, "{report}");
            assert!(agent["error"].is_null(), "{report}");
        }
        for config in [
            ".claude/settings.json",
            ".gemini/settings.json",
            ".codex/hooks.json",
        ] {
            let settings: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(home.path().join(config)).unwrap())
                    .unwrap();
            assert_eq!(
                has_sentinel_block(&settings),
                matches!(expected_status, "configured" | "present")
            );
            assert!(!project.path().join(config).exists());
        }
        assert!(!home.path().join(".omp").exists());
        assert!(!project.path().join(".omp").exists());
    }
}

#[test]
fn explicit_global_omp_is_rejected_even_when_not_installed() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let empty_path = tempfile::tempdir().unwrap();
    for args in [
        vec!["install", "--dry-run"],
        vec!["install"],
        vec!["uninstall", "--dry-run"],
        vec!["uninstall"],
        vec!["status"],
    ] {
        let output = bin()
            .current_dir(project.path())
            .env("HOME", home.path())
            .env("PATH", empty_path.path())
            .args(args)
            .args(["--json", "--global", "--agents", "omp"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let report: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["status"], "failed", "{report}");
        let agents = report["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["agent"], "omp");
        assert_eq!(agents[0]["status"], "failed");
        assert!(agents[0]["version"].is_null());
        assert!(agents[0]["config_path"].is_null());
        assert!(agents[0]["error"]
            .as_str()
            .unwrap()
            .contains("[E_INSTALL_SCOPE_UNSUPPORTED]"));
    }
    assert!(!home.path().join(".omp").exists());
    assert!(!project.path().join(".omp").exists());
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
