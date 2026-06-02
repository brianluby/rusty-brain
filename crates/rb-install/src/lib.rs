//! `rb-install` — the merge/uninstall/status engine for `rusty-brain-install`.
//!
//! Wires the four JSON-protocol CLIs (Claude Code, OpenCode, Gemini, Codex) to
//! the `rusty-brain-hooks` binary by deep-merging a sentinel-marked hook block
//! into each CLI's config. NEVER referenced by any core crate, so the default
//! `rusty-brain` build never compiles it.

#[cfg(test)]
mod skeleton_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    #[test]
    fn crate_links() {
        // Proves the crate compiles and links against rb-agents + rb-types.
        let _ = rb_agents::install::SENTINEL;
        let _ = rb_types::Namespace::Global;
        assert_eq!(rb_agents::install::SENTINEL, "rusty-brain");
    }
}
