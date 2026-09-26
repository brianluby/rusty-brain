//! Bounded git subprocess helper (compatibility facade).
//!
//! The implementations live in `rb_config::git_proc` since Vikunja #63 (the
//! daemon needs them for repo-state staleness and cannot depend on this
//! crate); they are re-exported here so existing callers keep their paths.

pub use rb_config::{run_git_bounded, run_git_status_bounded};

#[cfg(test)]
mod tests {
    use super::*;
    // The behavioral tests live beside the implementations in rb-config.
    // One canary here: the re-export is the SAME function.
    #[test]
    fn reexport_points_at_rb_config() {
        let a: fn(&std::path::Path, &[&str], std::time::Duration) -> Option<Vec<u8>> =
            run_git_bounded;
        let b: fn(&std::path::Path, &[&str], std::time::Duration) -> Option<Vec<u8>> =
            rb_config::run_git_bounded;
        assert_eq!(format!("{a:p}"), format!("{b:p}"));
    }
}
