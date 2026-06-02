//! Install-side contract consumed by Part Y (rb-install). Defines the install
//! scope, the sentinel-keyed JSON fragment to deep-merge into a CLI's config,
//! the `SENTINEL` marker that identifies OUR injected entries, and the
//! `AgentInstaller` trait. No implementations here — Part Y adds per-CLI ones.

use std::path::PathBuf;

use rb_types::Result;

use crate::cli::AgentId;

/// Where an install writes config. Project scope is the default; `--global`
/// targets the user-level config dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallScope {
    Project(PathBuf),
    Global,
}

/// A config-file path plus the sentinel-keyed JSON block to deep-merge into it.
/// `hook_fragment` produces this purely (no I/O); Part Y performs the atomic
/// merge-write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFragment {
    pub config_path: PathBuf,
    pub merge: serde_json::Value,
}

/// Marker key/comment identifying entries this installer owns. Uninstall removes
/// ONLY blocks carrying this sentinel; merge preserves all other user hooks.
pub const SENTINEL: &str = "rusty-brain";

/// Per-CLI installer: identity, PATH-based detection, and a PURE hook-fragment
/// builder. `detect` runs `<binary> --version` with a short timeout (NO shell);
/// `hook_fragment` performs no I/O.
pub trait AgentInstaller {
    fn id(&self) -> AgentId;
    fn detect(&self) -> Option<String>;
    fn hook_fragment(
        &self,
        hooks_bin: &std::path::Path,
        scope: &InstallScope,
    ) -> Result<HookFragment>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::cli::AgentId;
    use std::path::{Path, PathBuf};

    // A trait-object smoke installer proving `AgentInstaller` is usable as
    // `Box<dyn AgentInstaller>` and the contract types compose.
    struct FakeInstaller;

    impl AgentInstaller for FakeInstaller {
        fn id(&self) -> AgentId {
            AgentId::ClaudeCode
        }

        fn detect(&self) -> Option<String> {
            Some("1.2.3".to_string())
        }

        fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
            let config_path = match scope {
                InstallScope::Project(root) => root.join(".claude").join("settings.json"),
                InstallScope::Global => PathBuf::from("/home/user/.claude/settings.json"),
            };
            let merge = serde_json::json!({
                SENTINEL: { "hooks_bin": hooks_bin.display().to_string() }
            });
            Ok(HookFragment { config_path, merge })
        }
    }

    #[test]
    fn sentinel_is_rusty_brain() {
        assert_eq!(SENTINEL, "rusty-brain");
    }

    #[test]
    fn install_scope_variants_are_distinct() {
        let project = InstallScope::Project(PathBuf::from("/proj"));
        let global = InstallScope::Global;
        assert_ne!(project, global);
        assert_eq!(project, InstallScope::Project(PathBuf::from("/proj")));
    }

    #[test]
    fn trait_object_detect_and_id_work() {
        let installer: Box<dyn AgentInstaller> = Box::new(FakeInstaller);
        assert_eq!(installer.id(), AgentId::ClaudeCode);
        assert_eq!(installer.detect().as_deref(), Some("1.2.3"));
    }

    #[test]
    fn hook_fragment_is_pure_and_carries_sentinel_for_project_scope() {
        let installer = FakeInstaller;
        let fragment = installer
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Project(PathBuf::from("/proj")),
            )
            .unwrap();
        assert_eq!(
            fragment.config_path,
            PathBuf::from("/proj/.claude/settings.json")
        );
        assert_eq!(
            fragment.merge[SENTINEL]["hooks_bin"],
            "/usr/local/bin/rusty-brain-hooks"
        );
    }

    #[test]
    fn hook_fragment_global_scope_uses_global_path() {
        let installer = FakeInstaller;
        let fragment = installer
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Global,
            )
            .unwrap();
        assert_eq!(
            fragment.config_path,
            PathBuf::from("/home/user/.claude/settings.json")
        );
    }
}
