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
    /// The durable oplog sequence this change committed at (W2.7). Consumers
    /// track `max(seq)` as their replay cursor: `subscribe --since <seq>`
    /// replays missed changes from the oplog instead of silently dropping
    /// them. Additive + `#[serde(default)]`: events from an older daemon
    /// decode to `None` (no cursor advance), and a `None` seq serializes
    /// without the key so old frames stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
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
            seq: Some(7),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: MemoryChanged = serde_json::from_str(&json).unwrap();
        // PartialEq is required by streaming/wire tests downstream.
        assert_eq!(evt, back);
        // Clone is required so broadcast subscribers each get an owned copy.
        assert_eq!(evt.clone(), evt);
    }

    #[test]
    fn seq_is_wire_compatible_in_both_directions() {
        // Old frame (no `seq` key) decodes to None; a None seq serializes
        // WITHOUT the key, keeping the frame byte-identical to the pre-W2.7
        // shape — the `contested` additive-field precedent.
        let old = serde_json::json!({
            "id": MemoryId::new().to_string(),
            "namespace": Namespace::Global,
            "kind": "Created"
        });
        let back: MemoryChanged = serde_json::from_value(old).unwrap();
        assert_eq!(back.seq, None);

        let none = MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Global,
            kind: ChangeKind::Created,
            seq: None,
        };
        let json = serde_json::to_value(&none).unwrap();
        assert!(
            json.as_object().unwrap().get("seq").is_none(),
            "None seq must not serialize: {json}"
        );
    }
}
