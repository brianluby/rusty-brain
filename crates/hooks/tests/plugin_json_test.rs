//! T047: Plugin JSON structure validation tests.
//!
//! Verifies that `plugin.json` and `hooks.json` in the packaging directory have
//! the correct structure with hook paths pointing to Rust binaries (not JS files)
//! per FR-014.

use std::path::{Path, PathBuf};

/// Resolve the workspace root from the crate's manifest dir.
fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/hooks -> workspace root is two levels up
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root should exist")
        .to_path_buf()
}

fn read_plugin_json() -> serde_json::Value {
    let path = workspace_root().join("packaging/claude-code/.claude-plugin/plugin.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read plugin.json at {}: {e}", path.display()));
    serde_json::from_str(&content).expect("plugin.json must be valid JSON")
}

fn read_hooks_json() -> serde_json::Value {
    let path = workspace_root().join("packaging/claude-code/hooks/hooks.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read hooks.json at {}: {e}", path.display()));
    serde_json::from_str(&content).expect("hooks.json must be valid JSON")
}

// ===========================================================================
// plugin.json structure validation
// ===========================================================================

#[test]
fn plugin_json_has_required_top_level_fields() {
    let plugin = read_plugin_json();

    assert!(
        plugin.get("name").is_some(),
        "plugin.json must have 'name' field"
    );
    assert!(
        plugin.get("version").is_some(),
        "plugin.json must have 'version' field"
    );
    assert!(
        plugin.get("description").is_some(),
        "plugin.json must have 'description' field"
    );
    assert!(
        plugin.get("hooks").is_some(),
        "plugin.json must have 'hooks' field"
    );
}

#[test]
fn plugin_json_hooks_field_references_hooks_json() {
    let plugin = read_plugin_json();
    let hooks_ref = plugin
        .get("hooks")
        .and_then(|v| v.as_str())
        .expect("hooks field must be a string path");

    assert!(
        hooks_ref.contains("hooks.json"),
        "hooks field should reference hooks.json, got: {hooks_ref}"
    );
    assert!(
        !hooks_ref.contains("dist/"),
        "hooks field must NOT reference dist/ directory (JS paths): {hooks_ref}"
    );
    assert!(
        Path::new(hooks_ref)
            .extension()
            .is_none_or(|ext| ext != "js"),
        "hooks field must NOT reference .js files: {hooks_ref}"
    );
}

#[test]
fn plugin_json_name_is_rusty_brain() {
    let plugin = read_plugin_json();
    let name = plugin
        .get("name")
        .and_then(|v| v.as_str())
        .expect("name must be a string");
    assert_eq!(name, "rusty-brain", "plugin name should be rusty-brain");
}

#[test]
fn plugin_json_has_valid_semver_version() {
    let plugin = read_plugin_json();
    let version = plugin
        .get("version")
        .and_then(|v| v.as_str())
        .expect("version must be a string");

    // Basic semver format check: digits.digits.digits
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "version should be semver (x.y.z), got: {version}"
    );
    for part in &parts {
        assert!(
            part.parse::<u32>().is_ok(),
            "version component '{part}' should be numeric in: {version}"
        );
    }
}

// ===========================================================================
// hooks.json structure validation — Rust binary paths (FR-014)
// ===========================================================================

#[test]
fn hooks_json_has_hooks_object() {
    let hooks = read_hooks_json();
    assert!(
        hooks.get("hooks").is_some(),
        "hooks.json must have top-level 'hooks' object"
    );
    assert!(
        hooks["hooks"].is_object(),
        "hooks.json 'hooks' field must be an object"
    );
}

#[test]
fn hooks_json_defines_required_hook_types() {
    let hooks = read_hooks_json();
    let hook_map = hooks.get("hooks").expect("hooks object");

    let required_types = ["SessionStart", "PostToolUse", "Stop"];
    for hook_type in &required_types {
        assert!(
            hook_map.get(*hook_type).is_some(),
            "hooks.json must define '{hook_type}' hook type"
        );
    }
}

#[test]
fn hooks_json_commands_point_to_rust_binary() {
    let hooks = read_hooks_json();
    let hook_map = hooks
        .get("hooks")
        .expect("hooks object")
        .as_object()
        .unwrap();

    for (event_type, entries) in hook_map {
        let entries = entries
            .as_array()
            .unwrap_or_else(|| panic!("{event_type} should be an array"));

        for entry in entries {
            // hooks.json may have nested "hooks" arrays or flat command entries
            let commands = if let Some(inner_hooks) = entry.get("hooks") {
                inner_hooks
                    .as_array()
                    .unwrap_or_else(|| panic!("{event_type} inner hooks should be an array"))
                    .clone()
            } else {
                vec![entry.clone()]
            };

            for cmd_entry in &commands {
                let cmd_type = cmd_entry
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("{event_type} entry must have 'type' field"));

                assert_eq!(
                    cmd_type, "command",
                    "{event_type} hook type should be 'command', not a JS module reference"
                );

                let command = cmd_entry
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("{event_type} entry must have 'command' field"));

                // Verify it references a Rust binary, not JS
                assert!(
                    !command.contains(".js"),
                    "{event_type} command must NOT reference .js files (FR-014): {command}"
                );
                assert!(
                    !command.contains("node "),
                    "{event_type} command must NOT invoke node (FR-014): {command}"
                );
                assert!(
                    !command.contains("dist/"),
                    "{event_type} command must NOT reference dist/ directory (FR-014): {command}"
                );
                assert!(
                    command.contains("rusty-brain"),
                    "{event_type} command should reference rusty-brain binary: {command}"
                );
            }
        }
    }
}

