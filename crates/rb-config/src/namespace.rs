//! Canonical namespace identity (W0.3): the single implementation shared by
//! the CLI (`rusty-brain`) and the hook spine (`rb-agents`), so two binaries
//! can never disagree on which namespace a directory maps to.
//!
//! Resolution order (first hit wins):
//!   1. explicit override — `--namespace` flag or [`crate::NAMESPACE_ENV`];
//!   2. repo-committed [`REPO_CONFIG_FILE`] (`namespace = "..."`) at the git
//!      toplevel — identity survives cloning under any directory name;
//!   3. `CLAUDE.md` frontmatter `project:` — nearest file walking from the
//!      start dir up to and INCLUDING the git toplevel, never past it (F54),
//!      honored only when it equals the toplevel name or is pinned (F22).
//!      There is no first-H1 fallback, and outside a git repo this branch is
//!      skipped entirely (frontmatter is a repo-scoped mechanism);
//!   4. git-toplevel directory name;
//!   5. start (cwd) directory name; else `Global`.
//!
//! A repo-committed `CLAUDE.md` must not silently claim another project's
//! namespace: an unpinned differing `project:` is surfaced via
//! [`NamespaceResolution::unpinned_override`] while the toplevel name is used.
//! Interactive callers warn and may pin via [`accept_override`]
//! (known-hosts style, recorded in [`default_pins_path`]); non-interactive
//! consumers (hooks) log and never honor it.
//!
//! Never panics, never fails: every branch degrades to the next and ultimately
//! to `Namespace::Global`. Resolution shells out to git and reads files — call
//! it OFF any async runtime.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use rb_types::{Error, Namespace, Result};
use serde::{Deserialize, Serialize};

/// Repo-committed identity file, read from the git toplevel only.
pub const REPO_CONFIG_FILE: &str = ".rusty-brain.toml";

/// Outcome of resolution: the namespace to use, plus any frontmatter override
/// that was found but NOT honored (unpinned and differing from the toplevel
/// name) so callers can warn in their own channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceResolution {
    pub namespace: Namespace,
    pub unpinned_override: Option<UnpinnedOverride>,
}

/// A `CLAUDE.md` frontmatter `project:` that differs from the git-toplevel
/// name and is not pinned. `namespace` in the enclosing resolution is already
/// the safe fallback (`used`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpinnedOverride {
    /// The namespace the frontmatter claims.
    pub claimed: String,
    /// The git-toplevel directory name used instead.
    pub used: String,
    /// The git toplevel — the directory a pin would be recorded under.
    pub toplevel: PathBuf,
}

fn resolved(namespace: Namespace) -> NamespaceResolution {
    NamespaceResolution {
        namespace,
        unpinned_override: None,
    }
}

/// Resolve the namespace for the real process: `flag` > env > detection from
/// the real cwd with the on-disk pin store and real git. Synchronous: call
/// this OFF any async runtime.
pub fn resolve_namespace(flag: Option<&str>) -> NamespaceResolution {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let explicit = flag
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(explicit_namespace_from_env);
    resolve_namespace_in(&cwd, explicit, &NamespacePins::load(), git_toplevel)
}

