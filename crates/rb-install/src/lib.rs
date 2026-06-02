//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

pub mod detect;
pub mod installers;
pub mod uninstall;
pub mod writer;

pub use detect::{find_binary_on_path, parse_version, version_of};
pub use installers::{
    builtins, ClaudeCodeInstaller, CodexInstaller, GeminiInstaller, OpenCodeInstaller,
};
pub use uninstall::{strip_sentinel, uninstall_file};
pub use writer::{backup_path, merge_into_file, merge_value, read_config, write};

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }

    #[test]
    fn builtins_has_four_in_lead_order() {
        let b = super::builtins();
        assert_eq!(b.len(), 4);
        assert_eq!(b[0].id(), rb_agents::cli::AgentId::ClaudeCode);
    }
}
