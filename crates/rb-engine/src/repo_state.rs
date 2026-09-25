//! The repo-state seam for state-bound staleness (Vikunja #63).
//!
//! Recall-time staleness needs the CURRENT repository state (HEAD commit,
//! worktree cleanliness) to compare against a memory's commit anchors. The
//! engine must stay repo-agnostic, so the state enters through this provider
//! trait: the daemon wires [`GitRepoState`]-style implementations over its
//! own working directory, and tests/eval inject fixed snapshots.
//!
//! The trait method is deliberately SYNC (object-safe, no async-trait
//! dependency): implementations that shell out must bound themselves, and the
//! engine invokes them off the async path via `spawn_blocking` so a slow git
//! can never stall the runtime.

use rb_types::RepoSnapshot;

/// Source of the current repository state for staleness evaluation. See the
/// module docs; `snapshot` returning `None` means "state unresolvable" and
/// disables staleness claims entirely (see [`rb_types::evaluate_staleness`]).
pub trait RepoStateProvider: Send + Sync {
    fn snapshot(&self) -> Option<RepoSnapshot>;
}

/// A fixed snapshot (tests and eval): every recall sees exactly this state.
pub struct FixedRepoState(Option<RepoSnapshot>);

impl FixedRepoState {
    #[must_use]
    pub fn new(snapshot: Option<RepoSnapshot>) -> Self {
        Self(snapshot)
    }
}

impl RepoStateProvider for FixedRepoState {
    fn snapshot(&self) -> Option<RepoSnapshot> {
        self.0.clone()
    }
}
