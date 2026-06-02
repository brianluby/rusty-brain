//! Claude Code installer (the lead/reference adapter).
//!
//! Config target: project `<root>/.claude/settings.json`, global
//! `~/.claude/settings.json`. Hooks nest under the top-level `hooks` key.

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{home_join, hooks_block};
use crate::detect::{find_binary_on_path, version_of};

/// Installer for the Claude Code CLI.
pub struct ClaudeCodeInstaller;

impl AgentInstaller for ClaudeCodeInstaller {
    fn id(&self) -> AgentId {
        AgentId::ClaudeCode
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("claude")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let config_path = match scope {
            InstallScope::Project(root) => root.join(".claude").join("settings.json"),
            InstallScope::Global => home_join(".claude")?.join("settings.json"),
        };
        let merge = hooks_block(&hooks_bin.to_string_lossy(), AgentId::ClaudeCode.as_str());
        Ok(HookFragment { config_path, merge })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_agents::install::SENTINEL;
    use std::path::PathBuf;

    #[test]
    fn id_is_claude_code() {
        assert_eq!(ClaudeCodeInstaller.id(), AgentId::ClaudeCode);
    }

    #[test]
    fn fragment_project_path_and_shape() {
        let frag = ClaudeCodeInstaller
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/tmp/project")),
            )
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from("/tmp/project/.claude/settings.json")
        );
        let hooks = frag.merge.get("hooks").unwrap();
        for event in ["SessionStart", "PostToolUse", "Stop", "PreCompact"] {
            let arr = hooks.get(event).unwrap().as_array().unwrap();
            assert_eq!(arr.len(), 1);
            let group = &arr[0];
            assert_eq!(group.get(SENTINEL).unwrap(), &serde_json::json!(true));
            let inner = group.get("hooks").unwrap().as_array().unwrap();
            let cmd = inner[0].get("command").unwrap().as_str().unwrap();
            assert_eq!(cmd, "/usr/local/bin/rusty-brain-hooks --agent claude-code");
            assert_eq!(inner[0].get(SENTINEL).unwrap(), &serde_json::json!(true));
        }
        // PostToolUse carries a matcher; the others do not.
        let post = hooks.get("PostToolUse").unwrap().as_array().unwrap();
        assert_eq!(post[0].get("matcher").unwrap(), &serde_json::json!("*"));
        let stop = hooks.get("Stop").unwrap().as_array().unwrap();
        assert!(stop[0].get("matcher").is_none());
    }

    #[test]
    fn fragment_global_path() {
        // Read the real HOME rather than mutate the process-global env (no env
        // mutation => no test-thread races; on edition 2021 set_var is safe but
        // still globally racy under parallel tests).
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return; // No home dir in this environment; skip.
        };
        let frag = ClaudeCodeInstaller
            .hook_fragment(Path::new("/x/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from(home).join(".claude").join("settings.json")
        );
    }
}
