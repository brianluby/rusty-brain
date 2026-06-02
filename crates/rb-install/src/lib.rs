//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

pub mod detect;

pub use detect::{find_binary_on_path, parse_version, version_of};

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }
}
