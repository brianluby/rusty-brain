//! `rb_search`: pure, deterministic hybrid ranking for rusty-brain.
//!
//! No IO, no async. Combines normalized keyword / vector / graph / importance /
//! recency signals into a single weighted score (see `rank`). `build_signals`
//! folds the three retrieval result sets into the `Signals` that `rank` consumes.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod fusion;
mod merge;
mod rank;
mod weights;

pub use fusion::{rank_rrf, FusionMode, RrfConfig};
pub use merge::build_signals;
pub use rank::{rank, Signals, CONFIDENCE_FLOOR, HALF_LIFE};
pub use weights::Weights;
