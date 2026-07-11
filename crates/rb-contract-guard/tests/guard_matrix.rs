//! End-to-end pass/fail matrix for the contract-drift guard CLI, exercised
//! against fixture repos that mirror the real surface paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::Command;
use std::fs;
use std::path::Path;

const MESSAGES: &str = "\
pub const CONTRACT_VERSION: u32 = 2;

pub struct Handshake {
    pub contract_version: u32,
}

pub enum Request {
    Ping,
    Get { id: String },
}

pub enum Response {
    Pong,
    Error { message: String },
}
";

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn edit(root: &Path, rel: &str, from: &str, to: &str) {
    let path = root.join(rel);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "fixture edit anchor missing: {from}");
    fs::write(&path, text.replace(from, to)).unwrap();
}

/// A fixture repo laid out on the same relative paths as the real one.
fn fixture_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "crates/rb-proto/src/messages.rs", MESSAGES);
    for f in [
        "change.rs",
        "feedback_kind.rs",
        "job.rs",
        "link.rs",
        "link_type.rs",
        "memory_id.rs",
        "memory_type.rs",
        "namespace.rs",
        "query.rs",
    ] {
        write(root, &format!("crates/rb-types/src/{f}"), "");
    }
    write(
        root,
        "crates/rb-types/src/memory.rs",
        "pub struct MemoryNote {\n    pub content: String,\n}\n",
    );
    write(
        root,
        "crates/rb-store/migrations/001_init.sql",
        "CREATE TABLE memories (id TEXT PRIMARY KEY);\n",
    );
    dir
}

fn guard(root: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut cmd = Command::cargo_bin("rb-contract-guard").unwrap();
    cmd.arg("--root").arg(root).args(args);
    cmd.assert()
}

fn init(root: &Path) {
    guard(root, &["init", "--note", "baseline"]).success();
}

fn check_ok(root: &Path) {
    guard(root, &["check"]).success();
}

fn check_drifts(root: &Path, expect_in_output: &[&str]) {
    let assert = guard(root, &["check"]).code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stderr).to_string()
        + &String::from_utf8_lossy(&assert.get_output().stdout);
    for needle in expect_in_output {
        assert!(
            out.contains(needle),
            "check output should mention {needle:?}, got:\n{out}"
        );
    }
}

#[test]
fn init_then_check_passes_and_snapshot_is_written() {
    let repo = fixture_repo();
    init(repo.path());
    assert!(repo.path().join("contract-snapshot.toml").is_file());
    check_ok(repo.path());
}

#[test]
fn check_without_snapshot_fails_with_init_guidance() {
    let repo = fixture_repo();
    let assert = guard(repo.path(), &["check"]).code(2);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(err.contains("init"), "guidance points at init, got:\n{err}");
}

#[test]
fn comment_only_and_unrelated_changes_pass_without_a_marker() {
    let repo = fixture_repo();
    init(repo.path());
    edit(
        repo.path(),
        "crates/rb-proto/src/messages.rs",
        "pub struct Handshake {",
        "/// New documentation, still the same wire shape.\npub struct Handshake {",
    );
    write(
        repo.path(),
        "crates/rb-engine/src/lib.rs",
        "pub fn unrelated() {}\n",
    );
    check_ok(repo.path());
}

#[test]
fn silent_shape_change_fails_naming_the_item_and_the_marker_path() {
    let repo = fixture_repo();
    init(repo.path());
    edit(
        repo.path(),
        "crates/rb-proto/src/messages.rs",
        "    Ping,",
        "    Ping,\n    Stats,",
    );
    check_drifts(
        repo.path(),
        &[
            "crates/rb-proto/src/messages.rs::Request",
            "--intent additive",
            "--intent breaking",
        ],
    );
}

