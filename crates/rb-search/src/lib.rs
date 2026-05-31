//! `rb_search`: pure, deterministic hybrid ranking for rusty-brain.
//!
//! No IO, no async. Combines normalized keyword / vector / graph / importance /
//! recency signals into a single weighted score (see `rank`).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod rank;
mod weights;

pub use rank::{rank, Signals, HALF_LIFE};
pub use weights::Weights;
