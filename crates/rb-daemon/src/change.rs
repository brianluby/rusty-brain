use rb_types::{MemoryId, Namespace};
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
/// Notification only - never coordination. Enables the deferred `subscribe`
/// feature with no new machinery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryChanged {
    pub id: MemoryId,
    pub namespace: Namespace,
    pub kind: ChangeKind,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryId, Namespace};

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
    fn memory_changed_round_trips_and_clones() {
        let evt = MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Project("rusty-brain".into()),
            kind: ChangeKind::Created,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: MemoryChanged = serde_json::from_str(&json).unwrap();
        assert_eq!(evt.id, back.id);
        assert_eq!(evt.namespace, back.namespace);
        assert_eq!(evt.kind, back.kind);
        // Clone is required so broadcast subscribers each get an owned copy.
        assert_eq!(evt.clone().kind, ChangeKind::Created);
    }
}
