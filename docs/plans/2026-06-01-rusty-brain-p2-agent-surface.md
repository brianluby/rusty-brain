# rusty-brain — P2 (Agent Surface) Implementation Plan — completes v1

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Work in the worktree `~/repos/rusty-brain-p2` on branch `feat/p2-agent-surface` (based on merged `main` = P0+P1). NO AI attribution on commits.

**Goal:** Complete **v1** by building the agent-facing surface on top of the P0+P1 core: real namespace detection (git + `CLAUDE.md`), **graph-link generation** (so the graph third of hybrid search stops being inert), recall-time access tracking, the **MCP stdio adapter** (so agents — not just the human CLI — can use rusty-brain), opt-in LLM enrichment, and the PR#1 should-fix hardening.

**Architecture:** Builds on the merged P0+P1 (`rb-types`, `rb-store`, `rb-proto`, `rb-embed`, `rb-search`, `rb-engine`, `rb-daemon`, `rusty-brain` CLI). New work: `rb-mcp` (JSON-RPC 2.0 over stdio → `rb_proto::Client` over the daemon UDS), `rb-enrich` (opt-in Anthropic enricher/linker, heuristic default), a `Linker` trait + `SimilarityLinker` wired into `rb-engine::remember`, and new `rb-store` methods (`record_access`/`supersede`/`get_many`). All store mutations (`add_link`, `record_access`) go through the daemon's single writer thread; reads via `spawn_blocking`. Namespace isolation stays enforced server-side.

**Tech Stack:** Rust 2021, tokio, serde_json (MCP JSON-RPC), reqwest (Anthropic, opt-in), clap. Tests use offline `DeterministicProvider`/`HeuristicEnricher`, in-process MCP stdio pairs, and `wiremock` — **never** the live Voyage/Anthropic API. Reference spec: `~/repos/rusty-brain/docs/specs/2026-05-31-rusty-brain-architecture-design.md`. (Parts are lettered L–P to continue after P1's F–K.)

**Build order:** L (namespace) and M (store/engine graph-linking) and N (MCP) are largely independent; **O** (LLM enrich/link) extends M's `Linker`/`Enricher` traits so needs M; **P** (hardening) touches daemon/store/bin. Suggested sequence: **L → M → N → O → P**. M is the v1-critical gap; N is the v1-critical interface — prioritize both.

---

## Part L — Real namespace detection (git + CLAUDE.md)

### Task 1: Rework `namespace_detect` — pure 5-branch resolver with `CLAUDE.md` frontmatter + H1 (failing tests first)

Replace the P1 placeholder. Introduce the spine's 3-arg pure core `detect_namespace_with(start, find_claude_md_fn, git_root_fn)` plus a pure `parse_project_from_claude_md(text) -> Option<String>` that reads YAML-frontmatter `project: NAME` first, else the first `# H1` heading. Resolution order, first match wins: (1) nearest `CLAUDE.md` frontmatter `project:`; (2) that `CLAUDE.md` first `# H1`; (3) git-root dir name; (4) start (cwd) dir name; (5) `Global`. Lenient: never panics, never fails; malformed frontmatter and non-utf8 path components degrade to the next branch.

Note: the real P1 `detect_namespace_with` takes only TWO args `(start, git_root)`; this task widens it to THREE `(start, find_claude_md, git_root)`, which is why the existing P1 tests are overwritten.

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/namespace_detect.rs`

- [ ] **Step 1: Replace the test module with the full branch matrix (failing).** Overwrite the existing `#[cfg(test)] mod tests { ... }` block in `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/namespace_detect.rs` with the following. This drives the new 3-arg `detect_namespace_with` and the pure `parse_project_from_claude_md`. The `find_claude_md_fn` closure returns the already-read file contents for the nearest `CLAUDE.md` (so the pure core never touches the filesystem); the `git_root_fn` returns the git toplevel. (The `clippy::panic` allow is defensive only — `assert_eq!`/`assert!` do not trip the workspace `clippy::panic` lint, and the impl has no `panic!`.)

```rust
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
        let git_root = |_: &Path| -> Option<PathBuf> {
            Some(PathBuf::from("/home/alice/code/rusty-brain"))
        };
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
```

- [ ] **Step 2: Run it — expect FAIL (compile error).** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml namespace_detect`
  Expected: FAIL to compile — `detect_namespace_with` is the P1 2-arg form but tests pass 3 args (`E0061`); `parse_project_from_claude_md` is undefined (`E0425: cannot find function`).

- [ ] **Step 3: Replace the implementation with the 3-arg core + pure parser.** Replace the top of the file (everything ABOVE the `#[cfg(test)]` line) with:

```rust
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
```

  (verify against installed crate at execution; adjust if the API differs) — `Path::ancestors`, `Path::file_name`, and `std::fs::read_to_string` signatures shown are the standard-library forms.

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml namespace_detect`
  Expected: PASS — all 15 tests in the `namespace_detect::tests` module pass (7 parser + 8 core-resolution tests).

- [ ] **Step 5: Lint.** Run: `cargo clippy -p rusty-brain --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- -D warnings`
  Expected: no warnings. (`#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` covers the test module; the impl uses no `unwrap`/`expect`/`panic` and so is clean under the workspace `unwrap_used = "deny"` / `expect_used = "deny"` / `panic = "deny"` lints.)

- [ ] **Step 6: Format.** Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- --check`
  Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rusty-brain/src/namespace_detect.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rusty-brain): resolve namespace from CLAUDE.md frontmatter/H1 then git/cwd"`
  Expected: one commit created.

---

### Task 2: Filesystem integration tests for `CLAUDE.md` discovery (real walk-up, tempdir)

The pure core is covered; now prove the real `find_nearest_claude_md` walk-up against a real directory tree (nearest-wins, frontmatter, H1, malformed, none). These use a tempdir and exercise the `detect_namespace_with(start, find_nearest_claude_md, no-git)` path so the file-discovery and parsing wire up correctly end-to-end without git.

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/namespace_detect.rs`
- Modify (only if missing): `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/Cargo.toml`

- [ ] **Step 1: Confirm the `tempfile` dev-dependency (already present).** The real `crates/rusty-brain/Cargo.toml` already declares `tempfile = { workspace = true }` under `[dev-dependencies]` (the workspace pins `tempfile = "3"` in `[workspace.dependencies]`). Verify it is still there:

  Run: `cargo tree -p rusty-brain -e dev --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml | grep -i tempfile`
  Expected: `tempfile` appears. If — and only if — it is somehow absent, add under `[dev-dependencies]`:

```toml
tempfile = { workspace = true }
```

- [ ] **Step 2: Add the failing fs-walk test submodule.** Insert this module inside the existing `#[cfg(test)] mod tests { ... }` block (after the existing tests, before its closing brace) in `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/namespace_detect.rs`:

```rust
    mod fs_walk {
        #![allow(clippy::unwrap_used, clippy::expect_used)]
        use super::super::{detect_namespace_with, find_nearest_claude_md};
        use rb_types::Namespace;
        use std::fs;
        use std::path::Path;
        use tempfile::TempDir;

        // Build start dir + write a CLAUDE.md `levels` directories above it.
        fn tree_with_claude(levels: usize, body: &str) -> (TempDir, std::path::PathBuf) {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path().to_path_buf();
            let mut start = root.clone();
            for i in 0..levels {
                start = start.join(format!("d{i}"));
            }
            fs::create_dir_all(&start).unwrap();
            fs::write(root.join("CLAUDE.md"), body).unwrap();
            (tmp, start)
        }

        fn no_git(_: &Path) -> Option<std::path::PathBuf> {
            None
        }

        #[test]
        fn finds_claude_md_three_levels_up_and_uses_frontmatter() {
            let (tmp, start) =
                tree_with_claude(3, "---\nproject: walked-up\n---\n# Ignored\n");
            let ns = detect_namespace_with(&start, find_nearest_claude_md, no_git);
            assert_eq!(ns, Namespace::Project("walked-up".to_string()));
            drop(tmp);
        }

        #[test]
        fn uses_h1_from_real_file_when_no_frontmatter() {
            let (tmp, start) = tree_with_claude(2, "# Real Heading\nbody\n");
            let ns = detect_namespace_with(&start, find_nearest_claude_md, no_git);
            assert_eq!(ns, Namespace::Project("Real Heading".to_string()));
            drop(tmp);
        }

        #[test]
        fn nearest_claude_md_wins_over_higher_one() {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path().to_path_buf();
            let mid = root.join("mid");
            let leaf = mid.join("leaf");
            fs::create_dir_all(&leaf).unwrap();
            fs::write(root.join("CLAUDE.md"), "---\nproject: outer\n---\n").unwrap();
            fs::write(mid.join("CLAUDE.md"), "---\nproject: inner\n---\n").unwrap();
            let ns = detect_namespace_with(&leaf, find_nearest_claude_md, no_git);
            assert_eq!(ns, Namespace::Project("inner".to_string()));
            drop(tmp);
        }

        #[test]
        fn malformed_claude_md_degrades_to_cwd_dirname() {
            // CLAUDE.md present but no project + no h1 -> cwd dir name (no git).
            let tmp = TempDir::new().unwrap();
            let root = tmp.path().to_path_buf();
            let start = root.join("my-project-dir");
            fs::create_dir_all(&start).unwrap();
            fs::write(root.join("CLAUDE.md"), "---\nbroken\n").unwrap();
            let ns = detect_namespace_with(&start, find_nearest_claude_md, no_git);
            assert_eq!(ns, Namespace::Project("my-project-dir".to_string()));
            drop(tmp);
        }

        #[test]
        fn no_claude_md_anywhere_uses_cwd_dirname() {
            let tmp = TempDir::new().unwrap();
            let start = tmp.path().join("standalone");
            fs::create_dir_all(&start).unwrap();
            let ns = detect_namespace_with(&start, find_nearest_claude_md, no_git);
            assert_eq!(ns, Namespace::Project("standalone".to_string()));
            drop(tmp);
        }
    }
```

  Note on the malformed-frontmatter case: `tree_with_claude` and the standalone trees write `CLAUDE.md` at the tempdir ROOT and put the start dir BELOW it, so the walk-up finds no `CLAUDE.md` above the tempdir root within the test tree. The macOS tempdir lives under `/var/folders/...`; `find_nearest_claude_md` still walks to `/`, but no real `CLAUDE.md` exists on those ancestors in CI, so resolution stops at the test's own file. These tests are therefore deterministic in CI.

- [ ] **Step 2b: Confirm `find_nearest_claude_md` is reachable from the test submodule.** The test imports `super::super::find_nearest_claude_md`. It is a private `fn` in the module root; Rust grants descendant modules access to ancestor-module private items, and `super::super` from `tests::fs_walk` resolves to the module root — so no visibility change is needed. (verify against installed crate at execution; adjust if the API differs) — if the compiler unexpectedly reports it unreachable, change its declaration in the impl from `fn find_nearest_claude_md` to `pub(crate) fn find_nearest_claude_md`.

- [ ] **Step 3: Run it — observe GREEN (implementation already exists from Task 1).** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml namespace_detect::tests::fs_walk`
  Expected: PASS — `find_nearest_claude_md` and the parser already exist from Task 1, so these new tests are GREEN immediately. If instead you see a compile error, it is one of: `tempfile` missing (apply Step 1) or `find_nearest_claude_md` unreachable (apply Step 2b); fix and re-run to GREEN. (These are the only failure modes; there is no separate RED state for this task because the production code under test landed in Task 1.)

- [ ] **Step 4: Run the whole module — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml namespace_detect`
  Expected: PASS — all `namespace_detect` tests pass, including the 5 new `fs_walk` tests.

- [ ] **Step 5: Lint.** Run: `cargo clippy -p rusty-brain --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- -D warnings`
  Expected: no warnings.

- [ ] **Step 6: Format.** Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- --check`
  Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rusty-brain/src/namespace_detect.rs crates/rusty-brain/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "test(rusty-brain): cover real CLAUDE.md walk-up discovery and parsing"`
  Expected: one commit created.

---

### Task 3: Resolve the namespace OFF the async runtime — thread it through `run(cli, namespace)`

The P1 should-fix: `detect_namespace()` (which shells out to git and reads files) currently runs inside `run_client`, i.e. ON the tokio runtime. Move resolution out of the async path: `run` and `run_client` take an already-resolved `Namespace` parameter instead of calling `detect_namespace()` themselves. This task changes the library signatures + their internal use; Task 4 wires `main.rs` to compute the namespace before `block_on`.

The test must NOT spin up the real auto-start/connect path: the real `client::connect_or_start` retries 50× at 100ms (~5s) and spawns a detached child via `std::env::current_exe()` for a `NotFound`/`ConnectionRefused`-class error. To prove the namespace is threaded WITHOUT spawning a daemon or sleeping, we drive `client::connect_or_start` directly with the socket path pointing at a regular FILE: `UnixStream::connect` then returns `ENOTSOCK` ("Socket operation on non-socket"), which `should_auto_start` does NOT match, so it returns immediately — no spawn, no retries, no global env mutation.

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/run.rs`

- [ ] **Step 1: Add a failing test asserting `run` accepts a pre-resolved namespace and that the client path forwards it without auto-starting a daemon.** Append to the `#[cfg(test)] mod tests` block in `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/run.rs`:

```rust
    // Proves the namespace is threaded into the client connect path WITHOUT
    // triggering auto-start: a regular file at the socket path makes
    // UnixStream::connect fail with ENOTSOCK, which `should_auto_start` does NOT
    // match, so `connect_or_start` returns immediately (no spawned child, no
    // retry sleeps, no process-global env mutation). Uses an isolated tempdir.
    #[tokio::test]
    async fn connect_or_start_forwards_namespace_without_autostart() {
        use rb_types::Namespace;
        use std::time::Instant;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        // A regular file, not a socket: connect -> ENOTSOCK (non-startable).
        let sock = tmp.path().join("not-a-socket");
        std::fs::write(&sock, b"x").unwrap();
        let db = tmp.path().join("rb.db");
        // A self_exe that, if ever spawned, would do nothing harmful; it must NOT
        // be spawned because ENOTSOCK is not an auto-start error.
        let self_exe = std::path::PathBuf::from("/nonexistent/never-spawned");

        let ns = Namespace::Project("injected".to_string());
        let started = Instant::now();
        let result =
            crate::client::connect_or_start(&sock, &db, ns, self_exe).await;
        let elapsed = started.elapsed();

        // Returns an Err quickly (no 50-retry backoff, no daemon spawn).
        assert!(result.is_err(), "expected connect failure on a non-socket");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "connect_or_start must not enter the retry/backoff loop for a \
             non-startable error; took {elapsed:?}"
        );
    }

    // Compile-level guarantee that `run` now takes a pre-resolved Namespace
    // (the signature change this task is about). We do not await it against a
    // real daemon; we only bind a typed fn pointer to assert the arity/types.
    #[test]
    fn run_signature_accepts_cli_and_namespace() {
        use rb_types::Namespace;
        let _f: fn(
            crate::cli::Cli,
            Namespace,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<()>>>,
        > = |cli, ns| Box::pin(run(cli, ns));
    }
```

  (verify against installed crate at execution; adjust if the API differs) — `client::connect_or_start(socket: &Path, db: &Path, namespace: Namespace, self_exe: PathBuf) -> Result<Client>` and the `Cli { json, command }` fields match the real `crates/rusty-brain/src/{client.rs,cli.rs}`. If a platform reports a different errno than `ENOTSOCK` for "connect to a regular file", the only invariant that matters is that the error is NOT in `should_auto_start`'s substring set (`no such file` / `not found` / `connection refused` / `os error 2` / `os error 61` / `os error 111`); a regular file at the path guarantees this on macOS and Linux.

- [ ] **Step 2: Run it — expect FAIL (compile error).** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml run::tests`
  Expected: FAIL to compile — `run` currently takes ONE argument (`cli`), but `run_signature_accepts_cli_and_namespace` binds it as a 2-arg `fn(Cli, Namespace)` (`E0308`/`E0593` mismatch). (`connect_or_start_forwards_namespace_without_autostart` already compiles against the real `connect_or_start`, but the crate will not build until `run`'s signature is fixed.)

- [ ] **Step 3: Change `run` and `run_client` to accept the namespace; drop the in-async `detect_namespace()` call.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/run.rs`:

  Remove the now-unused import line `use crate::namespace_detect::detect_namespace;`.

  Change the `run` signature and forward the namespace:

```rust
/// Execute the parsed CLI with a pre-resolved `namespace` (resolved OFF the
/// async runtime by `main`, since detection shells out to git and reads files).
/// `serve` blocks until Ctrl-C; client commands connect (auto-starting the
/// daemon), issue one request, print to stdout, and return.
pub async fn run(cli: Cli, namespace: rb_types::Namespace) -> anyhow::Result<()> {
    let socket_path = paths::socket_path_from_env().context("resolving daemon socket path")?;
    let db_path = paths::db_path_from_env().context("resolving daemon database path")?;

    match cli.command {
        Command::Serve => {
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            serve::run_serve(socket_path, db_path, 4, shutdown)
                .await
                .context("daemon failed")?;
            Ok(())
        }
        other => run_client(other, cli.json, namespace, &socket_path, &db_path).await,
    }
}
```

  Change `run_client` to take the namespace by value instead of calling `detect_namespace()`. Replace the current header (signature + the first three statements, through the `connect_or_start` call) with:

```rust
/// Connect to the daemon and dispatch a single client request, scoped to the
/// pre-resolved `namespace`.
async fn run_client(
    command: Command,
    json: bool,
    namespace: rb_types::Namespace,
    socket_path: &std::path::Path,
    db_path: &std::path::Path,
) -> anyhow::Result<()> {
    let self_exe = std::env::current_exe().context("locating own executable")?;
    let mut client = client::connect_or_start(socket_path, db_path, namespace, self_exe)
        .await
        .context("connecting to daemon")?;
```

  (This deletes the old `let namespace = detect_namespace();` line and drops the `.clone()` on `namespace` — it is now moved in once. The rest of `run_client`'s `match command { ... }` body is unchanged.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml run::tests`
  Expected: PASS — the existing `parse_id` tests plus the new `run_signature_accepts_cli_and_namespace` and `connect_or_start_forwards_namespace_without_autostart` tests pass. The latter returns in well under 2s with no spawned daemon process.

- [ ] **Step 5: Lint.** Run: `cargo clippy -p rusty-brain --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- -D warnings`
  Expected: no warnings (confirms the `detect_namespace` import is gone with no unused-import / dead-code warning).

- [ ] **Step 6: Format.** Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- --check`
  Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rusty-brain/src/run.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "refactor(rusty-brain): inject pre-resolved namespace into run instead of detecting on-runtime"`
  Expected: one commit created.

---

### Task 4: Wire `main.rs` to detect the namespace before `block_on`

Complete the off-runtime guarantee: `main.rs` computes the namespace with `detect_namespace()` synchronously, BEFORE constructing the tokio runtime and calling `block_on(run(cli, namespace))`. Add a focused unit test in `main.rs` proving the ordering invariant (detection is a plain sync call that returns a `Namespace`, callable without any runtime).

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/main.rs`

- [ ] **Step 1: Add a failing test that detection runs with no tokio runtime present.** Append a test module to `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use rusty_brain::namespace_detect::detect_namespace;

    #[test]
    fn detect_namespace_runs_without_a_tokio_runtime() {
        // This test has NO #[tokio::test] and no runtime: detection must be a
        // plain synchronous call. It does spawn a `git` subprocess (no network)
        // and read files, which is exactly why it must run BEFORE block_on and
        // never on a tokio worker thread. A clean return here (it never fails;
        // it degrades to Global) proves it is runtime-free.
        let _ns = detect_namespace();
    }
}
```

  (verify against installed crate at execution; adjust if the API differs) — a binary crate's `main.rs` can host `#[cfg(test)]` modules; `detect_namespace` is re-exported via the `rusty_brain` library (`pub mod namespace_detect;` in `lib.rs`), so the path `rusty_brain::namespace_detect::detect_namespace` resolves from the bin target.

- [ ] **Step 2: Run it — expect FAIL (compile error).** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml --bin rusty-brain`
  Expected: FAIL to compile — after Task 3, `run` takes two args (`cli`, `namespace`) but `main` still calls `run(cli)` (`E0061: this function takes 2 arguments but 1 argument was supplied`), so the bin target does not build. (Scope the run to the bin target; the new test name is filtered in Step 4.)

- [ ] **Step 3: Compute the namespace before the runtime and pass it into `run`.** Replace the contents of `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/main.rs` ABOVE the `#[cfg(test)]` block with:

```rust
//! `rusty-brain` binary entry point: init logging, parse, resolve namespace OFF
//! the async runtime, then dispatch on the runtime, and map the exit code.

use clap::Parser;
use rusty_brain::cli::Cli;
use rusty_brain::logging::init_logging;
use rusty_brain::namespace_detect::detect_namespace;
use rusty_brain::run::run;
use std::process::ExitCode;

fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    // Resolve the namespace synchronously BEFORE the runtime exists: detection
    // shells out to git and reads `CLAUDE.md`, which must not run on a tokio
    // worker thread (P1 should-fix). It never fails (degrades to Global).
    let namespace = detect_namespace();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli, namespace)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml --bin rusty-brain detect_namespace_runs_without`
  Expected: PASS — the bin builds and `detect_namespace_runs_without_a_tokio_runtime` passes.

- [ ] **Step 5: Whole-crate test + lint + format.** Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: PASS (all `rusty-brain` unit + integration tests). Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- -D warnings`
  Expected: no warnings. Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- --check`
  Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rusty-brain/src/main.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "fix(rusty-brain): detect namespace off the async runtime before block_on"`
  Expected: one commit created.

## Part M — Graph-link generation + access tracking (closes the inert-graph v1 gap)

### Task 5: rb-store `record_access` + `supersede` — access bumping and supersession (Store trait + SqliteStore + tests)

**Files:**
- Modify: `crates/rb-store/src/store.rs` (extend the `Store` trait, add two `SqliteStore` methods, add tests)

> The schema columns already exist (`access_count`, `last_accessed_at`, `superseded_by`, `archived_at`), so no migration is needed. `record_access` bumps `access_count` and stamps `last_accessed_at`; `supersede` points an old memory at a new one AND archives the old one in ONE transaction. Both are write-path methods (they mutate), so in the daemon they will be routed through the single writer thread (Task 8). Here we add them to the synchronous `Store` trait + `SqliteStore` and prove them against an in-memory DB.

- [ ] **Step 1: Write the failing tests.** Append a new test module to the end of `crates/rb-store/src/store.rs` (after the existing `add_link_tests` module). These build real notes via `insert_memory`, then exercise the two new methods and assert the round-trip through `get_memory`:

```rust
#[cfg(test)]
mod access_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn node(store: &SqliteStore, c: &str) -> MemoryNote {
        let m = MemoryNote::new(Namespace::Project("rb".into()), c.into(), MemoryType::Insight, 5);
        store.insert_memory(&m, None).unwrap();
        m
    }

    #[test]
    fn record_access_bumps_count_and_sets_last_accessed() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = node(&store, "accessed");
        // Fresh note: access_count 0, last_accessed_at None.
        let before = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(before.access_count, 0);
        assert!(before.last_accessed_at.is_none());

        store.record_access(&a.id).unwrap();
        let after = store.get_memory(&a.id).unwrap().unwrap();
        assert_eq!(after.access_count, 1);
        assert!(after.last_accessed_at.is_some());

        // A second access increments again.
        store.record_access(&a.id).unwrap();
        assert_eq!(store.get_memory(&a.id).unwrap().unwrap().access_count, 2);
    }

    #[test]
    fn record_access_missing_id_is_ok_noop() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // No row updated; must not error (best-effort access tracking).
        store.record_access(&MemoryId::new()).unwrap();
    }

    #[test]
    fn supersede_sets_superseded_by_and_archives_old() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = node(&store, "old decision");
        let new = node(&store, "new decision");

        store.supersede(&old.id, &new.id).unwrap();

        let got = store.get_memory(&old.id).unwrap().unwrap();
        assert_eq!(got.superseded_by.as_ref(), Some(&new.id));
        assert!(got.archived_at.is_some(), "superseded note is archived");
        // The new note is untouched.
        let new_got = store.get_memory(&new.id).unwrap().unwrap();
        assert!(new_got.superseded_by.is_none());
        assert!(new_got.archived_at.is_none());
    }

    #[test]
    fn supersede_excludes_old_from_keyword_and_list() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let old = node(&store, "supersede excludes me");
        let new = node(&store, "supersede keeps me");
        store.supersede(&old.id, &new.id).unwrap();

        // old is archived -> excluded from keyword + list; new remains.
        let kw = store.keyword_search(&proj, "supersede", 10).unwrap();
        assert!(kw.contains(&new.id));
        assert!(!kw.contains(&old.id));
        let listed: Vec<MemoryId> = store.list(&proj, None, 10).unwrap().into_iter().map(|n| n.id).collect();
        assert!(listed.contains(&new.id));
        assert!(!listed.contains(&old.id));
    }

    #[test]
    fn supersede_missing_new_target_fails_fk() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = node(&store, "old");
        // superseded_by REFERENCES memories(memory_id); a missing target must fail
        // the FK and leave the old note unchanged (transaction rolled back).
        // foreign_keys=ON is set in SqliteStore::init, so the FK is enforced
        // immediately on the UPDATE statement (SQLite FKs are not deferred by default).
        let err = store.supersede(&old.id, &MemoryId::new()).unwrap_err();
        assert!(matches!(err, Error::Storage(_)));
        let got = store.get_memory(&old.id).unwrap().unwrap();
        assert!(got.superseded_by.is_none(), "rolled back: no superseded_by");
        assert!(got.archived_at.is_none(), "rolled back: not archived");
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-store access_tests` Expected: FAIL to compile (`no method named 'record_access'` / `'supersede'` found for `SqliteStore`).

- [ ] **Step 3: Declare the two methods on the `Store` trait.** In `crates/rb-store/src/store.rs`, inside `pub trait Store { ... }`, add these two method signatures immediately after the existing `fn add_link(&self, link: &MemoryLink) -> Result<()>;` line:

```rust
    /// Bump `access_count` and stamp `last_accessed_at = now` for `id`.
    /// A missing id is a no-op (best-effort access tracking never errors on absence).
    fn record_access(&self, id: &MemoryId) -> Result<()>;
    /// Mark `old` as superseded by `new` AND archive `old`, in one transaction.
    /// Fails closed (rolls back) if `new` does not exist (FK on `superseded_by`).
    fn supersede(&self, old: &MemoryId, new: &MemoryId) -> Result<()>;
