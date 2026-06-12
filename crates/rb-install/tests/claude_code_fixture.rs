//! W0.7 (F50): recorded-fixture verification of the Claude Code hook form.
//!
//! `fixtures/claude_code_settings.json` is a realistic pre-existing user
//! `settings.json` recorded from a real Claude Code installation shape: other
//! hooks (with matchers and a `timeout`), a `permissions` block, `env`,
//! `statusLine`. The documented hooks schema takes `command` as ONE
//! shell-interpreted string — there is no `args` field — so these tests pin:
//! (a) install preserves every unrelated entry and emits only documented-shape
//! entries, (b) uninstall restores the fixture semantically, and (c) the
//! emitted command string round-trips through real shell parsing back to our
//! binary + flags.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use rb_agents::install::{AgentInstaller, InstallScope, SENTINEL};
use rb_install::installers::ClaudeCodeInstaller;
use rb_install::uninstall::uninstall_file;
use rb_install::writer::merge_into_file;

/// The four Claude Code events the installer writes (mirrors `CLAUDE_EVENTS`).
const OUR_EVENTS: [&str; 4] = ["SessionStart", "PostToolUse", "Stop", "PreCompact"];

/// Parse the recorded fixture.
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/claude_code_settings.json"))
        .expect("fixture must be valid JSON")
}

/// Materialize the fixture at `<project>/.claude/settings.json` and return the
/// settings path.
fn write_fixture(project: &Path) -> std::path::PathBuf {
    let dir = project.join(".claude");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(&path, include_str!("fixtures/claude_code_settings.json")).unwrap();
    path
}

/// Install our Claude Code fragment for `hooks_bin` into the fixture on disk;
/// returns the settings path and the merged value.
fn install_into_fixture(
    project: &Path,
    hooks_bin: &Path,
) -> (std::path::PathBuf, serde_json::Value) {
    let path = write_fixture(project);
    let frag = ClaudeCodeInstaller
        .hook_fragment(hooks_bin, &InstallScope::Project(project.to_path_buf()))
        .unwrap();
    assert_eq!(
        frag.config_path, path,
        "fragment must target the fixture file"
    );
    let merged = merge_into_file(&path, &frag.merge).unwrap();
    (path, merged)
}

/// Split an event's groups into (user's, ours) by the group-level sentinel.
fn split_groups(
    merged: &serde_json::Value,
    event: &str,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let groups = merged
        .get("hooks")
        .unwrap()
        .get(event)
        .unwrap()
        .as_array()
        .unwrap();
    groups
        .iter()
        .cloned()
        .partition(|g| g.get(SENTINEL).is_none())
}

/// True if any object anywhere in `value` carries an `args` key.
fn contains_args_key(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key("args") || map.values().any(contains_args_key)
        }
        serde_json::Value::Array(items) => items.iter().any(contains_args_key),
        _ => false,
    }
}

#[test]
fn install_preserves_user_settings_and_emits_documented_shape() {
    let original = fixture();
    let dir = tempfile::tempdir().unwrap();
    let (_path, merged) =
        install_into_fixture(dir.path(), Path::new("/usr/local/bin/rusty-brain-hooks"));

    // Unrelated top-level entries survive byte-for-byte semantically.
    for key in ["model", "permissions", "env", "statusLine", "theme"] {
        assert_eq!(
            merged.get(key),
            original.get(key),
            "user `{key}` entry must survive install unchanged"
        );
    }
    // PreToolUse is not an event we install — the user's groups are untouched.
    assert_eq!(
        merged.get("hooks").unwrap().get("PreToolUse"),
        original.get("hooks").unwrap().get("PreToolUse"),
        "user PreToolUse hooks must survive install unchanged"
    );

    for event in OUR_EVENTS {
        let (user_groups, our_groups) = split_groups(&merged, event);
        // The user's pre-existing groups for this event survive unchanged.
        let original_groups = original
            .get("hooks")
            .unwrap()
            .get(event)
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        assert_eq!(
            user_groups, original_groups,
            "user {event} groups must survive install unchanged"
        );
        // Exactly one group of ours, with exactly one documented-shape entry:
        // {"type": "command", "command": "<shell string>"} — no `args` key, no
        // unknown keys (the sentinel lives on the group only).
        assert_eq!(our_groups.len(), 1, "exactly one of our groups for {event}");
        let inner = our_groups[0].get("hooks").unwrap().as_array().unwrap();
        assert_eq!(inner.len(), 1);
        let entry = inner[0].as_object().unwrap();
        let keys: Vec<&str> = entry.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["command", "type"],
            "{event} entry must have exactly the documented keys"
        );
        assert_eq!(entry.get("type").unwrap(), &serde_json::json!("command"));
        let cmd = entry.get("command").unwrap().as_str().unwrap();
        assert!(
            cmd.contains("rusty-brain-hooks") && cmd.ends_with("--agent claude-code"),
            "{event} command must be one shell string carrying the agent flag; got {cmd}"
        );
    }

    // The documented schema has no `args` field anywhere; neither the fixture
    // nor our injected entries may introduce one.
    assert!(
        !contains_args_key(&merged),
        "no `args` key anywhere after install"
    );
}

#[test]
fn uninstall_restores_fixture_semantically() {
    let original = fixture();
    let dir = tempfile::tempdir().unwrap();
    let (path, merged) =
        install_into_fixture(dir.path(), Path::new("/usr/local/bin/rusty-brain-hooks"));
    assert_ne!(merged, original, "install must have changed the settings");

    uninstall_file(&path).unwrap();
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        after, original,
        "uninstall must restore the recorded fixture semantically"
    );
}

/// Strongest form check: a real `/bin/sh` parses the emitted `command` string
/// back to OUR binary (its path containing a space and a single quote) plus
/// exactly `--agent claude-code` — precedent: `shell_quote_roundtrips_through_sh`
/// in `installers::tests`.
#[cfg(unix)]
#[test]
fn emitted_command_round_trips_through_shell_to_binary_and_flags() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("jo bloggs's tools");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let hooks_bin = bin_dir.join("rusty-brain-hooks");
    // Stand-in binary that prints each argv element on its own line.
    std::fs::write(&hooks_bin, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    std::fs::set_permissions(&hooks_bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let frag = ClaudeCodeInstaller
        .hook_fragment(&hooks_bin, &InstallScope::Project(dir.path().to_path_buf()))
        .unwrap();
    let (_, our_groups) = split_groups(&frag.merge, "PostToolUse");
    let cmd = our_groups[0].get("hooks").unwrap().as_array().unwrap()[0]
        .get("command")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    // Claude Code runs `command` through a shell; do exactly that.
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .expect("run the emitted command through /bin/sh");
    assert!(
        out.status.success(),
        "shell must reach the binary despite space+quote in its path; cmd={cmd:?} stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "--agent\nclaude-code\n",
        "the binary must receive exactly the agent flag arguments"
    );
}
