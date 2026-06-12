//! Hook-side namespace resolution: thin shim over [`rb_config::namespace`]
//! (the single W0.3 implementation), keeping this crate's bounded git
//! invocation so a wedged git can never hang the agent hook.
//!
//! Hooks are non-interactive: an unpinned `CLAUDE.md` frontmatter `project:`
//! override is NEVER honored — it logs a `tracing` warning and the git-toplevel
//! name is used instead (F22). Pins recorded by the CLI's
//! `--accept-namespace-override` are honored.
//!
//! Never panics; degrades to `Global`. MUST run OFF the async runtime (reads
//! files, shells out to git).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rb_config::namespace::{explicit_namespace_from_env, resolve_namespace_in, NamespacePins};
use rb_types::Namespace;

use crate::proc::run_git_bounded;

/// Detect the namespace for `cwd`. Reads the pin store, `.rusty-brain.toml`,
/// and `CLAUDE.md`; invokes git under a 1s bound. Synchronous: call this OFF
/// the tokio runtime.
pub fn detect_namespace(cwd: &Path) -> Namespace {
    let res = resolve_namespace_in(
        cwd,
        explicit_namespace_from_env(),
        &NamespacePins::load(),
        git_toplevel,
    );
    if let Some(o) = &res.unpinned_override {
        tracing::warn!(
            claimed = %o.claimed,
            used = %o.used,
            toplevel = %o.toplevel.display(),
            "ignoring unpinned CLAUDE.md namespace override; run \
             `rusty-brain --accept-namespace-override` in the repo to pin it"
        );
    }
    res.namespace
}

/// Find the git toplevel for `dir` by invoking git under a 1s bound; `None` if
/// not a repo (or git hung/failed — fail-open).
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let bytes = run_git_bounded(
        dir,
        &["rev-parse", "--show-toplevel"],
        Duration::from_secs(1),
    )?;
    let path = String::from_utf8(bytes).ok()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_types::Namespace;
    use std::fs;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    /// `git init` `dir`; false (test skips) when git is unavailable —
    /// mirroring the `proc.rs` test precedent.
    fn git_init(dir: &Path) -> bool {
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn walk_stops_at_git_toplevel_f54_regression() {
        // (a) F54: an ancestor CLAUDE.md OUTSIDE the repo (the ~/CLAUDE.md
        // scenario) must never name a CLAUDE.md-less repo below it.
        let tmp = TempDir::new().unwrap();
        // Canonicalize: git reports the physical toplevel (macOS /tmp symlink).
        let outer = tmp.path().canonicalize().unwrap();
        fs::write(
            outer.join("CLAUDE.md"),
            "---\nproject: outer-claim\n---\n# Outer Heading\n",
        )
        .unwrap();
        let repo = outer.join("inner-repo");
        let nested = repo.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        if !git_init(&repo) {
            return; // git unavailable; skip
        }
        assert_eq!(
            detect_namespace(&nested),
            Namespace::Project("inner-repo".to_string())
        );
    }

    #[test]
    fn hooks_never_honor_unpinned_frontmatter_f22() {
        // (c) F22: the hook path uses the git-toplevel name even when the
        // repo's own committed CLAUDE.md claims another project's namespace.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap().join("cloned-repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("CLAUDE.md"), "---\nproject: victim\n---\n").unwrap();
        if !git_init(&repo) {
            return; // git unavailable; skip
        }
        assert_eq!(
            detect_namespace(&repo),
            Namespace::Project("cloned-repo".to_string())
        );
    }

    #[test]
    fn repo_committed_toml_resolves_identically_across_clone_names() {
        // (b)+(d): `.rusty-brain.toml` outranks the directory name, so the
        // same repo under two clone names yields one namespace.
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let mut seen = Vec::new();
        for clone in ["clone-a", "renamed-checkout"] {
            let repo = base.join(clone);
            fs::create_dir_all(&repo).unwrap();
            fs::write(repo.join(".rusty-brain.toml"), "namespace = \"one-id\"\n").unwrap();
            if !git_init(&repo) {
                return; // git unavailable; skip
            }
            seen.push(detect_namespace(&repo));
        }
        assert_eq!(seen[0], seen[1]);
        assert_eq!(seen[0], Namespace::Project("one-id".to_string()));
    }

    #[test]
    fn non_repo_dir_uses_its_own_name_and_ignores_claude_md() {
        // Frontmatter is repo-scoped: outside a repo it is skipped entirely,
        // and there is no H1 fallback anywhere.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().canonicalize().unwrap().join("standalone");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("CLAUDE.md"),
            "---\nproject: nope\n---\n# Nor This\n",
        )
        .unwrap();
        assert_eq!(
            detect_namespace(&dir),
            Namespace::Project("standalone".to_string())
        );
    }
}
