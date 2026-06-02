//! Sentinel-only stripping: remove exactly our injected entries, leave the
//! user's keys and hooks untouched. Removal *is* the restore — no `.bak` needed.

use std::path::Path;

use rb_agents::install::SENTINEL;
use rb_types::{Error, Result};

use crate::writer::{read_config, write};

/// True if `value` carries our sentinel marker (`{SENTINEL: true}`).
fn is_sentinel(value: &serde_json::Value) -> bool {
    value
        .get(SENTINEL)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Recursively strip every sentinel-marked element from `value`.
///
/// Thin wrapper over [`strip_sentinel_tracked`] that discards the
/// removed-sentinel flag; the public contract is the cleaned value.
#[must_use]
pub fn strip_sentinel(value: serde_json::Value) -> serde_json::Value {
    strip_sentinel_tracked(value).0
}

/// Strip sentinel-marked entries, returning the cleaned value and whether any
/// sentinel-marked entry was actually removed beneath (or at) this value.
///
/// In arrays, sentinel-marked elements are dropped. In objects, the `SENTINEL`
/// key itself is removed and each remaining value is stripped recursively. A
/// container that became empty is pruned ONLY when a sentinel was actually
/// removed beneath it — so a user's deliberately-empty `{}`/`[]` that we never
/// touched survives uninstall verbatim.
#[must_use]
fn strip_sentinel_tracked(value: serde_json::Value) -> (serde_json::Value, bool) {
    match value {
        serde_json::Value::Array(items) => {
            let mut removed = false;
            let mut cleaned = Vec::with_capacity(items.len());
            for e in items {
                if is_sentinel(&e) {
                    removed = true;
                    continue;
                }
                let (child, child_removed) = strip_sentinel_tracked(e);
                removed |= child_removed;
                cleaned.push(child);
            }
            (serde_json::Value::Array(cleaned), removed)
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut removed = false;
            for (k, v) in map {
                if k == SENTINEL {
                    removed = true;
                    continue;
                }
                let (cleaned, child_removed) = strip_sentinel_tracked(v);
                removed |= child_removed;
                // Prune a container ONLY when our removal is what emptied it.
                let emptied_by_us = child_removed
                    && match &cleaned {
                        serde_json::Value::Array(a) => a.is_empty(),
                        serde_json::Value::Object(o) => o.is_empty(),
                        _ => false,
                    };
                if !emptied_by_us {
                    out.insert(k, cleaned);
                }
            }
            (serde_json::Value::Object(out), removed)
        }
        other => (other, false),
    }
}

/// Strip our entries from the config at `path` and write the result atomically.
///
/// If `path` does not exist, this is a no-op success. No `.bak` is written —
/// uninstall is itself the inverse of install.
///
/// # Errors
/// Returns [`Error::Io`]/[`Error::Serialization`] on read/parse/write failure.
pub fn uninstall_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let current = read_config(path)?;
    let cleaned = strip_sentinel(current);
    let body =
        serde_json::to_string_pretty(&cleaned).map_err(|e| Error::Serialization(e.to_string()))?;
    write(path, &body, false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::installers::ClaudeCodeInstaller;
    use crate::writer::{merge_into_file, merge_value};
    use rb_agents::install::{AgentInstaller, InstallScope};

    fn claude_fragment(root: &Path) -> serde_json::Value {
        ClaudeCodeInstaller
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Project(root.to_path_buf()),
            )
            .unwrap()
            .merge
    }

    #[test]
    fn install_then_uninstall_round_trips_to_original() {
        let original = serde_json::json!({
            "model": "claude-opus",
            "hooks": {
                "SessionStart": [
                    { "hooks": [ { "type": "command", "command": "user-tool" } ] }
                ]
            }
        });
        let frag = claude_fragment(Path::new("/tmp/p"));
        let installed = merge_value(original.clone(), &frag);
        // Our entries are present after install.
        assert_eq!(
            installed
                .get("hooks")
                .unwrap()
                .get("SessionStart")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let stripped = strip_sentinel(installed);
        assert_eq!(
            stripped, original,
            "uninstall must restore the pre-install value"
        );
    }

    #[test]
    fn uninstall_leaves_pure_user_config_untouched() {
        let user_only = serde_json::json!({
            "theme": "dark",
            "hooks": {
                "PostToolUse": [
                    { "matcher": "*", "hooks": [ { "type": "command", "command": "their-linter" } ] }
                ]
            }
        });
        let stripped = strip_sentinel(user_only.clone());
        assert_eq!(stripped, user_only);
    }

    #[test]
    fn uninstall_preserves_user_set_empty_containers() {
        // The user deliberately set empty containers we never touched. Uninstall
        // must NOT prune them (no sentinel was removed beneath them).
        let config = serde_json::json!({
            "emptyObj": {},
            "emptyArr": [],
            "nested": { "alsoEmpty": {} }
        });
        let stripped = strip_sentinel(config.clone());
        assert_eq!(
            stripped, config,
            "user-set empty containers must survive uninstall verbatim"
        );
    }

    #[test]
    fn uninstall_preserves_user_empty_container_beside_our_pruned_hook() {
        // A user-set empty `customEmpty` object sits next to our injected hooks.
        // After uninstall, ours is pruned but the user's empty object survives.
        let user = serde_json::json!({ "model": "x", "customEmpty": {} });
        let installed = merge_value(user, &claude_fragment(Path::new("/tmp/p")));
        let stripped = strip_sentinel(installed);
        assert_eq!(
            stripped,
            serde_json::json!({ "model": "x", "customEmpty": {} }),
            "user empty container survives while our hooks block is pruned"
        );
    }

    #[test]
    fn uninstall_prunes_empty_hooks_object_when_only_ours_existed() {
        let only_ours = merge_value(serde_json::json!({}), &claude_fragment(Path::new("/tmp/p")));
        let stripped = strip_sentinel(only_ours);
        // All four events were ours; the `hooks` object (and thus all keys) pruned to {}.
        assert_eq!(stripped, serde_json::json!({}));
    }

    #[test]
    fn uninstall_file_round_trips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "model": "x",
                "hooks": { "Stop": [ { "hooks": [ { "command": "keep-me" } ] } ] }
            }))
            .unwrap(),
        )
        .unwrap();
        let frag = claude_fragment(dir.path());
        merge_into_file(&path, &frag).unwrap();
        uninstall_file(&path).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // The user's Stop hook with "keep-me" survives; ours are gone.
        let stop = after
            .get("hooks")
            .unwrap()
            .get("Stop")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(stop.len(), 1);
        assert_eq!(
            stop[0].get("hooks").unwrap().as_array().unwrap()[0]
                .get("command")
                .unwrap(),
            &serde_json::json!("keep-me")
        );
        assert_eq!(after.get("model").unwrap(), &serde_json::json!("x"));
    }

    #[test]
    fn uninstall_file_missing_path_is_ok() {
        assert!(uninstall_file(Path::new("/tmp/__rb_missing_xyz__.json")).is_ok());
    }
}
