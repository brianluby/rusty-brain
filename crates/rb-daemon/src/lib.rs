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
mod server;
mod shared_embedder;
mod store_handle;

pub use change::{ChangeKind, MemoryChanged};
pub use jobs::{
    run_once, ConsolidationConfig, ImportanceConfig, JobKind, JobSummary, JobsConfig,
    LinkDecayConfig,
};
// Path defaults live in rb-config (the single source of truth shared with the
// CLI and hooks); re-exported here for existing consumers of the daemon API.
pub use rb_config::{default_db_path, default_socket_path};
// FusionMode re-exported so `serve` can fill `DaemonConfig::fusion_mode`
// without a direct rb-engine dependency (W2.2).
pub use rb_engine::FusionMode;
pub use server::{Daemon, DaemonConfig, EnrichEndpoint};
pub use shared_embedder::SharedEmbedder;
pub use store_handle::{accept_model_change, StoreHandle};
