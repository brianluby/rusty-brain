//! `rb_search`: pure, deterministic hybrid ranking for rusty-brain.
//!
//! No IO. Combines normalized keyword/vector/graph/importance/recency signals
//! into a single weighted score. Concrete types are added in subsequent tasks.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
