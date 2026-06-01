//! Client-side namespace detection (P2): `CLAUDE.md` frontmatter/H1, then git
//! root, then cwd, then `Global`.
//!
//! Pure core is parameterized over a "find nearest `CLAUDE.md`" closure and a
//! "git root" closure so every branch is unit-testable without touching the real
//! filesystem or shelling out to git. Never panics, never fails: degrades to the
//! next branch and ultimately to `Namespace::Global`.
//!
//! Resolution MUST run off the async runtime (it shells out to git and reads
//! files); `main.rs` computes it before `block_on`.

use rb_types::Namespace;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Detect the namespace for the real process. Reads the real cwd, searches for a
/// `CLAUDE.md`, and invokes git. Synchronous: call this OFF the tokio runtime.
pub fn detect_namespace() -> Namespace {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    detect_namespace_with(&cwd, find_nearest_claude_md, git_toplevel)
}

/// Pure core. `find_claude_md` returns the nearest `CLAUDE.md`'s `(path, text)`
/// (searching from `start` upward); `git_root` returns the git toplevel.
///
/// Order (first non-empty wins): (1) `CLAUDE.md` frontmatter `project:`,
/// (2) `CLAUDE.md` first `# H1`, (3) git-root dir name, (4) `start` dir name,
/// (5) `Global`.
pub fn detect_namespace_with<C, G>(start: &Path, find_claude_md: C, git_root: G) -> Namespace
where
    C: Fn(&Path) -> Option<(PathBuf, String)>,
    G: Fn(&Path) -> Option<PathBuf>,
{
    // (1)+(2): nearest CLAUDE.md -> frontmatter project, else first H1.
    if let Some((_path, text)) = find_claude_md(start) {
        if let Some(name) = parse_project_from_claude_md(&text) {
            return Namespace::Project(name);
        }
    }
    // (3): git-root directory name.
    if let Some(name) = git_root(start).as_deref().and_then(dir_name) {
        return Namespace::Project(name);
    }
    // (4): start (cwd) directory name.
    if let Some(name) = dir_name(start) {
        return Namespace::Project(name);
    }
    // (5): nothing usable.
    Namespace::Global
}

/// Extract a non-empty, utf8 final path component. `None` for `/`, empty, or
/// non-utf8 names — which makes the caller fall through to the next branch.
fn dir_name(p: &Path) -> Option<String> {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Pure: parse a `CLAUDE.md` body for a project name. Prefer YAML-frontmatter
/// `project: NAME` (leading `---` ... `---` block); else the first `# H1`.
/// Lenient hand parser — never panics; returns `None` if neither is present.
pub fn parse_project_from_claude_md(text: &str) -> Option<String> {
    if let Some(name) = project_from_frontmatter(text) {
        return Some(name);
    }
    first_h1(text)
}

/// Read `project: NAME` from a leading `---`-delimited frontmatter block.
fn project_from_frontmatter(text: &str) -> Option<String> {
    let mut lines = text.lines();
    // Frontmatter must start at the very first line as `---`.
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break; // end of frontmatter
        }
        if let Some(rest) = trimmed.strip_prefix("project:") {
            let value = rest.trim().trim_matches(|c| c == '"' || c == '\'').trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
            // Empty value: stop scanning frontmatter; defer to H1.
            return None;
        }
    }
    None
}

