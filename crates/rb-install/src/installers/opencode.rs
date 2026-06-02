//! OpenCode installer.
//!
//! Config target: project `<root>/opencode.json`, global
//! `~/.config/opencode/opencode.json`. Hooks nest under the top-level `hooks`
//! key (same shape we use for Claude Code; OpenCode ignores unknown keys).

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{home_join, hooks_block};
use crate::detect::{find_binary_on_path, version_of};

/// Installer for the OpenCode CLI.
pub struct OpenCodeInstaller;

impl AgentInstaller for OpenCodeInstaller {
    fn id(&self) -> AgentId {
        AgentId::OpenCode
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("opencode")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let config_path = match scope {
            InstallScope::Project(root) => root.join("opencode.json"),
            InstallScope::Global => home_join(".config")?.join("opencode").join("opencode.json"),
        };
        let merge = hooks_block(&hooks_bin.to_string_lossy(), AgentId::OpenCode.as_str());
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
    fn id_is_opencode() {
        assert_eq!(OpenCodeInstaller.id(), AgentId::OpenCode);
    }

    #[test]
    fn fragment_project_path_and_command() {
        let frag = OpenCodeInstaller
            .hook_fragment(
                Path::new("/opt/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/tmp/proj")),
            )
            .unwrap();
        assert_eq!(frag.config_path, PathBuf::from("/tmp/proj/opencode.json"));
        let cmd = frag
            .merge
            .get("hooks")
            .unwrap()
            .get("SessionStart")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("hooks")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("command")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cmd, "/opt/rusty-brain-hooks --agent opencode");
        // Sentinel marker present.
        assert_eq!(
            frag.merge
                .get("hooks")
                .unwrap()
                .get("Stop")
                .unwrap()
                .as_array()
                .unwrap()[0]
                .get(SENTINEL)
                .unwrap(),
            &serde_json::json!(true)
        );
    }

    #[test]
    fn fragment_global_path() {
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return; // No home dir in this environment; skip.
        };
        let frag = OpenCodeInstaller
            .hook_fragment(Path::new("/x/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from(home)
                .join(".config")
                .join("opencode")
                .join("opencode.json")
        );
    }
}