#[test]
fn additive_marker_unblocks_an_additive_change() {
    let repo = fixture_repo();
    init(repo.path());
    edit(
        repo.path(),
        "crates/rb-proto/src/messages.rs",
        "    Ping,",
        "    Ping,\n    Stats,",
    );
    guard(
        repo.path(),
        &[
            "update",
            "--intent",
            "additive",
            "--note",
            "Request::Stats variant",
        ],
    )
    .success();
    check_ok(repo.path());
    let snap = fs::read_to_string(repo.path().join("contract-snapshot.toml")).unwrap();
    assert!(
        snap.contains("Request::Stats variant"),
        "the note is the reviewable marker:\n{snap}"
    );
}

#[test]
fn rb_types_payload_shape_change_is_also_guarded() {
    let repo = fixture_repo();
    init(repo.path());
    edit(
        repo.path(),
        "crates/rb-types/src/memory.rs",
        "pub content: String,",
        "pub content: String,\n    pub contested: bool,",
    );
    check_drifts(repo.path(), &["crates/rb-types/src/memory.rs::MemoryNote"]);
}

#[test]
fn silent_migration_addition_fails_then_marker_unblocks() {
    let repo = fixture_repo();
    init(repo.path());
    write(
        repo.path(),
        "crates/rb-store/migrations/002_add_column.sql",
        "ALTER TABLE memories ADD COLUMN contested INTEGER;\n",
    );
    check_drifts(repo.path(), &["002_add_column.sql"]);
    guard(
        repo.path(),
        &[
            "update",
            "--intent",
            "additive",
            "--note",
            "add contested column",
        ],
    )
    .success();
    check_ok(repo.path());
}

#[test]
fn silent_version_bump_fails_and_needs_a_breaking_marker() {
    let repo = fixture_repo();
    init(repo.path());
    edit(
        repo.path(),
        "crates/rb-proto/src/messages.rs",
        "pub const CONTRACT_VERSION: u32 = 2;",
        "pub const CONTRACT_VERSION: u32 = 3;",
    );
    check_drifts(repo.path(), &["CONTRACT_VERSION"]);
    guard(
        repo.path(),
        &["update", "--intent", "breaking", "--note", "v3 handshake"],
    )
    .success();
    check_ok(repo.path());
}

#[test]
fn breaking_change_cannot_hide_behind_an_additive_marker() {
    let repo = fixture_repo();
    init(repo.path());
    edit(
        repo.path(),
        "crates/rb-proto/src/messages.rs",
        "pub const CONTRACT_VERSION: u32 = 2;",
        "pub const CONTRACT_VERSION: u32 = 3;",
    );
    edit(
        repo.path(),
        "crates/rb-proto/src/messages.rs",
        "Error { message: String },",
        "Error { kind: String, message: String },",
    );
    let assert = guard(
        repo.path(),
        &["update", "--intent", "additive", "--note", "sneaky"],
    )
    .code(2);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        err.contains("breaking"),
        "additive marker with a bump must be rejected, got:\n{err}"
    );
    // The guard stays red until the right marker is recorded.
    check_drifts(repo.path(), &["Response"]);
    guard(
        repo.path(),
        &[
            "update",
            "--intent",
            "breaking",
            "--note",
            "Error carries kind",
        ],
    )
    .success();
    check_ok(repo.path());
}

#[test]
fn breaking_marker_without_a_bump_is_rejected() {
    let repo = fixture_repo();
    init(repo.path());
    edit(
        repo.path(),
        "crates/rb-proto/src/messages.rs",
        "Error { message: String },",
        "Error { kind: String, message: String },",
    );
    let assert = guard(
        repo.path(),
        &[
            "update",
            "--intent",
            "breaking",
            "--note",
            "forgot the bump",
        ],
    )
    .code(2);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(
        err.contains("bump"),
        "breaking without a bump must demand one, got:\n{err}"
    );
}

#[test]
fn init_twice_is_rejected() {
    let repo = fixture_repo();
    init(repo.path());
    guard(repo.path(), &["init", "--note", "again"]).code(2);
}

#[test]
fn update_without_snapshot_points_at_init() {
    let repo = fixture_repo();
    let assert = guard(
        repo.path(),
        &[
            "update",
            "--intent",
            "additive",
            "--note",
            "no baseline yet",
        ],
    )
    .code(2);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(err.contains("init"), "guidance points at init, got:\n{err}");
}
