//! State-bound staleness (Vikunja #63): recall-time evaluation of whether a
//! memory's code-anchor evidence still describes the CURRENT repository
//! state, so anchor- or commit-bound evidence is never silently reused after
//! HEAD or the worktree moves past it.
//!
//! # What can be claimed, and by what
//!
//! Only COMMIT-bound memories are state-bound: a typed `commit` anchor
//! (migration 009) pins the memory's evidence to one repository state. Such a
//! memory is CURRENT exactly when the resolvable HEAD equals the anchored
//! commit AND the worktree is clean; if HEAD has moved past the anchored
//! commit, or tracked files carry uncommitted changes, the current state
//! differs from the evidence state and the result is STALE.
//!
//! File/symbol anchors alone bind a memory to a PATH, not a state — there is
//! no write-time snapshot to compare against, so staleness is unsubstantiable
//! for them. Claiming it anyway (e.g. flagging every memory anchored to a
//! dirty file) would mark fresh uncommitted session summaries stale on birth
//! and flip them back to current the moment the file is committed without the
//! memory ever being re-verified — a false claim in both directions, so it is
//! deliberately not made.
//!
//! Honesty cuts both ways: when no repository state is resolvable (no git, or
//! the daemon does not sit inside a repository), NOTHING is marked stale — a
//! staleness claim requires substantiation just like a trust-class claim.
//!
//! # Where the state comes from
//!
//! The engine takes the snapshot through a provider seam
//! (`rb_engine::RepoStateProvider`); the daemon wires a git-backed provider
//! over its own working directory, and tests inject fixed snapshots. The
//! snapshot is advisory metadata on the RESULT row (like `contested`), never
//! a mutation of the stored memory.

/// A resolved repository state at recall time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSnapshot {
    /// The current HEAD commit SHA as printed by `git rev-parse HEAD`, or
    /// `None` when HEAD is unresolvable (not a repository / git failed).
    pub head: Option<String>,
    /// Whether the worktree is clean of tracked modifications (staged or
    /// unstaged). Untracked files are deliberately ignored: they do not
    /// change the tracked state the anchored commit describes, and counting
    /// them would let untracked artifacts (build output, a database living
    /// inside the repo) poison every staleness verdict.
    pub clean_worktree: bool,
}

impl RepoSnapshot {
    /// A snapshot of a repository at `head` with a clean worktree — the state
    /// a commit-bound memory must find to be current (tests).
    #[must_use]
    pub fn at(head: &str) -> Self {
        Self {
            head: Some(head.to_string()),
            clean_worktree: true,
        }
    }
}

/// Whether a memory carrying exactly `anchors` is STALE against `snapshot`
/// (Vikunja #63). `None` snapshot (state unresolvable) or a memory with no
/// commit anchor (no state binding) is never stale — see the module docs for
/// why an unsubstantiated claim is not made. The SHA comparison ignores
/// ASCII case (users paste uppercase hex) and surrounding whitespace, mirroring
/// [`crate::normalize_anchor_value`].
#[must_use]
pub fn evaluate_staleness(
    anchors: &[crate::MemoryAnchor],
    snapshot: Option<&RepoSnapshot>,
) -> bool {
    let Some(snapshot) = snapshot else {
        return false;
    };
    let Some(head) = snapshot.head.as_deref() else {
        return false;
    };
    let head = head.trim();
    let commit_bound = anchors.iter().any(|a| a.kind == crate::AnchorKind::Commit);
    if !commit_bound {
        return false;
    }
    // Current only when EVERY bound commit IS the head and the worktree
    // matches it. Two different commit anchors can never both equal head,
    // so a multi-commit memory is stale the moment head resolves — correct:
    // its evidence spans states, and only one of them can be now.
    let on_head = anchors
        .iter()
        .filter(|a| a.kind == crate::AnchorKind::Commit)
        .all(|a| a.value.trim().eq_ignore_ascii_case(head));
    !(on_head && snapshot.clean_worktree)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::{AnchorKind, MemoryAnchor};

    fn commit(sha: &str) -> MemoryAnchor {
        MemoryAnchor::new(AnchorKind::Commit, sha).unwrap()
    }

    fn file(path: &str) -> MemoryAnchor {
        MemoryAnchor::new(AnchorKind::File, path).unwrap()
    }

    #[test]
    fn no_snapshot_never_stale() {
        assert!(!evaluate_staleness(&[commit("abc")], None));
    }

    #[test]
    fn unresolvable_head_never_stale() {
        let snapshot = RepoSnapshot {
            head: None,
            clean_worktree: true,
        };
        assert!(!evaluate_staleness(&[commit("abc")], Some(&snapshot)));
    }

    #[test]
    fn anchored_commit_at_clean_head_is_current() {
        let snapshot = RepoSnapshot::at("abc123");
        assert!(!evaluate_staleness(&[commit("abc123")], Some(&snapshot)));
        // Case and padding tolerate pasted hex.
        assert!(!evaluate_staleness(&[commit("  ABC123 ")], Some(&snapshot)));
        // Extra non-commit anchors do not unbind the state check.
        assert!(!evaluate_staleness(
            &[commit("abc123"), file("src/lib.rs")],
            Some(&snapshot)
        ));
    }

    #[test]
    fn moved_head_is_stale() {
        let snapshot = RepoSnapshot::at("def456");
        assert!(evaluate_staleness(&[commit("abc123")], Some(&snapshot)));
    }

    #[test]
    fn dirty_worktree_is_stale_even_on_matching_head() {
        let snapshot = RepoSnapshot {
            head: Some("abc123".to_string()),
            clean_worktree: false,
        };
        assert!(evaluate_staleness(&[commit("abc123")], Some(&snapshot)));
    }

    #[test]
    fn conflicting_commit_anchors_are_stale_when_head_resolves() {
        // Two distinct commits cannot both be HEAD: the evidence spans
        // states and only one can be current.
        let snapshot = RepoSnapshot::at("abc123");
        assert!(evaluate_staleness(
            &[commit("abc123"), commit("def456")],
            Some(&snapshot)
        ));
    }

    #[test]
    fn file_or_symbol_anchors_alone_are_not_state_bound() {
        let snapshot = RepoSnapshot {
            head: Some("abc123".to_string()),
            clean_worktree: false,
        };
        assert!(!evaluate_staleness(&[file("src/lib.rs")], Some(&snapshot)));
        assert!(!evaluate_staleness(&[], Some(&snapshot)));
        let symbol = MemoryAnchor::new(AnchorKind::Symbol, "Foo::bar").unwrap();
        assert!(!evaluate_staleness(&[symbol], Some(&snapshot)));
    }
}
