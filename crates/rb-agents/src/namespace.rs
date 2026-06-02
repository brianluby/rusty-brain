//! Self-contained namespace detection (ported from `rusty-brain/src/
//! namespace_detect.rs`) so the hook binary never links the `rusty-brain`
//! crate. Resolution order (first non-empty wins): (1) nearest `CLAUDE.md`
//! frontmatter `project:`, (2) that `CLAUDE.md`'s first `# H1`, (3) git-root dir
//! name, (4) `cwd` dir name, (5) `Global`. Never panics; degrades to `Global`.
//!
//! MUST run OFF the async runtime (reads files, shells out to git).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rb_types::Namespace;

use crate::proc::run_git_bounded;

/// Detect the namespace for `cwd`. Reads `CLAUDE.md`, invokes git. Synchronous:
/// call this OFF the tokio runtime.
pub fn detect_namespace(cwd: &Path) -> Namespace {
    detect_namespace_with(cwd, find_nearest_claude_md, git_toplevel)
}

/// Pure core, parameterized over the `CLAUDE.md` finder and git-root resolver so
/// every branch is unit-testable without touching the filesystem or git.
pub fn detect_namespace_with<C, G>(start: &Path, find_claude_md: C, git_root: G) -> Namespace
where
    C: Fn(&Path) -> Option<(PathBuf, String)>,
    G: Fn(&Path) -> Option<PathBuf>,
{
    if let Some((_path, text)) = find_claude_md(start) {
        if let Some(name) = parse_project_from_claude_md(&text) {
            return Namespace::Project(name);
        }
    }
    if let Some(name) = git_root(start).as_deref().and_then(dir_name) {
        return Namespace::Project(name);
    }
    if let Some(name) = dir_name(start) {
        return Namespace::Project(name);
    }
    Namespace::Global
}

/// Extract a non-empty, utf8 final path component. `None` for `/`, empty, or
/// non-utf8 names — caller then falls through to the next branch.
fn dir_name(p: &Path) -> Option<String> {
    p.file_name()
        .and_then(|n| n.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a `CLAUDE.md` body: prefer frontmatter `project: NAME`, else first `# H1`.
///
/// The H1 scan deliberately skips a leading `---`...`---` YAML frontmatter block
/// so a `#` comment INSIDE the frontmatter is never misread as the document
/// title. If the frontmatter has no closing `---`, fall back to scanning the
/// whole text (preserving prior behavior).
pub fn parse_project_from_claude_md(text: &str) -> Option<String> {
    if let Some(name) = project_from_frontmatter(text) {
        return Some(name);
    }
    first_h1_in(frontmatter_aware_lines(text))
}

/// Collect the lines the H1 scan should consider, skipping a leading
/// `---`...`---` frontmatter block. With an opening fence but NO closing fence,
/// preserve prior behavior by scanning the full text (all lines).
fn frontmatter_aware_lines(text: &str) -> Vec<&str> {
    let mut lines = text.lines();
    if lines.clone().next().map(str::trim) != Some("---") {
        return text.lines().collect();
    }
    // Consume the opening fence, then walk to the matching closing fence.
    lines.next();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
    }
    if !closed {
        // No closing fence: behave as before and scan everything.
        return text.lines().collect();
    }
    // Whatever remains after the closing fence is the scannable body.
    lines.collect()
}

/// Read `project: NAME` from a leading `---`-delimited frontmatter block.
fn project_from_frontmatter(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("project:") {
            let value = rest.trim().trim_matches(|c| c == '"' || c == '\'').trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
            return None;
        }
    }
    None
}

/// First markdown `# H1` heading text (exactly one leading `#`), trimmed, over
/// the supplied lines.
fn first_h1_in<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<String> {
    for line in lines {
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

/// Walk up from `start`, returning the first `CLAUDE.md`'s `(path, contents)`.
fn find_nearest_claude_md(start: &Path) -> Option<(PathBuf, String)> {
    for dir in start.ancestors() {
        let candidate = dir.join("CLAUDE.md");
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            return Some((candidate, text));
        }
    }
    None
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
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn no_claude(_: &Path) -> Option<(PathBuf, String)> {
        None
    }

    fn no_git(_: &Path) -> Option<PathBuf> {
        None
    }

    #[test]
    fn frontmatter_project_wins_over_h1() {
        let text = "---\nproject: from-frontmatter\nother: x\n---\n# From Heading\nbody\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("from-frontmatter".to_string())
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
    fn empty_text_is_none() {
        assert_eq!(parse_project_from_claude_md(""), None);
    }

    #[test]
    fn h1_inside_frontmatter_is_not_mistaken_for_the_title() {
        // The frontmatter carries a `#` line (no `project:` key); the real H1 is in
        // the body. The frontmatter `#` MUST be skipped, not read as the title.
        let text = "---\n# not-the-title\nfoo: bar\n---\nintro prose\n# real-project\nmore\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("real-project".to_string())
        );
    }

    #[test]
    fn unterminated_frontmatter_falls_back_to_full_scan() {
        // An opening `---` with no closing fence: preserve prior behavior and scan
        // the whole text, so a `# H1` anywhere is still found.
        let text = "---\nfoo: bar\n# only-heading\nbody\n";
        assert_eq!(
            parse_project_from_claude_md(text),
            Some("only-heading".to_string())
        );
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
    fn branch3_git_root_dirname_when_claude_md_useless() {
        let start = Path::new("/home/alice/code/rusty-brain/crates/rb-agents");
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
        let ns = detect_namespace_with(start, no_claude, no_git);
        assert_eq!(ns, Namespace::Project("notes".to_string()));
    }

    #[test]
    fn branch5_global_for_root_dir() {
        let start = Path::new("/");
        let ns = detect_namespace_with(start, no_claude, no_git);
        assert_eq!(ns, Namespace::Global);
    }

    #[test]
    fn real_fs_finds_claude_md_three_levels_up_and_uses_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let mut start = root.clone();
        for i in 0..3 {
            start = start.join(format!("d{i}"));
        }
        fs::create_dir_all(&start).unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            "---\nproject: walked-up\n---\n# Ignored\n",
        )
        .unwrap();
        let ns = detect_namespace_with(&start, find_nearest_claude_md, no_git);
        assert_eq!(ns, Namespace::Project("walked-up".to_string()));
        drop(tmp);
    }

    #[test]
    fn real_fs_no_claude_md_uses_cwd_dirname() {
        let tmp = TempDir::new().unwrap();
        let start = tmp.path().join("standalone");
        fs::create_dir_all(&start).unwrap();
        let ns = detect_namespace_with(&start, find_nearest_claude_md, no_git);
        assert_eq!(ns, Namespace::Project("standalone".to_string()));
        drop(tmp);
    }
}