```

- [ ] **Step 4: Implement both methods on `SqliteStore`.** In the `impl Store for SqliteStore { ... }` block, add these after the existing `fn add_link(...)` implementation (before the closing `}` of the impl). `record_access` uses a single `UPDATE`; `supersede` wraps two `UPDATE`s in an `IMMEDIATE` transaction (mirroring `insert_memory`'s commit/rollback pattern) so the supersession + archive are atomic and the FK violation rolls both back:

```rust
    fn record_access(&self, id: &MemoryId) -> Result<()> {
        self.conn
            .execute(
                "UPDATE memories
                 SET access_count = access_count + 1, last_accessed_at = ?1
                 WHERE memory_id = ?2",
                rusqlite::params![chrono::Utc::now().timestamp(), id.to_string()],
            )
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    fn supersede(&self, old: &MemoryId, new: &MemoryId) -> Result<()> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(|e| Error::Storage(e.to_string()))?;

        let now = chrono::Utc::now().timestamp();
        let result = (|| -> Result<()> {
            // Point old -> new. FK on superseded_by makes a missing `new` fail here,
            // rolling back the whole transaction (old stays unarchived).
            self.conn
                .execute(
                    "UPDATE memories SET superseded_by = ?1, updated_at = ?2 WHERE memory_id = ?3",
                    rusqlite::params![new.to_string(), now, old.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            // Archive old (idempotent: only if currently active).
            self.conn
                .execute(
                    "UPDATE memories SET archived_at = ?1, updated_at = ?1
                     WHERE memory_id = ?2 AND archived_at IS NULL",
                    rusqlite::params![now, old.to_string()],
                )
                .map_err(|e| Error::Storage(e.to_string()))?;
            Ok(())
        })();

        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT;")
                .map_err(|e| Error::Storage(e.to_string())),
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }
```

  (verify against installed `rusqlite` at execution; `execute`/`execute_batch`/`params!` are the same forms already used by `insert_memory` in this file, so they are known-good here.)

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-store access_tests` Expected: PASS (5 tests pass).

- [ ] **Step 6: Run the whole crate to catch any other `impl Store` that now needs the methods.** Run: `cargo test -p rb-store` Expected: PASS (the only production `Store` impl is `SqliteStore`; if a test-only mock `Store` exists elsewhere it must gain the two methods — add them mirroring the in-memory shape).

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-store/src/store.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-store): add record_access and transactional supersede"`

---

### Task 6: rb-store `get_many` — batch ns-scoped fetch (fixes recall N+1)

**Files:**
- Modify: `crates/rb-store/src/store.rs` (extend the `Store` trait, add the `SqliteStore` method, add tests)

> `recall` currently fetches each candidate with a separate `get_memory` round-trip (N+1). `get_many(ns, ids)` does ONE query: it loads all requested notes that belong to `ns`, returning them in the SAME order as `ids` (missing/out-of-namespace ids are skipped). Links are loaded per returned note via the existing `load_links` helper (called from `row_to_note`). Order preservation lets `recall` look results up by id without re-sorting.

- [ ] **Step 1: Write the failing tests.** Append a new test module to the end of `crates/rb-store/src/store.rs`:

```rust
#[cfg(test)]
mod get_many_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn node(store: &SqliteStore, ns: &Namespace, c: &str) -> MemoryId {
        let m = MemoryNote::new(ns.clone(), c.into(), MemoryType::Insight, 5);
        store.insert_memory(&m, None).unwrap();
        m.id
    }

    #[test]
    fn get_many_returns_notes_in_request_order() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        let a = node(&store, &ns, "alpha");
        let b = node(&store, &ns, "bravo");
        let c = node(&store, &ns, "charlie");

        // Request in a non-storage order; result must follow request order.
        let got = store.get_many(&ns, &[c.clone(), a.clone(), b.clone()]).unwrap();
        let ids: Vec<MemoryId> = got.iter().map(|n| n.id.clone()).collect();
        assert_eq!(ids, vec![c, a, b]);
    }

    #[test]
    fn get_many_skips_missing_and_out_of_namespace() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("rb".into());
        let other = Namespace::Project("other".into());
        let in_ns = node(&store, &proj, "in scope");
        let foreign = node(&store, &other, "foreign");
        let missing = MemoryId::new();

        let got = store
            .get_many(&proj, &[missing, foreign.clone(), in_ns.clone()])
            .unwrap();
        let ids: Vec<MemoryId> = got.iter().map(|n| n.id.clone()).collect();
        // Only the in-namespace, existing id is returned.
        assert_eq!(ids, vec![in_ns]);
    }

    #[test]
    fn get_many_empty_input_returns_empty() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        assert!(store.get_many(&ns, &[]).unwrap().is_empty());
    }

    #[test]
    fn get_many_loads_links_for_each_note() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ns = Namespace::Project("rb".into());
        let a = node(&store, &ns, "src");
        let b = node(&store, &ns, "dst");
        store
            .add_link(&rb_types::MemoryLink {
                source_id: a.clone(),
                target_id: b.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.8,
                reason: "rel".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();

        let got = store.get_many(&ns, &[a.clone()]).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].links.len(), 1);
        assert_eq!(got[0].links[0].target_id, b);
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-store get_many_tests` Expected: FAIL to compile (`no method named 'get_many'`).

- [ ] **Step 3: Add `get_many` to the `Store` trait.** In `pub trait Store`, add after the `supersede` signature from Task 5:

```rust
    /// Fetch all of `ids` that exist AND belong to `ns`, returned in the SAME
    /// order as `ids` (missing/out-of-namespace ids skipped). One query; fixes
    /// the recall N+1. Links are loaded per returned note.
    fn get_many(&self, ns: &Namespace, ids: &[MemoryId]) -> Result<Vec<MemoryNote>>;
```

- [ ] **Step 4: Implement `get_many` on `SqliteStore`.** Add to `impl Store for SqliteStore`, after `supersede`. It builds a single `IN (...)` query with one placeholder per id plus the namespace bind, decodes rows with the existing `row_to_note`, then re-orders to match the request. An empty `ids` short-circuits (an empty `IN ()` is invalid SQL). The SELECT column list is identical to `get_memory`/`list`:

```rust
    fn get_many(&self, ns: &Namespace, ids: &[MemoryId]) -> Result<Vec<MemoryNote>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        // Build "?2, ?3, ..." placeholders; ?1 is reserved for the namespace.
        let placeholders: Vec<String> = (0..ids.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "SELECT memory_id, namespace, created_at, updated_at, content, summary,
                    keywords, tags, context, memory_type, importance, confidence,
                    related_files, access_count, last_accessed_at, archived_at,
                    superseded_by, embedding_model
             FROM memories
             WHERE namespace = ?1 AND memory_id IN ({})",
            placeholders.join(", ")
        );

        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 1);
        params.push(Box::new(ns.as_db_string()));
        for id in ids {
            params.push(Box::new(id.to_string()));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let mut rows = stmt
            .query(refs.as_slice())
            .map_err(|e| Error::Storage(e.to_string()))?;

        // Decode into an id-keyed map, then re-emit in request order.
        let mut by_id: std::collections::HashMap<MemoryId, MemoryNote> = std::collections::HashMap::new();
        while let Some(row) = rows.next().map_err(|e| Error::Storage(e.to_string()))? {
            let note = row_to_note(&self.conn, row)?;
            by_id.insert(note.id.clone(), note);
        }

        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(note) = by_id.remove(id) {
                out.push(note);
            }
        }
        Ok(out)
    }
```

  (verify against installed `rusqlite` at execution; the dynamic-placeholder + boxed-`ToSql` pattern is identical to `update_memory` in this same file, and `stmt.query(&[&dyn ToSql])` accepts a `&[&dyn ToSql]` slice via the `Params` impl — so it is known-good here.)

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-store get_many_tests` Expected: PASS (4 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-store --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-store/src/store.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-store): add order-preserving ns-scoped get_many batch fetch"`

---

### Task 7: rb-engine `MemoryBackend` additions — `add_link`, `record_access`, `get_many`

**Files:**
- Modify: `crates/rb-engine/src/backend.rs` (extend the trait + the in-test mock impl)
- Modify: `crates/rb-engine/src/test_support.rs` (extend the shared mock impl + add introspection helpers)

> The engine is generic over `MemoryBackend`. To wire links (Task 10) and access tracking (Task 11) it needs three new backend methods: `add_link` (write path), `record_access` (write path), and `get_many` (read path, ns-scoped, fixes recall N+1). This task adds them to the trait and to BOTH in-test backends (`backend.rs`'s `MockBackend` and `test_support.rs`'s shared `MockBackend`) so the crate still compiles. The trait carries `#[async_trait::async_trait]`, so the new methods are written as bare `async fn` inside the trait body and inside each impl block. The daemon's `StoreHandle` gains them in Task 8.

- [ ] **Step 1: Write the failing test for the trait shape.** In `crates/rb-engine/src/backend.rs`, inside the existing `#[cfg(test)] mod tests`, add a test that drives the three new methods through the local `MockBackend` (place it after `mock_backend_archive_sets_archived_at`, before the module's closing brace):

```rust
    #[tokio::test]
    async fn mock_backend_supports_links_access_and_batch_fetch() {
        let backend = MockBackend::default();
        let ns = Namespace::Global;
        let a = MemoryNote::new(ns.clone(), "a".to_string(), MemoryType::Insight, 5);
        let b = MemoryNote::new(ns.clone(), "b".to_string(), MemoryType::Insight, 5);
        let (aid, bid) = (a.id.clone(), b.id.clone());
        backend.write(a, None).await.unwrap();
        backend.write(b, None).await.unwrap();

        // add_link is accepted (stored on the source note).
        backend
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.7,
                reason: "similar".to_string(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        // record_access bumps the count.
        backend.record_access(aid.clone()).await.unwrap();
        let got = backend.get(ns.clone(), aid.clone()).await.unwrap().unwrap();
        assert_eq!(got.access_count, 1);
        assert_eq!(got.links.len(), 1);

        // get_many returns ns-scoped notes in request order.
        let many = backend.get_many(ns, vec![bid.clone(), aid.clone()]).await.unwrap();
        let ids: Vec<rb_types::MemoryId> = many.iter().map(|n| n.id.clone()).collect();
        assert_eq!(ids, vec![bid, aid]);
    }
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine backend` Expected: FAIL to compile (`no method named 'add_link'` / `'record_access'` / `'get_many'` on the trait).

- [ ] **Step 3: Add the three methods to the trait.** In `crates/rb-engine/src/backend.rs`, inside `pub trait MemoryBackend` (which already has `#[async_trait::async_trait]`), add after the existing `archive` method:

```rust
    /// Persist a directed link (write path).
    async fn add_link(&self, link: rb_types::MemoryLink) -> rb_types::Result<()>;
    /// Bump access metadata for `id` (write path; best-effort at call sites).
    async fn record_access(&self, id: MemoryId) -> rb_types::Result<()>;
    /// Batch-fetch `ids` scoped to `ns`, in request order (read path).
    async fn get_many(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<Vec<MemoryNote>>;
```

  The trait's top-level `use rb_types::{MemoryId, MemoryNote, MemoryUpdates, Namespace};` already imports `MemoryId`, `MemoryNote`, `Namespace`; `MemoryLink` is referenced via the fully-qualified `rb_types::MemoryLink` so no new `use` is added (keeps the non-test build free of an unused-import warning).

- [ ] **Step 4: Implement the three methods on `backend.rs`'s in-test `MockBackend`.** Inside `impl MemoryBackend for MockBackend` in the `#[cfg(test)] mod tests` of `backend.rs`, add after `archive`:

```rust
        async fn add_link(&self, link: rb_types::MemoryLink) -> rb_types::Result<()> {
            let mut guard = self.notes.lock().unwrap();
            let note = guard
                .get_mut(&link.source_id)
                .ok_or_else(|| rb_types::Error::NotFound(link.source_id.clone()))?;
            note.links.push(link);
            Ok(())
        }
        async fn record_access(&self, id: MemoryId) -> rb_types::Result<()> {
            let mut guard = self.notes.lock().unwrap();
            if let Some(note) = guard.get_mut(&id) {
                note.access_count += 1;
                note.last_accessed_at = Some(chrono::Utc::now());
            }
            Ok(())
        }
        async fn get_many(
            &self,
            ns: Namespace,
            ids: Vec<MemoryId>,
        ) -> rb_types::Result<Vec<MemoryNote>> {
            let guard = self.notes.lock().unwrap();
            Ok(ids
                .iter()
                .filter_map(|id| guard.get(id).filter(|n| n.namespace == ns).cloned())
                .collect())
        }
```

- [ ] **Step 5: Implement the same three methods on the shared `test_support.rs` `MockBackend`.** This is the mock the engine tests use, so it must record links, support a configurable `record_access` failure (Task 11 asserts failure does not fail recall), and count `record_access` calls. First, add two fields + helpers to `test_support.rs`'s `MockBackend` struct.

  In the struct, add two fields after `vector_results`:

```rust
    record_access_calls: std::sync::atomic::AtomicUsize,
    fail_record_access: std::sync::atomic::AtomicBool,
```

  In `impl MockBackend`, add these helpers after `set_vector_results`:

```rust
    pub fn record_access_count(&self) -> usize {
        self.record_access_calls.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn set_fail_record_access(&self, fail: bool) {
        self.fail_record_access.store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn links_of(&self, id: &MemoryId) -> Vec<rb_types::MemoryLink> {
        self.notes.lock().unwrap().get(id).map(|n| n.links.clone()).unwrap_or_default()
    }
```

  Then add the three trait methods inside `impl MemoryBackend for MockBackend` (in `test_support.rs`), after `archive`:

```rust
    async fn add_link(&self, link: rb_types::MemoryLink) -> rb_types::Result<()> {
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&link.source_id)
            .ok_or_else(|| rb_types::Error::NotFound(link.source_id.clone()))?;
        note.links.push(link);
        Ok(())
    }
    async fn record_access(&self, id: MemoryId) -> rb_types::Result<()> {
        use std::sync::atomic::Ordering;
        self.record_access_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_record_access.load(Ordering::SeqCst) {
            return Err(rb_types::Error::Storage("record_access forced failure".to_string()));
        }
        if let Some(note) = self.notes.lock().unwrap().get_mut(&id) {
            note.access_count += 1;
            note.last_accessed_at = Some(chrono::Utc::now());
        }
        Ok(())
    }
    async fn get_many(
        &self,
        ns: Namespace,
        ids: Vec<MemoryId>,
    ) -> rb_types::Result<Vec<MemoryNote>> {
        let guard = self.notes.lock().unwrap();
        Ok(ids
            .iter()
            .filter_map(|id| guard.get(id).filter(|n| n.namespace == ns).cloned())
            .collect())
    }
```

  (Note: `#[derive(Default)]` on the struct still works — `AtomicUsize`/`AtomicBool` implement `Default` as 0/false. The file already has `#![allow(clippy::unwrap_used, clippy::expect_used)]` at its top, so the `.lock().unwrap()` calls in the new helpers/methods are allowed.)

- [ ] **Step 6: Run it — expect PASS.** Run: `cargo test -p rb-engine backend` Expected: PASS (the new `mock_backend_supports_links_access_and_batch_fetch` plus existing backend tests pass). Run: `cargo test -p rb-engine` Expected: PASS (the shared `test_support` mock now satisfies the extended trait; existing engine tests still pass).

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-engine/src/backend.rs crates/rb-engine/src/test_support.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-engine): extend MemoryBackend with add_link, record_access, get_many"`

---

### Task 8: rb-daemon `StoreHandle` — implement the three new backend methods (writer + read pool)

**Files:**
- Modify: `crates/rb-daemon/src/store_handle.rs` (add `WriteCommand` variants, writer-loop arms, and the three `MemoryBackend` methods; add an integration test)
- Modify: `crates/rb-daemon/Cargo.toml` (add `chrono` as a dev-dependency — the new test constructs a `MemoryLink` whose `created_at` requires `chrono::Utc::now()`, and rb-daemon does not currently depend on chrono)

> Adding `rb-engine`'s new trait methods makes `impl MemoryBackend for StoreHandle` incomplete, so the daemon will not compile until they exist. `add_link` and `record_access` MUTATE, so they go through the single writer thread (new `WriteCommand` variants + oneshot reply, mirroring `Insert`/`Archive`). `get_many` is a read, so it runs on the read pool via `with_read`. NEITHER `add_link` NOR `record_access` emits a `MemoryChanged` event: `record_access` is metadata churn on every recall, and `add_link` is an internal best-effort side effect of link generation (the SPINE does not require an event for it). Keeping both event-free also lets each writer arm run ALL of its store work inside `run_store_op` (the `catch_unwind` panic-safety wrapper), with no out-of-band reads on the single write connection.

- [ ] **Step 1: Add `chrono` to rb-daemon dev-dependencies.** In `crates/rb-daemon/Cargo.toml`, under `[dev-dependencies]`, add (alongside the existing `serde_json`/`tempfile`):

```toml
chrono = { workspace = true }
```

  (The new test in Step 2 builds `MemoryLink { ..., created_at: chrono::Utc::now() }`; rb-daemon has no chrono dependency today, so without this the test fails to compile.)

- [ ] **Step 2: Write the failing integration test.** Append to the existing `#[cfg(test)] mod tests` in `crates/rb-daemon/src/store_handle.rs` (after `writer_reopens_after_caught_store_panic`, before the module's closing brace). It uses a multi-thread runtime because the writer thread blocks:

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn store_handle_add_link_record_access_and_get_many() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("p2".to_string());

        let a = note(&ns, "source note");
        let b = note(&ns, "target note");
        let (aid, bid) = (a.id.clone(), b.id.clone());
        handle.write(a, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.write(b, Some(vec![0.2f32; DIM])).await.unwrap();

        // add_link goes through the writer and is visible via get (links loaded).
        handle
            .add_link(rb_types::MemoryLink {
                source_id: aid.clone(),
                target_id: bid.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.6,
                reason: "similar".to_string(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        let got = handle.get(ns.clone(), aid.clone()).await.unwrap().unwrap();
        assert_eq!(got.links.len(), 1);
        assert_eq!(got.links[0].target_id, bid);

        // record_access goes through the writer and bumps the count.
        handle.record_access(aid.clone()).await.unwrap();
        let after = handle.get(ns.clone(), aid.clone()).await.unwrap().unwrap();
        assert_eq!(after.access_count, 1);

        // get_many returns ns-scoped notes in request order via the read pool.
        let many = handle
            .get_many(ns, vec![bid.clone(), aid.clone()])
            .await
            .unwrap();
        let ids: Vec<rb_types::MemoryId> = many.iter().map(|n| n.id.clone()).collect();
        assert_eq!(ids, vec![bid, aid]);

        handle.shutdown().await;
    }
```

- [ ] **Step 3: Run it — expect FAIL.** Run: `cargo test -p rb-daemon store_handle_add_link_record_access_and_get_many` Expected: FAIL to compile (`StoreHandle` does not implement the new trait methods; `no method named 'add_link'` etc.).

- [ ] **Step 4: Add the two `WriteCommand` variants.** In `crates/rb-daemon/src/store_handle.rs`, inside `enum WriteCommand`, add after the `Archive { ... }` variant:

```rust
    AddLink {
        link: Box<rb_types::MemoryLink>,
        reply: oneshot::Sender<Result<()>>,
    },
    RecordAccess {
        id: MemoryId,
        reply: oneshot::Sender<Result<()>>,
    },
```

- [ ] **Step 5: Handle the new variants in `writer_loop`.** In the `match cmd { ... }` block of `writer_loop`, add these arms after the `WriteCommand::Archive { .. }` arm. Both run their store mutation through `run_store_op` (panic-safe) and emit NO `MemoryChanged` event — mirroring the panic-safety pattern of the other arms with no out-of-band read on the write connection:

```rust
            WriteCommand::AddLink { link, reply } => {
                let report =
                    run_store_op(&mut store, &db_path, embedding_dim, |s| s.add_link(&*link));
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
            WriteCommand::RecordAccess { id, reply } => {
                let report =
                    run_store_op(&mut store, &db_path, embedding_dim, |s| s.record_access(&id));
                let writer_usable = report.writer_usable;
                let _ = reply.send(report.result);
                if !writer_usable {
                    break;
                }
            }
```

  (verify against installed code at execution: `run_store_op`'s closure takes `&SqliteStore` and returns `Result<()>`; `s.add_link(&*link)` passes `&MemoryLink` from the `Box`, and `s.record_access(&id)` takes `&MemoryId` — both exist from Tasks 8/9, so these closures type-check.)

- [ ] **Step 6: Implement the three `MemoryBackend` methods on `StoreHandle`.** In `impl MemoryBackend for StoreHandle`, add after `archive`. Writes use `send_write` (the existing mpsc+oneshot helper); the read uses `with_read`:

```rust
    async fn add_link(&self, link: rb_types::MemoryLink) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::AddLink {
            link: Box::new(link),
            reply,
        };
        self.send_write(cmd, rx).await
    }

    async fn record_access(&self, id: MemoryId) -> Result<()> {
        let (reply, rx) = oneshot::channel();
        let cmd = WriteCommand::RecordAccess { id, reply };
        self.send_write(cmd, rx).await
    }

    async fn get_many(&self, ns: Namespace, ids: Vec<MemoryId>) -> Result<Vec<MemoryNote>> {
        self.with_read(move |store| store.get_many(&ns, &ids)).await
    }
```

  The `use rb_store::{SqliteStore, Store};` already in scope brings the new `Store::get_many`/`add_link`/`record_access` methods into scope on the closure's `&SqliteStore`.

- [ ] **Step 7: Run it — expect PASS.** Run: `cargo test -p rb-daemon store_handle_add_link_record_access_and_get_many` Expected: PASS. Run: `cargo test -p rb-daemon` Expected: PASS (existing daemon tests unaffected; the writer loop's new arms compile and the `#[cfg(test)] PanicForTest` arm is untouched).

- [ ] **Step 8: Lint + format.** Run: `cargo clippy -p rb-daemon --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 9: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-daemon/src/store_handle.rs crates/rb-daemon/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-daemon): route add_link/record_access through writer and get_many through read pool"`

---

### Task 9: rb-engine `Linker` trait + `SimilarityLinker` (pure, offline, deterministic)

**Files:**
- Create: `crates/rb-engine/src/linker.rs`
- Modify: `crates/rb-engine/src/lib.rs` (add `mod linker;` + re-exports)

> The hybrid-search graph dimension is currently inert because nothing CREATES links. The `Linker` trait turns vector-search candidates into `MemoryLink`s; the default `SimilarityLinker` emits a `References` link for every candidate within a distance threshold, with strength derived from distance, capped at `max_links`, skipping self. It is pure (no IO, no clock dependency beyond stamping `created_at`), so it is fully unit-testable. It is wired into `remember` in Task 10.

- [ ] **Step 1: Write the failing tests AND declare the module.** Add `mod linker;` to `crates/rb-engine/src/lib.rs` (after `mod engine;`), then create `crates/rb-engine/src/linker.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{LinkType, MemoryNote, MemoryType, Namespace};

    fn note(content: &str) -> MemoryNote {
        MemoryNote::new(Namespace::Project("rb".into()), content.to_string(), MemoryType::Insight, 5)
    }

    #[test]
    fn links_candidates_within_threshold_only() {
        let new = note("new memory");
        let near = note("near");
        let far = note("far");
        let candidates = vec![(near.clone(), 0.2_f32), (far.clone(), 1.9_f32)];
        let linker = SimilarityLinker::new(10, 1.0);
        let links = linker.link(&new, &candidates);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_id, new.id);
        assert_eq!(links[0].target_id, near.id);
        assert_eq!(links[0].link_type, LinkType::References);
        assert_eq!(links[0].reason, "similar");
    }

    #[test]
    fn strength_is_one_minus_half_distance_clamped() {
        let new = note("new");
        let c = note("c");
        let linker = SimilarityLinker::new(10, 2.0);
        // distance 0.0 -> strength 1.0
        let s0 = linker.link(&new, &[(c.clone(), 0.0)])[0].strength;
        assert!((s0 - 1.0).abs() < 1e-6);
        // distance 1.0 -> strength 0.5
        let s1 = linker.link(&new, &[(c.clone(), 1.0)])[0].strength;
        assert!((s1 - 0.5).abs() < 1e-6);
        // distance 2.0 -> strength 0.0 (clamp floor)
        let s2 = linker.link(&new, &[(c.clone(), 2.0)])[0].strength;
        assert!((s2 - 0.0).abs() < 1e-6);
    }

    #[test]
    fn caps_at_max_links_preserving_candidate_order() {
        let new = note("new");
        let a = note("a");
        let b = note("b");
        let c = note("c");
        let candidates = vec![(a.clone(), 0.1), (b.clone(), 0.2), (c.clone(), 0.3)];
        let linker = SimilarityLinker::new(2, 1.0);
        let links = linker.link(&new, &candidates);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target_id, a.id);
        assert_eq!(links[1].target_id, b.id);
    }

    #[test]
    fn skips_self_candidate() {
        let new = note("new");
        // A candidate whose id equals the new note's id must be skipped.
        let mut me = note("dup");
        me.id = new.id.clone();
        let other = note("other");
        let candidates = vec![(me, 0.1), (other.clone(), 0.2)];
        let linker = SimilarityLinker::new(10, 1.0);
        let links = linker.link(&new, &candidates);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_id, other.id);
    }

    #[test]
    fn empty_candidates_yields_no_links() {
        let new = note("new");
        let linker = SimilarityLinker::new(5, 1.0);
        assert!(linker.link(&new, &[]).is_empty());
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine linker` Expected: FAIL to compile (`cannot find type 'SimilarityLinker'` / trait `Linker`).

- [ ] **Step 3: Add the trait + `SimilarityLinker` above the test module.** Prepend to `crates/rb-engine/src/linker.rs`:

```rust
use rb_types::{LinkType, MemoryLink, MemoryNote};

/// Generates links for a newly-stored memory from a set of candidate
/// (note, vector_distance) pairs. Pure: no IO, deterministic for a given input.
pub trait Linker: Send + Sync {
    /// Produce links FROM `new` TO selected candidates. `candidates` are
    /// `(note, vector_distance)` where smaller distance = more similar.
    fn link(&self, new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> Vec<MemoryLink>;
}

/// Default linker: a `References` link to every candidate within
/// `distance_threshold`, strength = `(1 - distance/2).clamp(0,1)`, capped at
/// `max_links`, skipping the new note itself. Offline and deterministic.
pub struct SimilarityLinker {
    max_links: usize,
    distance_threshold: f32,
}

impl SimilarityLinker {
    pub fn new(max_links: usize, distance_threshold: f32) -> Self {
        Self {
            max_links,
            distance_threshold,
        }
    }
}

impl Default for SimilarityLinker {
    /// Conservative defaults: at most 5 links, only fairly-similar candidates.
    fn default() -> Self {
        Self {
            max_links: 5,
            distance_threshold: 0.6,
        }
    }
}

impl Linker for SimilarityLinker {
    fn link(&self, new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> Vec<MemoryLink> {
        let now = chrono::Utc::now();
        let mut links = Vec::new();
        for (candidate, distance) in candidates {
            if links.len() >= self.max_links {
                break;
            }
            if candidate.id == new.id {
                continue; // never link to self
            }
            if *distance > self.distance_threshold {
                continue;
            }
            let strength = (1.0 - distance / 2.0).clamp(0.0, 1.0);
            links.push(MemoryLink {
                source_id: new.id.clone(),
                target_id: candidate.id.clone(),
                link_type: LinkType::References,
                strength,
                reason: "similar".to_string(),
                created_at: now,
            });
        }
        links
    }
}
```

- [ ] **Step 4: Re-export from `lib.rs`.** In `crates/rb-engine/src/lib.rs`, add to the `pub use` block:

```rust
pub use linker::{Linker, SimilarityLinker};
```

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-engine linker` Expected: PASS (5 tests pass).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-engine/src/linker.rs crates/rb-engine/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-engine): add Linker trait and pure SimilarityLinker"`

---

### Task 10: rb-engine — wire `SimilarityLinker` into `remember` (best-effort link generation)

**Files:**
- Modify: `crates/rb-engine/src/engine.rs` (hold a `Linker`, generate links after `write`, add tests)
- Modify: `crates/rb-engine/src/test_support.rs` (add a `fail_add_link` toggle so a test can force `add_link` failure)
- Modify: `crates/rb-engine/Cargo.toml` (add `tracing` — the engine now logs best-effort link failures)

> After `remember` writes the new note, it vector-searches in-namespace for top candidates (excluding the new id), batch-fetches their notes with `get_many`, runs the linker, and calls `backend.add_link` for each produced link. Link creation is BEST-EFFORT: a failure is logged and skipped, never failing the `remember`. The engine gains a boxed `Linker` field defaulting to `SimilarityLinker::default()`.

- [ ] **Step 1: Write the failing tests.** Add these tests INSIDE the existing `#[cfg(test)] mod tests` block in `crates/rb-engine/src/engine.rs` (after the `remember_*` tests, before the recall tests):

```rust
    #[tokio::test]
    async fn remember_creates_links_to_similar_existing_memories() {
        let eng = engine();
        // First memory: nothing to link to.
        let first = eng.remember(input("single writer over sqlite wal", 5)).await.unwrap();
        assert!(eng.backend().links_of(&first).is_empty());

        // Second memory: the deterministic mock vector() returns the first as a
        // candidate at distance 0.0 (<= threshold), so a link is created.
        let second = eng.remember(input("concurrent readers never block", 5)).await.unwrap();
        let links = eng.backend().links_of(&second);
        assert!(!links.is_empty(), "remember should link to the prior similar memory");
        assert_eq!(links[0].source_id, second);
        assert!(links.iter().all(|l| l.target_id != second), "never links to self");
        assert!(links.iter().any(|l| l.target_id == first));
        assert!(links.iter().all(|l| l.link_type == rb_types::LinkType::References));
    }

    #[tokio::test]
    async fn remember_link_failure_does_not_fail_remember() {
        // A backend whose add_link always fails must not break remember.
        let eng = engine();
        let _first = eng.remember(input("anchor", 5)).await.unwrap();
        eng.backend().set_fail_add_link(true);
        // Should still succeed (best-effort linking).
        let id = eng.remember(input("second", 5)).await.unwrap();
        assert!(eng.backend().note_of(&id).is_some());
    }
```

  Add a toggle to the shared mock so the second test can force `add_link` to fail. In `crates/rb-engine/src/test_support.rs`, add a field to the struct (after `fail_record_access`):

```rust
    fail_add_link: std::sync::atomic::AtomicBool,
```

  add a setter in `impl MockBackend` (after `set_fail_record_access`):

```rust
    pub fn set_fail_add_link(&self, fail: bool) {
        self.fail_add_link.store(fail, std::sync::atomic::Ordering::SeqCst);
    }
```

  and make `add_link` honor it — replace the `test_support.rs` `add_link` body added in Task 7 with:

```rust
    async fn add_link(&self, link: rb_types::MemoryLink) -> rb_types::Result<()> {
        if self.fail_add_link.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(rb_types::Error::Storage("add_link forced failure".to_string()));
        }
        let mut guard = self.notes.lock().unwrap();
        let note = guard
            .get_mut(&link.source_id)
            .ok_or_else(|| rb_types::Error::NotFound(link.source_id.clone()))?;
        note.links.push(link);
        Ok(())
    }
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine engine::tests::remember_creates_links` Expected: FAIL (no links created yet — `remember` does not call the linker; the assertion `!links.is_empty()` fails). `set_fail_add_link` is also an unknown method until the field/setter compile.

- [ ] **Step 3: Add the `tracing` dependency.** In `crates/rb-engine/Cargo.toml`, under `[dependencies]`, add (alongside `chrono`):

```toml
tracing = { workspace = true }
```

  (`tracing = "0.1"` is already a workspace dependency, so `{ workspace = true }` resolves.)

- [ ] **Step 4: Add the `linker` field and link-generation to `remember`.** In `crates/rb-engine/src/engine.rs`, first extend the imports at the top:

```rust
use crate::linker::{Linker, SimilarityLinker};
```

  Add a `linker` field to the struct (after `namespace: Namespace,`):

```rust
    linker: Box<dyn Linker>,
```

  Initialize it in `new` (after `namespace,`):

```rust
            linker: Box::new(SimilarityLinker::default()),
```

  At the END of `remember`, replace the tail (`let id = note.id.clone(); self.backend.write(note, embedding).await?; Ok(id)`) with a version that captures the embedding before the move and then runs best-effort link generation:

```rust
        let id = note.id.clone();
        // Keep a copy of the embedding for candidate search before the note moves.
        let embedding_for_links = embedding.clone();
        self.backend.write(note, embedding).await?;

        // Best-effort link generation: never fails the remember.
        if let Some(emb) = embedding_for_links {
            if let Err(e) = self.generate_links(&id, emb).await {
                tracing::warn!(error = %e, memory_id = %id, "link generation failed; continuing");
            }
        }
        Ok(id)
```

  Then add the private helper to the `impl<B: MemoryBackend, P: EmbeddingProvider> MemoryEngine<B, P>` block (place it right after `remember`):

```rust
    /// Vector-search for candidates similar to the just-written memory, fetch
    /// their notes, run the linker, and persist the produced links. Best-effort:
    /// callers ignore the error. `add_link` failures are logged and skipped so a
    /// single bad link never aborts the rest.
    async fn generate_links(&self, new_id: &MemoryId, embedding: Vec<f32>) -> rb_types::Result<()> {
        const CANDIDATE_LIMIT: usize = 8;
        let pairs = self
            .backend
            .vector(self.namespace.clone(), embedding, CANDIDATE_LIMIT)
            .await?;
        // Candidate ids exclude the new note itself.
        let candidate_ids: Vec<MemoryId> = pairs
            .iter()
            .filter(|(id, _)| id != new_id)
            .map(|(id, _)| id.clone())
            .collect();
        if candidate_ids.is_empty() {
            return Ok(());
        }
        let dist: std::collections::HashMap<MemoryId, f32> = pairs.into_iter().collect();
        let notes = self
            .backend
            .get_many(self.namespace.clone(), candidate_ids)
            .await?;
        let new_note = match self.backend.get(self.namespace.clone(), new_id.clone()).await? {
            Some(n) => n,
            None => return Ok(()),
        };
        let candidates: Vec<(MemoryNote, f32)> = notes
            .into_iter()
            .map(|n| {
                let d = dist.get(&n.id).copied().unwrap_or(f32::MAX);
                (n, d)
            })
            .collect();
        for link in self.linker.link(&new_note, &candidates) {
            if let Err(e) = self.backend.add_link(link).await {
                tracing::warn!(error = %e, "add_link failed; skipping one link");
            }
        }
        Ok(())
    }
```

  (verify against installed code: the mock `vector()` returns ALL in-namespace notes at distance 0.0, so the prior memory is a candidate within the default 0.6 threshold; the `SimilarityLinker` default `max_links=5` admits it. The mock `graph()` reads a separate map, not `note.links`, so generated links do not perturb recall graph expansion.)

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-engine engine::tests::remember` Expected: PASS (link tests + existing remember tests). Run: `cargo test -p rb-engine` Expected: PASS (no regression; recall tests unaffected — links on notes do not change recall ordering here).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings (the `dyn Linker` field is read by `generate_links`, so no dead-code). Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-engine/src/engine.rs crates/rb-engine/src/test_support.rs crates/rb-engine/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-engine): generate similarity links on remember (best-effort)"`

---

### Task 11: rb-engine — wire `record_access` into recall/get and replace recall N+1 with `get_many`

**Files:**
- Modify: `crates/rb-engine/src/engine.rs` (replace per-candidate `get_scoped` loop in `recall` with `get_many`; record access on recall/get results; add tests)

> Two changes: (1) `recall` currently fetches each candidate with its own `get_scoped` round-trip (N+1) — replace that with one `get_many`, then apply the active/filter checks in Rust. (2) After `recall` and `get` return results, call `backend.record_access` on the returned ids, BEST-EFFORT: a failure must not fail the response. Best-effort here means the access calls are awaited but their errors are logged-and-ignored (the engine stays generic over `B` with no extra `Clone`/`'static` bound, and the response is already computed before access is recorded).

- [ ] **Step 1: Write the failing tests.** Add these INSIDE the existing `#[cfg(test)] mod tests` block in `crates/rb-engine/src/engine.rs` (after the recall tests, before the pass-through tests). They use the shared mock's `record_access_count`/`set_fail_record_access` helpers from Task 7:

```rust
    #[tokio::test]
    async fn recall_bumps_access_count_on_returned_results() {
        let eng = engine();
        seed(&eng, "alpha sqlite topic", MemoryType::Insight, 5, &[]).await;
        seed(&eng, "beta tokio topic", MemoryType::Insight, 5, &[]).await;
        let results = eng.recall("topic", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 2);
        // each returned id had its access recorded.
        for r in &results {
            let note = eng.backend().note_of(&r.memory.id).unwrap();
            assert_eq!(note.access_count, 1);
        }
    }

    #[tokio::test]
    async fn recall_record_access_failure_does_not_fail_recall() {
        let eng = engine();
        seed(&eng, "probe content", MemoryType::Insight, 5, &[]).await;
        eng.backend().set_fail_record_access(true);
        // Recall still returns its results despite record_access failing.
        let results = eng.recall("probe", 10, None, &[]).await.unwrap();
        assert_eq!(results.len(), 1);
        // record_access was attempted (best-effort), even though it errored.
        assert!(eng.backend().record_access_count() >= 1);
    }

    #[tokio::test]
    async fn get_bumps_access_count_when_found() {
        let eng = engine();
        let id = eng.remember(input("findable", 5)).await.unwrap();
        let before = eng.backend().note_of(&id).unwrap().access_count;
        let got = eng.get(id.clone()).await.unwrap().unwrap();
        assert_eq!(got.id, id);
        // access recorded after a successful get.
        assert_eq!(eng.backend().note_of(&id).unwrap().access_count, before + 1);
    }
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine engine::tests::recall_bumps_access_count` Expected: FAIL (`access_count` is still 0 because nothing records access yet).

- [ ] **Step 3: Replace the recall N+1 fetch with `get_many` and record access.** In `recall` (in `crates/rb-engine/src/engine.rs`), replace the per-candidate fetch loop:

```rust
        // Fetch each candidate once; build the note cache + the rank meta map.
        let mut notes: HashMap<MemoryId, MemoryNote> = HashMap::new();
        let mut meta: HashMap<MemoryId, (u8, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        for id in &order {
            if let Some(note) = self.get_scoped(id.clone()).await? {
                if !self.active_in_namespace(&note)
                    || !Self::matches_recall_filters(&note, type_filter, tags)
                {
                    continue;
                }
                meta.insert(id.clone(), (note.importance, note.created_at));
                notes.insert(id.clone(), note);
            }
        }
```

  with a single batch fetch (ns-scoping is done in `get_many`; we still apply active + filter checks in Rust):

```rust
        // ONE batch fetch (fixes the N+1). get_many is ns-scoped and order-preserving.
        let fetched = self
            .backend
            .get_many(self.namespace.clone(), order.clone())
            .await?;
        let mut notes: HashMap<MemoryId, MemoryNote> = HashMap::new();
        let mut meta: HashMap<MemoryId, (u8, chrono::DateTime<chrono::Utc>)> = HashMap::new();
        for note in fetched {
            if !self.active_in_namespace(&note)
                || !Self::matches_recall_filters(&note, type_filter, tags)
            {
                continue;
            }
            meta.insert(note.id.clone(), (note.importance, note.created_at));
            notes.insert(note.id.clone(), note);
        }
```

  Then, at the END of `recall`, after `results` is fully built and BEFORE `Ok(results)`, record access best-effort on the returned ids:

```rust
        // Best-effort access tracking: never fails the response.
        let returned_ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id.clone()).collect();
        self.record_accesses(&returned_ids).await;
        Ok(results)
```

- [ ] **Step 4: Record access on `get` and add the shared helper.** In `crates/rb-engine/src/engine.rs`, change `get` to record access on a hit, and add the `record_accesses` helper. Replace the existing `get`:

```rust
    /// Fetch a single memory by id in the engine namespace.
    pub async fn get(&self, id: MemoryId) -> rb_types::Result<Option<MemoryNote>> {
        let found = self.get_scoped(id.clone()).await?;
        if found.is_some() {
            self.record_accesses(std::slice::from_ref(&id)).await;
        }
        Ok(found)
    }
```

  Add the helper to the `impl` block (place it after `context`):

```rust
    /// Record access for each id, best-effort. Errors are logged and ignored so
    /// access tracking never affects the response. Awaited inline (the response
    /// is already computed); the engine stays generic over `B` with no Clone bound.
    async fn record_accesses(&self, ids: &[MemoryId]) {
        for id in ids {
            if let Err(e) = self.backend.record_access(id.clone()).await {
                tracing::debug!(error = %e, memory_id = %id, "record_access failed; ignoring");
            }
        }
    }
```

  (`tracing` was added to `rb-engine`'s `Cargo.toml` in Task 10, so `tracing::debug!` resolves here.)

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-engine engine::tests` Expected: PASS (the three new access tests plus all existing engine tests). Run: `cargo test -p rb-engine` Expected: PASS.

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 7: Final cross-crate gate for Part M.** The three crates touched in this part build and pass together; run the workspace gates: Run: `cargo test -p rb-store -p rb-engine -p rb-daemon` Expected: PASS. Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-engine/src/engine.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-engine): batch recall fetch via get_many and record access on recall/get"`

## Part N — MCP stdio adapter (the primary agent interface)

### Task 12: scaffold the `rb-mcp` crate (workspace member + JSON-RPC envelope types)

This stands up a new focused crate `rb-mcp` that implements the MCP stdio adapter. It depends only on `rb-types`, `rb-proto`, `serde`, `serde_json`, `tokio`, `async-trait`, and `tracing`. This first task adds the crate to the workspace and defines the JSON-RPC 2.0 envelope types (`JsonRpcRequest`, `JsonRpcResponse`, `JsonRpcError`, and the well-known error codes) with serde round-trip tests. No transport, no routing yet — those land in Tasks 24-26.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/Cargo.toml`
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs`
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/jsonrpc.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/Cargo.toml` (add `crates/rb-mcp` to `members`)

- [ ] **Step 1: Add the crate to the workspace `members` list.** Edit `/Users/bluby/repos/rusty-brain-p2/Cargo.toml` so the `members` array includes the new crate immediately before `"crates/rusty-brain"`:

```toml
members = [
    "crates/rb-types",
    "crates/rb-store",
    "crates/rb-proto",
    "crates/rb-embed",
    "crates/rb-search",
    "crates/rb-engine",
    "crates/rb-daemon",
    "crates/rb-mcp",
    "crates/rusty-brain",
]
```

- [ ] **Step 2: Write the crate manifest.** Create `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/Cargo.toml` with exactly the dependencies the crate actually uses (no `tokio-util`/`thiserror`: the transport uses `tokio::io` directly and the error type is a plain struct):

```toml
[package]
name = "rb-mcp"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "rusty-brain: MCP (Model Context Protocol) stdio adapter."

[lib]
name = "rb_mcp"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
rb-proto = { path = "../rb-proto" }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
async-trait = { workspace = true }
tracing = { workspace = true }

[lints]
workspace = true
```

  (verify against the root manifest at execution: `async-trait`, `serde`, `serde_json`, `tokio`, `tracing` are all declared in `[workspace.dependencies]`; if any name differs, adjust this table. `serde_json` and `tokio` are also exercised by the `#[cfg(test)]` modules — they are normal dependencies and so are visible to the in-file tests without a separate `[dev-dependencies]` block.)

- [ ] **Step 3: Write the failing JSON-RPC envelope tests AND declare the modules.** Create `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs`:

```rust
//! `rb_mcp`: a thin Model Context Protocol (MCP) stdio adapter for rusty-brain.
//!
//! Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout (stdout carries ONLY
//! JSON-RPC frames; all logging goes to stderr). Each `tools/call` is routed to
//! an `rb_proto::Request` and forwarded to the daemon over the Unix socket via a
//! `DaemonProxy`. The adapter holds no storage of its own.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod jsonrpc;
```

  Then create `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/jsonrpc.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use serde_json::json;

    #[test]
    fn request_with_id_round_trips() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(json!(1)));
        assert!(req.params.is_some());
    }

    #[test]
    fn notification_has_no_id() {
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.method, "notifications/initialized");
        assert!(req.id.is_none(), "notification must have no id");
        assert!(req.is_notification());
    }

    #[test]
    fn explicit_null_id_is_treated_as_no_id() {
        // serde maps a JSON `null` for an `Option<Value>` field to `None`, so a
        // frame with `"id":null` is a notification, not a request with a null id.
        let raw = r#"{"jsonrpc":"2.0","id":null,"method":"notifications/initialized"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert!(req.id.is_none());
        assert!(req.is_notification());
    }

    #[test]
    fn success_response_serializes_result_not_error() {
        let resp = JsonRpcResponse::success(json!(7), json!({"ok": true}));
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"jsonrpc\":\"2.0\""), "{s}");
        assert!(s.contains("\"result\""), "{s}");
        assert!(!s.contains("\"error\""), "success must omit error: {s}");
        assert!(s.contains("\"id\":7"), "{s}");
    }

    #[test]
    fn error_response_serializes_error_not_result() {
        let resp = JsonRpcResponse::error(
            json!(7),
            JsonRpcError::new(METHOD_NOT_FOUND, "no such method".into()),
        );
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"error\""), "{s}");
        assert!(!s.contains("\"result\""), "error must omit result: {s}");
        assert!(s.contains("-32601"), "method-not-found code present: {s}");
    }

    #[test]
    fn error_codes_match_jsonrpc_spec() {
        assert_eq!(PARSE_ERROR, -32700);
        assert_eq!(INVALID_REQUEST, -32600);
        assert_eq!(METHOD_NOT_FOUND, -32601);
        assert_eq!(INVALID_PARAMS, -32602);
        assert_eq!(INTERNAL_ERROR, -32603);
    }
}
```

- [ ] **Step 4: Run it — expect a compile failure.**
  Run: `cargo test -p rb-mcp jsonrpc`
  Expected: FAIL to compile — `cannot find type 'JsonRpcRequest' in this scope` etc. Confirms the test drives new code.

- [ ] **Step 5: Add the envelope implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/jsonrpc.rs`:

```rust
//! JSON-RPC 2.0 envelope types for the MCP adapter.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard JSON-RPC 2.0 error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// One incoming JSON-RPC request or notification. A request carries an `id`; a
/// notification omits it (and gets no response). A JSON `null` id deserializes
/// to `None`, so it is also treated as a notification.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// A request with no `id` is a notification and must NOT be answered.
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// One outgoing JSON-RPC response: exactly one of `result` / `error` is set.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// A successful response carrying `result`.
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// A failed response carrying `error`.
    pub fn error(id: Value, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    /// Build an error with a code and message (no extra data).
    pub fn new(code: i64, message: String) -> Self {
        Self {
            code,
            message,
            data: None,
        }
    }
}
```

- [ ] **Step 6: Run it — expect PASS.**
  Run: `cargo test -p rb-mcp jsonrpc`
  Expected: PASS (6 tests: request-with-id, notification, explicit-null-id, success serialization, error serialization, error-code constants).

- [ ] **Step 7: Lint + format.**
  Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: no diff.

- [ ] **Step 8: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p2 add Cargo.toml crates/rb-mcp/Cargo.toml crates/rb-mcp/src/lib.rs crates/rb-mcp/src/jsonrpc.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-mcp): scaffold MCP crate with JSON-RPC 2.0 envelope types"`
  Expected: one commit created.

---

### Task 13: `tools.rs` — the 8 MCP tool definitions with JSON-Schema input

`tools/list` must return the 8 tools (`remember`, `recall`, `get`, `list`, `graph`, `update`, `delete`, `context`) each with a `name`, `description`, and a JSON-Schema `inputSchema`. This task defines a pure `tool_definitions() -> Vec<ToolDef>` returning serde-serializable tool descriptors, with tests asserting the exact tool set, the required vs optional fields per the spine, and that each `inputSchema` is a valid JSON object with `"type":"object"`. No daemon, no transport — pure data. (Note: the `importance`/`min_importance` 1..=10 bounds advertised here are advisory hints to agents; the daemon engine is the authoritative validator, so an out-of-range value fails closed server-side as a `Response::Error` rather than being clamped by the adapter.)

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/tools.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` (declare `pub mod tools;`)

- [ ] **Step 1: Write the failing tests AND declare the module.** Add `pub mod tools;` to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` (after `pub mod jsonrpc;`), then create `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/tools.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn exposes_exactly_the_eight_spine_tools() {
        let names: BTreeSet<&str> = tool_definitions().iter().map(|t| t.name).collect();
        let expected: BTreeSet<&str> = [
            "remember", "recall", "get", "list", "graph", "update", "delete", "context",
        ]
        .into_iter()
        .collect();
        assert_eq!(names, expected, "tool set must match the spine exactly");
        assert_eq!(tool_definitions().len(), 8);
    }

    #[test]
    fn every_tool_has_object_input_schema_and_nonempty_description() {
        for t in tool_definitions() {
            assert!(!t.description.is_empty(), "tool {} needs a description", t.name);
            assert_eq!(
                t.input_schema["type"], "object",
                "tool {} inputSchema must be an object",
                t.name
            );
            // `properties` must be present (possibly empty for `context`).
            assert!(
                t.input_schema.get("properties").is_some(),
                "tool {} inputSchema needs properties",
                t.name
            );
        }
    }

    #[test]
    fn remember_requires_content_only() {
        let t = tool_definitions()
            .into_iter()
            .find(|t| t.name == "remember")
            .unwrap();
        let required: Vec<&str> = t.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["content"]);
        let props = t.input_schema["properties"].as_object().unwrap();
        for opt in ["context", "type", "importance", "tags"] {
            assert!(props.contains_key(opt), "remember should accept optional {opt}");
        }
    }

    #[test]
    fn recall_requires_query_and_get_requires_id() {
        let recall = tool_definitions()
            .into_iter()
            .find(|t| t.name == "recall")
            .unwrap();
        assert_eq!(recall.input_schema["required"][0], "query");
        let get = tool_definitions()
            .into_iter()
            .find(|t| t.name == "get")
            .unwrap();
        assert_eq!(get.input_schema["required"][0], "id");
    }

    #[test]
    fn context_takes_no_required_input() {
        let t = tool_definitions()
            .into_iter()
            .find(|t| t.name == "context")
            .unwrap();
        // No `required` key, or an empty required array — either is acceptable.
        let required_empty = match t.input_schema.get("required") {
            None => true,
            Some(v) => v.as_array().map(|a| a.is_empty()).unwrap_or(false),
        };
        assert!(required_empty, "context must require no input");
    }

    #[test]
    fn tool_list_serializes_with_camelcase_input_schema() {
        // The MCP wire shape uses `inputSchema` (camelCase).
        let t = &tool_definitions()[0];
        let s = serde_json::to_string(t).unwrap();
        assert!(s.contains("\"inputSchema\""), "must serialize as inputSchema: {s}");
        assert!(!s.contains("input_schema"), "must not leak snake_case: {s}");
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-mcp tools`
  Expected: FAIL to compile — `cannot find function 'tool_definitions'`. Confirms the test drives new code.

- [ ] **Step 3: Add the tool definitions above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/tools.rs`:

```rust
//! The 8 MCP tools rusty-brain exposes, each with a JSON-Schema input contract.

use serde::Serialize;
use serde_json::{json, Value};

/// One MCP tool descriptor as returned by `tools/list`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// The valid `memory_type` db strings, surfaced as a JSON-Schema enum so agents
/// pick a legal value. Mirrors `rb_types::MemoryType::as_str`.
fn memory_type_enum() -> Value {
    json!([
        "architecture_decision",
        "code_pattern",
        "bug_fix",
        "configuration",
        "constraint",
        "entity",
        "insight",
        "reference",
        "preference"
    ])
}

/// All 8 tool definitions. Pure: no IO, deterministic ordering.
pub fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "remember",
            description: "Store a new memory in the shared store. Returns the new memory id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The text to remember." },
                    "context": { "type": "string", "description": "Optional surrounding context." },
                    "type": { "type": "string", "enum": memory_type_enum(),
                              "description": "Memory category (default: insight)." },
                    "importance": { "type": "integer", "minimum": 1, "maximum": 10,
                                    "description": "Importance 1-10 (default: 5)." },
                    "tags": { "type": "array", "items": { "type": "string" },
                              "description": "Optional tags." }
                },
                "required": ["content"]
            }),
        },
        ToolDef {
            name: "recall",
            description: "Hybrid (keyword + vector + graph) recall of memories matching a query.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Free-text query." },
                    "limit": { "type": "integer", "minimum": 1,
                               "description": "Max results (default: 10)." },
                    "type": { "type": "string", "enum": memory_type_enum(),
                              "description": "Restrict to a memory type." },
                    "tags": { "type": "array", "items": { "type": "string" },
                              "description": "Filter by tags." }
                },
                "required": ["query"]
            }),
        },
        ToolDef {
            name: "get",
            description: "Fetch a single memory (full content + links) by id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Memory id (UUID)." }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "list",
            description: "List memories in the current namespace.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "minimum": 1,
                               "description": "Max results (default: 20)." },
                    "min_importance": { "type": "integer", "minimum": 1, "maximum": 10,
                                        "description": "Only memories at/above this importance." }
                }
            }),
        },
        ToolDef {
            name: "graph",
            description: "Show memories connected to an id by graph links.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Memory id (UUID)." },
                    "depth": { "type": "integer", "minimum": 1,
                               "description": "Traversal depth (default: 1)." }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "update",
            description: "Apply a partial update to a memory (only provided fields change).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Memory id (UUID)." },
                    "content": { "type": "string" },
                    "summary": { "type": "string" },
                    "importance": { "type": "integer", "minimum": 1, "maximum": 10 },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "context": { "type": "string" }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "delete",
            description: "Soft-delete (archive) a memory by id.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Memory id (UUID)." }
                },
                "required": ["id"]
            }),
        },
        ToolDef {
            name: "context",
            description: "Project context payload: recent + important memories and a total count.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}
```

- [ ] **Step 4: Run it — expect PASS.**
  Run: `cargo test -p rb-mcp tools`
  Expected: PASS (6 tests: exact tool set, object schemas, remember required/optional, recall/get required, context no-required, camelCase serialization).

- [ ] **Step 5: Lint + format.**
  Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: no diff.

- [ ] **Step 6: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-mcp/src/tools.rs crates/rb-mcp/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-mcp): define the 8 MCP tools with JSON-Schema input contracts"`
  Expected: one commit created.

---

### Task 14: `proxy.rs` — `DaemonProxy` trait + `tools/call` argument → `rb_proto::Request` routing

The adapter is a thin daemon client: a `tools/call` maps a tool name + JSON arguments to an `rb_proto::Request`, forwards it through a `DaemonProxy` (so the real `rb_proto::Client` and an in-memory test fake are interchangeable), and maps the `rb_proto::Response` to MCP tool-result content. This task defines the `DaemonProxy` async trait, the pure `build_request(name, args) -> Result<Request, ToolError>` router, and the pure `response_to_content(Response) -> Value` mapper. Argument validation (missing `id`, bad `importance`) fails closed with `INVALID_PARAMS`. All pure/offline; a fake proxy is used in tests. Note: `DaemonProxy::call` returns `rb_types::Result<Response>` and, mirroring `rb_proto::Client::request`, a daemon-reported error arrives as `Ok(Response::Error { .. })` (transport failures are the `Err` case).

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/proxy.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` (declare `pub mod proxy;`)

- [ ] **Step 1: Write the failing tests AND declare the module.** Add `pub mod proxy;` to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` (after `pub mod tools;`), then create `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/proxy.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_proto::Request;
    use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace, SearchResult};
    use serde_json::json;

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rusty-brain".into()),
            "one db one transaction".into(),
            MemoryType::ArchitectureDecision,
            8,
        )
    }

    #[test]
    fn build_remember_request_with_defaults() {
        let req = build_request("remember", &json!({ "content": "hello" })).unwrap();
        match req {
            Request::Remember {
                content,
                context,
                memory_type,
                importance,
                tags,
                ..
            } => {
                assert_eq!(content, "hello");
                assert!(context.is_none());
                assert_eq!(memory_type, MemoryType::Insight, "default type is insight");
                assert_eq!(importance, 5, "default importance is 5");
                assert!(tags.is_empty());
            }
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn build_remember_request_with_all_fields() {
        let req = build_request(
            "remember",
            &json!({
                "content": "c",
                "context": "ctx",
                "type": "bug_fix",
                "importance": 9,
                "tags": ["a", "b"]
            }),
        )
        .unwrap();
        match req {
            Request::Remember {
                memory_type,
                importance,
                tags,
                context,
                ..
            } => {
                assert_eq!(memory_type, MemoryType::BugFix);
                assert_eq!(importance, 9);
                assert_eq!(tags, vec!["a".to_string(), "b".to_string()]);
                assert_eq!(context.as_deref(), Some("ctx"));
            }
            other => panic!("expected Remember, got {other:?}"),
        }
    }

    #[test]
    fn build_recall_request_maps_query_limit_type_tags() {
        let req = build_request(
            "recall",
            &json!({ "query": "q", "limit": 3, "type": "insight", "tags": ["t"] }),
        )
        .unwrap();
        match req {
            Request::Recall {
                query,
                memory_type,
                tags,
                limit,
            } => {
                assert_eq!(query, "q");
                assert_eq!(limit, 3);
                assert_eq!(memory_type, Some(MemoryType::Insight));
                assert_eq!(tags, vec!["t".to_string()]);
            }
            other => panic!("expected Recall, got {other:?}"),
        }
    }

    #[test]
    fn build_get_graph_delete_parse_ids() {
        let id = MemoryId::new();
        let g = build_request("get", &json!({ "id": id.to_string() })).unwrap();
        assert!(matches!(g, Request::Get { .. }));
        let gr = build_request("graph", &json!({ "id": id.to_string(), "depth": 2 })).unwrap();
        match gr {
            Request::Graph { depth, .. } => assert_eq!(depth, 2),
            other => panic!("expected Graph, got {other:?}"),
        }
        let d = build_request("delete", &json!({ "id": id.to_string() })).unwrap();
        assert!(matches!(d, Request::Delete { .. }));
    }

    #[test]
    fn build_update_maps_partial_fields() {
        let id = MemoryId::new();
        let u = build_request(
            "update",
            &json!({ "id": id.to_string(), "importance": 7, "tags": ["x"] }),
        )
        .unwrap();
        match u {
            Request::Update { updates, .. } => {
                assert_eq!(updates.importance, Some(7));
                assert_eq!(updates.tags, Some(vec!["x".to_string()]));
                assert!(updates.content.is_none());
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn build_context_and_list_defaults() {
        assert!(matches!(
            build_request("context", &json!({})).unwrap(),
            Request::Context
        ));
        match build_request("list", &json!({})).unwrap() {
            Request::List {
                min_importance,
                limit,
            } => {
                assert!(min_importance.is_none());
                assert_eq!(limit, 20, "default list limit is 20");
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn missing_required_arg_is_invalid_params() {
        let err = build_request("remember", &json!({})).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
        let err = build_request("get", &json!({})).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[test]
    fn bad_id_and_bad_type_are_invalid_params() {
        let err = build_request("get", &json!({ "id": "not-a-uuid" })).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
        let err =
            build_request("remember", &json!({ "content": "c", "type": "nope" })).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[test]
    fn unknown_tool_is_method_not_found() {
        let err = build_request("frobnicate", &json!({})).unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::METHOD_NOT_FOUND);
    }

    #[test]
    fn response_to_content_renders_each_variant_as_json() {
        use rb_proto::Response;
        let id = MemoryId::new();
        let remembered = response_to_content(Response::Remembered { id: id.clone() });
        assert_eq!(remembered["id"], id.to_string());

        let recalled = response_to_content(Response::Recalled {
            results: vec![SearchResult { memory: note(), score: 0.5 }],
        });
        assert!(recalled["results"].is_array());
        assert_eq!(recalled["results"][0]["score"], 0.5);

        let got = response_to_content(Response::Got { memory: Some(note()) });
        assert!(got["memory"]["content"].is_string());

        let none = response_to_content(Response::Got { memory: None });
        assert!(none["memory"].is_null());

        let ctx = response_to_content(Response::ContextResult {
            recent: vec![note()],
            important: vec![note()],
            total: 2,
        });
        assert_eq!(ctx["total"], 2);

        let err = response_to_content(Response::Error {
            kind: "not_found".into(),
            message: "nope".into(),
        });
        assert!(err.get("error").is_some(), "error variant carries an `error` key");
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-mcp proxy`
  Expected: FAIL to compile — `cannot find function 'build_request'` / `cannot find trait 'DaemonProxy'`. Confirms the test drives new code.

- [ ] **Step 3: Add the proxy trait, router, and mapper above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/proxy.rs`:

```rust
//! `DaemonProxy`: the seam between the MCP adapter and the daemon. The real
//! `rb_proto::Client` implements it (in the bin); tests inject an in-memory fake.
//! Plus the pure tool-call router (`build_request`) and response mapper.

use crate::jsonrpc::{JsonRpcError, INVALID_PARAMS, METHOD_NOT_FOUND};
use async_trait::async_trait;
use rb_proto::{Request, Response};
use rb_types::{MemoryId, MemoryType, MemoryUpdates};
use serde_json::{json, Value};
use std::str::FromStr;

/// The daemon-facing capability the adapter needs: send one `Request`, get one
/// `Response`. Implemented by `rb_proto::Client` (via a thin wrapper in the bin)
/// and by an in-memory fake in tests. Mirrors `Client::request`: a daemon-side
/// error is `Ok(Response::Error { .. })`; only transport failures are `Err`.
#[async_trait]
pub trait DaemonProxy: Send {
    /// Forward one request to the daemon and return its response.
    async fn call(&mut self, request: Request) -> rb_types::Result<Response>;
}

/// A tool-routing error already shaped as a JSON-RPC error (code + message).
pub type ToolError = JsonRpcError;

fn invalid(msg: impl Into<String>) -> ToolError {
    JsonRpcError::new(INVALID_PARAMS, msg.into())
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("missing required string argument '{key}'")))
}

