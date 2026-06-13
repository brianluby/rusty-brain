//! Change-notification re-export. The canonical `MemoryChanged`/`ChangeKind`
//! types live in `rb-types` (the leaf crate) so both `rb-proto` (wire
//! `Response`) and this daemon (the broadcast channel) can name them without a
//! dependency cycle. This module re-exports them so existing intra-crate paths
//! (`crate::change::{ChangeKind, MemoryChanged}`) keep working verbatim.

pub use rb_types::{ChangeKind, MemoryChanged};

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryId, Namespace};

    #[test]
    fn reexported_change_types_are_the_rb_types_definitions() {
        // A value constructed via the rb-daemon path is byte-identical to one
        // constructed via the rb-types path — proving there is exactly ONE
        // definition, re-exported, not a divergent copy.
        let via_daemon = MemoryChanged {
            id: MemoryId::new(),
            namespace: Namespace::Global,
            kind: ChangeKind::Updated,
            seq: Some(1),
        };
        let direct: rb_types::MemoryChanged = via_daemon.clone();
        assert_eq!(via_daemon, direct);
        assert_eq!(via_daemon.kind, rb_types::ChangeKind::Updated);
    }
}
