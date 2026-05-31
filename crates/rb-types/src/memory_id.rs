use crate::error::Error;
use serde::{Deserialize, Serialize};

/// Stable, unique identifier for a single `MemoryNote`. Wraps a v4 UUID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(uuid::Uuid);

impl MemoryId {
    /// Generate a fresh random identifier.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Return the underlying UUID by value.
    pub fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MemoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for MemoryId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = uuid::Uuid::parse_str(s)
            .map_err(|e| Error::Storage(format!("invalid memory id '{s}': {e}")))?;
        Ok(Self(uuid))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::Error;
    use std::str::FromStr;

    #[test]
    fn new_ids_are_unique() {
        assert_ne!(MemoryId::new(), MemoryId::new());
    }

    #[test]
    fn default_equals_new_shape() {
        let id = MemoryId::default();
        // round-trips through its own string form
        let parsed = MemoryId::from_str(&id.to_string()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn display_and_fromstr_round_trip() {
        let id = MemoryId::new();
        let s = id.to_string();
        let back = MemoryId::from_str(&s).unwrap();
        assert_eq!(id, back);
        assert_eq!(back.as_uuid(), id.as_uuid());
    }

    #[test]
    fn fromstr_rejects_bad_uuid() {
        let err = MemoryId::from_str("not-a-uuid").unwrap_err();
        assert!(matches!(err, Error::Storage(_)));
    }

    #[test]
    fn serde_json_round_trip_is_plain_uuid_string() {
        let id = MemoryId::new();
        let json = serde_json::to_string(&id).unwrap();
        // serde derive on a single-field tuple struct serializes as the inner value
        assert_eq!(json, format!("\"{}\"", id.as_uuid()));
        let back: MemoryId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