fn opt_string(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn opt_string_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn opt_u8(args: &Value, key: &str) -> Result<Option<u8>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| invalid(format!("'{key}' must be an integer in 0..=255"))),
    }
}

fn opt_usize(args: &Value, key: &str, default: usize) -> Result<usize, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .ok_or_else(|| invalid(format!("'{key}' must be a non-negative integer"))),
    }
}

fn parse_type(args: &Value, key: &str) -> Result<Option<MemoryType>, ToolError> {
    match args.get(key).and_then(Value::as_str) {
        None => Ok(None),
        Some(s) => MemoryType::parse(s)
            .map(Some)
            .map_err(|e| invalid(format!("invalid '{key}': {e}"))),
    }
}

fn parse_id(args: &Value) -> Result<MemoryId, ToolError> {
    let raw = require_str(args, "id")?;
    MemoryId::from_str(raw).map_err(|e| invalid(format!("invalid memory id '{raw}': {e}")))
}

/// Route a `tools/call` (tool name + JSON arguments) to an `rb_proto::Request`.
/// Unknown tools fail with `METHOD_NOT_FOUND`; bad arguments with `INVALID_PARAMS`.
pub fn build_request(name: &str, args: &Value) -> Result<Request, ToolError> {
    match name {
        "remember" => {
            let content = require_str(args, "content")?.to_owned();
            let memory_type = parse_type(args, "type")?.unwrap_or(MemoryType::Insight);
            let importance = opt_u8(args, "importance")?.unwrap_or(5);
            Ok(Request::Remember {
                content,
                context: opt_string(args, "context"),
                memory_type,
                importance,
                keywords: Vec::new(),
                tags: opt_string_vec(args, "tags"),
                related_files: Vec::new(),
            })
        }
        "recall" => Ok(Request::Recall {
            query: require_str(args, "query")?.to_owned(),
            memory_type: parse_type(args, "type")?,
            tags: opt_string_vec(args, "tags"),
            limit: opt_usize(args, "limit", 10)?,
        }),
        "get" => Ok(Request::Get { id: parse_id(args)? }),
        "list" => Ok(Request::List {
            min_importance: opt_u8(args, "min_importance")?,
            limit: opt_usize(args, "limit", 20)?,
        }),
        "graph" => {
            let depth = opt_u8(args, "depth")?.unwrap_or(1);
            Ok(Request::Graph {
                id: parse_id(args)?,
                depth,
            })
        }
        "update" => {
            let id = parse_id(args)?;
            let updates = MemoryUpdates {
                content: opt_string(args, "content"),
                summary: opt_string(args, "summary"),
                importance: opt_u8(args, "importance")?,
                tags: args
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|_| opt_string_vec(args, "tags")),
                context: opt_string(args, "context"),
            };
            Ok(Request::Update { id, updates })
        }
        "delete" => Ok(Request::Delete { id: parse_id(args)? }),
        "context" => Ok(Request::Context),
        other => Err(JsonRpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown tool '{other}'"),
        )),
    }
}

