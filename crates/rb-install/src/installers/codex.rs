//! Codex CLI installer.
//!
//! Config target: project `<root>/.codex/hooks.json`, global
//! `~/.codex/hooks.json`. The dedicated hooks file holds only the `hooks` block.

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{home_join, hooks_block};
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
        let merge = hooks_block(&hooks_bin.to_string_lossy(), AgentId::Codex.as_str());
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
        let entry = frag
            .merge
            .get("hooks")
            .unwrap()
            .get("PostToolUse")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("hooks")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .clone();
        // EXEC form: `command` is the raw binary path; flags live in `args`.
        assert_eq!(
            entry.get("command").unwrap().as_str().unwrap(),
            "/bin/rusty-brain-hooks"
        );
        assert_eq!(
            entry.get("args").unwrap(),
            &serde_json::json!(["--agent", "codex"])
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
