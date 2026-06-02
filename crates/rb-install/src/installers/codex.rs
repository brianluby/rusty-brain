//! Codex CLI installer.
//!
//! Config target: project `<root>/.codex/hooks.json`, global
//! `~/.codex/hooks.json`. The dedicated hooks file holds only the `hooks` block.

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{home_join, hooks_block, CODEX_EVENTS};
use crate::detect::{find_binary_on_path, version_of};

/// Installer for the Codex CLI.
pub struct CodexInstaller;

impl AgentInstaller for CodexInstaller {
    fn id(&self) -> AgentId {
        AgentId::Codex
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("codex")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let config_path = match scope {
            InstallScope::Project(root) => root.join(".codex").join("hooks.json"),
            InstallScope::Global => home_join(".codex")?.join("hooks.json"),
        };
        // Codex: SessionStart/PostToolUse/Stop/PreCompact event names, tool event
        // `PostToolUse`, INLINE form (Codex has no `args` field — the `--agent`
        // flag must live inside the single command string).
        let merge = hooks_block(
            &hooks_bin.to_string_lossy(),
            AgentId::Codex.as_str(),
            &CODEX_EVENTS,
            "PostToolUse",
            false,
        );
        Ok(HookFragment { config_path, merge })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn id_is_codex() {
        assert_eq!(CodexInstaller.id(), AgentId::Codex);
    }

    #[test]
    fn fragment_project_path_and_command() {
        let frag = CodexInstaller
            .hook_fragment(
                Path::new("/bin/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/tmp/c")),
            )
            .unwrap();
        assert_eq!(frag.config_path, PathBuf::from("/tmp/c/.codex/hooks.json"));
        let hooks = frag.merge.get("hooks").unwrap();
        // Codex uses Claude's event names — but the command form differs.
        for event in ["SessionStart", "PostToolUse", "Stop", "PreCompact"] {
            assert!(
                hooks.get(event).is_some(),
                "expected Codex event key {event}"
            );
        }
        // PostToolUse carries a matcher; Stop does not.
        let post = hooks.get("PostToolUse").unwrap().as_array().unwrap();
        assert_eq!(post[0].get("matcher").unwrap(), &serde_json::json!("*"));
        let stop = hooks.get("Stop").unwrap().as_array().unwrap();
        assert!(stop[0].get("matcher").is_none());

        let entry = post[0].get("hooks").unwrap().as_array().unwrap()[0].clone();
        // INLINE form: one shell string with the binary SHELL-QUOTED (the exact
        // quoting is verified in `installers::tests`); there is NO separate `args`
        // key.
        let cmd = entry.get("command").unwrap().as_str().unwrap();
        assert!(
            cmd.contains("/bin/rusty-brain-hooks"),
            "command must reference the hooks binary path; got {cmd}"
        );
        assert!(
            cmd.ends_with("--agent codex"),
            "command must pass the codex agent id; got {cmd}"
        );
        assert!(
            entry.get("args").is_none(),
            "Codex entries must NOT carry an `args` key (it would be dropped)"
        );
    }

    #[test]
    fn fragment_global_path() {
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return; // No home dir in this environment; skip.
        };
        let frag = CodexInstaller
            .hook_fragment(Path::new("/x/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from(home).join(".codex").join("hooks.json")
        );
    }
}
