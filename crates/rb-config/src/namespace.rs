//! Canonical namespace identity (W0.3): the single implementation shared by
//! the CLI (`rusty-brain`) and the hook spine (`rb-agents`), so two binaries
//! can never disagree on which namespace a directory maps to.
//!
//! Resolution order (first hit wins):
//!   1. explicit override — `--namespace` flag or [`crate::NAMESPACE_ENV`];
//!   2. repo-committed [`REPO_CONFIG_FILE`] (`namespace = "..."`), read from
//!      the blob committed at `HEAD` (`git show HEAD:.rusty-brain.toml`), so
//!      identity survives cloning under any directory name. ONLY the committed
//!      content counts: an untracked or locally-modified worktree file can
//!      never redirect the namespace — divergence is surfaced via
//!      [`NamespaceResolution::repo_config_divergence`] so callers can warn;
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

/// Outcome of resolution: the namespace to use, plus anything found but NOT
/// honored — a frontmatter override (unpinned and differing from the toplevel
/// name) or a diverging worktree [`REPO_CONFIG_FILE`] — so callers can warn
/// in their own channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceResolution {
    pub namespace: Namespace,
    pub unpinned_override: Option<UnpinnedOverride>,
    pub repo_config_divergence: Option<RepoConfigDivergence>,
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

/// A worktree [`REPO_CONFIG_FILE`] that diverges from the blob committed at
/// `HEAD`. Only the committed content is ever honored; this is surfaced so
/// callers can warn in their own channel (CLI stderr / hook tracing), like
/// [`NamespaceResolution::unpinned_override`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoConfigDivergence {
    /// The git toplevel whose worktree file diverges.
    pub toplevel: PathBuf,
    /// How the worktree file diverges from `HEAD`.
    pub kind: RepoConfigDivergenceKind,
}

/// How a worktree [`REPO_CONFIG_FILE`] diverges from `HEAD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoConfigDivergenceKind {
    /// No committed counterpart at `HEAD` (untracked file, or `HEAD` is
    /// unborn): the worktree file is ignored entirely.
    Untracked,
    /// Tracked, but the worktree content differs: the committed content wins.
    Modified,
}

fn resolved(namespace: Namespace) -> NamespaceResolution {
    resolved_with(namespace, None)
}

