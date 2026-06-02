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
/// In arrays, sentinel-marked elements are dropped. In objects, the `SENTINEL`
/// key itself is removed and each remaining value is stripped recursively. Empty
/// hook-event arrays left behind by removal are pruned, and an emptied `hooks`
/// object is removed entirely so the file returns to its pre-install shape.
#[must_use]
pub fn strip_sentinel(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(items) => {
            let cleaned: Vec<serde_json::Value> = items
                .into_iter()
                .filter(|e| !is_sentinel(e))
                .map(strip_sentinel)
                .collect();
            serde_json::Value::Array(cleaned)
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if k == SENTINEL {
                    continue;
                }
                let cleaned = strip_sentinel(v);
                // Prune arrays/objects that became empty purely from our removal.
                let prune = match &cleaned {
                    serde_json::Value::Array(a) => a.is_empty(),
                    serde_json::Value::Object(o) => o.is_empty(),
                    _ => false,
                };
                if !prune {
                    out.insert(k, cleaned);
                }
            }
            serde_json::Value::Object(out)
        }
        other => other,
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
