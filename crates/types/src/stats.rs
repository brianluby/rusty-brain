//! Read-only statistics snapshot of the memory store.
//!
//! [`MindStats`] provides a point-in-time view of the memory database: counts,
//! time range, file size, and observation type distribution. It is intended for
//! diagnostic output and does not carry mutable state.

use crate::observation::ObservationType;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Point-in-time statistics snapshot of the memory store.
///
/// Serialized with camelCase keys. `file_size_bytes` serializes as `"fileSize"`
/// and `type_counts` serializes as `"topTypes"` to match the upstream JSON contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MindStats {
    /// Total number of observations stored.
    pub total_observations: u64,
    /// Total number of completed sessions recorded.
    pub total_sessions: u64,
    /// Timestamp of the earliest observation, or `None` if the store is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_memory: Option<DateTime<Utc>>,
    /// Timestamp of the most recent observation, or `None` if the store is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_memory: Option<DateTime<Utc>>,
    /// Size of the memory database file in bytes. Serialized as `"fileSize"`.
    #[serde(rename = "fileSize")]
    pub file_size_bytes: u64,
    /// Count of observations per [`ObservationType`]. Serialized as `"topTypes"`.
    #[serde(rename = "topTypes")]
    #[serde(default)]
    pub type_counts: HashMap<ObservationType, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observation::ObservationType;
    use chrono::Utc;
    use std::collections::HashMap;

    // --- Construction ---

    #[test]
    fn mind_stats_constructs_with_all_fields() {
        let now = Utc::now();
        let mut type_counts = HashMap::new();
        type_counts.insert(ObservationType::Discovery, 3u64);
        type_counts.insert(ObservationType::Decision, 1u64);

        let stats = MindStats {
            total_observations: 4,
            total_sessions: 2,
            oldest_memory: Some(now),
            newest_memory: Some(now),
            file_size_bytes: 8192,
            type_counts: type_counts.clone(),
        };

        assert_eq!(stats.total_observations, 4);
        assert_eq!(stats.total_sessions, 2);
        assert!(stats.oldest_memory.is_some());
        assert!(stats.newest_memory.is_some());
        assert_eq!(stats.file_size_bytes, 8192);
        assert_eq!(stats.type_counts.len(), 2);
        assert_eq!(stats.type_counts[&ObservationType::Discovery], 3);
        assert_eq!(stats.type_counts[&ObservationType::Decision], 1);
    }

    // --- Empty store: timestamps are None ---

    #[test]
    fn mind_stats_oldest_memory_is_none_for_empty_store() {
        let stats = MindStats {
            total_observations: 0,
            total_sessions: 0,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 0,
            type_counts: HashMap::new(),
        };

        assert!(stats.oldest_memory.is_none());
    }

    #[test]
    fn mind_stats_newest_memory_is_none_for_empty_store() {
        let stats = MindStats {
            total_observations: 0,
            total_sessions: 0,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 0,
            type_counts: HashMap::new(),
        };

        assert!(stats.newest_memory.is_none());
    }

    // --- type_counts defaults to empty HashMap ---

    #[test]
    fn mind_stats_type_counts_empty_by_default_when_no_observations() {
        let stats = MindStats {
            total_observations: 0,
            total_sessions: 0,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 0,
            type_counts: HashMap::new(),
        };

        assert!(stats.type_counts.is_empty());
    }

    // --- Populated type_counts ---

    #[test]
    fn mind_stats_type_counts_accepts_all_observation_type_variants() {
        let mut type_counts = HashMap::new();
        type_counts.insert(ObservationType::Discovery, 10u64);
        type_counts.insert(ObservationType::Decision, 5u64);
        type_counts.insert(ObservationType::Problem, 3u64);
        type_counts.insert(ObservationType::Solution, 3u64);
        type_counts.insert(ObservationType::Pattern, 2u64);
        type_counts.insert(ObservationType::Warning, 1u64);
        type_counts.insert(ObservationType::Success, 4u64);
        type_counts.insert(ObservationType::Refactor, 2u64);
        type_counts.insert(ObservationType::Bugfix, 7u64);
        type_counts.insert(ObservationType::Feature, 6u64);

        let oldest = Utc::now();
        let newest = Utc::now();

        let stats = MindStats {
            total_observations: 43,
            total_sessions: 8,
            oldest_memory: Some(oldest),
            newest_memory: Some(newest),
            file_size_bytes: 65536,
            type_counts: type_counts.clone(),
        };

        assert_eq!(stats.type_counts.len(), 10);
        assert_eq!(stats.type_counts[&ObservationType::Discovery], 10);
        assert_eq!(stats.type_counts[&ObservationType::Bugfix], 7);
        assert_eq!(stats.type_counts[&ObservationType::Feature], 6);
    }

    // --- Field types: u64 allows large values ---

    #[test]
    fn mind_stats_total_observations_is_u64() {
        let stats = MindStats {
            total_observations: u64::MAX,
            total_sessions: u64::MAX,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: u64::MAX,
            type_counts: HashMap::new(),
        };

        assert_eq!(stats.total_observations, u64::MAX);
        assert_eq!(stats.total_sessions, u64::MAX);
        assert_eq!(stats.file_size_bytes, u64::MAX);
    }

    // --- DateTime<Utc> fields carry correct timestamps ---

    #[test]
    fn mind_stats_timestamps_preserve_oldest_before_newest() {
        use chrono::Duration;

        let older = Utc::now() - Duration::hours(24);
        let newer = Utc::now();

        let stats = MindStats {
            total_observations: 2,
            total_sessions: 1,
            oldest_memory: Some(older),
            newest_memory: Some(newer),
            file_size_bytes: 512,
            type_counts: HashMap::new(),
        };

        let oldest = stats.oldest_memory.expect("oldest_memory should be Some");
        let newest = stats.newest_memory.expect("newest_memory should be Some");
        assert!(
            oldest < newest,
            "oldest_memory must be earlier than newest_memory"
        );
    }

    // -------------------------------------------------------------------------
    // T020: Round-trip serialization tests
    // -------------------------------------------------------------------------

    #[test]
    fn mind_stats_json_round_trip_with_timestamps() {
        use chrono::Duration;

        let older = Utc::now() - Duration::hours(48);
        let newer = Utc::now();

        let mut type_counts = HashMap::new();
        type_counts.insert(ObservationType::Discovery, 5u64);

        let original = MindStats {
            total_observations: 10,
            total_sessions: 3,
            oldest_memory: Some(older),
            newest_memory: Some(newer),
            file_size_bytes: 4096,
            type_counts,
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: MindStats =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve MindStats with Some timestamps"
        );

        // Also verify the None case round-trips correctly.
        let original_none = MindStats {
            total_observations: 0,
            total_sessions: 0,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 0,
            type_counts: HashMap::new(),
        };

        let json_none = serde_json::to_string(&original_none).expect("serialization must succeed");
        let deserialized_none: MindStats =
            serde_json::from_str(&json_none).expect("deserialization must succeed");

        assert_eq!(
            original_none, deserialized_none,
            "round-trip must preserve MindStats with None timestamps"
        );
        assert!(deserialized_none.oldest_memory.is_none());
        assert!(deserialized_none.newest_memory.is_none());
    }

    #[test]
    fn mind_stats_json_round_trip_type_counts() {
        let mut type_counts = HashMap::new();
        type_counts.insert(ObservationType::Discovery, 10u64);
        type_counts.insert(ObservationType::Decision, 5u64);
        type_counts.insert(ObservationType::Bugfix, 3u64);
        type_counts.insert(ObservationType::Feature, 7u64);
        type_counts.insert(ObservationType::Warning, 2u64);

        let original = MindStats {
            total_observations: 27,
            total_sessions: 4,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 8192,
            type_counts,
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: MindStats =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve HashMap<ObservationType, u64>"
        );
        assert_eq!(deserialized.type_counts.len(), 5);
        assert_eq!(
            deserialized.type_counts[&ObservationType::Discovery],
            10,
            "Discovery count must survive round-trip"
        );
        assert_eq!(
            deserialized.type_counts[&ObservationType::Feature],
            7,
            "Feature count must survive round-trip"
        );
    }

    #[test]
    fn mind_stats_json_verify_file_size_key() {
        let original = MindStats {
            total_observations: 1,
            total_sessions: 1,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 1024,
            type_counts: HashMap::new(),
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");

        assert!(
            json.contains("\"fileSize\""),
            "JSON must use key 'fileSize', got: {json}"
        );
        assert!(
            !json.contains("\"fileSizeBytes\""),
            "JSON must NOT use key 'fileSizeBytes', got: {json}"
        );
    }

    #[test]
    fn mind_stats_json_verify_top_types_key() {
        let mut type_counts = HashMap::new();
        type_counts.insert(ObservationType::Pattern, 4u64);

        let original = MindStats {
            total_observations: 4,
            total_sessions: 1,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 512,
            type_counts,
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");

        assert!(
            json.contains("\"topTypes\""),
            "JSON must use key 'topTypes', got: {json}"
        );
        assert!(
            !json.contains("\"typeCounts\""),
            "JSON must NOT use key 'typeCounts', got: {json}"
        );
    }
}
