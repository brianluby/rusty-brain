//! Change-notification vocabulary: what happened to a memory, broadcast after a
//! successful write. Notification only — never coordination. Lives in `rb-types`
//! (the leaf crate) so both `rb-proto` (wire `Response`) and `rb-daemon` (the
//! broadcast channel) can name it without a dependency cycle.

use crate::{MemoryId, Namespace};
use serde::{Deserialize, Serialize};

/// What happened to a memory. Published after a successful commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    Created,
    Updated,
    Archived,
}

/// Change-notification event broadcast on every successful write (spec §8).
///
/// Notification only — never coordination. Enables the `subscribe` feature with
/// no new machinery: the daemon already publishes one of these per committed
/// write on a `tokio::sync::broadcast` channel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryChanged {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub kind: ChangeKind,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::{MemoryId, Namespace};

    #[test]
    fn change_kind_round_trips_all_variants() {
        for kind in [
            ChangeKind::Created,
            ChangeKind::Updated,
            ChangeKind::Archived,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: ChangeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn memory_changed_round_trips_clones_and_eq() {
        let evt = MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Project("rusty-brain".into()),
            kind: ChangeKind::Created,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: MemoryChanged = serde_json::from_str(&json).unwrap();
        // PartialEq is required by streaming/wire tests downstream.
        assert_eq!(evt, back);
        // Clone is required so broadcast subscribers each get an owned copy.
        assert_eq!(evt.clone(), evt);
    }
}
