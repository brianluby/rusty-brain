//! `rb_daemon`: single-writer service over `rb_store`.
//!
//! One dedicated OS thread owns the write `SqliteStore` (rusqlite is `!Sync`,
//! so the write connection never crosses threads); a bounded pool of read
//! stores serves concurrent reads via `spawn_blocking`; a Unix-domain-socket
//! listener frames `rb_proto` requests to a per-connection `MemoryEngine`.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod change;
mod error_map;
mod jobs;
mod paths;
mod server;
mod shared_embedder;
mod store_handle;

pub use change::{ChangeKind, MemoryChanged};
pub use jobs::{ConsolidationConfig, ImportanceConfig, JobsConfig, LinkDecayConfig};
pub use paths::{default_db_path, default_socket_path};
pub use server::{Daemon, DaemonConfig};
pub use shared_embedder::SharedEmbedder;
pub use store_handle::StoreHandle;
