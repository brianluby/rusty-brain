//! CLI surface tests: help, version, and argument-validation exit codes.
//! These never start the daemon (parser-only paths).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("rusty-brain").unwrap()
}

#[test]
fn help_lists_all_subcommands() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains("remember"))
        .stdout(predicate::str::contains("recall"))
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("graph"))
        .stdout(predicate::str::contains("delete"))
        .stdout(predicate::str::contains("context"))
        .stdout(predicate::str::contains("status"));
}

#[test]
fn version_prints_a_version() {
    bin()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("rusty-brain"));
}

#[test]
fn unknown_subcommand_fails_with_nonzero_exit() {
    bin().arg("frobnicate").assert().failure();
}

#[test]
fn remember_requires_content_argument() {
    bin()
        .arg("remember")
        .assert()
        .failure()
        .stderr(predicate::str::contains("content").or(predicate::str::contains("required")));
}

#[test]
fn remember_rejects_invalid_memory_type() {
    bin()
        .args(["remember", "some content", "--type", "not_a_type"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid").or(predicate::str::contains("not_a_type")));
}

#[test]
fn recall_help_shows_flags() {
    bin()
        .args(["recall", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--limit"))
        .stdout(predicate::str::contains("--type"))
        .stdout(predicate::str::contains("--tags"));
}