/// The explicit namespace from [`crate::NAMESPACE_ENV`], if set non-empty.
pub fn explicit_namespace_from_env() -> Option<String> {
    std::env::var(crate::NAMESPACE_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Pure-ish core, parameterized over the git-root resolver (callers supply
/// their own process bounds) and an in-memory pin set, so every branch is
/// testable against temp trees without the real state file.
pub fn resolve_namespace_in<G>(
    start: &Path,
    explicit: Option<String>,
    pins: &NamespacePins,
    git_root: G,
) -> NamespaceResolution
where
    G: Fn(&Path) -> Option<PathBuf>,
{
    // (1) explicit always wins: no detection, nothing to warn about.
    if let Some(name) = explicit
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return resolved(Namespace::Project(name));
    }
    if let Some(top) = git_root(start).as_deref() {
        // (2) repo-committed identity: same namespace under any clone name.
        if let Some(name) = repo_committed_namespace(top) {
            return resolved(Namespace::Project(name));
        }
        // (3) CLAUDE.md frontmatter, bounded at the toplevel, pin-gated.
        let top_name = dir_name(top);
        match (frontmatter_project_within(start, top), top_name) {
            (Some(claimed), Some(used)) if claimed == used => {
                // Not an override: frontmatter agrees with the toplevel name.
                return resolved(Namespace::Project(claimed));
            }
            (Some(claimed), Some(used)) => {
                if pins.pinned(top) == Some(claimed.as_str()) {
                    return resolved(Namespace::Project(claimed));
                }
                // Unpinned differing override: fail safe to the toplevel name
                // (F22) and report it so the caller can warn/pin.
                return NamespaceResolution {
                    namespace: Namespace::Project(used.clone()),
                    unpinned_override: Some(UnpinnedOverride {
                        claimed,
                        used,
                        toplevel: top.to_path_buf(),
                    }),
                };
            }
            // (4) toplevel directory name.
            (None, Some(used)) => return resolved(Namespace::Project(used)),
            // Toplevel has no usable utf8 name (e.g. `/`): fall through.
            (_, None) => {}
        }
    }
    // (5) start (cwd) directory name; else Global.
    match dir_name(start) {
        Some(name) => resolved(Namespace::Project(name)),
        None => resolved(Namespace::Global),
    }
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

#[derive(Deserialize)]
struct RepoConfig {
    namespace: Option<String>,
}

/// Read `namespace = "..."` from [`REPO_CONFIG_FILE`] at the git toplevel.
/// Missing file, parse failure, and empty values all degrade to `None`.
fn repo_committed_namespace(toplevel: &Path) -> Option<String> {
    let text = std::fs::read_to_string(toplevel.join(REPO_CONFIG_FILE)).ok()?;
    let cfg: RepoConfig = toml::from_str(&text).ok()?;
    cfg.namespace
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Nearest `CLAUDE.md` frontmatter `project:` walking `start` up to and
/// INCLUDING `toplevel`, never past it — an ancestor `CLAUDE.md` outside the
/// repo must not name it (F54). The nearest `CLAUDE.md` is the governing file:
/// if it has no usable `project:`, the search ends (no fallback to a higher
/// file, matching the original nearest-wins behavior).
fn frontmatter_project_within(start: &Path, toplevel: &Path) -> Option<String> {
    if !start.starts_with(toplevel) {
        // Defensive (symlinked cwd vs. git's physical toplevel): the bound
        // cannot be applied to the walk, so consider only the toplevel itself.
        let text = std::fs::read_to_string(toplevel.join("CLAUDE.md")).ok()?;
        return project_from_frontmatter(&text);
    }
    for dir in start.ancestors() {
        if let Ok(text) = std::fs::read_to_string(dir.join("CLAUDE.md")) {
            return project_from_frontmatter(&text);
        }
        if dir == toplevel {
            break; // bound: never walk past the git toplevel
        }
    }
    None
}

/// Read `project: NAME` from a leading `---`-delimited frontmatter block.
/// Lenient hand parser — never panics; `None` when absent or empty. There is
/// deliberately NO first-H1 fallback (removed in W0.3: prose headings are not
/// identity).
pub fn project_from_frontmatter(text: &str) -> Option<String> {
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
            // Empty value: no override.
            return None;
        }
    }
    None
}

/// Find the git toplevel for `dir` by invoking git; `None` if not a repo.
/// Callers needing a process bound (hooks) supply their own resolver to
/// [`resolve_namespace_in`].
pub fn git_toplevel(dir: &Path) -> Option<PathBuf> {
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

/// Local pin store: `dir -> namespace` records for accepted `CLAUDE.md`
/// frontmatter overrides (known-hosts style). Tiny by design: whole-file
/// read/write of one TOML table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespacePins {
    #[serde(default)]
    pins: BTreeMap<String, String>,
}

impl NamespacePins {
    /// Load from [`default_pins_path`]; any failure degrades to empty, which
    /// treats every override as unpinned — the safe direction.
    pub fn load() -> Self {
        default_pins_path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    /// Load from `path`; missing or unparsable degrades to empty.
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// The pinned namespace for `dir`, if any.
    pub fn pinned(&self, dir: &Path) -> Option<&str> {
        self.pins.get(&pin_key(dir)).map(String::as_str)
    }

    /// Record (or replace) the pin for `dir`.
    pub fn insert(&mut self, dir: &Path, namespace: &str) {
        self.pins.insert(pin_key(dir), namespace.to_string());
    }

    /// Persist to `path`, creating parent directories.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Io(format!("create {}: {e}", parent.display())))?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| Error::Serialization(e.to_string()))?;
        std::fs::write(path, text).map_err(|e| Error::Io(format!("write {}: {e}", path.display())))
    }

    /// Persist to [`default_pins_path`].
    pub fn save(&self) -> Result<()> {
        self.save_to(&default_pins_path()?)
    }
}

/// Pin keys are the lossy-utf8 toplevel path; a non-utf8 path simply never
/// matches its pin and stays in the safe unpinned state.
fn pin_key(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

/// Where pins live: `<state dir>/rusty-brain/namespace-pins.toml`
/// (`~/.local/state` on Linux; `XDG_STATE_HOME` always wins).
pub fn default_pins_path() -> Result<PathBuf> {
    Ok(crate::state_base_dir()?
        .join("rusty-brain")
        .join("namespace-pins.toml"))
}

/// Pin `o` (records `toplevel -> claimed` in the on-disk store) and return the
/// now-honored namespace. Explicit user consent only — call on
/// `--accept-namespace-override`.
pub fn accept_override(o: &UnpinnedOverride) -> Result<Namespace> {
    let mut pins = NamespacePins::load();
    pins.insert(&o.toplevel, &o.claimed);
    pins.save()?;
    Ok(Namespace::Project(o.claimed.clone()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn no_pins() -> NamespacePins {
        NamespacePins::default()
    }

    fn no_git(_: &Path) -> Option<PathBuf> {
        None
    }

    // --- project_from_frontmatter (pure text parsing) ---

    #[test]
    fn frontmatter_project_is_parsed_and_trimmed() {
        let text = "---\nproject:   \"my proj\"  \nother: x\n---\n# Heading\n";
        assert_eq!(project_from_frontmatter(text), Some("my proj".to_string()));
    }

    #[test]
    fn first_h1_is_never_a_namespace() {
        // W0.3: the H1 fallback is removed — a heading-only CLAUDE.md yields
        // no override at all.
        assert_eq!(project_from_frontmatter("# Just A Heading\nbody\n"), None);
    }

    #[test]
    fn empty_project_value_and_malformed_frontmatter_are_none() {
        assert_eq!(project_from_frontmatter("---\nproject:   \n---\n"), None);
        assert_eq!(project_from_frontmatter("---\nproject\nnonsense\n"), None);
        assert_eq!(project_from_frontmatter(""), None);
    }

    // --- resolution order ---

    #[test]
    fn explicit_beats_everything() {
        // (e): even with a repo config AND a pinned frontmatter override
        // available, an explicit namespace short-circuits detection.
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("repo");
        fs::create_dir_all(&top).unwrap();
        fs::write(
            top.join(REPO_CONFIG_FILE),
            "namespace = \"from-repo-config\"\n",
        )
        .unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, Some("explicit".to_string()), &no_pins(), git);
        assert_eq!(res.namespace, Namespace::Project("explicit".to_string()));
        assert_eq!(res.unpinned_override, None);
    }

    #[test]
    fn env_explicit_beats_detection_via_real_entrypoint() {
        // (e): the env half of "explicit always wins", through the real
        // `resolve_namespace` wrapper. Guard the process-global env.
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _lock = ENV_LOCK.lock().unwrap();
        std::env::set_var(crate::NAMESPACE_ENV, "env-wins");
        let res = resolve_namespace(None);
        assert_eq!(res.namespace, Namespace::Project("env-wins".to_string()));
        // The flag outranks the env var.
        let res = resolve_namespace(Some("flag-wins"));
        assert_eq!(res.namespace, Namespace::Project("flag-wins".to_string()));
        std::env::remove_var(crate::NAMESPACE_ENV);
    }

    #[test]
    fn repo_config_beats_directory_name_and_frontmatter() {
        // (b): `.rusty-brain.toml` outranks both the toplevel dir name and a
        // CLAUDE.md frontmatter project (even one that would need a pin).
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("some-clone-name");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join(REPO_CONFIG_FILE), "namespace = \"canonical\"\n").unwrap();
        fs::write(top.join("CLAUDE.md"), "---\nproject: pretender\n---\n").unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, None, &no_pins(), git);
        assert_eq!(res.namespace, Namespace::Project("canonical".to_string()));
        assert_eq!(res.unpinned_override, None);
    }

    #[test]
    fn same_repo_under_two_clone_names_resolves_identically() {
        // (d): identity travels with the committed file, not the directory.
        let tmp = TempDir::new().unwrap();
        let mut seen = Vec::new();
        for clone in ["clone-a", "work-checkout-b"] {
            let top = tmp.path().join(clone);
            fs::create_dir_all(&top).unwrap();
            fs::write(top.join(REPO_CONFIG_FILE), "namespace = \"one-project\"\n").unwrap();
            let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
            seen.push(resolve_namespace_in(&top, None, &no_pins(), git).namespace);
        }
        assert_eq!(seen[0], seen[1]);
        assert_eq!(seen[0], Namespace::Project("one-project".to_string()));
    }

    #[test]
    fn malformed_repo_config_degrades_to_next_branch() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("repo");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join(REPO_CONFIG_FILE), "namespace = [not toml\n").unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, None, &no_pins(), git);
        assert_eq!(res.namespace, Namespace::Project("repo".to_string()));
    }

    #[test]
    fn walk_stops_at_git_toplevel_outer_claude_md_is_ignored() {
        // (a) F54 regression: outer CLAUDE.md above the repo (e.g. ~/CLAUDE.md)
        // must never name a repo that lacks its own. Both frontmatter and H1
        // variants stay outside the bound.
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path();
        fs::write(
            outer.join("CLAUDE.md"),
            "---\nproject: outer-claim\n---\n# Outer Heading\n",
        )
        .unwrap();
        let top = outer.join("inner-repo");
        let nested = top.join("src").join("deep");
        fs::create_dir_all(&nested).unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&nested, None, &no_pins(), git);
        assert_eq!(res.namespace, Namespace::Project("inner-repo".to_string()));
        assert_eq!(res.unpinned_override, None);
    }

    #[test]
    fn unpinned_frontmatter_override_is_not_honored() {
        // (c) F22: a repo-committed CLAUDE.md claiming another project's
        // namespace falls back to the toplevel name and is reported.
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("malicious-repo");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join("CLAUDE.md"), "---\nproject: victim\n---\n").unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, None, &no_pins(), git);
        assert_eq!(
            res.namespace,
            Namespace::Project("malicious-repo".to_string())
        );
        let o = res.unpinned_override.expect("override must be surfaced");
        assert_eq!(o.claimed, "victim");
        assert_eq!(o.used, "malicious-repo");
        assert_eq!(o.toplevel, top);
    }

    #[test]
    fn pinned_frontmatter_override_is_honored() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("dir-name");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join("CLAUDE.md"), "---\nproject: real-name\n---\n").unwrap();
        let mut pins = NamespacePins::default();
        pins.insert(&top, "real-name");
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, None, &pins, git);
        assert_eq!(res.namespace, Namespace::Project("real-name".to_string()));
        assert_eq!(res.unpinned_override, None);
    }

    #[test]
    fn pin_for_a_different_value_does_not_cover_a_new_claim() {
        // A stale pin must not bless a CHANGED frontmatter value.
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("dir-name");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join("CLAUDE.md"), "---\nproject: new-claim\n---\n").unwrap();
        let mut pins = NamespacePins::default();
        pins.insert(&top, "old-accepted");
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, None, &pins, git);
        assert_eq!(res.namespace, Namespace::Project("dir-name".to_string()));
        assert_eq!(
            res.unpinned_override.map(|o| o.claimed),
            Some("new-claim".to_string())
        );
    }

    #[test]
    fn frontmatter_matching_toplevel_name_needs_no_pin() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("same-name");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join("CLAUDE.md"), "---\nproject: same-name\n---\n").unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, None, &no_pins(), git);
        assert_eq!(res.namespace, Namespace::Project("same-name".to_string()));
        assert_eq!(res.unpinned_override, None);
    }

    #[test]
    fn outside_a_repo_claude_md_is_ignored_and_cwd_name_wins() {
        // Frontmatter is a repo-scoped mechanism: without a git toplevel there
        // is no bound, so the branch is skipped entirely.
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("plain-dir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("CLAUDE.md"), "---\nproject: ignored\n---\n").unwrap();
        let res = resolve_namespace_in(&dir, None, &no_pins(), no_git);
        assert_eq!(res.namespace, Namespace::Project("plain-dir".to_string()));
        assert_eq!(res.unpinned_override, None);
    }

    #[test]
    fn root_dir_without_repo_degrades_to_global() {
        let res = resolve_namespace_in(Path::new("/"), None, &no_pins(), no_git);
        assert_eq!(res.namespace, Namespace::Global);
    }

    // --- pin store ---

    #[test]
    fn pins_round_trip_through_the_toml_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state").join("namespace-pins.toml");
        let mut pins = NamespacePins::default();
        pins.insert(Path::new("/home/u/code/dir-name"), "real-name");
        pins.insert(Path::new("/home/u/other"), "second");
        pins.save_to(&path).unwrap();
        let loaded = NamespacePins::load_from(&path);
        assert_eq!(loaded, pins);
        assert_eq!(
            loaded.pinned(Path::new("/home/u/code/dir-name")),
            Some("real-name")
        );
        assert_eq!(loaded.pinned(Path::new("/home/u/unknown")), None);
    }

    #[test]
    fn missing_or_garbage_pin_file_loads_empty() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("nope.toml");
        assert_eq!(NamespacePins::load_from(&missing), NamespacePins::default());
        let garbage = tmp.path().join("bad.toml");
        fs::write(&garbage, "{{{{ not toml").unwrap();
        assert_eq!(NamespacePins::load_from(&garbage), NamespacePins::default());
    }
}