/// Map a daemon `Response` to the JSON value embedded in an MCP tool result.
/// Domain types already derive `Serialize`, so this is a structural projection.
pub fn response_to_content(resp: Response) -> Value {
    match resp {
        Response::Remembered { id } => json!({ "id": id.to_string() }),
        Response::Recalled { results } => json!({ "results": results }),
        Response::Got { memory } => json!({ "memory": memory }),
        Response::Listed { memories } => json!({ "memories": memories }),
        Response::GraphResult { memories } => json!({ "memories": memories }),
        Response::Updated => json!({ "ok": true }),
        Response::Deleted => json!({ "ok": true }),
        Response::ContextResult {
            recent,
            important,
            total,
        } => json!({ "recent": recent, "important": important, "total": total }),
        Response::Pong { contract_version } => json!({ "contract_version": contract_version }),
        Response::Error { kind, message } => json!({ "error": { "kind": kind, "message": message } }),
    }
}
```

- [ ] **Step 4: Run it — expect PASS.**
  Run: `cargo test -p rb-mcp proxy`
  Expected: PASS (10 tests: remember defaults/full, recall mapping, get/graph/delete id parse, update partial, context/list defaults, missing-arg INVALID_PARAMS, bad-id/bad-type INVALID_PARAMS, unknown-tool METHOD_NOT_FOUND, response mapping incl. error variant).

- [ ] **Step 5: Lint + format.**
  Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: no diff.

- [ ] **Step 6: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-mcp/src/proxy.rs crates/rb-mcp/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-mcp): add DaemonProxy trait, tools/call router, and response mapper"`
  Expected: one commit created.

---

### Task 15: `server.rs` — the MCP request dispatcher (initialize / initialized / tools/list / tools/call)

This is the protocol brain: given one decoded `JsonRpcRequest` and a `&mut dyn DaemonProxy`, produce `Option<JsonRpcResponse>` (`None` for notifications, which get no reply). It handles `initialize` (echo protocolVersion, return serverInfo + capabilities + the rusty-brain contractVersion), `notifications/initialized` (ack -> no response), `tools/list` (the 8 tools), `tools/call` (route via `build_request`, forward through the proxy, wrap the response as a tool result; a daemon `Response::Error` becomes a tool result with `isError: true` rather than a transport-level JSON-RPC error), and unknown methods (`METHOD_NOT_FOUND`). All async logic is tested in-process with a fake `DaemonProxy` — no daemon, no sockets.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/server.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` (declare `pub mod server;` + re-exports)

- [ ] **Step 1: Write the failing tests AND declare the module.** Add `pub mod server;` to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` (after `pub mod proxy;`), then create `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/server.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::jsonrpc::JsonRpcRequest;
    use crate::proxy::DaemonProxy;
    use async_trait::async_trait;
    use rb_proto::{Request, Response};
    use rb_types::{MemoryId, MemoryNote, MemoryType, Namespace, SearchResult};
    use serde_json::json;

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("p".into()),
            "remembered body".into(),
            MemoryType::Insight,
            6,
        )
    }

    /// A fake proxy that records the last request and returns a canned response
    /// per request kind, so the dispatcher is tested without a daemon.
    struct FakeProxy {
        id: MemoryId,
        last: Option<Request>,
        force_error: bool,
    }

    #[async_trait]
    impl DaemonProxy for FakeProxy {
        async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
            self.last = Some(request.clone());
            if self.force_error {
                return Ok(Response::Error {
                    kind: "not_found".into(),
                    message: "no such memory".into(),
                });
            }
            Ok(match request {
                Request::Remember { .. } => Response::Remembered { id: self.id.clone() },
                Request::Recall { .. } => Response::Recalled {
                    results: vec![SearchResult { memory: note(), score: 0.9 }],
                },
                Request::Get { .. } => Response::Got { memory: Some(note()) },
                Request::List { .. } => Response::Listed { memories: vec![note()] },
                Request::Graph { .. } => Response::GraphResult { memories: vec![note()] },
                Request::Update { .. } => Response::Updated,
                Request::Delete { .. } => Response::Deleted,
                Request::Context => Response::ContextResult {
                    recent: vec![note()],
                    important: vec![note()],
                    total: 1,
                },
                Request::Ping => Response::Pong { contract_version: 1 },
            })
        }
    }

    fn fake() -> FakeProxy {
        FakeProxy { id: MemoryId::new(), last: None, force_error: false }
    }

    fn req(method: &str, id: Option<i64>, params: serde_json::Value) -> JsonRpcRequest {
        let raw = json!({
            "jsonrpc": "2.0",
            "method": method,
            "id": id,
            "params": params
        });
        serde_json::from_value(raw).unwrap()
    }

    #[tokio::test]
    async fn initialize_returns_server_info_and_capabilities() {
        let mut proxy = fake();
        let r = req("initialize", Some(1), json!({ "protocolVersion": "2024-11-05" }));
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["serverInfo"]["name"], "rusty-brain");
        assert!(result["serverInfo"]["version"].is_string());
        assert!(result["capabilities"]["tools"].is_object());
        // Echoes the client's requested protocol version.
        assert_eq!(result["protocolVersion"], "2024-11-05");
        // Surfaces the rusty-brain wire contract version (a u32; serde_json
        // Value implements PartialEq<u32>).
        assert_eq!(result["serverInfo"]["contractVersion"], rb_proto::CONTRACT_VERSION);
    }

    #[tokio::test]
    async fn initialized_notification_gets_no_response() {
        let mut proxy = fake();
        let r = req("notifications/initialized", None, json!({}));
        let resp = handle_request(r, &mut proxy).await;
        assert!(resp.is_none(), "notifications must not be answered");
    }

    #[tokio::test]
    async fn tools_list_returns_eight_tools() {
        let mut proxy = fake();
        let r = req("tools/list", Some(2), json!({}));
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 8);
        assert!(tools.iter().any(|t| t["name"] == "remember"));
        assert!(tools[0]["inputSchema"]["type"] == "object");
    }

    #[tokio::test]
    async fn tools_call_remember_forwards_and_wraps_result() {
        let mut proxy = fake();
        let want_id = proxy.id.clone();
        let r = req(
            "tools/call",
            Some(3),
            json!({ "name": "remember", "arguments": { "content": "hi" } }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let result = resp.result.unwrap();
        // MCP tool result: content array with a text item, isError false/absent.
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(&want_id.to_string()), "id in result text: {text}");
        assert_ne!(result["isError"], json!(true));
        assert!(matches!(proxy.last, Some(Request::Remember { .. })));
    }

    #[tokio::test]
    async fn tools_call_unknown_tool_is_method_not_found() {
        let mut proxy = fake();
        let r = req(
            "tools/call",
            Some(4),
            json!({ "name": "frobnicate", "arguments": {} }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, crate::jsonrpc::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn tools_call_bad_arguments_is_invalid_params() {
        let mut proxy = fake();
        let r = req(
            "tools/call",
            Some(5),
            json!({ "name": "get", "arguments": { "id": "not-a-uuid" } }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        assert_eq!(resp.error.unwrap().code, crate::jsonrpc::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn tools_call_daemon_error_becomes_iserror_tool_result() {
        let mut proxy = fake();
        proxy.force_error = true;
        let r = req(
            "tools/call",
            Some(6),
            json!({ "name": "get", "arguments": { "id": MemoryId::new().to_string() } }),
        );
        let resp = handle_request(r, &mut proxy).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], json!(true), "daemon error -> isError result");
        // The transport itself stays successful (result, not error).
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let mut proxy = fake();
        let r = req("does/not/exist", Some(7), json!({}));
        let resp = handle_request(r, &mut proxy).await.unwrap();
        assert_eq!(resp.error.unwrap().code, crate::jsonrpc::METHOD_NOT_FOUND);
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-mcp server`
  Expected: FAIL to compile — `cannot find function 'handle_request'`. Confirms the test drives new code.

- [ ] **Step 3: Add the dispatcher above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/server.rs`:

```rust
//! MCP method dispatch: one decoded JSON-RPC request -> optional response.

use crate::jsonrpc::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS,
    METHOD_NOT_FOUND,
};
use crate::proxy::{build_request, response_to_content, DaemonProxy};
use crate::tools::tool_definitions;
use serde_json::{json, Value};

/// The MCP protocol revision this adapter targets when the client omits one.
pub const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Handle one decoded JSON-RPC request. Returns `Some(response)` for requests and
/// `None` for notifications (which JSON-RPC forbids answering).
pub async fn handle_request(
    request: JsonRpcRequest,
    proxy: &mut dyn DaemonProxy,
) -> Option<JsonRpcResponse> {
    // Notifications (no id) are acknowledged silently with no response frame.
    if request.is_notification() {
        // `notifications/initialized` and any other notification: nothing to send.
        tracing::debug!(method = %request.method, "notification (no response)");
        return None;
    }

    // Safe: non-notification means id is Some.
    let id = request.id.clone().unwrap_or(Value::Null);
    let params = request.params.clone().unwrap_or_else(|| json!({}));

    let response = match request.method.as_str() {
        "initialize" => JsonRpcResponse::success(id, initialize_result(&params)),
        "tools/list" => JsonRpcResponse::success(id, tools_list_result()),
        "tools/call" => handle_tools_call(id, &params, proxy).await,
        other => JsonRpcResponse::error(
            id,
            JsonRpcError::new(METHOD_NOT_FOUND, format!("unknown method '{other}'")),
        ),
    };
    Some(response)
}

/// Build the `initialize` result: echo the client's protocolVersion (or the
/// default), advertise the tools capability, and surface server identity +
/// the rusty-brain wire contract version.
fn initialize_result(params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "rusty-brain",
            "version": env!("CARGO_PKG_VERSION"),
            "contractVersion": rb_proto::CONTRACT_VERSION
        }
    })
}

/// Build the `tools/list` result from the static tool definitions.
fn tools_list_result() -> Value {
    json!({ "tools": tool_definitions() })
}

/// Handle `tools/call`: route name+arguments to a `Request`, forward via the
/// proxy, and wrap the response. Routing errors (unknown tool, bad args) become
/// JSON-RPC errors; daemon-reported errors become `isError` tool results.
async fn handle_tools_call(
    id: Value,
    params: &Value,
    proxy: &mut dyn DaemonProxy,
) -> JsonRpcResponse {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::error(
            id,
            JsonRpcError::new(INVALID_PARAMS, "tools/call requires a 'name'".into()),
        );
    };
    let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let request = match build_request(name, &arguments) {
        Ok(r) => r,
        Err(err) => return JsonRpcResponse::error(id, err),
    };

    match proxy.call(request).await {
        Ok(resp) => {
            let content = response_to_content(resp);
            // A daemon-side error surfaces as a tool result with isError=true so
            // the agent sees the message instead of a transport failure.
            let is_error = content.get("error").is_some();
            JsonRpcResponse::success(id, tool_result(content, is_error))
        }
        Err(e) => JsonRpcResponse::error(
            id,
            // A transport/daemon failure (socket dropped, etc.) is a real
            // JSON-RPC error. The message is the sanitized domain error string.
            JsonRpcError::new(INTERNAL_ERROR, format!("daemon call failed: {e}")),
        ),
    }
}

/// Wrap a JSON payload as an MCP tool result (a single JSON text content item).
fn tool_result(content: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&content)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialize failed: {e}\"}}"));
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error
    })
}
```

  (verify against the installed MCP host at execution: the `tools/call` result shape — a `content` array of `{type:"text", text:...}` items plus `isError` — is the stable MCP tool-result contract; if the host you target expects structured content, add a `structuredContent` sibling — the test only asserts `content[0].text` and `isError`.)

- [ ] **Step 4: Add re-exports for the public surface available so far.** Update `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` to re-export the public surface. The `transport` module is added in Task 16, so it is NOT referenced here yet — keeping every task's tree compiling and committable on its own. The full file becomes:

```rust
//! `rb_mcp`: a thin Model Context Protocol (MCP) stdio adapter for rusty-brain.
//!
//! Speaks newline-delimited JSON-RPC 2.0 on stdin/stdout (stdout carries ONLY
//! JSON-RPC frames; all logging goes to stderr). Each `tools/call` is routed to
//! an `rb_proto::Request` and forwarded to the daemon over the Unix socket via a
//! `DaemonProxy`. The adapter holds no storage of its own.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod jsonrpc;
pub mod proxy;
pub mod server;
pub mod tools;

pub use jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use proxy::{build_request, response_to_content, DaemonProxy};
pub use server::handle_request;
pub use tools::{tool_definitions, ToolDef};
```

- [ ] **Step 5: Run it — expect PASS.**
  Run: `cargo test -p rb-mcp server`
  Expected: PASS (8 tests: initialize, initialized-no-response, tools/list count, tools/call remember wrap, unknown-tool, bad-args, daemon-error isError, unknown-method).

- [ ] **Step 6: Lint + format.**
  Run: `cargo clippy -p rb-mcp --all-targets -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: no diff.

- [ ] **Step 7: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-mcp/src/server.rs crates/rb-mcp/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-mcp): add MCP dispatcher (initialize, tools/list, tools/call)"`
  Expected: one commit created.

---

### Task 16: `transport.rs` — newline-delimited JSON-RPC over generic async stdio

The dispatcher (Task 15) is transport-free. This task adds the read/write loop: `serve_stdio<R, W, P>(reader, writer, proxy)` reads newline-delimited JSON-RPC frames from `R: AsyncBufReadExt`, dispatches each via `handle_request`, and writes each response as one `\n`-terminated JSON line to `W: AsyncWrite`. Notifications produce no output. A malformed line yields a JSON-RPC parse error with a null id (never crashes the loop). Being generic over the streams lets the contract test drive it over a `tokio::io::duplex` pair (no real stdin/stdout, no daemon) while production wires `tokio::io::stdin()`/`stdout()`. This guarantees stdout carries ONLY JSON-RPC frames.

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/transport.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` (declare `pub mod transport;` + re-export `serve_stdio`)

- [ ] **Step 1: Declare the new module + re-export, then write the failing in-process contract test.** Add `pub mod transport;` to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/lib.rs` (after `pub mod tools;`) and add `pub use transport::serve_stdio;` to the re-export block (after `pub use tools::{tool_definitions, ToolDef};`). Then create `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/transport.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::proxy::DaemonProxy;
    use async_trait::async_trait;
    use rb_proto::{Request, Response};
    use rb_types::MemoryId;
    use serde_json::{json, Value};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Minimal fake daemon: remembers return a fixed id; everything else Pongs.
    struct Fake {
        id: MemoryId,
    }
    #[async_trait]
    impl DaemonProxy for Fake {
        async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
            Ok(match request {
                Request::Remember { .. } => Response::Remembered { id: self.id.clone() },
                _ => Response::Pong { contract_version: rb_proto::CONTRACT_VERSION },
            })
        }
    }

    /// Drive the adapter end-to-end over an in-memory duplex pair: the test plays
    /// the MCP client (writes requests, reads response lines); `serve_stdio` is
    /// the server. Asserts initialize, the no-reply notification, tools/list, and
    /// a tools/call round-trip all behave per JSON-RPC over the byte stream.
    #[tokio::test]
    async fn full_stdio_contract_round_trip() {
        // client_* is the test's end; server_* is fed to serve_stdio.
        let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, server_to_client) = tokio::io::duplex(64 * 1024);

        let fixed = MemoryId::new();
        let proxy = Fake { id: fixed.clone() };

        let server = tokio::spawn(async move {
            let reader = BufReader::new(server_reader);
            serve_stdio(reader, server_writer, proxy).await
        });

        // Write four frames: initialize, initialized (notification), tools/list,
        // tools/call(remember). Then close the write half to end the loop.
        let mut to_server = client_to_server;
        let frames = [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                   "params":{"protocolVersion":"2024-11-05"}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                   "params":{"name":"remember","arguments":{"content":"hi"}}}),
        ];
        for f in frames {
            let line = format!("{}\n", serde_json::to_string(&f).unwrap());
            to_server.write_all(line.as_bytes()).await.unwrap();
        }
        to_server.flush().await.unwrap();
        drop(to_server); // EOF -> serve_stdio returns

        // Read every response line the server produced.
        let mut lines = BufReader::new(server_to_client).lines();
        let mut responses: Vec<Value> = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if line.trim().is_empty() {
                continue;
            }
            responses.push(serde_json::from_str(&line).unwrap());
        }

        // Exactly three responses: the notification produced none.
        assert_eq!(responses.len(), 3, "got: {responses:?}");

        // initialize (id 1)
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "rusty-brain");

        // tools/list (id 2)
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 8);

        // tools/call (id 3) -> remembered id appears in the tool result text
        assert_eq!(responses[2]["id"], 3);
        let text = responses[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(&fixed.to_string()), "id in result: {text}");

        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn malformed_line_yields_parse_error_and_keeps_serving() {
        let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, server_to_client) = tokio::io::duplex(64 * 1024);
        let proxy = Fake { id: MemoryId::new() };

        let server = tokio::spawn(async move {
            serve_stdio(BufReader::new(server_reader), server_writer, proxy).await
        });

        let mut to_server = client_to_server;
        // 1) garbage line, then 2) a valid tools/list to prove the loop survived.
        to_server.write_all(b"this is not json\n").await.unwrap();
        let good = json!({"jsonrpc":"2.0","id":9,"method":"tools/list","params":{}});
        to_server
            .write_all(format!("{}\n", serde_json::to_string(&good).unwrap()).as_bytes())
            .await
            .unwrap();
        to_server.flush().await.unwrap();
        drop(to_server);

        let mut lines = BufReader::new(server_to_client).lines();
        let mut responses: Vec<Value> = Vec::new();
        while let Some(line) = lines.next_line().await.unwrap() {
            if !line.trim().is_empty() {
                responses.push(serde_json::from_str(&line).unwrap());
            }
        }
        assert_eq!(responses.len(), 2, "parse error + tools/list reply: {responses:?}");
        assert_eq!(responses[0]["error"]["code"], crate::jsonrpc::PARSE_ERROR);
        assert!(responses[0]["id"].is_null(), "parse error has null id");
        assert_eq!(responses[1]["id"], 9);
        server.await.unwrap().unwrap();
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.**
  Run: `cargo test -p rb-mcp transport`
  Expected: FAIL to compile — `cannot find function 'serve_stdio'`. Confirms the test drives new code.

- [ ] **Step 3: Add the transport loop above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p2/crates/rb-mcp/src/transport.rs`. Note the three-parameter generic header `<R, W, P>` — `P` is the proxy type and MUST be in the generic list:

```rust
//! Newline-delimited JSON-RPC transport over generic async byte streams.
//!
//! Generic over the reader/writer/proxy so the same loop drives an in-memory
//! duplex pair (contract tests) and real stdin/stdout (production). stdout
//! receives ONLY response frames; all logging goes to stderr via `tracing`.

use crate::jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};
use crate::proxy::DaemonProxy;
use crate::server::handle_request;
use rb_types::{Error, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Serve MCP over a line-delimited byte stream until EOF on `reader`.
///
/// Each input line is parsed as a `JsonRpcRequest`; requests are dispatched and
/// their responses written as one `\n`-terminated JSON line. Notifications get
/// no output. A line that fails to parse yields a JSON-RPC parse error (null id)
/// and the loop continues — one bad frame never tears down the session.
pub async fn serve_stdio<R, W, P>(mut reader: R, mut writer: W, mut proxy: P) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWrite + Unpin,
    P: DaemonProxy,
{
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| Error::Io(format!("mcp stdin read: {e}")))?;
        if n == 0 {
            // EOF: the client closed stdin; shut the adapter down cleanly.
            tracing::debug!("mcp stdin closed; shutting down adapter");
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(request) => handle_request(request, &mut proxy).await,
            Err(e) => {
                tracing::warn!(error = %e, "malformed JSON-RPC frame");
                Some(JsonRpcResponse::error(
                    Value::Null,
                    JsonRpcError::new(PARSE_ERROR, format!("parse error: {e}")),
                ))
            }
        };

        if let Some(response) = response {
            write_response(&mut writer, &response).await?;
        }
    }
}

/// Serialize one response and write it as a single `\n`-terminated line.
async fn write_response<W>(writer: &mut W, response: &JsonRpcResponse) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut bytes = serde_json::to_vec(response)
        .map_err(|e| Error::Serialization(format!("mcp response serialize: {e}")))?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .await
        .map_err(|e| Error::Io(format!("mcp stdout write: {e}")))?;
    writer
        .flush()
        .await
        .map_err(|e| Error::Io(format!("mcp stdout flush: {e}")))?;
    Ok(())
}
```

  (verify against the installed tokio at execution: `AsyncBufReadExt` provides `read_line`; `AsyncWriteExt` provides `write_all`/`flush`. Both extension traits are in scope via the `use tokio::io::...` line. `handle_request` takes `&mut dyn DaemonProxy`, and `&mut proxy` where `P: DaemonProxy` coerces to `&mut dyn DaemonProxy`.)

