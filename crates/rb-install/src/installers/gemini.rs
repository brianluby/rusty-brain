//! Gemini CLI installer.
//!
//! Config target: project `<root>/.gemini/settings.json`, global
//! `~/.gemini/settings.json`. Hooks nest under the top-level `hooks` key.

use std::path::Path;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::Result;

use super::{home_join, hooks_block, GEMINI_EVENTS};
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
        // Gemini: its own event names (SessionStart/AfterTool/SessionEnd/
        // PreCompress), tool event `AfterTool`, INLINE form (Gemini has no `args`
        // field — the `--agent` flag must live inside the single command string).
        let merge = hooks_block(
            &hooks_bin.to_string_lossy(),
            AgentId::Gemini.as_str(),
            &GEMINI_EVENTS,
            "AfterTool",
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
        let hooks = frag.merge.get("hooks").unwrap();
        // Gemini's native event keys — NOT Claude's PostToolUse/Stop/PreCompact.
        for event in ["SessionStart", "AfterTool", "SessionEnd", "PreCompress"] {
            assert!(
                hooks.get(event).is_some(),
                "expected Gemini event key {event}"
            );
        }
        // The Claude-only keys must be absent.
        assert!(hooks.get("PostToolUse").is_none());
        assert!(hooks.get("Stop").is_none());
        assert!(hooks.get("PreCompact").is_none());
        // AfterTool carries a matcher; SessionEnd does not.
        let after = hooks.get("AfterTool").unwrap().as_array().unwrap();
        assert_eq!(after[0].get("matcher").unwrap(), &serde_json::json!("*"));
        let session_end = hooks.get("SessionEnd").unwrap().as_array().unwrap();
        assert!(session_end[0].get("matcher").is_none());

        let entry = hooks.get("PreCompress").unwrap().as_array().unwrap()[0]
            .get("hooks")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .clone();
        // INLINE form: the whole invocation is one shell string with the binary
        // double-quoted; there is NO separate `args` key.
        assert_eq!(
            entry.get("command").unwrap().as_str().unwrap(),
            "\"/bin/rusty-brain-hooks\" --agent gemini"
        );
        assert!(
            entry.get("args").is_none(),
            "Gemini entries must NOT carry an `args` key (it would be dropped)"
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
