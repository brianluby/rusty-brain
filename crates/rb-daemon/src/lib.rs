//! `rb_daemon`: the single-writer rusty-brain service.
//!
//! One dedicated OS thread owns the write connection (rusqlite is `!Sync`);
//! reads run on a bounded pool via `spawn_blocking`; commits broadcast a
//! `MemoryChanged` event. A UDS listener dispatches per-connection engines
//! with server-side namespace isolation. Concrete types are added later.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
