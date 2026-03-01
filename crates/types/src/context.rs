//! Context payload injected into an agent's system prompt.
//!
//! [`InjectedContext`] bundles the observations, memories, and session summaries
//! that the memory engine selects for the current agent turn. All fields default
//! to empty/zero, so a default-constructed context represents "no memory."

use serde::{Deserialize, Serialize};

use crate::observation::Observation;
use crate::session::SessionSummary;

/// The memory context payload injected into an agent's system prompt.
///
/// Serialized with camelCase keys. All fields default to their zero values
/// via `#[serde(default)]`, allowing partial JSON deserialization.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectedContext {
    /// Most recent observations from the current session.
    #[serde(default)]
    pub recent_observations: Vec<Observation>,
    /// Observations retrieved by semantic similarity search.
    #[serde(default)]
    pub relevant_memories: Vec<Observation>,
    /// Summaries of previous sessions for long-term context.
    #[serde(default)]
    pub session_summaries: Vec<SessionSummary>,
    /// Approximate token count consumed by this context payload.
    #[serde(default)]
    pub token_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // T007: Unit tests for InjectedContext
    //
    // These tests are written FIRST (RED phase). They reference InjectedContext,
    // Observation, and SessionSummary which are not yet defined in this module.
    // The tests must fail to compile until T012 (implementation) is complete.

    // ---------------------------------------------------------------------------
    // Helper: build a minimal Observation for use in context tests.
    //
    // Observation::new() is the validated constructor defined in observation.rs.
    // Using it here means these tests will also fail until observation types exist.
    // ---------------------------------------------------------------------------

    fn make_observation() -> Observation {
        use crate::observation::ObservationType;
        Observation::new(
            ObservationType::Discovery,
            "test_tool".to_string(),
            "test summary".to_string(),
            "test content".to_string(),
            None,
        )
        .expect("valid observation should construct without error")
    }

    fn make_session_summary() -> SessionSummary {
        use chrono::Utc;
        let start = Utc::now();
        let end = start;
        SessionSummary::new(
            "session-001".to_string(),
            start,
            end,
            0,
            vec![],
            vec![],
            "session summary text".to_string(),
        )
        .expect("valid session summary should construct without error")
    }

    // ---------------------------------------------------------------------------
    // Default behaviour
    // ---------------------------------------------------------------------------

    #[test]
    fn default_produces_empty_recent_observations() {
        let ctx = InjectedContext::default();
        assert!(
            ctx.recent_observations.is_empty(),
            "recent_observations should be empty by default"
        );
    }

    #[test]
    fn default_produces_empty_relevant_memories() {
        let ctx = InjectedContext::default();
        assert!(
            ctx.relevant_memories.is_empty(),
            "relevant_memories should be empty by default"
        );
    }

    #[test]
    fn default_produces_empty_session_summaries() {
        let ctx = InjectedContext::default();
        assert!(
            ctx.session_summaries.is_empty(),
            "session_summaries should be empty by default"
        );
    }

    #[test]
    fn default_produces_zero_token_count() {
        let ctx = InjectedContext::default();
        assert_eq!(ctx.token_count, 0, "token_count should be 0 by default");
    }

    // Verify the derived Default impl works end-to-end in a single call.
    #[test]
    fn default_impl_compiles_and_all_fields_are_at_zero_state() {
        let ctx = InjectedContext::default();
        assert!(ctx.recent_observations.is_empty());
        assert!(ctx.relevant_memories.is_empty());
        assert!(ctx.session_summaries.is_empty());
        assert_eq!(ctx.token_count, 0);
    }

    // ---------------------------------------------------------------------------
    // Construction with all fields populated
    // ---------------------------------------------------------------------------

    #[test]
    fn construction_with_all_fields_stores_observations_in_recent_observations() {
        let obs = make_observation();
        let ctx = InjectedContext {
            recent_observations: vec![obs],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 0,
        };
        assert_eq!(ctx.recent_observations.len(), 1);
    }

    #[test]
    fn construction_with_all_fields_stores_observations_in_relevant_memories() {
        let obs = make_observation();
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![obs],
            session_summaries: vec![],
            token_count: 0,
        };
        assert_eq!(ctx.relevant_memories.len(), 1);
    }

    #[test]
    fn construction_with_all_fields_stores_session_summaries() {
        let summary = make_session_summary();
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![summary],
            token_count: 0,
        };
        assert_eq!(ctx.session_summaries.len(), 1);
    }

    #[test]
    fn construction_with_all_fields_stores_token_count() {
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 1_234,
        };
        assert_eq!(ctx.token_count, 1_234);
    }

    #[test]
    fn construction_with_multiple_entries_in_each_vec() {
        let obs1 = make_observation();
        let obs2 = make_observation();
        let obs3 = make_observation();
        let obs4 = make_observation();
        let summary1 = make_session_summary();
        let summary2 = make_session_summary();

        let ctx = InjectedContext {
            recent_observations: vec![obs1, obs2],
            relevant_memories: vec![obs3, obs4],
            session_summaries: vec![summary1, summary2],
            token_count: 42,
        };

        assert_eq!(ctx.recent_observations.len(), 2);
        assert_eq!(ctx.relevant_memories.len(), 2);
        assert_eq!(ctx.session_summaries.len(), 2);
        assert_eq!(ctx.token_count, 42);
    }

    // ---------------------------------------------------------------------------
    // Token count boundary values
    // ---------------------------------------------------------------------------

    #[test]
    fn token_count_accepts_zero() {
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 0,
        };
        assert_eq!(ctx.token_count, 0);
    }

    #[test]
    fn token_count_accepts_max_u64() {
        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: u64::MAX,
        };
        assert_eq!(ctx.token_count, u64::MAX);
    }

    // ---------------------------------------------------------------------------
    // Derive trait: Clone
    // ---------------------------------------------------------------------------

    #[test]
    fn injected_context_is_clone() {
        let obs = make_observation();
        let ctx = InjectedContext {
            recent_observations: vec![obs],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 5,
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.recent_observations.len(), 1);
        assert_eq!(cloned.token_count, 5);
    }

    // ---------------------------------------------------------------------------
    // Derive trait: PartialEq
    // ---------------------------------------------------------------------------

    #[test]
    fn two_default_injected_contexts_are_equal() {
        let a = InjectedContext::default();
        let b = InjectedContext::default();
        assert_eq!(a, b);
    }

    #[test]
    fn contexts_with_different_token_counts_are_not_equal() {
        let a = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 1,
        };
        let b = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 2,
        };
        assert_ne!(a, b);
    }

    // ---------------------------------------------------------------------------
    // Derive trait: Debug
    // ---------------------------------------------------------------------------

    #[test]
    fn injected_context_is_debug_formattable() {
        let ctx = InjectedContext::default();
        let debug_str = format!("{ctx:?}");
        // The struct name must appear in the debug output.
        assert!(
            debug_str.contains("InjectedContext"),
            "Debug output should contain 'InjectedContext', got: {debug_str}"
        );
    }

    // ---------------------------------------------------------------------------
    // T018: Round-trip serialization tests
    // ---------------------------------------------------------------------------

    #[test]
    fn injected_context_json_round_trip_nested() {
        let obs1 = make_observation();
        let obs2 = make_observation();
        let obs3 = make_observation();
        let summary = make_session_summary();

        let original = InjectedContext {
            recent_observations: vec![obs1, obs2],
            relevant_memories: vec![obs3],
            session_summaries: vec![summary],
            token_count: 1_024,
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: InjectedContext =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve nested Observation and SessionSummary instances"
        );
        assert_eq!(deserialized.recent_observations.len(), 2);
        assert_eq!(deserialized.relevant_memories.len(), 1);
        assert_eq!(deserialized.session_summaries.len(), 1);
        assert_eq!(deserialized.token_count, 1_024);
    }

    #[test]
    fn injected_context_json_round_trip_empty_default() {
        let original = InjectedContext::default();

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: InjectedContext =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "default InjectedContext must round-trip without data loss"
        );
        assert!(deserialized.recent_observations.is_empty());
        assert!(deserialized.relevant_memories.is_empty());
        assert!(deserialized.session_summaries.is_empty());
        assert_eq!(deserialized.token_count, 0);
    }
}