fn resolved_with(
    namespace: Namespace,
    repo_config_divergence: Option<RepoConfigDivergence>,
) -> NamespaceResolution {
    NamespaceResolution {
        namespace,
        unpinned_override: None,
        repo_config_divergence,
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
    resolve_namespace_in(
        &cwd,
        explicit,
        &NamespacePins::load(),
        git_toplevel,
        head_repo_config,
    )
}

/// The explicit namespace from [`crate::NAMESPACE_ENV`], if set non-empty.
pub fn explicit_namespace_from_env() -> Option<String> {
    std::env::var(crate::NAMESPACE_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Pure-ish core, parameterized over the git-root resolver and the committed
/// [`REPO_CONFIG_FILE`]-at-`HEAD` reader (callers supply their own process
/// bounds) and an in-memory pin set, so every branch is testable against temp
/// trees without the real state file.
pub fn resolve_namespace_in<G, B>(
    start: &Path,
    explicit: Option<String>,
    pins: &NamespacePins,
    git_root: G,
    head_config: B,
) -> NamespaceResolution
where
    G: Fn(&Path) -> Option<PathBuf>,
    B: Fn(&Path) -> Option<String>,
{
    // (1) explicit always wins: no detection, nothing to warn about.
    if let Some(name) = explicit
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return resolved(Namespace::Project(name));
    }
    // Set when the worktree REPO_CONFIG_FILE diverges from HEAD; carried on
    // whichever resolution wins so callers can warn in their own channel.
    let mut divergence = None;
    if let Some(top) = git_root(start).as_deref() {
        // (2) repo-committed identity: same namespace under any clone name.
        // ONLY the blob at HEAD counts — an untracked or locally-modified
        // worktree file must never redirect the namespace.
        let committed = head_config(top);
        divergence = repo_config_divergence(top, committed.as_deref());
        if let Some(name) = committed.as_deref().and_then(repo_config_namespace) {
            return resolved_with(Namespace::Project(name), divergence);
        }
        // (3) CLAUDE.md frontmatter, bounded at the toplevel, pin-gated.
        let top_name = dir_name(top);
        match (frontmatter_project_within(start, top), top_name) {
            (Some(claimed), Some(used)) if claimed == used => {
                // Not an override: frontmatter agrees with the toplevel name.
                return resolved_with(Namespace::Project(claimed), divergence);
            }
            (Some(claimed), Some(used)) => {
                if pins.pinned(top) == Some(claimed.as_str()) {
                    return resolved_with(Namespace::Project(claimed), divergence);
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
                    repo_config_divergence: divergence,
                };
            }
            // (4) toplevel directory name.
            (None, Some(used)) => return resolved_with(Namespace::Project(used), divergence),
            // Toplevel has no usable utf8 name (e.g. `/`): fall through.
            (_, None) => {}
        }
    }
    // (5) start (cwd) directory name; else Global.
    match dir_name(start) {
        Some(name) => resolved_with(Namespace::Project(name), divergence),
        None => resolved_with(Namespace::Global, divergence),
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

/// NAMESPACE-IDENTITY-ONLY (C1): `.rusty-brain.toml` is untrusted repo
/// content — cloning a repository must never be able to set sockets, database
/// paths, embedding backends, or any other daemon knob. Those live exclusively
/// in the user-owned config file (`crate::file`, `~/.config/rusty-brain/
/// config.toml`), which is never read from a repository. This struct having no
/// other field is the enforcement: any extra key in the repo file is inert by
/// construction (pinned by `repo_config_daemon_knob_keys_are_inert`).
#[derive(Deserialize)]
struct RepoConfig {
    namespace: Option<String>,
}

/// The committed content of [`REPO_CONFIG_FILE`] at `HEAD`, read via
/// `git -C <toplevel> show HEAD:.rusty-brain.toml` so a worktree-only file can
/// never redirect the namespace. File not committed at `HEAD`, an unborn
/// `HEAD` (fresh repo with no commits), and a missing git all degrade to
/// `None` — "no committed identity". Callers needing a process bound (hooks)
/// supply their own reader to [`resolve_namespace_in`].
pub fn head_repo_config(toplevel: &Path) -> Option<String> {
    let spec = format!("HEAD:{REPO_CONFIG_FILE}");
    let output = Command::new("git")
        .arg("-C")
        .arg(toplevel)
        .args(["show", &spec])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Parse `namespace = "..."` from committed [`REPO_CONFIG_FILE`] content.
/// Parse failure and empty values degrade to `None`.
fn repo_config_namespace(text: &str) -> Option<String> {
    let cfg: RepoConfig = toml::from_str(text).ok()?;
    cfg.namespace
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Compare the worktree [`REPO_CONFIG_FILE`] (if present) against the
/// committed blob so callers can warn that only the committed content takes
/// effect. An absent or unreadable worktree file reports nothing — a locally
/// DELETED tracked file still resolves from `HEAD`, silently.
fn repo_config_divergence(
    toplevel: &Path,
    committed: Option<&str>,
) -> Option<RepoConfigDivergence> {
    let worktree = std::fs::read_to_string(toplevel.join(REPO_CONFIG_FILE)).ok()?;
    let kind = match committed {
        None => RepoConfigDivergenceKind::Untracked,
        Some(blob) if blob != worktree => RepoConfigDivergenceKind::Modified,
        Some(_) => return None,
    };
    Some(RepoConfigDivergence {
        toplevel: toplevel.to_path_buf(),
        kind,
    })
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
    use std::process::Stdio;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn no_pins() -> NamespacePins {
        NamespacePins::default()
    }

    fn no_git(_: &Path) -> Option<PathBuf> {
        None
    }

    /// No committed [`REPO_CONFIG_FILE`] at HEAD (untracked, unborn HEAD, …).
    fn no_head_config(_: &Path) -> Option<String> {
        None
    }

    /// A committed [`REPO_CONFIG_FILE`] blob with the given content.
    fn committed(text: &str) -> impl Fn(&Path) -> Option<String> + '_ {
        move |_| Some(text.to_string())
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
        // (e): even with a committed repo config AND a pinned frontmatter
        // override available, an explicit namespace short-circuits detection.
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("repo");
        fs::create_dir_all(&top).unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(
            &top,
            Some("explicit".to_string()),
            &no_pins(),
            git,
            committed("namespace = \"from-repo-config\"\n"),
        );
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
        // (b): a committed `.rusty-brain.toml` outranks both the toplevel dir
        // name and a CLAUDE.md frontmatter project (even one needing a pin).
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("some-clone-name");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join("CLAUDE.md"), "---\nproject: pretender\n---\n").unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(
            &top,
            None,
            &no_pins(),
            git,
            committed("namespace = \"canonical\"\n"),
        );
        assert_eq!(res.namespace, Namespace::Project("canonical".to_string()));
        assert_eq!(res.unpinned_override, None);
    }

    #[test]
    fn same_repo_under_two_clone_names_resolves_identically() {
        // (d): identity travels with the committed blob, not the directory.
        let tmp = TempDir::new().unwrap();
        let mut seen = Vec::new();
        for clone in ["clone-a", "work-checkout-b"] {
            let top = tmp.path().join(clone);
            fs::create_dir_all(&top).unwrap();
            let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
            seen.push(
                resolve_namespace_in(
                    &top,
                    None,
                    &no_pins(),
                    git,
                    committed("namespace = \"one-project\"\n"),
                )
                .namespace,
            );
        }
        assert_eq!(seen[0], seen[1]);
        assert_eq!(seen[0], Namespace::Project("one-project".to_string()));
    }

    // C1: the repo-committed file is namespace-identity-ONLY. Daemon knobs
    // (sockets, paths, backends) planted by a hostile repo are inert — the
    // namespace still resolves, and `RepoConfig` has no field that could carry
    // them into any configuration surface (the user config file under
    // XDG_CONFIG_HOME is the only knob source; see crate::file).
    #[test]
    fn repo_config_daemon_knob_keys_are_inert() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("repo");
        fs::create_dir_all(&top).unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let hostile = r#"
            namespace = "claimed"
            socket_path = "/tmp/evil.sock"
            db_path = "/tmp/evil.db"
            idle_timeout_secs = 1

            [embed]
            backend = "local"
        "#;
        let res = resolve_namespace_in(&top, None, &no_pins(), git, committed(hostile));
        // The identity key is honored; every knob key has nowhere to go.
        assert_eq!(res.namespace, Namespace::Project("claimed".to_string()));
    }

    #[test]
    fn malformed_repo_config_degrades_to_next_branch() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("repo");
        fs::create_dir_all(&top).unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(
            &top,
            None,
            &no_pins(),
            git,
            committed("namespace = [not toml\n"),
        );
        assert_eq!(res.namespace, Namespace::Project("repo".to_string()));
    }

    #[test]
    fn untracked_worktree_toml_is_ignored_and_reported() {
        // INVERSE regression for the namespace-hijack fix: a worktree
        // `.rusty-brain.toml` with no committed counterpart at HEAD must NOT
        // redirect the namespace — resolution falls through to the git-root
        // name, and the divergence is surfaced so callers can warn.
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("host-repo");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join(REPO_CONFIG_FILE), "namespace = \"hijacked\"\n").unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, None, &no_pins(), git, no_head_config);
        assert_eq!(res.namespace, Namespace::Project("host-repo".to_string()));
        let d = res.repo_config_divergence.expect("divergence is surfaced");
        assert_eq!(d.kind, RepoConfigDivergenceKind::Untracked);
        assert_eq!(d.toplevel, top);
    }

    #[test]
    fn locally_modified_toml_head_blob_wins() {
        // (c): tracked but locally modified — the committed content is used
        // and the modification is surfaced.
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("repo");
        fs::create_dir_all(&top).unwrap();
        fs::write(top.join(REPO_CONFIG_FILE), "namespace = \"hijacked\"\n").unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(
            &top,
            None,
            &no_pins(),
            git,
            committed("namespace = \"canonical\"\n"),
        );
        assert_eq!(res.namespace, Namespace::Project("canonical".to_string()));
        let d = res.repo_config_divergence.expect("divergence is surfaced");
        assert_eq!(d.kind, RepoConfigDivergenceKind::Modified);
    }

    #[test]
    fn worktree_matching_committed_toml_reports_no_divergence() {
        let tmp = TempDir::new().unwrap();
        let top = tmp.path().join("repo");
        fs::create_dir_all(&top).unwrap();
        let text = "namespace = \"canonical\"\n";
        fs::write(top.join(REPO_CONFIG_FILE), text).unwrap();
        let git = |_: &Path| -> Option<PathBuf> { Some(top.clone()) };
        let res = resolve_namespace_in(&top, None, &no_pins(), git, committed(text));
        assert_eq!(res.namespace, Namespace::Project("canonical".to_string()));
        assert_eq!(res.repo_config_divergence, None);
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
        let res = resolve_namespace_in(&nested, None, &no_pins(), git, no_head_config);
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
        let res = resolve_namespace_in(&top, None, &no_pins(), git, no_head_config);
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
        let res = resolve_namespace_in(&top, None, &pins, git, no_head_config);
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
        let res = resolve_namespace_in(&top, None, &pins, git, no_head_config);
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
        let res = resolve_namespace_in(&top, None, &no_pins(), git, no_head_config);
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
        let res = resolve_namespace_in(&dir, None, &no_pins(), no_git, no_head_config);
        assert_eq!(res.namespace, Namespace::Project("plain-dir".to_string()));
        assert_eq!(res.unpinned_override, None);
    }

    #[test]
    fn root_dir_without_repo_degrades_to_global() {
        let res = resolve_namespace_in(Path::new("/"), None, &no_pins(), no_git, no_head_config);
        assert_eq!(res.namespace, Namespace::Global);
    }

    // --- head_repo_config (real git) ---

    /// Run `git <args>` in `dir`; false (test skips) when git is unavailable.
    fn git_run(dir: &Path, args: &[&str]) -> bool {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// `git init` with a repo-local identity so commits work everywhere.
    fn git_init(dir: &Path) -> bool {
        git_run(dir, &["init", "-q"])
            && git_run(dir, &["config", "user.email", "test@example.com"])
            && git_run(dir, &["config", "user.name", "Test"])
    }

    /// Stage everything and commit.
    fn git_commit_all(dir: &Path) -> bool {
        git_run(dir, &["add", "-A"])
            && git_run(dir, &["commit", "-q", "-m", "init", "--no-gpg-sign"])
    }

    #[test]
    fn head_repo_config_reads_the_committed_blob_not_the_worktree() {
        // (a)+(c) against real git: the blob at HEAD is returned even after a
        // local (uncommitted) modification to the worktree file.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap().join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(REPO_CONFIG_FILE), "namespace = \"canonical\"\n").unwrap();
        if !git_init(&repo) || !git_commit_all(&repo) {
            return; // git unavailable; skip
        }
        fs::write(repo.join(REPO_CONFIG_FILE), "namespace = \"hijacked\"\n").unwrap();
        assert_eq!(
            head_repo_config(&repo).as_deref(),
            Some("namespace = \"canonical\"\n")
        );
    }

    #[test]
    fn head_repo_config_is_none_for_untracked_file() {
        // (b): a worktree-only file has no blob at HEAD.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap().join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("README.md"), "x\n").unwrap();
        if !git_init(&repo) || !git_commit_all(&repo) {
            return; // git unavailable; skip
        }
        fs::write(repo.join(REPO_CONFIG_FILE), "namespace = \"hijacked\"\n").unwrap();
        assert_eq!(head_repo_config(&repo), None);
    }

    #[test]
    fn head_repo_config_is_none_for_unborn_head_and_non_repo() {
        // (d): a fresh `git init` repo has no HEAD to read from — degrade to
        // "no committed identity", never an error. Same for a plain dir.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap().join("fresh");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(REPO_CONFIG_FILE), "namespace = \"hijacked\"\n").unwrap();
        if !git_init(&repo) {
            return; // git unavailable; skip
        }
        assert_eq!(head_repo_config(&repo), None);
        let plain = tmp.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert_eq!(head_repo_config(&plain), None);
    }

    #[test]
    fn untracked_toml_falls_through_to_git_root_name_end_to_end() {
        // The owner-requested inverse regression test against real git: an
        // untracked `.rusty-brain.toml` claiming another namespace is ignored
        // and resolution uses the git-root name.
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().canonicalize().unwrap().join("host-repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("README.md"), "x\n").unwrap();
        if !git_init(&repo) || !git_commit_all(&repo) {
            return; // git unavailable; skip
        }
        fs::write(repo.join(REPO_CONFIG_FILE), "namespace = \"hijacked\"\n").unwrap();
        let res = resolve_namespace_in(&repo, None, &no_pins(), git_toplevel, head_repo_config);
        assert_eq!(res.namespace, Namespace::Project("host-repo".to_string()));
        assert_eq!(
            res.repo_config_divergence.map(|d| d.kind),
            Some(RepoConfigDivergenceKind::Untracked)
        );
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
