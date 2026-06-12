//! Client-side namespace resolution: thin shim over [`rb_config::namespace`]
//! (the single W0.3 implementation). The CLI is the interactive consumer: an
//! unpinned `CLAUDE.md` frontmatter `project:` override warns on stderr and
//! falls back to the git-toplevel name unless `--accept-namespace-override`
//! pins it (known-hosts style).
//!
//! Resolution MUST run off the async runtime (it shells out to git and reads
//! files); `main.rs` computes it before `block_on`. Never panics, never fails:
//! degrades branch by branch to `Namespace::Global`.

use rb_config::namespace::{accept_override, resolve_namespace, NamespaceResolution};
use rb_types::Namespace;

/// Resolve for the CLI: `--namespace` flag > `RUSTY_BRAIN_NAMESPACE` env >
/// detection. `accept` records the pin for an unpinned frontmatter override
/// (explicit user consent). Synchronous: call this OFF the tokio runtime.
pub fn resolve_for_cli(flag: Option<&str>, accept: bool) -> Namespace {
    let NamespaceResolution {
        namespace,
        unpinned_override,
    } = resolve_namespace(flag);
    let Some(o) = unpinned_override else {
        return namespace;
    };
    if accept {
        return match accept_override(&o) {
            Ok(ns) => {
                eprintln!(
                    "pinned namespace override for {}: `{}`",
                    o.toplevel.display(),
                    o.claimed
                );
                ns
            }
            // The user consented explicitly: honor the override for this run
            // even if persisting the pin failed.
            Err(e) => {
                eprintln!(
                    "warning: could not record namespace pin ({e}); using `{}` for this run only",
                    o.claimed
                );
                Namespace::Project(o.claimed)
            }
        };
    }
    eprintln!(
        "WARNING: CLAUDE.md in {} claims namespace `{}`, but this repository is `{}`.\n\
         Using `{}`. If `{}` is intentional, re-run with --accept-namespace-override to pin it.",
        o.toplevel.display(),
        o.claimed,
        o.used,
        o.used,
        o.claimed
    );
    namespace
}

/// Compat entry point (original P2 API): plain detection, no flags.
pub fn detect_namespace() -> Namespace {
    resolve_for_cli(None, false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rb_types::Namespace;

    #[test]
    fn explicit_flag_beats_everything() {
        // (e): the flag short-circuits detection entirely — no fs, no git, no
        // warning path.
        let ns = resolve_for_cli(Some("forced-name"), false);
        assert_eq!(ns, Namespace::Project("forced-name".to_string()));
    }

    #[test]
    fn blank_flag_falls_back_to_detection() {
        // A whitespace-only flag is not an explicit namespace.
        let ns = resolve_for_cli(Some("   "), false);
        assert_ne!(ns, Namespace::Project("   ".to_string()));
    }

    // End-to-end resolution against real temp trees and real git, exercised
    // through the shared rb-config implementation this shim delegates to.
    mod fs_walk {
        use rb_config::namespace::{git_toplevel, resolve_namespace_in, NamespacePins};
        use rb_types::Namespace;
        use std::fs;
        use std::path::Path;
        use std::process::{Command, Stdio};
        use tempfile::TempDir;

        /// `git init` `dir`; false (test skips) when git is unavailable.
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

        fn resolve(start: &Path) -> Namespace {
            resolve_namespace_in(start, None, &NamespacePins::default(), git_toplevel).namespace
        }

        #[test]
        fn walk_stops_at_git_toplevel_f54_regression() {
            // (a) F54: outer CLAUDE.md (the ~/CLAUDE.md scenario) + inner
            // CLAUDE.md-less repo -> the repo's own name, never the outer file.
            let tmp = TempDir::new().unwrap();
            // Canonicalize: git reports the physical toplevel (macOS /tmp).
            let outer = tmp.path().canonicalize().unwrap();
            fs::write(
                outer.join("CLAUDE.md"),
                "---\nproject: outer-claim\n---\n# Outer Heading\n",
            )
            .unwrap();
            let repo = outer.join("inner-repo");
            let nested = repo.join("src");
            fs::create_dir_all(&nested).unwrap();
            if !git_init(&repo) {
                return; // git unavailable; skip
            }
            assert_eq!(
                resolve(&nested),
                Namespace::Project("inner-repo".to_string())
            );
        }

        #[test]
        fn repo_committed_toml_wins_over_directory_name() {
            // (b)+(d): identity travels with `.rusty-brain.toml`.
            let tmp = TempDir::new().unwrap();
            let repo = tmp.path().canonicalize().unwrap().join("any-clone-name");
            fs::create_dir_all(&repo).unwrap();
            fs::write(
                repo.join(".rusty-brain.toml"),
                "namespace = \"pinned-id\"\n",
            )
            .unwrap();
            if !git_init(&repo) {
                return; // git unavailable; skip
            }
            assert_eq!(resolve(&repo), Namespace::Project("pinned-id".to_string()));
        }

        #[test]
        fn malicious_repo_frontmatter_is_not_honored_unpinned() {
            // (c) F22: a cloned repo's own CLAUDE.md cannot claim another
            // project's namespace without an explicit pin.
            let tmp = TempDir::new().unwrap();
            let repo = tmp.path().canonicalize().unwrap().join("cloned-repo");
            fs::create_dir_all(&repo).unwrap();
            fs::write(repo.join("CLAUDE.md"), "---\nproject: victim\n---\n").unwrap();
            if !git_init(&repo) {
                return; // git unavailable; skip
            }
            assert_eq!(
                resolve(&repo),
                Namespace::Project("cloned-repo".to_string())
            );
        }
    }
}