- [ ] **Step 4: Run it — expect PASS.**
  Run: `cargo test -p rb-mcp transport`
  Expected: PASS (2 tests: full stdio round-trip with 3 responses for 4 frames — the notification yields none — and malformed-line parse-error-then-recover).

- [ ] **Step 5: Run the whole crate suite + gates.**
  Run: `cargo test -p rb-mcp`
  Expected: PASS (jsonrpc + tools + proxy + server + transport tests all green).
  Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: no diff (exit 0).

- [ ] **Step 6: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-mcp/src/transport.rs crates/rb-mcp/src/lib.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-mcp): add newline-delimited JSON-RPC stdio transport loop"`
  Expected: one commit created.

---

### Task 17: bin `mcp` subcommand — `ClientProxy` over `rb_proto::Client` + wire stdin/stdout

This wires the adapter into the binary so `rusty-brain mcp` is what Claude Code / agents spawn. It adds a `ClientProxy` newtype in the bin implementing `rb_mcp::DaemonProxy` over the real `rb_proto::Client` (the bin owns this impl because `rb-mcp` does not depend on the bin's auto-start), an `Mcp` clap subcommand, and a `run_mcp` path that detect_namespace()s (Part L), connects (auto-starting the daemon via the existing `connect_or_start`), and runs `serve_stdio(stdin, stdout, proxy)`. Logging stays on stderr (already configured); stdout is reserved for JSON-RPC frames. (`ClientProxy::call` forwards via the raw `Client::request`, so a daemon `Response::Error` arrives as `Ok(Response::Error { .. })` — exactly what the adapter turns into an `isError` tool result.)

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/Cargo.toml` (add `rb-mcp` + `async-trait` deps)
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/mcp.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/cli.rs` (add `Mcp` subcommand)
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/lib.rs` (declare `pub mod mcp;`)
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/run.rs` (dispatch `Mcp`)

- [ ] **Step 1: Add the `rb-mcp` and `async-trait` dependencies to the bin manifest.** Edit `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/Cargo.toml` so the `[dependencies]` table gains the two lines (place after `rb-embed`):

```toml
rb-mcp = { path = "../rb-mcp" }
async-trait = { workspace = true }
```

- [ ] **Step 2: Write the `ClientProxy` with a failing unit test AND declare the module.** Add `pub mod mcp;` to `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/lib.rs` (after `pub mod logging;`). Create `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/mcp.rs` with ONLY the test module first (the proxy delegates to a real daemon, so the unit test only checks the constructor wiring is sound — the full behavior is covered by the daemon-backed e2e in Task 18):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    // The proxy is a thin newtype over rb_proto::Client; there is no offline way
    // to construct a connected Client without a daemon, so behavior is proven in
    // the daemon-backed e2e (Task 18). Here we only assert the type implements
    // the rb_mcp::DaemonProxy trait (a compile-time guarantee via this fn).
    fn _assert_impls_daemon_proxy<T: rb_mcp::DaemonProxy>() {}

    #[test]
    fn client_proxy_implements_daemon_proxy() {
        // If ClientProxy did not implement DaemonProxy this would not compile.
        let _ = _assert_impls_daemon_proxy::<ClientProxy>;
    }
}
```

- [ ] **Step 3: Run it — expect a compile failure.**
  Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml mcp::`
  Expected: FAIL to compile — `cannot find type 'ClientProxy'`. The `::` suffix scopes the filter to the `mcp` module.

- [ ] **Step 4: Add the proxy + run_mcp implementation above the test module.** Prepend to `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/mcp.rs`:

```rust
//! `mcp` subcommand: run the MCP stdio adapter against the daemon.
//!
//! Detects the namespace (Part L), connects to the daemon (auto-starting it if
//! the socket is absent), and serves newline-delimited JSON-RPC on stdin/stdout.
//! stdout carries ONLY JSON-RPC frames; tracing goes to stderr.

use crate::client::connect_or_start;
use crate::namespace_detect::detect_namespace;
use async_trait::async_trait;
use rb_mcp::{serve_stdio, DaemonProxy};
use rb_proto::{Client, Request, Response};
use std::path::Path;
use tokio::io::BufReader;

/// Adapts the daemon `rb_proto::Client` to the adapter's `DaemonProxy` seam.
pub struct ClientProxy {
    client: Client,
}

impl ClientProxy {
    /// Wrap a connected daemon client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl DaemonProxy for ClientProxy {
    async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
        // The adapter already built a well-formed Request; forward it verbatim
        // via the raw `request` method (not the typed wrappers) so the daemon's
        // Response::Error stays a Response (surfaced as an isError tool result),
        // and only transport failures become Err.
        self.client.request(request).await
    }
}

/// Run the MCP adapter: resolve namespace, connect (auto-start), serve stdio.
///
/// NOTE: `detect_namespace()` runs a synchronous `git` lookup once at startup.
/// Moving that off the runtime is the Part L / Part P-6 should-fix; this wiring
/// reuses the same call the bin's `run_client` already makes, for consistency.
pub async fn run_mcp(socket_path: &Path, db_path: &Path) -> anyhow::Result<()> {
    let namespace = detect_namespace();
    let self_exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("locating own executable: {e}"))?;
    let client = connect_or_start(socket_path, db_path, namespace, self_exe)
        .await
        .map_err(|e| anyhow::anyhow!("connecting to daemon: {e}"))?;

    let proxy = ClientProxy::new(client);
    let stdin = BufReader::new(tokio::io::stdin());
    let stdout = tokio::io::stdout();
    serve_stdio(stdin, stdout, proxy)
        .await
        .map_err(|e| anyhow::anyhow!("mcp adapter failed: {e}"))?;
    Ok(())
}
```

  (verify against the installed `rb-mcp` at execution: `serve_stdio` is generic `<R: AsyncBufReadExt + Unpin, W: AsyncWrite + Unpin, P: DaemonProxy>`; `BufReader<Stdin>` satisfies `AsyncBufReadExt + Unpin` and `Stdout` satisfies `AsyncWrite + Unpin`.)

- [ ] **Step 5: Add the `Mcp` subcommand to the CLI.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/cli.rs`, add a new variant to the `Command` enum (place it immediately after the `Serve` variant so it reads naturally as a long-running mode):

```rust
    /// Run the MCP (Model Context Protocol) stdio server for agents.
    Mcp,
```

- [ ] **Step 6: Dispatch `Mcp` in the run dispatcher.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/run.rs`, handle `Mcp` alongside `Serve` in `run()` (before the `other => run_client(...)` fallthrough), because — like `serve` — it is a long-running mode that does NOT go through the single-request `run_client` path. Change the `match cli.command` block in `run()` to:

```rust
    match cli.command {
        Command::Serve => {
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            serve::run_serve(socket_path, db_path, 4, shutdown)
                .await
                .context("daemon failed")?;
            Ok(())
        }
        Command::Mcp => crate::mcp::run_mcp(&socket_path, &db_path)
            .await
            .context("mcp adapter failed"),
        other => run_client(other, cli.json, &socket_path, &db_path).await,
    }
```

  The `run_client` `match command` block currently handles every `Command` variant exhaustively with no wildcard arm; adding the `Mcp` variant to the enum would make that match non-exhaustive. Add a guard arm next to the existing `Command::Serve` bail so it stays exhaustive and panic-free if `Mcp` ever reaches `run_client`:

```rust
        Command::Mcp => anyhow::bail!("internal: mcp must be handled before run_client"),
```

- [ ] **Step 7: Run the unit test — expect PASS, and confirm the bin builds.**
  Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml mcp::`
  Expected: PASS (1 test: `ClientProxy` implements `DaemonProxy`).
  Run: `cargo build -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: `Finished` (exit 0).
  Run: `cargo run -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- --help`
  Expected: usage now lists the `mcp` subcommand alongside `serve`, `remember`, ... `status` (exit 0).

- [ ] **Step 8: Lint + format.**
  Run: `cargo clippy -p rusty-brain --all-targets --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: no diff.

- [ ] **Step 9: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rusty-brain/Cargo.toml crates/rusty-brain/src/mcp.rs crates/rusty-brain/src/cli.rs crates/rusty-brain/src/lib.rs crates/rusty-brain/src/run.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rusty-brain): add mcp subcommand wiring the MCP stdio adapter to the daemon"`
  Expected: one commit created.

---

### Task 18: daemon-backed e2e — `remember` then `recall` through the real `mcp` adapter

This is the v1 acceptance test for the agent surface: spawn the real `rusty-brain mcp` process against a real daemon on a tempdir socket+DB with the offline `DeterministicProvider` (no network), speak JSON-RPC on its stdin/stdout, `initialize`, `tools/call` `remember`, then `tools/call` `recall`, and assert the remembered content comes back. The `mcp` child auto-starts the daemon itself (socket absent), so this exercises the full Part L + auto-start + adapter + daemon path. The `mcp` child is owned by a `Reap` guard so it is always killed and waited on. (The daemon the child auto-starts is a detached grandchild that the test does not own; it is intentionally left to its idle timeout — the socket+DB live in the tempdir, mirroring the accepted pattern in `end_to_end.rs`.)

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/tests/mcp_e2e.rs`

- [ ] **Step 1: Write the daemon-backed e2e test.** Create `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/tests/mcp_e2e.rs`:

```rust
//! End-to-end: drive the real `rusty-brain mcp` adapter over its stdin/stdout
//! against a real auto-started daemon (tempdir socket+DB, offline
//! DeterministicProvider). Asserts a remembered memory is recalled through MCP.
//! VOYAGE_API_KEY is cleared so CI never contacts a live embedding API.
//!
//! The adapter (`mcp`) child is reaped by `Reap`. The daemon it auto-starts is a
//! detached grandchild on the tempdir socket; it is left to its idle timeout and
//! is not owned here (matches `end_to_end.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use assert_cmd::cargo::cargo_bin;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Owns the spawned `mcp` child and reaps it on drop, even if an assertion
/// panics and unwinds the test.
struct Reap(Child);
impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Write one JSON-RPC frame as a single `\n`-terminated line to the child stdin.
fn send(stdin: &mut std::process::ChildStdin, frame: &Value) {
    let line = format!("{}\n", serde_json::to_string(frame).unwrap());
    stdin.write_all(line.as_bytes()).expect("write frame");
    stdin.flush().expect("flush frame");
}

/// Read response lines until one with the given `id` is found (skipping any
/// frames without that id), or the stream ends. Panics on EOF before the match.
fn read_until_id(reader: &mut impl BufRead, id: i64) -> Value {
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).expect("read response line");
        assert!(n > 0, "stream ended before response id {id} arrived");
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed)
            .unwrap_or_else(|e| panic!("non-JSON line from adapter: {trimmed:?} ({e})"));
        if value.get("id").and_then(Value::as_i64) == Some(id) {
            return value;
        }
        // Otherwise it is a notification/log we don't expect on stdout; ignore.
    }
}

#[test]
fn mcp_remember_then_recall_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("rb.sock");
    let db = dir.path().join("rb.db");
    let exe = cargo_bin("rusty-brain");

    // Launch the MCP adapter; it auto-starts the daemon on the temp socket+DB.
    let mut child = Command::new(&exe)
        .arg("mcp")
        .env("RUSTY_BRAIN_SOCKET", &socket)
        .env("RUSTY_BRAIN_DB", &db)
        .env_remove("VOYAGE_API_KEY") // force offline DeterministicProvider
        .env("RUST_LOG", "warn")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mcp adapter");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");
    let _reap = Reap(child);
    let mut reader = BufReader::new(stdout);

    // 1) initialize
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":"2024-11-05"}}),
    );
    let init = read_until_id(&mut reader, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "rusty-brain");
    assert_eq!(
        init["result"]["serverInfo"]["contractVersion"],
        rb_proto::CONTRACT_VERSION
    );

    // 2) initialized notification (no response expected)
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );

    // 3) tools/call remember
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"remember",
            "arguments":{
                "content":"always use one database and one transaction",
                "type":"architecture_decision",
                "importance":9
            }
        }}),
    );
    let remembered = read_until_id(&mut reader, 2);
    assert_ne!(remembered["result"]["isError"], json!(true), "remember failed: {remembered}");
    let remember_text = remembered["result"]["content"][0]["text"]
        .as_str()
        .expect("remember tool text");
    // The result text is JSON: {"id":"<uuid>"}.
    let remember_payload: Value = serde_json::from_str(remember_text).unwrap();
    assert!(remember_payload["id"].is_string(), "remember returned an id: {remember_text}");

    // 4) tools/call recall — the stored content must come back.
    send(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"recall",
            "arguments":{ "query":"one database transaction", "limit":10 }
        }}),
    );
    let recalled = read_until_id(&mut reader, 3);
    assert_ne!(recalled["result"]["isError"], json!(true), "recall errored: {recalled}");
    let recall_text = recalled["result"]["content"][0]["text"]
        .as_str()
        .expect("recall tool text");
    assert!(
        recall_text.contains("one database and one transaction"),
        "recalled content missing from MCP result; got: {recall_text}"
    );

    // Close stdin so the adapter shuts down; the Reap guard kills/reaps the mcp
    // child regardless. The detached daemon grandchild times out on its own.
    drop(stdin);
    std::thread::sleep(Duration::from_millis(50));
}
```

- [ ] **Step 2: Ensure the e2e test can reference `rb_proto`.** The test uses `rb_proto::CONTRACT_VERSION`. The bin already declares `rb-proto` as a normal `[dependencies]` entry, which is visible to integration tests, so no change is typically required. If `rb_proto` does NOT resolve in the test at execution, add it to `[dev-dependencies]` in `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/Cargo.toml`:

```toml
rb-proto = { path = "../rb-proto" }
```

  (verify at execution: a normal `[dependencies]` entry IS visible to integration tests in this crate, so the most likely outcome is no manifest change.)

- [ ] **Step 3: Run the e2e — expect PASS.**
  Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml --test mcp_e2e -- --nocapture`
  Expected: PASS — `mcp_remember_then_recall_round_trips ... ok`. If it hangs at `read_until_id(_, 1)`, the daemon failed to auto-start: run `RUST_LOG=debug rusty-brain mcp` manually with the same `RUSTY_BRAIN_SOCKET`/`RUSTY_BRAIN_DB` and watch stderr for the bind error. If `recall` returns empty, the DeterministicProvider must yield identical vectors for identical text and the engine's keyword path must match on the shared tokens ("one database … transaction") — the existing `end_to_end.rs` proves this same content/query pair round-trips.

- [ ] **Step 4: Run the whole bin suite + workspace gates.**
  Run: `cargo test -p rusty-brain --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: PASS — unit modules (`paths`, `namespace_detect`, `logging`, `output`, `serve`, `client`, `run`, `mcp`) plus integration tests (`cli_surface`, `end_to_end`, `mcp_e2e`) all green.
  Run: `cargo test -p rb-mcp --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: PASS (all rb-mcp module tests green).
  Run: `cargo clippy --workspace --all-targets --all-features --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml -- -D warnings`
  Expected: no warnings.
  Run: `cargo fmt --all --check --manifest-path /Users/bluby/repos/rusty-brain-p2/Cargo.toml`
  Expected: no diff (exit 0).

- [ ] **Step 5: Commit.**
  Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rusty-brain/tests/mcp_e2e.rs crates/rusty-brain/Cargo.toml && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "test(rusty-brain): daemon-backed MCP e2e remember/recall through the adapter"`
  Expected: one commit created.

## Part O — Opt-in LLM enrichment & linking (rb-enrich; default heuristic/off)

### Task 19: rb-types — add `Error::Enrichment` variant (wire-safe, fail-closed) + rb-proto wire mapping

**Files:**
- Modify: `crates/rb-types/src/error.rs`
- Modify: `crates/rb-proto/src/error.rs` (REQUIRED — its `error_kind` match is exhaustive)

> The opt-in LLM enricher needs a dedicated error so callers can distinguish enrichment failures from storage/embedding ones (and so the heuristic fallback path is explicit). This is a one-variant, additive change to the domain enum. VERIFIED against the tree: `rb-proto/src/error.rs::error_kind` is an EXHAUSTIVE `match` over every `rb_types::Error` variant with NO `_ =>` arm, so adding a variant WILL break rb-proto compilation until a wire-kind arm is added. We add a dedicated `"enrichment"` wire kind (symmetric in both directions) rather than aliasing it to `"embedding"`, so a round-tripped enrichment error reconstructs faithfully and fails closed. The key constraint: the variant message must never contain the API key (the `AnthropicEnricher` controls what it puts here; this task only adds the carrier).

- [ ] **Step 1: Write the failing rb-types test.** Append to the `tests` module in `crates/rb-types/src/error.rs` (inside the existing `mod tests { ... }`, alongside `display_messages_match_spine`):

```rust
    #[test]
    fn enrichment_message_matches_spine() {
        assert_eq!(
            Error::Enrichment("model unavailable".into()).to_string(),
            "enrichment error: model unavailable"
        );
    }
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-types error::tests::enrichment_message_matches_spine`
  Expected: FAIL to compile (`no variant or associated item named 'Enrichment' found for enum 'Error'`).

- [ ] **Step 3: Add the variant.** In `crates/rb-types/src/error.rs`, add the new variant to the `Error` enum immediately after the `Embedding` variant:

```rust
    #[error("enrichment error: {0}")]
    Enrichment(String),
```

- [ ] **Step 4: Run the rb-types test — expect PASS.** Run: `cargo test -p rb-types error`
  Expected: PASS (all error tests, including `enrichment_message_matches_spine`).

- [ ] **Step 5: Write the failing rb-proto round-trip test.** Adding the variant now BREAKS `rb-proto` (its `error_kind` match is non-exhaustive). Append a round-trip test to the `tests` module in `crates/rb-proto/src/error.rs` (it uses the existing `round_trip` helper pattern visible at lines ~89/142):

```rust
    #[test]
    fn enrichment_round_trips_as_enrichment_kind() {
        // error_kind -> "enrichment", and reconstruct must stay Enrichment
        // (fail-closed: no silent downgrade to a different variant).
        let resp = error_to_response(&Error::Enrichment("llm down".into()));
        let crate::Response::Error { kind, message } = resp else {
            panic!("expected Response::Error");
        };
        assert_eq!(kind, "enrichment");
        assert!(matches!(
            response_error_to_error(&kind, &message),
            Error::Enrichment(_)
        ));
    }
```

  NOTE: this test module uses `panic!`; the existing module header already carries `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` (verified at line 60), so no new allow is needed. (verify against installed rb-proto at execution; adjust the destructuring if `Response::Error` field names differ.)

- [ ] **Step 6: Run it — expect FAIL (compile).** Run: `cargo test -p rb-proto error`
  Expected: FAIL to compile — `error_kind`'s `match` is non-exhaustive (`pattern &Enrichment(_) not covered`).

- [ ] **Step 7: Add the rb-proto wire-kind arms (both directions).** In `crates/rb-proto/src/error.rs`, add an arm to `error_kind` after `Error::Embedding(_) => "embedding",`:

```rust
        Error::Enrichment(_) => "enrichment",
```

  and add a matching arm to `response_error_to_error` after `"embedding" => Error::Embedding(message.to_string()),`:

```rust
        "enrichment" => Error::Enrichment(message.to_string()),
```

- [ ] **Step 8: Run it — expect PASS.** Run: `cargo test -p rb-proto error`
  Expected: PASS (including `enrichment_round_trips_as_enrichment_kind`).

- [ ] **Step 9: Build the dependents that match on `Error`.** Run: `cargo build -p rb-proto -p rb-engine -p rb-daemon`
  Expected: `Finished` with no errors (confirms no other exhaustive match downstream broke).

- [ ] **Step 10: Lint + format.** Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  Expected: no warnings. Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 11: Commit.** Run: `git add crates/rb-types/src/error.rs crates/rb-proto/src/error.rs && git commit -m "feat(rb-types): add Error::Enrichment variant with rb-proto wire mapping"`

---

### Task 20: rb-engine — define the `Enricher` async trait + `Enrichment` struct (trait home; no impls)

**Files:**
- Create: `crates/rb-engine/src/enricher.rs`
- Modify: `crates/rb-engine/src/lib.rs` (add `mod enricher;` + re-exports)

> The `Enricher` trait and `Enrichment` value type live in `rb-engine` so the engine can hold `Option<Arc<dyn Enricher>>` without depending on `rb-enrich` (which would create a cycle). `rb-enrich` then provides the concrete `HeuristicEnricher`/`AnthropicEnricher`. This task adds ONLY the trait + the value type + a tiny contract test; the offline `HeuristicEnricher` lands in `rb-enrich` (Task 22). VERIFIED: current `rb-engine/src/lib.rs` declares `mod backend; mod engine; mod enrich; #[cfg(test)] mod test_support;` and re-exports `engine::{MemoryEngine, RememberInput}`; `async-trait` is already a dependency.

- [ ] **Step 1: Write the failing test AND declare the module.** Add `mod enricher;` to `crates/rb-engine/src/lib.rs` after `mod enrich;`. Create `crates/rb-engine/src/enricher.rs` with the test-only module first:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::MemoryType;

    // A trivial in-test Enricher proving the trait is object-safe and awaitable.
    struct ConstEnricher;

    #[async_trait::async_trait]
    impl Enricher for ConstEnricher {
        async fn enrich(
            &self,
            _content: &str,
            _context: Option<&str>,
        ) -> rb_types::Result<Enrichment> {
            Ok(Enrichment {
                summary: Some("s".to_string()),
                keywords: vec!["k".to_string()],
                tags: vec!["t".to_string()],
                memory_type: Some(MemoryType::Insight),
                importance: Some(7),
            })
        }
    }

    #[tokio::test]
    async fn enricher_is_object_safe_and_awaitable() {
        let e: std::sync::Arc<dyn Enricher> = std::sync::Arc::new(ConstEnricher);
        let out = e.enrich("body", Some("ctx")).await.unwrap();
        assert_eq!(out.summary.as_deref(), Some("s"));
        assert_eq!(out.keywords, vec!["k".to_string()]);
        assert_eq!(out.tags, vec!["t".to_string()]);
        assert_eq!(out.memory_type, Some(MemoryType::Insight));
        assert_eq!(out.importance, Some(7));
    }

    #[test]
    fn enrichment_default_is_all_empty() {
        let d = Enrichment::default();
        assert!(d.summary.is_none());
        assert!(d.keywords.is_empty());
        assert!(d.tags.is_empty());
        assert!(d.memory_type.is_none());
        assert!(d.importance.is_none());
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine enricher`
  Expected: FAIL to compile (`cannot find trait 'Enricher'` / `cannot find struct 'Enrichment'`).

- [ ] **Step 3: Add the trait + struct above the test module.** Prepend to `crates/rb-engine/src/enricher.rs`:

```rust
use rb_types::MemoryType;

/// Output of an [`Enricher`]. Every field is optional: an enricher fills only
/// what it is confident about, and the engine uses a value ONLY when the caller
/// left the corresponding input empty. Defaults to "no enrichment".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Enrichment {
    /// Replacement summary (else the heuristic ~150-char prefix is used).
    pub summary: Option<String>,
    /// Derived keywords (used only if the caller supplied none).
    pub keywords: Vec<String>,
    /// Derived tags (used only if the caller supplied none).
    pub tags: Vec<String>,
    /// Inferred memory type (advisory; used only if the caller did not set one).
    pub memory_type: Option<MemoryType>,
    /// Inferred importance 1..=10 (advisory; used only if the caller did not set one).
    pub importance: Option<u8>,
}

/// Opt-in enrichment over raw memory content. The default path is heuristic and
/// offline; an LLM-backed implementation is opt-in and lives in `rb-enrich`.
/// Implementations degrade gracefully: a failure returns `Err(Error::Enrichment(_))`
/// and the engine falls back to heuristics (enrichment never fails a remember).
#[async_trait::async_trait]
pub trait Enricher: Send + Sync {
    async fn enrich(
        &self,
        content: &str,
        context: Option<&str>,
    ) -> rb_types::Result<Enrichment>;
}
```

  Set the re-exports in `crates/rb-engine/src/lib.rs` (add a line to the `pub use` block):

```rust
pub use enricher::{Enricher, Enrichment};
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-engine enricher`
  Expected: PASS (2 tests).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets -- -D warnings`
  Expected: no warnings. Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-engine/src/enricher.rs crates/rb-engine/src/lib.rs && git commit -m "feat(rb-engine): add Enricher async trait and Enrichment value type"`

---

### Task 21: Scaffold the `rb-enrich` crate (manifest + empty lib, wired into the workspace)

**Files:**
- Modify: `Cargo.toml` (workspace `members` + add `blocking` to the `reqwest` workspace dep)
- Create: `crates/rb-enrich/Cargo.toml`
- Create: `crates/rb-enrich/src/lib.rs`

> Stand up `rb-enrich` as a buildable empty crate so the next tasks have a home. It depends on `rb-types` (errors/types), `rb-engine` (the `Enricher`/`Linker` traits), `reqwest` (with `blocking` for the sync `Linker` impl + async for the enricher), `secrecy`, `serde`/`serde_json`, `async-trait`, `chrono` (the `AnthropicLinker` stamps `MemoryLink.created_at`), `tracing`, and dev-deps `tokio` + `wiremock`. The crate is OPT-IN: nothing in the default daemon build path links it unless the binary wires it (a later sibling task). VERIFIED: workspace members order is rb-types, rb-store, rb-proto, rb-embed, rb-search, rb-engine, rb-daemon, rusty-brain (rb-engine then rb-daemon), and the reqwest workspace line is `reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }`. `chrono`, `tracing`, `secrecy`, `wiremock`, `async-trait`, `serde`, `serde_json`, `tokio` are all already workspace deps.

- [ ] **Step 1: Add `reqwest`'s `blocking` feature at the workspace level.** In the root `Cargo.toml`, change the `reqwest` workspace dependency line to add the `blocking` feature (keep `json`, `rustls-tls`, `default-features = false`):

```toml
reqwest = { version = "0.12", features = ["json", "rustls-tls", "blocking"], default-features = false }
```

  (verify against installed reqwest at execution; `blocking` is a standard reqwest 0.12 feature and uses the already-enabled `rustls-tls` for the blocking client. This is additive — existing async users are unaffected.)

- [ ] **Step 2: Add the crate to the workspace members.** In the root `Cargo.toml`, add `"crates/rb-enrich"` to the `members` array (after `"crates/rb-engine"`):

```toml
members = [
    "crates/rb-types",
    "crates/rb-store",
    "crates/rb-proto",
    "crates/rb-embed",
    "crates/rb-search",
    "crates/rb-engine",
    "crates/rb-enrich",
    "crates/rb-daemon",
    "crates/rusty-brain",
]
```

- [ ] **Step 3: Create the manifest.** Write `crates/rb-enrich/Cargo.toml`:

```toml
[package]
name = "rb-enrich"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Opt-in LLM enrichment (summary/keywords/type/importance) and semantic linking; default heuristic, offline."

[lib]
name = "rb_enrich"
path = "src/lib.rs"

[dependencies]
rb-types = { path = "../rb-types" }
rb-engine = { path = "../rb-engine" }
async-trait = { workspace = true }
reqwest = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
secrecy = { workspace = true }
chrono = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
wiremock = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 4: Create the empty lib.** Write `crates/rb-enrich/src/lib.rs`:

```rust
//! `rb_enrich`: opt-in LLM enrichment and semantic linking for rusty-brain.
//!
//! The default path is the offline, deterministic [`HeuristicEnricher`]. The
//! opt-in [`AnthropicEnricher`] and [`AnthropicLinker`] talk to the Anthropic
//! API and are NEVER required; absence of `ANTHROPIC_API_KEY` degrades to the
//! heuristic path. No live network is touched by the test suite (wiremock only).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
```

