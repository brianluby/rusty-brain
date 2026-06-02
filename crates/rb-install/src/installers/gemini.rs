//! Gemini CLI installer.
//!
//! Config target: project `<root>/.gemini/settings.json`, global
//! `~/.gemini/settings.json`. Hooks nest under the top-level `hooks` key.

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{home_join, hooks_block};
use crate::detect::{find_binary_on_path, version_of};

/// Installer for the Gemini CLI.
pub struct GeminiInstaller;

impl AgentInstaller for GeminiInstaller {
    fn id(&self) -> AgentId {
        AgentId::Gemini
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("gemini")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let config_path = match scope {
            InstallScope::Project(root) => root.join(".gemini").join("settings.json"),
            InstallScope::Global => home_join(".gemini")?.join("settings.json"),
        };
        let merge = hooks_block(&hooks_bin.to_string_lossy(), AgentId::Gemini.as_str());
        Ok(HookFragment { config_path, merge })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn id_is_gemini() {
        assert_eq!(GeminiInstaller.id(), AgentId::Gemini);
    }

    #[test]
    fn fragment_project_path_and_command() {
        let frag = GeminiInstaller
            .hook_fragment(
                Path::new("/bin/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/tmp/g")),
            )
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from("/tmp/g/.gemini/settings.json")
        );
        let entry = frag
            .merge
            .get("hooks")
            .unwrap()
            .get("PreCompact")
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
            &serde_json::json!(["--agent", "gemini"])
        );
    }

    #[test]
    fn fragment_global_path() {
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return; // No home dir in this environment; skip.
        };
        let frag = GeminiInstaller
            .hook_fragment(Path::new("/x/rusty-brain-hooks"), &InstallScope::Global)
            .unwrap();
        assert_eq!(
            frag.config_path,
            PathBuf::from(home).join(".gemini").join("settings.json")
        );
    }
}
