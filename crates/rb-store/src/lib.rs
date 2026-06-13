//! `rb-store`: SQLite + sqlite-vec storage engine for rusty-brain.
//!
//! One database file holds memories, FTS index, vectors and links so that a
//! `remember` is a single transaction (no dual-DB desync). The embedding
//! dimension is a single configured value, enforced fail-closed at `open`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;
mod migrations;
mod store;

pub use migrations::run_migrations;
pub use store::{
    AccessBump, ConsolidationCandidate, LinkRow, NamespaceRenameOutcome, OplogReplayPage, RecalRow,
    ScrubOutcome, SqliteStore, Store,
};