#[test]
fn hooks_json_session_start_uses_correct_subcommand() {
    let hooks = read_hooks_json();
    let session_start = &hooks["hooks"]["SessionStart"];
    let entries = session_start.as_array().expect("SessionStart array");

    let has_session_start_cmd = entries.iter().any(|entry| {
        // Check nested hooks array structure
        if let Some(inner) = entry.get("hooks") {
            inner.as_array().is_some_and(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|v| v.as_str())
                        .is_some_and(|cmd| cmd.contains("session-start"))
                })
            })
        } else {
            entry
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(|cmd| cmd.contains("session-start"))
        }
    });

    assert!(
        has_session_start_cmd,
        "SessionStart hook must use 'session-start' subcommand"
    );
}

#[test]
fn hooks_json_post_tool_use_uses_correct_subcommand() {
    let hooks = read_hooks_json();
    let post_tool = &hooks["hooks"]["PostToolUse"];
    let entries = post_tool.as_array().expect("PostToolUse array");

    let has_post_tool_cmd = entries.iter().any(|entry| {
        if let Some(inner) = entry.get("hooks") {
            inner.as_array().is_some_and(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|v| v.as_str())
                        .is_some_and(|cmd| cmd.contains("post-tool-use"))
                })
            })
        } else {
            entry
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(|cmd| cmd.contains("post-tool-use"))
        }
    });

    assert!(
        has_post_tool_cmd,
        "PostToolUse hook must use 'post-tool-use' subcommand"
    );
}

#[test]
fn hooks_json_stop_uses_correct_subcommand() {
    let hooks = read_hooks_json();
    let stop = &hooks["hooks"]["Stop"];
    let entries = stop.as_array().expect("Stop array");

    let has_stop_cmd = entries.iter().any(|entry| {
        if let Some(inner) = entry.get("hooks") {
            inner.as_array().is_some_and(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|v| v.as_str())
                        .is_some_and(|cmd| cmd.contains("stop"))
                })
            })
        } else {
            entry
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(|cmd| cmd.contains("stop"))
        }
    });

    assert!(has_stop_cmd, "Stop hook must use 'stop' subcommand");
}

// ===========================================================================
// Generated manifest consistency with static hooks.json
// ===========================================================================

#[test]
fn generated_manifest_hook_types_match_static_hooks_json() {
    let static_hooks = read_hooks_json();
    let static_types: Vec<String> = static_hooks["hooks"]
        .as_object()
        .expect("hooks object")
        .keys()
        .cloned()
        .collect();

    let generated = hooks::manifest::generate_manifest("rusty-brain-hooks");
    let gen_parsed: serde_json::Value = serde_json::from_str(&generated).unwrap();
    let gen_types: Vec<String> = gen_parsed["hooks"]
        .as_object()
        .expect("generated hooks object")
        .keys()
        .cloned()
        .collect();

    // Static hooks.json should contain at least the core hook types from
    // the generated manifest (generated may have additional like Notification)
    for hook_type in &static_types {
        assert!(
            gen_types.contains(hook_type),
            "static hooks.json defines '{hook_type}' which should also be in generated manifest. \
             Generated types: {gen_types:?}"
        );
    }
}

#[test]
fn hooks_json_commands_use_plugin_root_variable() {
    let hooks = read_hooks_json();
    let hook_map = hooks["hooks"].as_object().expect("hooks object");

    for (event_type, entries) in hook_map {
        let entries = entries
            .as_array()
            .unwrap_or_else(|| panic!("{event_type} should be an array"));

        for entry in entries {
            let commands = if let Some(inner_hooks) = entry.get("hooks") {
                inner_hooks
                    .as_array()
                    .unwrap_or_else(|| panic!("{event_type} inner hooks should be an array"))
                    .clone()
            } else {
                vec![entry.clone()]
            };

            for cmd_entry in &commands {
                let command = cmd_entry
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("{event_type} entry must have 'command' field"));

                assert!(
                    command.contains("${CLAUDE_PLUGIN_ROOT}"),
                    "{event_type} command should use ${{CLAUDE_PLUGIN_ROOT}} variable \
                     for portable plugin paths: {command}"
                );
            }
        }
    }
}
