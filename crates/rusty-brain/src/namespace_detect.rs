//! Minimal namespace detection for P1: git-root or cwd directory name.
//!
//! Full git/`CLAUDE.md` resolution is deferred to P2. This is intentionally a
//! single, predictable rule so behavior is obvious from the working directory.

use rb_types::Namespace;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Detect the namespace for the real process, using the current directory and a
/// `git rev-parse --show-toplevel` lookup. Never fails: degrades to `Global`.
pub fn detect_namespace() -> Namespace {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    detect_namespace_with(&cwd, git_toplevel)
}

/// Core logic, parameterized for tests: pick the git-root dir name if a repo is
/// found, else the start dir name, else `Global`.
pub fn detect_namespace_with<F>(start: &Path, git_root: F) -> Namespace
where
    F: Fn(&Path) -> Option<PathBuf>,
{
    let base = git_root(start).unwrap_or_else(|| start.to_path_buf());
    match base.file_name().and_then(|n| n.to_str()) {
        Some(name) if !name.is_empty() => Namespace::Project(name.to_string()),
        _ => Namespace::Global,
    }
}

/// Find the git toplevel for `dir` by invoking git; `None` if not a repo.
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::Namespace;
    use std::path::{Path, PathBuf};

    #[test]
    fn uses_git_root_dirname_when_in_repo() {
        let start = Path::new("/home/alice/code/rusty-brain/crates/rusty-brain");
        let git_root =
            |_: &Path| -> Option<PathBuf> { Some(PathBuf::from("/home/alice/code/rusty-brain")) };
        let ns = detect_namespace_with(start, git_root);
        assert_eq!(ns, Namespace::Project("rusty-brain".to_string()));
    }

    #[test]
    fn falls_back_to_cwd_dirname_outside_repo() {
        let start = Path::new("/home/alice/scratch/notes");
        let git_root = |_: &Path| -> Option<PathBuf> { None };
        let ns = detect_namespace_with(start, git_root);
        assert_eq!(ns, Namespace::Project("notes".to_string()));
    }

    #[test]
    fn falls_back_to_global_for_root_dir() {
        let start = Path::new("/");
        let git_root = |_: &Path| -> Option<PathBuf> { None };
        let ns = detect_namespace_with(start, git_root);
        assert_eq!(ns, Namespace::Global);
    }
}
