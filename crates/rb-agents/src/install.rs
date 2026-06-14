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

/// A whole file the installer writes on install and deletes on uninstall (e.g.
/// the W3.2(b) memory skill). The file is installer-owned: it has no in-file
/// marker, so uninstall removes it by path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedFile {
    /// Absolute path to write.
    pub path: PathBuf,
    /// Full file contents (UTF-8).
    pub contents: String,
}

/// A marker-delimited block appended to a UTF-8 text file (e.g. the W3.2(b)
/// memory-policy block in a project `CLAUDE.md`). The writer wraps `body` in
/// begin/end markers keyed by `marker_id`, so re-install REPLACES the block
/// between the markers (idempotent) and uninstall removes exactly it, leaving
/// the user's surrounding prose untouched. A structured document cannot carry
/// the JSON sentinel, so the text markers are the reversibility mechanism here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedTextBlock {
    /// Absolute path of the text file to append to (created if absent).
    pub path: PathBuf,
    /// Stable id embedded in the begin/end markers (must be constant across
    /// install/uninstall so the block can be found again).
    pub marker_id: String,
    /// The block body inserted between the markers (the markers are added by
    /// the writer, never by the caller).
    pub body: String,
}

/// A config-file path plus the sentinel-keyed JSON block to deep-merge into it,
/// and the non-JSON managed side-effects (permission allowlist entries, whole
/// files, marker-delimited text blocks) the engine applies on install and
/// reverses on uninstall. `hook_fragment` produces this purely (no I/O); Part Y
/// performs the atomic writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFragment {
    pub config_path: PathBuf,
    pub merge: serde_json::Value,
    /// W3.2(c): entries unioned into the config's `permissions.allow` array
    /// (idempotent add on install, removed on uninstall). Empty for installers
    /// that need none. A permission string cannot carry the JSON sentinel, so it
    /// is added/removed by exact value rather than via the sentinel strip.
    pub allow_entries: Vec<String>,
    /// W3.2(b): whole files written on install, deleted on uninstall.
    pub managed_files: Vec<ManagedFile>,
    /// W3.2(b): marker-delimited text blocks appended on install, removed on
    /// uninstall.
    pub text_blocks: Vec<ManagedTextBlock>,
}

impl HookFragment {
    /// A hooks-only fragment (no managed permissions / files / text blocks) — the
    /// shape every installer started with. Use the `with_*` builders to add the
    /// W3.2 elicitation side-effects.
    #[must_use]
    pub fn new(config_path: PathBuf, merge: serde_json::Value) -> Self {
        Self {
            config_path,
            merge,
            allow_entries: Vec::new(),
            managed_files: Vec::new(),
            text_blocks: Vec::new(),
        }
    }

    /// Set the `permissions.allow` entries (W3.2(c)).
    #[must_use]
    pub fn with_allow_entries(mut self, entries: Vec<String>) -> Self {
        self.allow_entries = entries;
        self
    }

    /// Set the whole managed files (W3.2(b)).
    #[must_use]
    pub fn with_managed_files(mut self, files: Vec<ManagedFile>) -> Self {
        self.managed_files = files;
        self
    }

    /// Set the marker-delimited text blocks (W3.2(b)).
    #[must_use]
    pub fn with_text_blocks(mut self, blocks: Vec<ManagedTextBlock>) -> Self {
        self.text_blocks = blocks;
        self
    }
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
            Ok(HookFragment::new(config_path, merge))
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