/// First markdown `# H1` heading text (exactly one leading `#`), trimmed.
fn first_h1(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let heading = rest.trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

/// Walk up from `start` to the filesystem root, returning the first
/// `CLAUDE.md`'s `(path, contents)`. `None` if none found or unreadable.
fn find_nearest_claude_md(start: &Path) -> Option<(PathBuf, String)> {
    for dir in start.ancestors() {
        let candidate = dir.join("CLAUDE.md");
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            return Some((candidate, text));
        }
    }
    None
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_types::Namespace;
    use std::path::{Path, PathBuf};

    // --- parse_project_from_claude_md (pure text parsing) ---

    #[test]
    fn frontmatter_project_wins_over_h1() {
        let text = "---\nproject: from-frontmatter\nother: x\n---\n# From Heading\nbody\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("from-frontmatter".to_string())
        );
    }

    #[test]
    fn frontmatter_project_with_quotes_and_spaces_is_trimmed() {
        let text = "---\nproject:   \"my proj\"  \n---\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("my proj".to_string())
        );
    }

    #[test]
    fn falls_back_to_first_h1_when_no_frontmatter_project() {
        let text = "---\nother: x\n---\nintro\n#  Heading Name  \n## sub\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("Heading Name".to_string())
        );
    }

    #[test]
    fn h1_used_when_no_frontmatter_at_all() {
        let text = "# Just A Heading\nbody\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("Just A Heading".to_string())
        );
    }

    #[test]
    fn malformed_frontmatter_never_panics_and_degrades() {
        // Unterminated frontmatter, no project key, no h1 -> None.
        let text = "---\nproject\nnonsense: : :\n";
        assert_eq!(parse_project_from_claude_md(text), None);
    }

    #[test]
    fn empty_project_value_is_ignored() {
        // project: with empty value must not yield an empty namespace.
        let text = "---\nproject:   \n---\n# Heading\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("Heading".to_string())
        );
    }

    #[test]
    fn empty_text_is_none() {
        assert_eq!(parse_project_from_claude_md(""), None);
    }

    // --- detect_namespace_with (3-arg pure core) ---

    fn no_claude(_: &Path) -> Option<(PathBuf, String)> {
        None
    }

    #[test]
    fn branch1_claude_md_frontmatter_project() {
        let start = Path::new("/home/alice/code/app/src");
        let find_claude = |_: &Path| -> Option<(PathBuf, String)> {
            Some((
                PathBuf::from("/home/alice/code/app/CLAUDE.md"),
                "---\nproject: cool-app\n---\n# Other\n".to_string(),
            ))
        };
        let git_root =
            |_: &Path| -> Option<PathBuf> { Some(PathBuf::from("/home/alice/code/app")) };
        let ns = detect_namespace_with(start, find_claude, git_root);
        assert_eq!(ns, Namespace::Project("cool-app".to_string()));
    }

    #[test]
    fn branch2_claude_md_h1_heading() {
        let start = Path::new("/home/alice/code/app/src");
        let find_claude = |_: &Path| -> Option<(PathBuf, String)> {
            Some((
                PathBuf::from("/home/alice/code/app/CLAUDE.md"),
                "# Heading Project\nbody\n".to_string(),
            ))
        };
        let git_root =
            |_: &Path| -> Option<PathBuf> { Some(PathBuf::from("/home/alice/code/app")) };
        let ns = detect_namespace_with(start, find_claude, git_root);
        assert_eq!(ns, Namespace::Project("Heading Project".to_string()));
    }

    #[test]
    fn branch3_git_root_dirname_when_claude_md_useless() {
        let start = Path::new("/home/alice/code/rusty-brain/crates/rusty-brain");
        // CLAUDE.md exists but has neither project nor h1 -> skip to git root.
        let find_claude = |_: &Path| -> Option<(PathBuf, String)> {
            Some((
                PathBuf::from("/home/alice/code/rusty-brain/CLAUDE.md"),
                "just some prose with no heading\n".to_string(),
            ))
        };
        let git_root =
            |_: &Path| -> Option<PathBuf> { Some(PathBuf::from("/home/alice/code/rusty-brain")) };
        let ns = detect_namespace_with(start, find_claude, git_root);
        assert_eq!(ns, Namespace::Project("rusty-brain".to_string()));
    }

    #[test]
    fn branch4_cwd_dirname_outside_repo() {
        let start = Path::new("/home/alice/scratch/notes");
        let git_root = |_: &Path| -> Option<PathBuf> { None };
        let ns = detect_namespace_with(start, no_claude, git_root);
        assert_eq!(ns, Namespace::Project("notes".to_string()));
    }

    #[test]
    fn branch5_global_for_root_dir() {
        let start = Path::new("/");
        let git_root = |_: &Path| -> Option<PathBuf> { None };
        let ns = detect_namespace_with(start, no_claude, git_root);
        assert_eq!(ns, Namespace::Global);
    }

    #[test]
    fn non_utf8_git_root_dirname_degrades_to_cwd_then_global() {
        // git root has a non-utf8 final component; start is "/" so we end at Global.
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let mut bytes = b"/tmp/".to_vec();
        bytes.extend_from_slice(&[0x66, 0x80, 0x6f]); // "f\x80o" invalid utf8
        let bad = PathBuf::from(OsString::from_vec(bytes));
        let start = Path::new("/");
        let git_root = move |_: &Path| -> Option<PathBuf> { Some(bad.clone()) };
        let ns = detect_namespace_with(start, no_claude, git_root);
        // git-root name is non-utf8 -> None; start "/" has no name -> Global.
        assert_eq!(ns, Namespace::Global);
    }

    #[test]
    fn empty_frontmatter_project_skips_to_git_root() {
        let start = Path::new("/home/alice/code/app/src");
        let find_claude = |_: &Path| -> Option<(PathBuf, String)> {
            Some((
                PathBuf::from("/home/alice/code/app/CLAUDE.md"),
                "---\nproject:   \n---\n".to_string(),
            ))
        };
        let git_root =
            |_: &Path| -> Option<PathBuf> { Some(PathBuf::from("/home/alice/code/app")) };
        let ns = detect_namespace_with(start, find_claude, git_root);
        // empty project + no h1 -> git root name.
        assert_eq!(ns, Namespace::Project("app".to_string()));
    }
}