- [ ] **Step 5: Build the workspace — expect PASS.** Run: `cargo build -p rb-enrich`
  Expected: `Finished` (empty crate compiles). Run: `cargo metadata --format-version 1 --no-deps >/dev/null && echo metadata-ok`
  Expected: prints `metadata-ok` (the new member resolves).

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-enrich --all-targets -- -D warnings`
  Expected: no warnings. Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 7: Commit.** Run: `git add Cargo.toml crates/rb-enrich/Cargo.toml crates/rb-enrich/src/lib.rs && git commit -m "chore(rb-enrich): scaffold opt-in enrichment crate"`

---

### Task 22: rb-enrich — `HeuristicEnricher` (offline default, deterministic, never errors)

**Files:**
- Create: `crates/rb-enrich/src/heuristic.rs`
- Modify: `crates/rb-enrich/src/lib.rs` (add `mod heuristic;` + re-export)

> `HeuristicEnricher` reproduces the P1 behavior as an `Enricher`: a ~150-char summary and up to 5 lowercased keyword tokens (>= 4 chars, deduped, order-preserving). It sets no tags/type/importance (those stay caller-driven). It is pure, offline, deterministic, and CANNOT error (always `Ok`). This is the default the engine uses when no enricher is configured AND the explicit fallback when `AnthropicEnricher::from_env` finds no key. The summary/keyword logic is self-contained (rb-engine's `enrich` module is `pub(crate)`, so it cannot be reused across crates — the logic is intentionally duplicated and unit-tested to match P1 byte-for-byte).

- [ ] **Step 1: Write the failing tests AND declare the module.** Add `mod heuristic;` to `crates/rb-enrich/src/lib.rs`, plus the re-export `pub use heuristic::HeuristicEnricher;`. Create `crates/rb-enrich/src/heuristic.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_engine::Enricher;

    #[tokio::test]
    async fn summary_is_trimmed_prefix_for_short_content() {
        let e = HeuristicEnricher::default();
        let out = e.enrich("  one DB one transaction  ", None).await.unwrap();
        assert_eq!(out.summary.as_deref(), Some("one DB one transaction"));
    }

    #[tokio::test]
    async fn summary_truncates_to_150_chars_on_char_boundary() {
        let e = HeuristicEnricher::default();
        let content = "é".repeat(200); // 2 bytes each; must not split a code point
        let out = e.enrich(&content, None).await.unwrap();
        let summary = out.summary.unwrap();
        assert_eq!(summary.chars().count(), 150);
        assert!(summary.chars().all(|c| c == 'é'));
    }

    #[tokio::test]
    async fn keywords_lowercased_deduped_capped_at_five() {
        let e = HeuristicEnricher::default();
        let out = e
            .enrich(
                "SQLite WAL mode enables concurrent SQLITE readers and writers safely",
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            out.keywords,
            vec!["sqlite", "mode", "enables", "concurrent", "readers"]
        );
    }

    #[tokio::test]
    async fn sets_no_tags_type_or_importance() {
        let e = HeuristicEnricher::default();
        let out = e.enrich("body text here", Some("ctx")).await.unwrap();
        assert!(out.tags.is_empty());
        assert!(out.memory_type.is_none());
        assert!(out.importance.is_none());
    }

    #[tokio::test]
    async fn is_deterministic_same_input_same_output() {
        let e = HeuristicEnricher::default();
        let a = e.enrich("repeatable content for hashing", None).await.unwrap();
        let b = e.enrich("repeatable content for hashing", None).await.unwrap();
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-enrich heuristic`
  Expected: FAIL to compile (`cannot find type 'HeuristicEnricher'`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `crates/rb-enrich/src/heuristic.rs`:

```rust
use async_trait::async_trait;
use rb_engine::{Enricher, Enrichment};

/// Maximum characters retained in a heuristic summary.
const SUMMARY_MAX_CHARS: usize = 150;
/// Maximum number of derived keywords.
const MAX_KEYWORDS: usize = 5;
/// Minimum token length (in characters) kept as a keyword.
const MIN_KEYWORD_LEN: usize = 4;

/// Offline, deterministic enricher: a trimmed ~150-char summary and up to five
/// lowercased keyword tokens. Sets no tags/type/importance. Never errors.
#[derive(Debug, Default, Clone)]
pub struct HeuristicEnricher;

fn default_summary(content: &str) -> String {
    content.trim().chars().take(SUMMARY_MAX_CHARS).collect()
}

fn derive_keywords(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in content.split(|c: char| !c.is_alphanumeric()) {
        if raw.chars().count() < MIN_KEYWORD_LEN {
            continue;
        }
        let token = raw.to_lowercase();
        if !out.iter().any(|existing| existing == &token) {
            out.push(token);
        }
        if out.len() == MAX_KEYWORDS {
            break;
        }
    }
    out
}

#[async_trait]
impl Enricher for HeuristicEnricher {
    async fn enrich(
        &self,
        content: &str,
        _context: Option<&str>,
    ) -> rb_types::Result<Enrichment> {
        Ok(Enrichment {
            summary: Some(default_summary(content)),
            keywords: derive_keywords(content),
            tags: Vec::new(),
            memory_type: None,
            importance: None,
        })
    }
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-enrich heuristic`
  Expected: PASS (5 tests).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-enrich --all-targets -- -D warnings`
  Expected: no warnings. Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-enrich/src/heuristic.rs crates/rb-enrich/src/lib.rs && git commit -m "feat(rb-enrich): add offline deterministic HeuristicEnricher"`

---

### Task 23: rb-enrich — `AnthropicEnricher` (opt-in, reqwest, key never leaks, wiremock-tested)

**Files:**
- Create: `crates/rb-enrich/src/anthropic.rs`
- Modify: `crates/rb-enrich/src/lib.rs` (add `mod anthropic;` + re-export)

> `AnthropicEnricher` POSTs to `{base_url}/messages` (Anthropic Messages API) using `claude-haiku-4-5`, a structured prompt that asks for JSON `{summary, keywords, tags, memory_type, importance}`, a request timeout, and parses the model's JSON out of the response. `from_env` reads `ANTHROPIC_API_KEY`; if absent it returns `Ok(None)` so the caller cleanly falls back to the heuristic. Every failure maps to `Error::Enrichment`, and the key is a `SecretString` that NEVER appears in a log, error, or `Debug`. All wire tests use wiremock; the real API is `#[ignore]`. This mirrors the established `rb-embed::VoyageProvider` async pattern (secrecy 0.10 `SecretString::from`/`expose_secret`, reqwest 0.12 `.header`/`error_for_status`/async `json`, `for_test` building the client with `unwrap_or_else` under `#[cfg(test)]`).

- [ ] **Step 1: Write the failing tests AND declare the module.** Add `mod anthropic;` + `pub use anthropic::AnthropicEnricher;` to `crates/rb-enrich/src/lib.rs`. Create `crates/rb-enrich/src/anthropic.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_engine::Enricher;
    use rb_types::MemoryType;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn enricher_for(base_url: &str) -> AnthropicEnricher {
        AnthropicEnricher::for_test("claude-haiku-4-5", "test-key", base_url)
    }

    // Anthropic returns content blocks; our enricher reads the first text block,
    // which the model is prompted to make a JSON object.
    fn message_response(json_text: &str) -> serde_json::Value {
        serde_json::json!({
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "content": [ { "type": "text", "text": json_text } ]
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sends_correct_request_and_parses_enrichment() {
        let server = MockServer::start().await;
        let model_json = serde_json::json!({
            "summary": "single writer over sqlite wal",
            "keywords": ["sqlite", "wal", "writer"],
            "tags": ["architecture"],
            "memory_type": "architecture_decision",
            "importance": 8
        })
        .to_string();

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-version", "2023-06-01"))
            .and(body_partial_json(serde_json::json!({
                "model": "claude-haiku-4-5"
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(message_response(&model_json)),
            )
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let e = enricher_for(&base);
        let out = e
            .enrich("agents share one sqlite db via a single writer", Some("ctx"))
            .await
            .unwrap();

        assert_eq!(out.summary.as_deref(), Some("single writer over sqlite wal"));
        assert_eq!(
            out.keywords,
            vec!["sqlite".to_string(), "wal".to_string(), "writer".to_string()]
        );
        assert_eq!(out.tags, vec!["architecture".to_string()]);
        assert_eq!(out.memory_type, Some(MemoryType::ArchitectureDecision));
        assert_eq!(out.importance, Some(8));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_error_status_is_enrichment_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let e = enricher_for(&base);
        let err = e.enrich("x", None).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Enrichment(_)),
            "expected Error::Enrichment, got {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unparseable_model_json_is_enrichment_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(message_response("not json at all")),
            )
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let e = enricher_for(&base);
        let err = e.enrich("x", None).await.unwrap_err();
        assert!(matches!(err, rb_types::Error::Enrichment(_)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_key_never_leaks_into_error_messages() {
        // Point at a closed port so the request fails at the transport layer; the
        // resulting error message must not contain the secret key. reqwest error
        // Display includes the URL + OS error but never request header values.
        let e = AnthropicEnricher::for_test(
            "claude-haiku-4-5",
            "super-secret-key-value",
            "http://127.0.0.1:1/v1",
        );
        let err = e.enrich("x", None).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("super-secret-key-value"),
            "error message leaked the api key: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn from_env_absent_key_returns_none() {
        // NOTE: this mutates the process-global ANTHROPIC_API_KEY. It is safe in
        // this crate only because no other (non-ignored) test reads that env var
        // concurrently; restore the prior value to avoid leaking into ignored
        // tests. (edition 2021: set_var/remove_var are not unsafe.)
        let prev = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::remove_var("ANTHROPIC_API_KEY");
        let got = AnthropicEnricher::from_env();
        if let Some(p) = prev {
            std::env::set_var("ANTHROPIC_API_KEY", p);
        }
        assert!(got.unwrap().is_none());
    }

    // Real-API smoke test. Ignored by default; run with:
    //   ANTHROPIC_API_KEY=... cargo test -p rb-enrich -- --ignored anthropic_real_api
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires ANTHROPIC_API_KEY and network access"]
    async fn anthropic_real_api_smoke() {
        let e = AnthropicEnricher::from_env().unwrap().unwrap();
        let out = e
            .enrich("use one sqlite database with a single writer thread", None)
            .await
            .unwrap();
        assert!(out.summary.is_some());
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-enrich anthropic`
  Expected: FAIL to compile (`cannot find type 'AnthropicEnricher'` / `no function 'for_test'` / `from_env`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `crates/rb-enrich/src/anthropic.rs`. The struct derives NO `Debug` (so the key cannot be printed); errors are built from `e.to_string()` of reqwest/serde which never include the `x-api-key` value:

```rust
use async_trait::async_trait;
use rb_engine::{Enricher, Enrichment};
use rb_types::{Error, MemoryType};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

/// Default model used for enrichment.
const DEFAULT_MODEL: &str = "claude-haiku-4-5";
/// Anthropic API base; `/messages` is appended per request.
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Outbound request timeout (all enrichment calls are timed out).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on tokens the model may produce for the enrichment JSON.
const MAX_TOKENS: u32 = 512;

/// Opt-in LLM enricher backed by the Anthropic Messages API. The key is held as
/// a `SecretString` and exposed only when building the request header; it never
/// appears in logs, errors, or Debug output (this type derives no `Debug`).
pub struct AnthropicEnricher {
    client: reqwest::Client,
    api_key: SecretString,
    model: String,
    base_url: String,
}

/// Anthropic `/messages` response — only the fields we read.
#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

/// The JSON object the model is prompted to emit.
#[derive(Deserialize)]
struct ModelEnrichment {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    memory_type: Option<String>,
    #[serde(default)]
    importance: Option<u8>,
}

impl AnthropicEnricher {
    /// Build from the environment. Returns `Ok(None)` when `ANTHROPIC_API_KEY`
    /// is absent so the caller falls back to the heuristic enricher. Returns
    /// `Err(Error::Enrichment)` only if the HTTP client cannot be built.
    pub fn from_env() -> rb_types::Result<Option<Self>> {
        match std::env::var("ANTHROPIC_API_KEY") {
            Ok(key) if !key.is_empty() => {
                Ok(Some(Self::build(DEFAULT_MODEL, key, DEFAULT_BASE_URL)?))
            }
            _ => Ok(None),
        }
    }

    /// Test-only constructor: explicit key + base URL, no environment access.
    #[cfg(test)]
    pub(crate) fn for_test(model: &str, api_key: &str, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            api_key: SecretString::from(api_key.to_string()),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn build(model: &str, api_key: String, base_url: &str) -> rb_types::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| Error::Enrichment(format!("failed to build http client: {e}")))?;
        Ok(Self {
            client,
            api_key: SecretString::from(api_key),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn prompt(content: &str, context: Option<&str>) -> String {
        let ctx = context.unwrap_or("");
        format!(
            "You enrich a developer memory. Respond with ONLY a JSON object \
             (no prose, no code fences) with keys: summary (string, <=150 chars), \
             keywords (array of <=5 lowercase strings), tags (array of strings), \
             memory_type (one of: architecture_decision, code_pattern, bug_fix, \
             configuration, constraint, entity, insight, reference, preference), \
             importance (integer 1-10).\n\nCONTEXT:\n{ctx}\n\nCONTENT:\n{content}"
        )
    }
}

#[async_trait]
impl Enricher for AnthropicEnricher {
    async fn enrich(
        &self,
        content: &str,
        context: Option<&str>,
    ) -> rb_types::Result<Enrichment> {
        let url = format!("{}/messages", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": [
                { "role": "user", "content": Self::prompt(content, context) }
            ]
        });

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Enrichment(format!("anthropic request failed: {e}")))?;

        let resp = resp.error_for_status().map_err(|e| {
            Error::Enrichment(format!("anthropic returned an error status: {e}"))
        })?;

        let parsed: MessagesResponse = resp
            .json()
            .await
            .map_err(|e| Error::Enrichment(format!("failed to parse anthropic response: {e}")))?;

        let text = parsed
            .content
            .into_iter()
            .find(|b| b.kind == "text")
            .map(|b| b.text)
            .ok_or_else(|| Error::Enrichment("anthropic response had no text block".to_string()))?;

        let model: ModelEnrichment = serde_json::from_str(text.trim())
            .map_err(|e| Error::Enrichment(format!("model did not return valid JSON: {e}")))?;

        let memory_type = match model.memory_type {
            Some(s) => Some(
                MemoryType::parse(&s)
                    .map_err(|e| Error::Enrichment(format!("model returned bad memory_type: {e}")))?,
            ),
            None => None,
        };
        let importance = match model.importance {
            Some(i) if (1..=10).contains(&i) => Some(i),
            Some(i) => {
                return Err(Error::Enrichment(format!(
                    "model returned importance {i} out of range 1..=10"
                )))
            }
            None => None,
        };

        Ok(Enrichment {
            summary: model.summary,
            keywords: model.keywords,
            tags: model.tags,
            memory_type,
            importance,
        })
    }
}
```

  (verify against installed crates at execution; reqwest 0.12 `.header(name, value)`/`error_for_status`/async `json`, secrecy 0.10 `SecretString::from`/`expose_secret` returning `&str`, and the Anthropic Messages request/response shape — adjust the JSON keys if the API differs. The `unwrap_or_else` in `for_test` is `#[cfg(test)]`-gated so no `unwrap`/`expect`/`panic` workspace lint fires in non-test code.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-enrich anthropic`
  Expected: PASS. The 5 non-ignored tests pass; `anthropic_real_api_smoke` is reported `ignored`.

- [ ] **Step 5: Confirm the real-API test is gated (does NOT run by default).** Run: `cargo test -p rb-enrich anthropic 2>&1 | grep -E "anthropic_real_api_smoke|test result"`
  Expected: a line `anthropic_real_api_smoke ... ignored` and a `test result: ok.` summary showing `1 ignored`. CI never depends on a live Anthropic key.

- [ ] **Step 6: Lint + format.** Run: `cargo clippy -p rb-enrich --all-targets -- -D warnings`
  Expected: no warnings. Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-enrich/src/anthropic.rs crates/rb-enrich/src/lib.rs && git commit -m "feat(rb-enrich): add opt-in AnthropicEnricher (reqwest, key-redacted, wiremock-tested)"`

---

### Task 24: rb-enrich — `AnthropicLinker` impl of `rb_engine::Linker` (sync trait, blocking client, wiremock-tested)

**Files:**
- Create: `crates/rb-enrich/src/linker.rs`
- Modify: `crates/rb-enrich/src/lib.rs` (add `mod linker;` + re-export)

> The spine's `rb_engine::Linker` (from Part M) is a SYNC trait: `fn link(&self, new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> Vec<MemoryLink>`. `AnthropicLinker` asks the model for semantic link types among candidates. Because the trait is sync and the linker does network IO, it uses `reqwest::blocking::Client` with a timeout and is meant to be invoked OFF the async reactor (the engine wires linkers via `spawn_blocking`). The default linker stays `SimilarityLinker`. On ANY failure (HTTP, parse, bad type) `AnthropicLinker::link` returns an EMPTY `Vec` (best-effort: a linking failure must never break `remember`) and logs at `warn` — it never panics and never returns a partial-bad link. The key is a `SecretString`, never logged.
>
> HARD DEPENDENCY ON Part M (the graph-link cluster): requires `rb_engine::Linker`, `rb_engine::SimilarityLinker`, and the `rb_types::MemoryLink` fields `{source_id, target_id, link_type, strength, reason, created_at}` (VERIFIED present in rb_types) to exist. As of this review `rb_engine::Linker`/`SimilarityLinker` do NOT yet exist in the tree — `impl Linker for AnthropicLinker` will not compile until Part M lands. DO NOT start this task before Part M is merged into the worktree.
>
> CONCURRENCY NOTE: building a `reqwest::blocking::Client` inside a tokio runtime context does not panic (the inner current-thread runtime is constructed lazily); the panic risk is only in calling `.send()` (which internally `block_on`s) from a runtime worker thread. Every test therefore drives `link()` through `tokio::task::spawn_blocking`, so the blocking request runs on a blocking-pool thread, never a reactor worker.

- [ ] **Step 1: Write the failing tests AND declare the module.** Add `mod linker;` + `pub use linker::AnthropicLinker;` to `crates/rb-enrich/src/lib.rs`. Create `crates/rb-enrich/src/linker.rs` with the test module only:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_engine::Linker;
    use rb_types::{LinkType, MemoryNote, MemoryType, Namespace};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn note(content: &str) -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rb".into()),
            content.to_string(),
            MemoryType::Insight,
            5,
        )
    }

    fn message_response(json_text: &str) -> serde_json::Value {
        serde_json::json!({
            "content": [ { "type": "text", "text": json_text } ]
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn maps_model_link_types_for_candidates() {
        let server = MockServer::start().await;
        let new = note("introduce single-writer daemon");
        let cand = note("agents shared a file via flock");
        let cand_id = cand.id.clone();
        let new_id = new.id.clone();

        // Model is told candidate index 0 EXTENDS the new memory with strength 0.9.
        let model_json = serde_json::json!({
            "links": [
                { "index": 0, "link_type": "extends", "strength": 0.9 }
            ]
        })
        .to_string();

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(message_response(&model_json)),
            )
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let linker = AnthropicLinker::for_test("claude-haiku-4-5", "k", &base);
        let candidates = vec![(cand, 0.4_f32)];
        let links = tokio::task::spawn_blocking(move || linker.link(&new, &candidates))
            .await
            .unwrap();

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source_id, new_id);
        assert_eq!(links[0].target_id, cand_id);
        assert_eq!(links[0].link_type, LinkType::Extends);
        assert!((links[0].strength - 0.9).abs() < 1e-6);
        assert_eq!(links[0].reason, "llm");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_error_yields_empty_links_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let linker = AnthropicLinker::for_test("claude-haiku-4-5", "k", &base);
        let new = note("a");
        let candidates = vec![(note("b"), 0.5_f32)];
        let links = tokio::task::spawn_blocking(move || linker.link(&new, &candidates))
            .await
            .unwrap();
        assert!(links.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn out_of_range_index_is_skipped() {
        let server = MockServer::start().await;
        let model_json = serde_json::json!({
            "links": [ { "index": 99, "link_type": "references", "strength": 0.5 } ]
        })
        .to_string();
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(message_response(&model_json)),
            )
            .mount(&server)
            .await;

        let base = format!("{}/v1", server.uri());
        let linker = AnthropicLinker::for_test("claude-haiku-4-5", "k", &base);
        let new = note("a");
        let candidates = vec![(note("b"), 0.5_f32)];
        let links = tokio::task::spawn_blocking(move || linker.link(&new, &candidates))
            .await
            .unwrap();
        assert!(links.is_empty());
    }

    #[test]
    fn empty_candidates_short_circuits_without_network() {
        // No server: if link() called out it would fail to connect. With no
        // candidates it must return empty immediately (no blocking IO).
        let linker = AnthropicLinker::for_test("claude-haiku-4-5", "k", "http://127.0.0.1:1/v1");
        let new = note("a");
        let links = linker.link(&new, &[]);
        assert!(links.is_empty());
    }
}
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-enrich linker`
  Expected: FAIL to compile (`cannot find type 'AnthropicLinker'`; and, until Part M lands, `cannot find trait 'Linker' in 'rb_engine'`).

- [ ] **Step 3: Add the implementation above the test module.** Prepend to `crates/rb-enrich/src/linker.rs`. `link` is infallible at the signature level (returns `Vec`), so internal errors degrade to an empty vec + a `warn` log:

```rust
use rb_engine::Linker;
use rb_types::{LinkType, MemoryLink, MemoryNote};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

const DEFAULT_MODEL: &str = "claude-haiku-4-5";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TOKENS: u32 = 512;

/// Opt-in semantic linker backed by the Anthropic Messages API. Implements the
/// SYNCHRONOUS `rb_engine::Linker`; it performs blocking IO and is meant to be
/// driven via `spawn_blocking`. Any failure degrades to an empty link set
/// (best-effort) and a `warn` log. The key never appears in logs or output.
pub struct AnthropicLinker {
    client: reqwest::blocking::Client,
    api_key: SecretString,
    model: String,
    base_url: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct ModelLinks {
    #[serde(default)]
    links: Vec<ModelLink>,
}

#[derive(Deserialize)]
struct ModelLink {
    index: usize,
    link_type: String,
    strength: f32,
}

impl AnthropicLinker {
    /// Build from the environment. `None` when `ANTHROPIC_API_KEY` is absent.
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())?;
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .ok()?;
        Some(Self {
            client,
            api_key: SecretString::from(key),
            model: DEFAULT_MODEL.to_string(),
            base_url: DEFAULT_BASE_URL.trim_end_matches('/').to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(model: &str, api_key: &str, base_url: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());
        Self {
            client,
            api_key: SecretString::from(api_key.to_string()),
            model: model.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    fn prompt(new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> String {
        let mut lines = String::new();
        for (i, (note, dist)) in candidates.iter().enumerate() {
            lines.push_str(&format!("[{i}] (distance {dist:.3}) {}\n", note.content));
        }
        format!(
            "Given a NEW memory and CANDIDATE memories, respond with ONLY JSON \
             (no prose) of the form {{\"links\":[{{\"index\":<int>,\"link_type\":\
             <one of extends|contradicts|implements|references|supersedes>,\
             \"strength\":<0.0-1.0>}}]}}. Omit weak relations.\n\nNEW:\n{}\n\n\
             CANDIDATES:\n{lines}",
            new.content
        )
    }

    /// Inner fallible body; `link` wraps this and degrades errors to empty.
    fn try_link(
        &self,
        new: &MemoryNote,
        candidates: &[(MemoryNote, f32)],
    ) -> rb_types::Result<Vec<MemoryLink>> {
        let url = format!("{}/messages", self.base_url);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": [
                { "role": "user", "content": Self::prompt(new, candidates) }
            ]
        });

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .map_err(|e| rb_types::Error::Enrichment(format!("link request failed: {e}")))?
            .error_for_status()
            .map_err(|e| rb_types::Error::Enrichment(format!("link error status: {e}")))?;

        let parsed: MessagesResponse = resp
            .json()
            .map_err(|e| rb_types::Error::Enrichment(format!("link parse failed: {e}")))?;

        let text = parsed
            .content
            .into_iter()
            .find(|b| b.kind == "text")
            .map(|b| b.text)
            .ok_or_else(|| rb_types::Error::Enrichment("no text block".to_string()))?;

        let model: ModelLinks = serde_json::from_str(text.trim())
            .map_err(|e| rb_types::Error::Enrichment(format!("link json invalid: {e}")))?;

        let now = chrono::Utc::now();
        let mut out = Vec::new();
        for ml in model.links {
            let Some((cand, _)) = candidates.get(ml.index) else {
                continue; // model referenced a non-existent candidate; skip.
            };
            if cand.id == new.id {
                continue; // never self-link.
            }
            let Ok(link_type) = LinkType::parse(&ml.link_type) else {
                continue; // unknown type; skip rather than fail.
            };
            out.push(MemoryLink {
                source_id: new.id.clone(),
                target_id: cand.id.clone(),
                link_type,
                strength: ml.strength.clamp(0.0, 1.0),
                reason: "llm".to_string(),
                created_at: now,
            });
        }
        Ok(out)
    }
}

impl Linker for AnthropicLinker {
    fn link(&self, new: &MemoryNote, candidates: &[(MemoryNote, f32)]) -> Vec<MemoryLink> {
        if candidates.is_empty() {
            return Vec::new();
        }
        match self.try_link(new, candidates) {
            Ok(links) => links,
            Err(e) => {
                tracing::warn!(error = %e, "anthropic linker failed; returning no links");
                Vec::new()
            }
        }
    }
}
```

  (verify against installed crates at execution; the `rb_engine::Linker` trait signature, `reqwest::blocking` 0.12 `Client::builder().timeout(..).build()`/`.header(..)`/`error_for_status`/`json`, `chrono::Utc::now()`, and the Anthropic shape — adjust if any differ. The `unwrap_or_else` in `for_test` is `#[cfg(test)]`-gated. `chrono` is a dependency of `rb-enrich` as of Task 21.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-enrich linker`
  Expected: PASS (4 tests).

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-enrich --all-targets -- -D warnings`
  Expected: no warnings. Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 6: Commit.** Run: `git add crates/rb-enrich/src/linker.rs crates/rb-enrich/src/lib.rs && git commit -m "feat(rb-enrich): add opt-in AnthropicLinker (blocking client, best-effort, wiremock-tested)"`

---

### Task 25: rb-engine — wire opt-in `Enricher` into `remember` (fill only empty fields; heuristic fallback)

**Files:**
- Modify: `crates/rb-engine/src/engine.rs` (add `enricher` field, `with_enricher` builder, enrich-then-fill in `remember`)
- Modify: `crates/rb-engine/src/test_support.rs` (add deterministic in-test `Enricher`s for assertions)
- Modify: `crates/rb-engine/Cargo.toml` (add `tracing`)

> Wire the `Enricher` into `remember` OPT-IN: a new `enricher: Option<Arc<dyn Enricher>>` field defaults to `None` (existing `new` callers and the daemon are unaffected — VERIFIED the only construction site is `MemoryEngine::new(store, embedder, namespace)` in `rb-daemon/src/server.rs:370`). A `with_enricher` builder turns it on. When set, `remember` calls `enricher.enrich(content, context)`; on success it fills the summary (when the model returns one) plus keywords/tags ONLY when the caller left them empty. On enricher ERROR, `remember` falls back to the heuristic path (best-effort: enrichment must never fail a remember). This keeps the engine's default behavior byte-identical to P1 when no enricher is configured. (For v1, model-supplied `memory_type`/`importance` are NOT applied — `RememberInput` always carries both as required fields, so there is no "empty" slot for them; that is left for a future opt-in caller change.)

- [ ] **Step 1: Add deterministic in-test enrichers to `test_support.rs`.** Append to `crates/rb-engine/src/test_support.rs` (it is `#[cfg(test)]`-only and already imports rb_types items at the top):

```rust
use crate::enricher::{Enricher, Enrichment};

/// In-test enricher returning fixed values so `remember` wiring is assertable
/// without any network. Used to prove the engine fills empty fields from it.
pub(crate) struct FixedEnricher;

#[async_trait::async_trait]
impl Enricher for FixedEnricher {
    async fn enrich(
        &self,
        _content: &str,
        _context: Option<&str>,
    ) -> rb_types::Result<Enrichment> {
        Ok(Enrichment {
            summary: Some("enriched summary".to_string()),
            keywords: vec!["enrkw".to_string()],
            tags: vec!["enrtag".to_string()],
            memory_type: None,
            importance: None,
        })
    }
}

/// In-test enricher that always fails, to prove `remember` falls back cleanly.
pub(crate) struct FailingEnricher;

#[async_trait::async_trait]
impl Enricher for FailingEnricher {
    async fn enrich(
        &self,
        _content: &str,
        _context: Option<&str>,
    ) -> rb_types::Result<Enrichment> {
        Err(rb_types::Error::Enrichment("boom".to_string()))
    }
}
```

- [ ] **Step 2: Write the failing tests.** Append to the `tests` module in `crates/rb-engine/src/engine.rs` (inside the existing `mod tests { ... }`, reusing its `engine()`/`input()` helpers). NOTE: `use std::sync::Arc;` ALREADY exists in this test module (line 333) — do NOT re-import it; only add the `test_support` import:

```rust
    use crate::test_support::{FailingEnricher, FixedEnricher};

    #[tokio::test]
    async fn remember_with_enricher_fills_empty_keywords_tags_and_summary() {
        let eng = engine().with_enricher(Arc::new(FixedEnricher));
        // caller leaves keywords/tags empty -> enricher fills them.
        let id = eng.remember(input("some content body", 5)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.summary, "enriched summary");
        assert_eq!(note.keywords, vec!["enrkw".to_string()]);
        assert_eq!(note.tags, vec!["enrtag".to_string()]);
    }

    #[tokio::test]
    async fn remember_with_enricher_preserves_caller_supplied_keywords_and_tags() {
        let eng = engine().with_enricher(Arc::new(FixedEnricher));
        let mut inp = input("body", 5);
        inp.keywords = vec!["caller".to_string()];
        inp.tags = vec!["callertag".to_string()];
        let id = eng.remember(inp).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        // explicit caller values win over the enricher.
        assert_eq!(note.keywords, vec!["caller".to_string()]);
        assert_eq!(note.tags, vec!["callertag".to_string()]);
        // summary still comes from the enricher (caller never supplies it).
        assert_eq!(note.summary, "enriched summary");
    }

    #[tokio::test]
    async fn remember_falls_back_to_heuristic_when_enricher_errors() {
        let eng = engine().with_enricher(Arc::new(FailingEnricher));
        let content = "concurrent readers never block the single writer thread";
        let id = eng.remember(input(content, 6)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        // heuristic summary == trimmed content (< 150 chars); keywords non-empty.
        assert_eq!(note.summary, content);
        assert!(!note.keywords.is_empty());
    }

    #[tokio::test]
    async fn remember_without_enricher_is_unchanged_heuristic_path() {
        let eng = engine(); // no enricher
        let content = "single writer over sqlite wal keeps things correct";
        let id = eng.remember(input(content, 7)).await.unwrap();
        let note = eng.backend().note_of(&id).unwrap();
        assert_eq!(note.summary, content);
        assert!(!note.keywords.is_empty());
    }
```

- [ ] **Step 3: Run it — expect FAIL.** Run: `cargo test -p rb-engine engine::tests::remember_with_enricher`
  Expected: FAIL to compile (`no method named 'with_enricher'`).

- [ ] **Step 4: Add the field, builder, and enrich-then-fill logic.** In `crates/rb-engine/src/engine.rs`:

  (a) Extend the module-level imports at the top (the test module's `use std::sync::Arc;` shadows this glob harmlessly):

```rust
use crate::enricher::Enricher;
use std::sync::Arc;
```

  (b) Add the field to the struct (after `namespace`):

```rust
    enricher: Option<Arc<dyn Enricher>>,
```

  (c) Initialize it to `None` in `new` (add to the constructed `Self { .. }`):

```rust
            enricher: None,
```

  (d) Add the builder method inside the `impl` block (after `weights`):

```rust
    /// Enable opt-in enrichment. When set, `remember` asks the enricher to fill
    /// fields the caller left empty; on enricher error it falls back to the
    /// heuristic path (enrichment never fails a remember).
    pub fn with_enricher(mut self, enricher: Arc<dyn Enricher>) -> Self {
        self.enricher = Some(enricher);
        self
    }
```

  (e) Replace the heuristic-enrichment block inside `remember`. Find the existing block (VERIFIED at lines 98-105):

```rust
        // Heuristic enrichment (no LLM in P1).
        note.summary = default_summary(&note.content);
        note.keywords = if input.keywords.is_empty() {
            derive_keywords(&note.content)
        } else {
            input.keywords
        };
        note.tags = input.tags;
```

  and replace it with:

```rust
        // Enrichment: opt-in LLM, else heuristic. The enricher only fills fields
        // the caller left empty; an enricher error degrades to the heuristic.
        let enrichment = match &self.enricher {
            Some(e) => match e.enrich(&note.content, input.context.as_deref()).await {
                Ok(en) => Some(en),
                Err(err) => {
                    tracing::warn!(error = %err, "enricher failed; using heuristic enrichment");
                    None
                }
            },
            None => None,
        };

        note.summary = match enrichment.as_ref().and_then(|e| e.summary.clone()) {
            Some(s) => s,
            None => default_summary(&note.content),
        };
        note.keywords = if !input.keywords.is_empty() {
            input.keywords
        } else if let Some(en) = enrichment.as_ref().filter(|e| !e.keywords.is_empty()) {
            en.keywords.clone()
        } else {
            derive_keywords(&note.content)
        };
        note.tags = if !input.tags.is_empty() {
            input.tags
        } else {
            enrichment
                .as_ref()
                .map(|e| e.tags.clone())
                .unwrap_or_default()
        };
```

  Then add the `tracing` dependency to `crates/rb-engine/Cargo.toml` under `[dependencies]`:

```toml
tracing = { workspace = true }
```

  (verify against installed `async-trait`/`tracing` at execution; `input.context.as_deref()` yields `Option<&str>` matching `Enricher::enrich`. The enricher's `.await` completes before `input.context` is consumed later by the existing `if let Some(ctx) = input.context` block, so the borrow is released — but if the borrow checker complains, replace `input.context.as_deref()` with `input.context.clone().as_deref()` bound to a local `let ctx = input.context.clone();` before the match.)

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-engine engine::tests`
  Expected: PASS (the 4 new enricher tests plus all pre-existing `remember`/`recall` tests).

- [ ] **Step 6: Full workspace gate.** Run: `cargo test -p rb-engine -p rb-enrich`
  Expected: PASS. Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  Expected: no warnings. Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 7: Commit.** Run: `git add crates/rb-engine/src/engine.rs crates/rb-engine/src/test_support.rs crates/rb-engine/Cargo.toml && git commit -m "feat(rb-engine): wire opt-in Enricher into remember with heuristic fallback"`

## Part P — v1 hardening (the PR#1 should-fix backlog)

### Task 26: rb-types `Error::InvalidArgument` variant + wire-error mapping (rb-daemon AND rb-proto)

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/error.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/error_map.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-proto/src/error.rs`

Argument errors (out-of-range importance, etc.) are currently reported as `Error::Storage`, which is misleading — they are caller mistakes, not storage faults. This task adds `Error::InvalidArgument(String)` with a stable `Display` and a stable wire `kind` (`invalid_argument`) so the daemon can surface it distinctly. CRITICAL: both `rb-daemon/src/error_map.rs::error_to_response` AND `rb-proto/src/error.rs::error_kind` are NON-wildcard exhaustive `match`es on `rb_types::Error`; adding a variant breaks BOTH at compile time, so both must gain an arm in this task or the workspace will not build. The validator (Task 27) and the engine (Task 28) switch to it afterwards.

- [ ] **Step 1: Write the failing rb-types test.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/error.rs`, add this assertion inside the existing `display_messages_match_spine` test (after the `Embedding` assertion, before the closing `}` of that `#[test]`):

```rust
        assert_eq!(
            Error::InvalidArgument("importance 0 is out of range 1..=10".into()).to_string(),
            "invalid argument: importance 0 is out of range 1..=10"
        );
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-types error::tests::display_messages_match_spine` Expected: FAIL to compile — `no variant named 'InvalidArgument' found for enum 'Error'`. This confirms the test drives the new variant.

- [ ] **Step 3: Add the variant.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/error.rs`, add the variant to the `Error` enum, immediately after the `Embedding` variant (keep the existing `#[error(...)]` style):

```rust
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
```

- [ ] **Step 4: Run it — expect PASS for rb-types, FAIL to compile for the two consumers.** Run: `cargo test -p rb-types error::tests` Expected: PASS (all error tests, including the new assertion). Then run `cargo build -p rb-proto` Expected: FAIL to compile — `non-exhaustive patterns: '&Error::InvalidArgument(_)' not covered` in `error_kind`. This proves the proto match must be updated (the daemon match too); we fix both in the following steps.

- [ ] **Step 5: Write the failing daemon error-map test.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/error_map.rs`, add a new case to the `cases` vec inside `maps_each_error_to_stable_kind` (after the existing `Error::Io(...)` case):

```rust
            (Error::InvalidArgument("x".into()), "invalid_argument"),
```

- [ ] **Step 6: Run it — expect a compile/match failure.** Run: `cargo test -p rb-daemon error_map` Expected: FAIL to compile — the `match &err` in `error_to_response` is non-exhaustive (`pattern &Error::InvalidArgument(_) not covered`). Confirms the daemon mapping must be added.

- [ ] **Step 7: Map the new variant in the daemon (client-safe, message passes through).** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/error_map.rs`, the match yields `(kind, message)` TUPLES on every arm. `InvalidArgument` is a caller mistake, not an internal fault, so it belongs with the CLIENT-SAFE arms (the caller must learn WHICH argument was bad). Add this arm immediately after the `Error::DimensionMismatch { .. } => ("dimension_mismatch", err.to_string()),` arm (i.e. among the client-safe group, before the internal `Error::Storage(_) => { ... }` arm):

```rust
        Error::InvalidArgument(_) => ("invalid_argument", err.to_string()),
```

- [ ] **Step 8: Map the new variant in rb-proto's `error_kind`.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-proto/src/error.rs`, the `error_kind(err) -> &'static str` function returns a bare `&'static str` per arm. Add the arm after the `Error::Io(_) => "io",` arm:

```rust
        Error::InvalidArgument(_) => "invalid_argument",
```

  Then make `invalid_argument` reconstruct on the client side: in the same file, in `response_error_to_error`, add a match arm after the `"embedding" => Error::Embedding(message.to_string()),` arm:

```rust
        "invalid_argument" => Error::InvalidArgument(message.to_string()),
```

  Add a round-trip test inside the existing `#[cfg(test)] mod tests` block in `error.rs` (after `embedding_round_trips`):

```rust
    #[test]
    fn invalid_argument_round_trips() {
        assert!(matches!(
            round_trip(Error::InvalidArgument("importance 0".into())),
            Error::InvalidArgument(_)
        ));
    }
```

- [ ] **Step 9: Run it — expect PASS across all three crates.** Run: `cargo test -p rb-types error::tests` Expected: PASS. Run: `cargo test -p rb-daemon error_map` Expected: PASS (both error-map tests). Run: `cargo test -p rb-proto error` Expected: PASS (all proto error round-trip tests including `invalid_argument_round_trips`).

- [ ] **Step 10: Lint + format.** Run: `cargo clippy -p rb-types -p rb-daemon -p rb-proto --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 11: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-types/src/error.rs crates/rb-daemon/src/error_map.rs crates/rb-proto/src/error.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-types): add Error::InvalidArgument and map it to a stable wire kind"`

---

### Task 27: rb-types `validate_importance` — ONE shared 1..=10 validator, used by rb-store insert + update

**Files:**
- Create: `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/validate.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/lib.rs` (add `mod validate;` + re-export)
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-store/src/store.rs`

Importance range validation (1..=10) is currently duplicated in three places (rb-store `insert_memory`, rb-store `update_memory`, rb-engine `remember`/`update`). This task extracts ONE `validate_importance(u8) -> Result<()>` into `rb-types` (the only crate all three already depend on), returning the new `Error::InvalidArgument`, and switches both rb-store sites to it. rb-engine is switched in Task 28.

- [ ] **Step 1: Write the failing validator test AND declare the module.** Add `mod validate;` to `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/lib.rs` (alongside the other `mod` lines) and the re-export `pub use validate::validate_importance;` (alongside the other `pub use` lines). Create `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/validate.rs` with ONLY the test module:

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::Error;

    #[test]
    fn accepts_the_inclusive_1_to_10_range() {
        for imp in 1u8..=10 {
            assert!(validate_importance(imp).is_ok(), "{imp} must be valid");
        }
    }

    #[test]
    fn rejects_below_and_above_range_as_invalid_argument() {
        for bad in [0u8, 11, 255] {
            let err = validate_importance(bad).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument(_)),
                "expected InvalidArgument for {bad}, got {err:?}"
            );
            assert!(
                err.to_string().contains("importance") && err.to_string().contains(&bad.to_string()),
                "message must name the field and value: {err}"
            );
        }
    }
}
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-types validate` Expected: FAIL to compile — `cannot find function 'validate_importance' in this scope`. Confirms the test drives the new function.

- [ ] **Step 3: Implement the validator.** Prepend to `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/validate.rs`, above the test module:

```rust
use crate::error::{Error, Result};

/// The single source of truth for the valid importance range (inclusive 1..=10),
/// matching the `importance INTEGER CHECK (importance BETWEEN 1 AND 10)` schema
/// constraint. Returns [`Error::InvalidArgument`] on a value outside the range so
/// callers report a caller mistake, not a storage fault.
pub fn validate_importance(importance: u8) -> Result<()> {
    if (1..=10).contains(&importance) {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "importance {importance} is out of range 1..=10"
        )))
    }
}
```

  (verified at execution: rb-types declares `mod error;` and re-exports `Error`/`Result` at the crate root, so `use crate::error::{Error, Result};` resolves. If the module path ever differs, import from the crate root `use crate::{Error, Result};` instead — the only allowed hedge.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-types validate` Expected: PASS (2 tests).

- [ ] **Step 5: Switch rb-store `insert_memory` to the shared validator.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-store/src/store.rs`, replace the inline importance check in `insert_memory`:

```rust
        if note.importance < 1 || note.importance > 10 {
            return Err(Error::Storage(format!(
                "importance {} is out of range 1..=10",
                note.importance
            )));
        }
```

  with a call to the shared validator:

```rust
        rb_types::validate_importance(note.importance)?;
```

- [ ] **Step 6: Switch rb-store `update_memory` to the shared validator.** In the same file, replace the inline check in `update_memory`:

```rust
        if let Some(imp) = updates.importance {
            if !(1..=10).contains(&imp) {
                return Err(Error::Storage(format!(
                    "importance {imp} is out of range 1..=10"
                )));
            }
        }
```

  with:

```rust
        if let Some(imp) = updates.importance {
            rb_types::validate_importance(imp)?;
        }
```

- [ ] **Step 7: Update the THREE rb-store assertions that pin `Error::Storage` for importance.** There are three `matches!(err, Error::Storage(ref s) if s.contains("importance"))` assertions across TWO test modules (NOT two assertions in one place). Change each to `Error::InvalidArgument`:
  - In `mod insert_tests`, function `insert_rejects_out_of_range_importance`: BOTH assertions (the importance=0 case and the importance=11 case) — change `matches!(err, Error::Storage(ref s) if s.contains("importance"))` to `matches!(err, Error::InvalidArgument(ref s) if s.contains("importance"))` (also relax the assertion message text from "storage error" to "invalid argument" for clarity).
  - In `mod update_tests`, function `rejects_out_of_range_importance`: the single assertion inside the `for bad in [0u8, 11u8]` loop — change `matches!(err, Error::Storage(ref s) if s.contains("importance"))` to `matches!(err, Error::InvalidArgument(ref s) if s.contains("importance"))`.

  Leave the two `confidence` assertions in `insert_rejects_out_of_range_confidence` as `Error::Storage` — confidence validation is unchanged.

- [ ] **Step 8: Run the WHOLE rb-store crate (the importance tests live in `insert_tests`/`update_tests`, not `tests`).** Run: `cargo test -p rb-store` Expected: PASS — `insert_tests::insert_rejects_out_of_range_importance` and `update_tests::rejects_out_of_range_importance` now assert `InvalidArgument`; all other store tests unchanged. (Do NOT use a `store::tests` filter: the modified tests are in modules `store::insert_tests` and `store::update_tests`, whose paths do not contain `store::tests`, so such a filter would run none of them.) If any importance test still expects `Storage`, an assertion was missed in Step 7; fix the variant there.

- [ ] **Step 9: Lint + format.** Run: `cargo clippy -p rb-types -p rb-store --all-targets --all-features -- -D warnings` Expected: no warnings (the `Error` import in store.rs is still used by other arms — `archive_memory`/`add_link`/confidence use `Error::Storage`). Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 10: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-types/src/validate.rs crates/rb-types/src/lib.rs crates/rb-store/src/store.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "refactor(rb-store): use one shared validate_importance returning InvalidArgument"`

---

### Task 28: rb-engine `remember`/`update` use the shared validator (InvalidArgument)

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-engine/src/engine.rs`

The engine still validates importance inline with `Error::Storage` in both `remember` and `update`. This task removes that third duplication, routing both through `rb_types::validate_importance` so out-of-range importance now surfaces as `Error::InvalidArgument` consistently from the engine — and confirms `update` validates importance exactly like `remember` (the existing `update_rejects_out_of_range_importance` test already covers update; this task makes a new test assert the new variant on both paths).

- [ ] **Step 1: Write the failing test.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-engine/src/engine.rs`, inside the existing `#[cfg(test)] mod tests` block, add a test that pins the variant for BOTH paths (place it after `invalid_importance_is_rejected_before_embedding`):

```rust
    #[tokio::test]
    async fn out_of_range_importance_is_invalid_argument_on_both_paths() {
        let eng = engine();

        // remember path.
        let err = eng
            .remember(input("bad importance on remember", 0))
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "remember must reject with InvalidArgument, got {err:?}"
        );

        // update path.
        let id = eng.remember(input("valid body", 5)).await.unwrap();
        let err = eng
            .update(
                id,
                rb_types::MemoryUpdates {
                    importance: Some(11),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "update must reject with InvalidArgument, got {err:?}"
        );
    }
```

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rb-engine engine::tests::out_of_range_importance_is_invalid_argument_on_both_paths` Expected: FAIL — the assertion `matches!(err, Error::InvalidArgument(_))` fails because `remember`/`update` still produce `Error::Storage`. Confirms the test drives the switch.

- [ ] **Step 3: Switch `remember` to the shared validator.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-engine/src/engine.rs`, replace the inline check at the top of `remember`:

```rust
        if !(1..=10).contains(&input.importance) {
            return Err(rb_types::Error::Storage(format!(
                "importance {} is out of range 1..=10",
                input.importance
            )));
        }
```

  with:

```rust
        rb_types::validate_importance(input.importance)?;
```

- [ ] **Step 4: Switch `update` to the shared validator.** In the same file, replace the inline check in `update`:

```rust
        if let Some(importance) = updates.importance {
            if !(1..=10).contains(&importance) {
                return Err(rb_types::Error::Storage(format!(
                    "importance {importance} is out of range 1..=10"
                )));
            }
        }
```

  with:

```rust
        if let Some(importance) = updates.importance {
            rb_types::validate_importance(importance)?;
        }
```

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rb-engine engine::tests::out_of_range_importance_is_invalid_argument_on_both_paths` Expected: PASS. Then run the existing importance tests too: `cargo test -p rb-engine engine::tests::update_rejects_out_of_range_importance engine::tests::invalid_importance_is_rejected_before_embedding` Expected: PASS — both still pass because they assert on the message containing `"importance"` (variant-agnostic).

- [ ] **Step 6: Run the whole engine crate.** Run: `cargo test -p rb-engine` Expected: PASS (all engine unit + integration tests).

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rb-engine --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-engine/src/engine.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "refactor(rb-engine): validate importance via shared validator on remember and update"`

---

### Task 29: rb-store `checkpoint_truncate()` + WAL checkpoint on graceful writer shutdown

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-store/src/store.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`

The P1 writer relied on connection-close to flush the WAL. On graceful shutdown the daemon should run `PRAGMA wal_checkpoint(TRUNCATE)` to fold the WAL back into the main DB and truncate it, leaving a clean single-file state. This task adds `SqliteStore::checkpoint_truncate()` and calls it in the writer loop's `Shutdown` arm before dropping the store.

- [ ] **Step 1: Write the failing rb-store test.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-store/src/store.rs`, add a NEW test module at the end of the file (after the last existing test module) — keeping the test next to other file-backed tests is fine, but a fresh module avoids importing churn:

```rust
#[cfg(test)]
mod checkpoint_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    #[test]
    fn checkpoint_truncate_is_ok_on_file_and_memory_dbs() {
        // In-memory DB: journal_mode is "memory"; checkpoint is a harmless no-op.
        let mem = SqliteStore::open_in_memory(8).unwrap();
        mem.checkpoint_truncate().unwrap();

        // File-backed DB in WAL: insert one row, checkpoint, row still present.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let store = SqliteStore::open(&db, 8).unwrap();
        let ns = Namespace::Project("ckpt".to_string());
        let note = MemoryNote::new(ns, "checkpoint me".to_string(), MemoryType::Insight, 5);
        let id = note.id.clone();
        store.insert_memory(&note, Some(&vec![0.1f32; 8])).unwrap();

        store.checkpoint_truncate().unwrap();

        let got = store.get_memory(&id).unwrap();
        assert!(got.is_some(), "row survives a wal_checkpoint(TRUNCATE)");
        assert_eq!(got.unwrap().content, "checkpoint me");
    }
}
```

  (`tempfile` is already a `[dev-dependencies]` of rb-store, confirmed at execution against `crates/rb-store/Cargo.toml`.)

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-store checkpoint_truncate_is_ok_on_file_and_memory_dbs` Expected: FAIL to compile — `no method named 'checkpoint_truncate' found for struct 'SqliteStore'`. Confirms the test drives the method.

- [ ] **Step 3: Implement `checkpoint_truncate` via `execute_batch` (the only form that works for a parenthesized pragma in rusqlite 0.32).** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-store/src/store.rs`, add a method to the inherent `impl SqliteStore { ... }` block (the one containing `open`/`open_in_memory`/`init`, NOT the `impl Store for SqliteStore` block), immediately after `init` and before that block's closing `}`:

```rust
    /// Fold the WAL back into the main database file and truncate it to zero.
    ///
    /// Used on graceful daemon shutdown so the on-disk DB is a clean single file
    /// with no trailing WAL frames. On an in-memory or non-WAL connection SQLite
    /// reports the operation as a no-op and returns `SQLITE_OK`, so this never
    /// errors for those DBs.
    ///
    /// Uses `execute_batch` rather than `pragma_query`: rusqlite's `pragma_query`
    /// routes the pragma name through `push_keyword`, which rejects the
    /// parenthesized `wal_checkpoint(TRUNCATE)` form as a non-identifier. A raw
    /// `PRAGMA ...;` statement executed via `execute_batch` has the same
    /// semantics and accepts the argument syntax.
    pub fn checkpoint_truncate(&self) -> Result<()> {
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(storage_err)?;
        Ok(())
    }
```

  (`storage_err` is already imported at the top of the file via `use crate::error::{io_err, storage_err};`; `execute_batch` is already used in `init` and `insert_memory`, so the call form is verified against the installed rusqlite at execution.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-store checkpoint_truncate_is_ok_on_file_and_memory_dbs` Expected: PASS. This store-level test is the BEHAVIORAL proof that the checkpoint runs without error and preserves data.

- [ ] **Step 5: Write a daemon-level non-regression test for the shutdown path.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`, add to the `#[cfg(test)] mod tests` block a test that proves data survives a full handle shutdown to a fresh reopen. NOTE: connection-close already flushes the WAL, so this test alone does not prove the checkpoint executes — it is an integration guard that the wired checkpoint does not BREAK shutdown/persistence; Step 4 is the behavioral proof that the checkpoint itself runs:

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_checkpoints_so_data_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");

        let ns = Namespace::Project("ckpt-shutdown".to_string());
        let id;
        {
            let handle = StoreHandle::start(db.clone(), DIM, 2).unwrap();
            let n = note(&ns, "survive the shutdown checkpoint");
            id = n.id.clone();
            handle.write(n, Some(vec![0.2f32; DIM])).await.unwrap();
            // Graceful shutdown runs PRAGMA wal_checkpoint(TRUNCATE) in the writer
            // Shutdown arm, then joins the writer thread.
            handle.shutdown().await;
        }

        // Reopen a brand-new handle on the same file; the row must be present.
        let reopened = StoreHandle::start(db, DIM, 1).unwrap();
        let got = reopened.get(ns, id).await.unwrap();
        assert!(got.is_some(), "row must persist across a checkpointed shutdown");
        reopened.shutdown().await;
    }
```

- [ ] **Step 6: Run it — expect PASS, then wire the explicit checkpoint into the Shutdown arm.** Run: `cargo test -p rb-daemon --lib shutdown_checkpoints_so_data_survives_reopen` Expected: PASS (connection-close already persists). Now make the checkpoint explicit and deterministic: in `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`, in `writer_loop`, change the `WriteCommand::Shutdown` arm from:

```rust
            WriteCommand::Shutdown { reply } => {
                let _ = reply.send(());
                break;
            }
```

  to run the checkpoint on the live write store BEFORE replying and breaking (`store` is `Option<SqliteStore>` here):

```rust
            WriteCommand::Shutdown { reply } => {
                // Fold the WAL back into the main file before the connection is
                // dropped, so the on-disk DB is a clean single file. Best-effort:
                // a checkpoint failure is logged but must not block shutdown.
                if let Some(active) = store.as_ref() {
                    if let Err(e) = active.checkpoint_truncate() {
                        tracing::warn!(error = %e, "WAL checkpoint on shutdown failed");
                    }
                }
                let _ = reply.send(());
                break;
            }
```

- [ ] **Step 7: Run it — expect PASS.** Run: `cargo test -p rb-daemon --lib shutdown_checkpoints_so_data_survives_reopen` Expected: PASS. Run the full crate to confirm no regression: `cargo test -p rb-daemon` Expected: PASS.

- [ ] **Step 8: Lint + format.** Run: `cargo clippy -p rb-store -p rb-daemon --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 9: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-store/src/store.rs crates/rb-daemon/src/store_handle.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-store): add checkpoint_truncate and run WAL checkpoint on graceful shutdown"`

---

### Task 30: rb-daemon broadcast-lag observability — warn + counter on dropped change events

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`

The writer broadcasts `MemoryChanged` after every commit with `let _ = events.send(...)`, silently swallowing the `Err` that `tokio::broadcast::Sender::send` returns when there are zero live receivers. This task routes all three sends through one helper that increments a global counter and logs a `warn` when a send returns `Err`, giving operators visibility into dropped notifications without changing behavior (notifications stay best-effort).

- [ ] **Step 1: Write the failing test.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`, add to the `#[cfg(test)] mod tests` block a test that writes with NO subscriber and asserts the dropped-event counter advances. The counter is process-global and tests run in parallel, so the test asserts `>` (strictly greater than the pre-write reading) rather than an exact delta — its OWN no-subscriber write guarantees the increase regardless of concurrent writers:

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_without_subscriber_increments_dropped_event_counter() {
        let before = dropped_broadcast_count();

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 1).unwrap();
        // Deliberately do NOT subscribe: the broadcast send will return Err
        // (no receivers), which must be counted, not silently dropped.
        let ns = Namespace::Project("no-subscriber".to_string());
        let n = note(&ns, "nobody is listening");
        handle.write(n, Some(vec![0.1f32; DIM])).await.unwrap();
        handle.shutdown().await;

        assert!(
            dropped_broadcast_count() > before,
            "a broadcast with no receivers must increment the dropped counter \
             (before={before}, after={})",
            dropped_broadcast_count()
        );
    }
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rb-daemon --lib write_without_subscriber_increments_dropped_event_counter` Expected: FAIL to compile — `cannot find function 'dropped_broadcast_count' in this scope`. Confirms the test drives the new helper.

- [ ] **Step 3: Add the counter, the publish helper, and route all three sends through it.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`, add near the top of the file (after the existing `use` block and the `const` declarations) the counter + a test-visible accessor + the helper:

```rust
/// Count of `MemoryChanged` broadcasts that could not be delivered (no live
/// receivers). Best-effort notification only — a non-zero value is
/// observability, not an error.
static DROPPED_BROADCASTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Read the cumulative count of dropped change-event broadcasts. Exposed for
/// observability and tests; production callers use it for metrics/logging only.
pub fn dropped_broadcast_count() -> u64 {
    DROPPED_BROADCASTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Publish a change event, counting + logging when there is no receiver to take
/// it. `broadcast::Sender::send` returns `Err(SendError)` precisely when there
/// are zero receivers; that is the signal we surface.
fn publish_change(events: &broadcast::Sender<MemoryChanged>, evt: MemoryChanged) {
    if events.send(evt).is_err() {
        let n = DROPPED_BROADCASTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        tracing::warn!(
            dropped_total = n,
            "MemoryChanged broadcast had no receivers; change notification dropped"
        );
    }
}
```

  Then replace each of the three `let _ = events.send(MemoryChanged { ... });` calls in `writer_loop` (the `Insert`, `Update`, and `Archive` arms) with a `publish_change` call. For the `Insert` arm:

```rust
                if changed {
                    publish_change(
                        &events,
                        MemoryChanged {
                            id,
                            namespace,
                            kind: ChangeKind::Created,
                        },
                    );
                }
```

  For the `Update` arm (kind `ChangeKind::Updated`) and the `Archive` arm (kind `ChangeKind::Archived`), make the identical substitution — replace `let _ = events.send(MemoryChanged { id, namespace, kind: <Kind> });` with `publish_change(&events, MemoryChanged { id, namespace, kind: <Kind> });`. (`events` is the owned `broadcast::Sender<MemoryChanged>` local to `writer_loop`; passing `&events` borrows it without moving.)

  (verified against installed tokio 1.52: `broadcast::Sender::send(&self, value) -> Result<usize, SendError<T>>` returns `Err` only when there are zero receivers; lagging receivers do NOT make `send` fail — they surface as `RecvError::Lagged` on the receiver side. This task therefore counts the no-receiver case at the sender; lagged-receiver counting is a receiver-side concern handled wherever a subscriber loop is added, out of P1 scope.)

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rb-daemon --lib write_without_subscriber_increments_dropped_event_counter` Expected: PASS. Confirm the existing event tests still pass (they subscribe first, so their writes do not increment the counter): `cargo test -p rb-daemon` Expected: PASS.

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-daemon --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-daemon/src/store_handle.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "feat(rb-daemon): warn and count dropped MemoryChanged broadcasts"`

---

### Task 31: rusty-brain auto-start retry decides via `io::ErrorKind`, not error-string matching

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/error.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/error_map.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-proto/src/error.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-proto/src/client.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/client.rs`

`should_auto_start` currently lowercases the error message and substring-matches `"no such file"`, `"os error 2"`, etc. — brittle and locale/OS-dependent. This task makes the connect path classify the connect failure by `std::io::ErrorKind` (`NotFound` / `ConnectionRefused` → auto-start; anything else → do not retry/auto-start a permanent error). To classify without parsing strings, `Client::connect` failures carry the kind via a new `Error::IoKind { kind, message }` variant (the existing `Error::Io(String)` discards the kind). CRITICAL: adding this variant breaks BOTH non-wildcard `match`es on `Error` (`rb-daemon/src/error_map.rs` and `rb-proto/src/error.rs::error_kind`) and the existing rb-proto connect test that asserts `Error::Io(_)`; all are fixed here.

- [ ] **Step 1: Write the failing classifier test.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/client.rs`, add to the `#[cfg(test)] mod tests` block tests that pin `should_auto_start` to `ErrorKind`-based classification via a new pure helper `should_auto_start_kind`:

```rust
    #[test]
    fn auto_starts_only_for_notfound_and_connection_refused() {
        assert!(should_auto_start_kind(Some(std::io::ErrorKind::NotFound)));
        assert!(should_auto_start_kind(Some(
            std::io::ErrorKind::ConnectionRefused
        )));
    }

    #[test]
    fn does_not_auto_start_for_permanent_or_unknown_errors() {
        // Permission denied is permanent: spawning a child will not fix it.
        assert!(!should_auto_start_kind(Some(
            std::io::ErrorKind::PermissionDenied
        )));
        // A non-io error (no ErrorKind) is never auto-started.
        assert!(!should_auto_start_kind(None));
    }
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rusty-brain client::tests::auto_starts_only_for_notfound_and_connection_refused` Expected: FAIL to compile — `cannot find function 'should_auto_start_kind'`. Confirms the test drives the new helper.

- [ ] **Step 3: Add the `IoKind` variant, the `from_io` constructor, and the `io_kind()` accessor to rb-types.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-types/src/error.rs`, add the variant to the `Error` enum, immediately after the `InvalidArgument` variant added in Task 26:

```rust
    #[error("io error ({kind:?}): {message}")]
    IoKind {
        kind: std::io::ErrorKind,
        message: String,
    },
```

  Then add an `impl Error` block after the enum (a single coherent block — there is no `recovered_io_kind` field; `IoKind` itself carries the kind):

```rust
impl Error {
    /// Build an `IoKind` error preserving the originating `std::io::ErrorKind`.
    pub fn from_io(e: &std::io::Error) -> Self {
        Error::IoKind {
            kind: e.kind(),
            message: e.to_string(),
        }
    }

    /// Best-effort recovery of the originating `std::io::ErrorKind`. Returns
    /// `Some` only for `IoKind` errors; `None` for every other variant. Callers
    /// use this to decide retry/auto-start policy WITHOUT substring-matching the
    /// message.
    pub fn io_kind(&self) -> Option<std::io::ErrorKind> {
        match self {
            Error::IoKind { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}
```

  Add an rb-types test for the round-trip inside the existing `#[cfg(test)] mod tests` block in `error.rs`:

```rust
    #[test]
    fn io_kind_round_trips_through_from_io() {
        let raw = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
        let err = Error::from_io(&raw);
        assert_eq!(err.io_kind(), Some(std::io::ErrorKind::ConnectionRefused));
        assert_eq!(Error::Storage("x".into()).io_kind(), None);
    }
```

- [ ] **Step 4: Run rb-types — expect PASS — then fix BOTH exhaustive `Error` matches.** Run: `cargo test -p rb-types error` Expected: PASS (including `io_kind_round_trips_through_from_io`). Now the two non-wildcard matches on `Error` are non-exhaustive:
  - In `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/error_map.rs`, add an arm to the `match &err` in `error_to_response`, grouped with the internal arms (it carries a `std::io::Error`-derived message that may contain a path, so treat it as INTERNAL — log + generic sentinel — exactly like `Error::Io`). Add after the `Error::Io(_) => { ... }` arm:

```rust
        Error::IoKind { .. } => {
            warn!(error = %err, "internal io error");
            ("io", "internal error".to_string())
        }
```

    and add a case to `maps_each_error_to_stable_kind`:

```rust
            (
                Error::IoKind {
                    kind: std::io::ErrorKind::NotFound,
                    message: "x".into(),
                },
                "io",
            ),
```

  - In `/Users/bluby/repos/rusty-brain-p2/crates/rb-proto/src/error.rs`, add an arm to `error_kind` after `Error::Io(_) => "io",`:

```rust
        Error::IoKind { .. } => "io",
```

    (`response_error_to_error` already has a `_ => Error::Storage(...)` wildcard, so the inbound `"io"` kind maps back to `Error::Io` — acceptable, since `io_kind()` classification is only used client-side at connect time, before any wire round-trip.)

  Run: `cargo test -p rb-daemon error_map` Expected: PASS. Run: `cargo test -p rb-proto error` Expected: PASS.

- [ ] **Step 5: Make `Client::connect` failures carry the kind.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-proto/src/client.rs`, the connect mapping currently reads (line ~27):

```rust
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| Error::Io(format!("connect {}: {e}", socket_path.display())))?;
```

  Replace ONLY that `map_err` so the originating kind is preserved:

```rust
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| Error::from_io(&e))?;
```

  (Request-time io errors elsewhere may remain `Error::Io`; only the connect-time mapping needs the kind. Verified at execution: the connect call lives in `Client::connect`.) Now UPDATE the existing rb-proto test that pinned the old variant — `connect_to_missing_socket_is_io_error` asserts `matches!(err, rb_types::Error::Io(_))`, which is no longer true. Change its body assertion to:

```rust
        // Connect failure now carries the io::ErrorKind via Error::IoKind so the
        // client can classify auto-start policy without string matching.
        assert!(
            err.io_kind().is_some(),
            "missing socket should carry an io::ErrorKind, got {err:?}"
        );
```

- [ ] **Step 6: Implement `should_auto_start_kind` and rewire `should_auto_start`.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/client.rs`, replace the entire existing `should_auto_start` function (the string-lowercasing version) with the pure kind-classifier plus a thin adapter:

```rust
/// Classify whether a connect failure is one a freshly-spawned daemon would fix.
/// Only `NotFound` (socket file absent) and `ConnectionRefused` (no listener)
/// are transient-on-start; everything else (incl. `PermissionDenied`) is
/// permanent and must NOT trigger a spawn or further retries.
fn should_auto_start_kind(kind: Option<std::io::ErrorKind>) -> bool {
    matches!(
        kind,
        Some(std::io::ErrorKind::NotFound) | Some(std::io::ErrorKind::ConnectionRefused)
    )
}

fn should_auto_start(error: &Error) -> bool {
    should_auto_start_kind(error.io_kind())
}
```

- [ ] **Step 7: Update the existing string-based client retry tests to use kind-carrying errors.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/client.rs`, the tests `spawns_then_retries_until_connected` and `gives_up_after_max_attempts` construct `Error::Io("No such file or directory")` / `Error::Io("Connection refused")`, whose `io_kind()` is now `None` (so they would no longer auto-start). Replace those constructions:
  - `rb_types::Error::Io("No such file or directory".into())` → `rb_types::Error::from_io(&std::io::Error::from(std::io::ErrorKind::NotFound))`
  - `rb_types::Error::Io("Connection refused".into())` → `rb_types::Error::from_io(&std::io::Error::from(std::io::ErrorKind::ConnectionRefused))`
  - In `gives_up_after_max_attempts`, also change the final assertion `matches!(err, rb_types::Error::Io(_))` to `err.io_kind().is_some()` (the error is now `IoKind`).
  - Leave `does_not_spawn_for_non_startable_errors` as-is: its `Error::Storage("contract version mismatch")` has `io_kind() == None`, so it still must NOT auto-start.

- [ ] **Step 8: Run it — expect PASS.** Run: `cargo test -p rusty-brain client` Expected: PASS — the classifier tests and the updated retry tests pass; `does_not_spawn_for_non_startable_errors` still asserts zero spawns for the `Storage` error. Run: `cargo test -p rb-proto` Expected: PASS — the updated `connect_to_missing_socket_is_io_error` now asserts on `io_kind()`.

- [ ] **Step 9: Lint + format.** Run: `cargo clippy -p rb-types -p rb-proto -p rb-daemon -p rusty-brain --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 10: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-types/src/error.rs crates/rb-daemon/src/error_map.rs crates/rb-proto/src/error.rs crates/rb-proto/src/client.rs crates/rusty-brain/src/client.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "fix(rusty-brain): classify auto-start by io::ErrorKind instead of message strings"`

---

### Task 32: rusty-brain `env_clear()` before spawning the auto-start daemon child

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/client.rs`

The auto-start child currently inherits the full parent environment. Per the security rule (subprocess spawning must `env_clear()` then set only needed vars), this task clears the child env and forwards only the variables the daemon needs: the resolved socket/db paths, the embedding/enrichment API keys, and the minimal `HOME`/`PATH`/`XDG_*` needed to resolve default paths. This prevents leaking unrelated parent env (other secrets, shell state) into a long-lived detached daemon.

- [ ] **Step 1: Write the failing test.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/client.rs`, replace the existing `daemon_command_inherits_resolved_socket_and_db_paths` test (its name is now wrong — the child no longer inherits) with a test asserting a cleared-and-allowlisted env. The test sets ONLY its own marker vars (it does NOT assert on ambient `PATH`/`HOME`, which other parallel tests could mutate — env is process-global and there is no `serial_test` dependency, so depending on inherited values would be flaky):

```rust
    #[test]
    fn daemon_command_clears_env_and_forwards_only_allowlisted_vars() {
        // An allowlisted var the parent has set must be forwarded.
        std::env::set_var("VOYAGE_API_KEY", "voyage-key");
        // A parent var NOT on the allowlist must not reach the child.
        std::env::set_var("RB_TEST_SHOULD_NOT_LEAK", "secret");

        let socket = Path::new("/tmp/rb.sock");
        let db = Path::new("/tmp/rb.db");
        let cmd = daemon_command(Path::new("/bin/echo"), socket, db);

        let envs: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_os_string(), value.to_os_string())))
            .collect();

        // Forwarded: resolved paths (always set explicitly).
        assert_eq!(
            envs.get(std::ffi::OsStr::new(crate::paths::SOCKET_ENV)),
            Some(&socket.as_os_str().to_os_string())
        );
        assert_eq!(
            envs.get(std::ffi::OsStr::new(crate::paths::DB_ENV)),
            Some(&db.as_os_str().to_os_string())
        );
        // Forwarded: allowlisted API key present in the parent.
        assert_eq!(
            envs.get(std::ffi::OsStr::new("VOYAGE_API_KEY")),
            Some(&std::ffi::OsString::from("voyage-key"))
        );
        // NOT forwarded: an arbitrary parent var.
        assert!(
            !envs.contains_key(std::ffi::OsStr::new("RB_TEST_SHOULD_NOT_LEAK")),
            "non-allowlisted parent vars must be cleared"
        );

        std::env::remove_var("RB_TEST_SHOULD_NOT_LEAK");
        std::env::remove_var("VOYAGE_API_KEY");
    }
```

  NOTE: after `Command::env_clear()`, `get_envs()` returns ONLY the vars added via `.env`/`.envs` (the inherited environment is excluded), so asserting on `get_envs` proves both the clear and the allowlist. (verified against std at execution: `env_clear()` then `.env(...)` yields exactly the explicitly-set set from `get_envs()`.)

- [ ] **Step 2: Run it — expect FAIL.** Run: `cargo test -p rusty-brain client::tests::daemon_command_clears_env_and_forwards_only_allowlisted_vars` Expected: FAIL — before the change `daemon_command` only sets SOCKET/DB and never sets `VOYAGE_API_KEY`, so `get_envs()` does not contain it and the `VOYAGE_API_KEY` assertion fails. This confirms the allowlist must be set explicitly after a clear.

- [ ] **Step 3: Clear and allowlist the child env.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/client.rs`, change `daemon_command` to clear then forward only the allowlist:

```rust
fn daemon_command(self_exe: &Path, socket_path: &Path, db_path: &Path) -> Command {
    let mut cmd = Command::new(self_exe);
    cmd.arg("serve");
    // Security: never inherit the parent environment into a long-lived detached
    // daemon. Clear it, then forward ONLY what the daemon needs.
    cmd.env_clear();
    cmd.env(crate::paths::SOCKET_ENV, socket_path);
    cmd.env(crate::paths::DB_ENV, db_path);
    // Forward each allowlisted var that is actually set in the parent.
    const FORWARD: &[&str] = &[
        "VOYAGE_API_KEY",
        "ANTHROPIC_API_KEY",
        "HOME",
        "PATH",
        "XDG_RUNTIME_DIR",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
    ];
    for key in FORWARD {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd
}
```

- [ ] **Step 4: Run it — expect PASS.** Run: `cargo test -p rusty-brain client::tests::daemon_command_clears_env_and_forwards_only_allowlisted_vars` Expected: PASS. Run the whole client module to confirm no regression: `cargo test -p rusty-brain client` Expected: PASS.

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --all-features -- -D warnings` Expected: no warnings. Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 6: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rusty-brain/src/client.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "fix(rusty-brain): env_clear before auto-start spawn and forward only allowlisted vars"`

---

### Task 33: rusty-brain detect_namespace runs OFF the async runtime

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/main.rs`
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/run.rs`

`detect_namespace()` invokes `git rev-parse` (a blocking subprocess); it currently runs inside `run_client`, i.e. ON the tokio runtime thread, blocking a worker. This task computes the namespace in `main.rs` BEFORE `runtime.block_on(...)` and threads the resolved `Namespace` into `run`, so no blocking git call ever happens on the runtime. (Part L replaces the detection internals; this task only changes WHERE it is called.) Verified at execution: `detect_namespace()` is `pub fn detect_namespace() -> Namespace` (no args) and `namespace_detect` is a `pub mod`.

- [ ] **Step 1: Write the failing test that `run` accepts a pre-resolved namespace.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/run.rs`, add to the `#[cfg(test)] mod tests` block a compile-time arity guard (it does no runtime work; it only fails to compile until `run` takes `(Cli, Namespace)`):

```rust
    #[test]
    fn run_signature_accepts_preresolved_namespace() {
        // Compile-time guard: `run` must accept (Cli, Namespace). This fails to
        // compile until the namespace is threaded in from main (off-runtime).
        fn _assert_run_takes_namespace(
        ) -> fn(crate::cli::Cli, rb_types::Namespace) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<()>>>,
        > {
            |cli, ns| Box::pin(run(cli, ns))
        }
        let _ = _assert_run_takes_namespace;
    }
```

- [ ] **Step 2: Run it — expect a compile failure.** Run: `cargo test -p rusty-brain run::tests::run_signature_accepts_preresolved_namespace` Expected: FAIL to compile — `run` currently takes only `Cli`, so `run(cli, ns)` is a wrong-arity call. Confirms the test drives the signature change.

- [ ] **Step 3: Thread the namespace through `run`.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/run.rs`, change `run` to accept the pre-resolved namespace and pass it to `run_client` instead of detecting inside the runtime. Replace the `run` signature and body head:

```rust
pub async fn run(cli: Cli, namespace: rb_types::Namespace) -> anyhow::Result<()> {
    let socket_path = paths::socket_path_from_env().context("resolving daemon socket path")?;
    let db_path = paths::db_path_from_env().context("resolving daemon database path")?;

    match cli.command {
        Command::Serve => {
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            serve::run_serve(socket_path, db_path, 4, shutdown)
                .await
                .context("daemon failed")?;
            Ok(())
        }
        other => run_client(other, cli.json, &socket_path, &db_path, namespace).await,
    }
}
```

  Change `run_client` to take the namespace and drop the in-runtime detection. Replace its signature and the `let namespace = detect_namespace();` line:

```rust
async fn run_client(
    command: Command,
    json: bool,
    socket_path: &std::path::Path,
    db_path: &std::path::Path,
    namespace: rb_types::Namespace,
) -> anyhow::Result<()> {
    let self_exe = std::env::current_exe().context("locating own executable")?;
    let mut client = client::connect_or_start(socket_path, db_path, namespace.clone(), self_exe)
        .await
        .context("connecting to daemon")?;
```

  Remove the now-unused `use crate::namespace_detect::detect_namespace;` import at the top of `run.rs` (detection moves to `main.rs`).

- [ ] **Step 4: Call detection before `block_on` in `main.rs`.** In `/Users/bluby/repos/rusty-brain-p2/crates/rusty-brain/src/main.rs`, resolve the namespace on the main thread (off the runtime) and pass it into `run`:

```rust
fn main() -> ExitCode {
    init_logging();
    let cli = Cli::parse();

    // Resolve the namespace BEFORE entering the async runtime: detection runs a
    // blocking `git` subprocess and must never run on a tokio worker thread.
    let namespace = rusty_brain::namespace_detect::detect_namespace();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(run(cli, namespace)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
```

  (verified at execution: `detect_namespace` takes no args and returns `Namespace`. If Part L later gives it a parameter, pass the resolved start dir here — detection still happens before `block_on`. The only allowed hedge.)

- [ ] **Step 5: Run it — expect PASS.** Run: `cargo test -p rusty-brain run::tests::run_signature_accepts_preresolved_namespace` Expected: PASS. Build the binary to confirm `main.rs` wiring: `cargo build -p rusty-brain` Expected: `Finished` (exit 0).

- [ ] **Step 6: Run the binary's e2e + CLI tests.** Run: `cargo test -p rusty-brain` Expected: PASS — the assert_cmd CLI tests and the end-to-end remember→recall test still pass (the namespace is now resolved off-runtime but identically). (There are no test callers of `run` in the repo, so no in-test call-site updates are needed; if a future test calls `run(cli)` with one arg, update it to `run(cli, rb_types::Namespace::Project("test".into()))`.)

- [ ] **Step 7: Lint + format.** Run: `cargo clippy -p rusty-brain --all-targets --all-features -- -D warnings` Expected: no warnings (the `detect_namespace` import is removed from `run.rs`; if clippy flags it as unused, that import was missed in Step 3). Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 8: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rusty-brain/src/main.rs crates/rusty-brain/src/run.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "fix(rusty-brain): resolve namespace off the async runtime before block_on"`

---

### Task 34: rb-daemon writer `catch_unwind` — partial-write isolation test + documented guarantee

**Files:**
- Modify: `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`

The writer thread already contains the `catch_unwind` recovery (`catch_store_op`/`run_store_op` with `AssertUnwindSafe` + reopen) and a passing `writer_reopens_after_caught_store_panic` test, plus a test-only `PanicForTest` command. Per the assignment ("add a test if a store panic can be induced cleanly... if not, document"), this task ADDS a stronger guarantee test — a panic mid-operation must not leave the next real write corrupted and must not lose subsequent writes — and documents the invariant in code, rather than re-implementing the existing mechanism.

- [ ] **Step 1: Write the failing/strengthening test.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`, add to the `#[cfg(test)] mod tests` block a test that interleaves a panic with real writes and asserts isolation + continuity (`oneshot`, `WriteCommand`, and `Error` are all in scope via `use super::*;`):

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn caught_writer_panic_isolates_and_does_not_lose_later_writes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        let handle = StoreHandle::start(db, DIM, 2).unwrap();
        let ns = Namespace::Project("panic-isolation".to_string());

        // 1. A successful write before the panic.
        let before = note(&ns, "written before the panic");
        let before_id = before.id.clone();
        handle.write(before, Some(vec![0.1f32; DIM])).await.unwrap();

        // 2. Induce a caught writer panic via the test-only command.
        let (reply, rx) = oneshot::channel();
        handle
            .writer_tx
            .send(WriteCommand::PanicForTest { reply })
            .await
            .unwrap();
        let err = rx.await.unwrap().unwrap_err();
        assert!(
            matches!(err, Error::Storage(_)),
            "caught panic must surface as a storage error, got {err:?}"
        );

        // 3. A successful write AFTER the panic (writer reopened its connection).
        let after = note(&ns, "written after the panic");
        let after_id = after.id.clone();
        handle.write(after, Some(vec![0.2f32; DIM])).await.unwrap();

        // 4. Both real writes are present; the panic left no partial/corrupt row.
        assert!(
            handle.get(ns.clone(), before_id).await.unwrap().is_some(),
            "pre-panic write must survive"
        );
        assert!(
            handle.get(ns.clone(), after_id).await.unwrap().is_some(),
            "post-panic write must commit on the reopened connection"
        );
        let listed = handle.list(ns, None, 50).await.unwrap();
        assert_eq!(
            listed.len(),
            2,
            "exactly the two real writes exist; the panic added nothing"
        );

        handle.shutdown().await;
    }
```

- [ ] **Step 2: Run it — expect PASS (mechanism already present).** Run: `cargo test -p rb-daemon --lib caught_writer_panic_isolates_and_does_not_lose_later_writes` Expected: PASS — the existing `catch_store_op`/`run_store_op` reopen logic already isolates the panic and keeps the writer usable, so this stronger assertion passes without code changes. If it FAILS (e.g. the post-panic write errors, or `list` shows a stray row), the reopen path has a bug: confirm the `PanicForTest` arm calls `run_store_op` (so the panic is caught and the store reopened) and that `panic_for_test_store_op` panics BEFORE any partial SQL write.

- [ ] **Step 3: Document the guarantee in code.** In `/Users/bluby/repos/rusty-brain-p2/crates/rb-daemon/src/store_handle.rs`, add an explicit guarantee note as a doc comment immediately above `fn run_store_op` (keeping the existing `catch_store_op` comment coherent — do not delete it):

```rust
/// Run one store operation on the writer thread, containing any panic so a
/// single bad command cannot take down the daemon.
///
/// GUARANTEE (tested by `caught_writer_panic_isolates_and_does_not_lose_later_writes`):
/// a caught panic (a) is reported to the caller as `Error::Storage`, (b) drops
/// and reopens the write connection so no partial transaction leaks into later
/// writes, and (c) keeps the writer loop alive so subsequent commands commit
/// normally. Only a failed REOPEN (not the panic itself) stops the writer.
```

- [ ] **Step 4: Re-run the writer-recovery tests together.** Run: `cargo test -p rb-daemon --lib writer_reopens_after_caught_store_panic caught_writer_panic_isolates_and_does_not_lose_later_writes` Expected: PASS (both). Run the full crate once for safety: `cargo test -p rb-daemon` Expected: PASS.

- [ ] **Step 5: Lint + format.** Run: `cargo clippy -p rb-daemon --all-targets --all-features -- -D warnings` Expected: no warnings (the test module already has `#![allow(clippy::unwrap_used, clippy::expect_used)]`; the panic-inducing helper `panic_for_test_store_op` already carries `#[allow(clippy::panic)]`). Run: `cargo fmt --all --check` Expected: no diff.

- [ ] **Step 6: Workspace gate.** Run: `cargo test --workspace` Expected: PASS (proves Tasks 46-54 integrate; in particular the new `Error` variants compile across rb-proto/rb-daemon and the updated rb-proto connect test passes). Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` Expected: `Finished` with no warnings (exit 0). Run: `cargo fmt --all --check` Expected: no output, exit 0.

- [ ] **Step 7: Commit.** Run: `git -C /Users/bluby/repos/rusty-brain-p2 add crates/rb-daemon/src/store_handle.rs && git -C /Users/bluby/repos/rusty-brain-p2 commit -m "test(rb-daemon): prove caught writer panic isolates and preserves later writes"`


---

## After P2 — v1 ships; P3/P4 remain explicitly deferred

With Parts L–P merged, **v1 is complete**: a lean shared-memory daemon that agents use over MCP, with working hybrid (FTS + vector + graph) retrieval, real project namespaces, and the hardening backlog cleared.

### P3 — Deferred (behind existing seams; NOT v1)
- `subscribe` change-stream over the daemon's `tokio::broadcast` (cross-agent awareness).
- Memory evolution: consolidation / link decay / importance recalibration as opt-in daemon jobs.
- `local` ONNX embedding feature in `rb-embed`.

### P4 — Broader agent surface (deferred; NOT v1)
- `rb-hooks` / `rb-install`: capture hooks + an `install` command configuring Claude Code / OpenCode / Copilot CLI / Codex CLI / Gemini CLI — fail-open, `ContractVersion`-gated. Separate crates, never compiled into core.

### Notes
- LLM enrichment/linking (Part O) is **opt-in** (no `ANTHROPIC_API_KEY` → heuristic path); it is included in v1 as a capability but the default build/run requires no LLM.
- `batch link-loading` (the P0 N+1) is resolved in Part M via `get_many`.


---

## Plan provenance

Authored by a 5-cluster fan-out against a fixed spine built on the merged P0+P1 APIs; each cluster adversarially reviewed for placeholders, spine drift, TDD form, and no-live-network. Reviewer fixes per cluster: namespace (4); graph-linking (3); mcp (6); llm-enrich (6); hardening (11). Tasks renumbered globally.
