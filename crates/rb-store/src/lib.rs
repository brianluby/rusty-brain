//! `rb_store`: SQLite + sqlite-vec storage engine for rusty-brain.
//!
//! Provides the `Store` trait and `SqliteStore` implementation (added in
//! subsequent tasks): one database, one transaction, file-discovered
//! checksummed migrations, FTS5 keyword search, sqlite-vec KNN, and a
//! recursive-CTE graph walk.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
