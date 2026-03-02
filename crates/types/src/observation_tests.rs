use super::*;
use crate::error::{RustyBrainError, error_codes};
use chrono::Utc;
use std::collections::HashMap;
use ulid::Ulid;

// -------------------------------------------------------------------------
// T005: ObservationType — all 10 variants constructable
// -------------------------------------------------------------------------

#[test]
fn observation_type_discovery_is_constructable() {
    let ot = ObservationType::Discovery;
    assert_eq!(ot, ObservationType::Discovery);
}

#[test]
fn observation_type_decision_is_constructable() {
    let ot = ObservationType::Decision;
    assert_eq!(ot, ObservationType::Decision);
}

#[test]
fn observation_type_problem_is_constructable() {
    let ot = ObservationType::Problem;
    assert_eq!(ot, ObservationType::Problem);
}

#[test]
fn observation_type_solution_is_constructable() {
    let ot = ObservationType::Solution;
    assert_eq!(ot, ObservationType::Solution);
}

#[test]
fn observation_type_pattern_is_constructable() {
    let ot = ObservationType::Pattern;
    assert_eq!(ot, ObservationType::Pattern);
}

#[test]
fn observation_type_warning_is_constructable() {
    let ot = ObservationType::Warning;
    assert_eq!(ot, ObservationType::Warning);
}

#[test]
fn observation_type_success_is_constructable() {
    let ot = ObservationType::Success;
    assert_eq!(ot, ObservationType::Success);
}

#[test]
fn observation_type_refactor_is_constructable() {
    let ot = ObservationType::Refactor;
    assert_eq!(ot, ObservationType::Refactor);
}

#[test]
fn observation_type_bugfix_is_constructable() {
    let ot = ObservationType::Bugfix;
    assert_eq!(ot, ObservationType::Bugfix);
}

#[test]
fn observation_type_feature_is_constructable() {
    let ot = ObservationType::Feature;
    assert_eq!(ot, ObservationType::Feature);
}

// -------------------------------------------------------------------------
// T005: ObservationType — Copy + Clone + Eq + Hash traits
// -------------------------------------------------------------------------

#[test]
fn observation_type_implements_copy() {
    let original = ObservationType::Discovery;
    let copied = original; // Copy: original still usable after move
    assert_eq!(original, copied);
}

#[test]
fn observation_type_implements_clone() {
    let original = ObservationType::Decision;
    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn observation_type_implements_eq() {
    assert_eq!(ObservationType::Problem, ObservationType::Problem);
    assert_ne!(ObservationType::Problem, ObservationType::Solution);
}

#[test]
fn observation_type_implements_hash() {
    let mut map: HashMap<ObservationType, u64> = HashMap::new();
    map.insert(ObservationType::Discovery, 1);
    map.insert(ObservationType::Feature, 2);
    assert_eq!(map.get(&ObservationType::Discovery), Some(&1));
    assert_eq!(map.get(&ObservationType::Feature), Some(&2));
    assert_eq!(map.get(&ObservationType::Bugfix), None);
}

#[test]
fn observation_type_all_variants_usable_as_hash_keys() {
    let mut map: HashMap<ObservationType, &str> = HashMap::new();
    map.insert(ObservationType::Discovery, "discovery");
    map.insert(ObservationType::Decision, "decision");
    map.insert(ObservationType::Problem, "problem");
    map.insert(ObservationType::Solution, "solution");
    map.insert(ObservationType::Pattern, "pattern");
    map.insert(ObservationType::Warning, "warning");
    map.insert(ObservationType::Success, "success");
    map.insert(ObservationType::Refactor, "refactor");
    map.insert(ObservationType::Bugfix, "bugfix");
    map.insert(ObservationType::Feature, "feature");
    assert_eq!(map.len(), 10);
}

// -------------------------------------------------------------------------
// T005: Observation — construction with all fields
// -------------------------------------------------------------------------

#[test]
fn observation_struct_fields_are_publicly_readable() {
    let id = Ulid::new();
    let timestamp = Utc::now();
    let obs = Observation {
        id,
        timestamp,
        obs_type: ObservationType::Discovery,
        tool_name: "bash".to_string(),
        summary: "Found a pattern".to_string(),
        content: Some("Detailed description of the discovery".to_string()),
        metadata: None,
    };
    assert_eq!(obs.id, id);
    assert_eq!(obs.timestamp, timestamp);
    assert_eq!(obs.obs_type, ObservationType::Discovery);
    assert_eq!(obs.tool_name, "bash");
    assert_eq!(obs.summary, "Found a pattern");
    assert_eq!(
        obs.content.as_deref(),
        Some("Detailed description of the discovery")
    );
    assert!(obs.metadata.is_none());
}

#[test]
fn observation_struct_fields_with_metadata() {
    let id = Ulid::new();
    let timestamp = Utc::now();
    let metadata = ObservationMetadata {
        files: vec!["src/main.rs".to_string()],
        platform: "darwin".to_string(),
        project_key: "rusty-brain".to_string(),
        compressed: false,
        session_id: Some("ses-001".to_string()),
        extra: HashMap::new(),
    };
    let obs = Observation {
        id,
        timestamp,
        obs_type: ObservationType::Bugfix,
        tool_name: "edit".to_string(),
        summary: "Fixed null pointer".to_string(),
        content: Some("Replaced unsafe deref with Option handling".to_string()),
        metadata: Some(metadata),
    };
    assert!(obs.metadata.is_some());
    let meta = obs.metadata.unwrap();
    assert_eq!(meta.files, vec!["src/main.rs"]);
    assert_eq!(meta.platform, "darwin");
    assert_eq!(meta.project_key, "rusty-brain");
    assert!(!meta.compressed);
    assert_eq!(meta.session_id, Some("ses-001".to_string()));
}

// -------------------------------------------------------------------------
// T005: Observation::new() — valid construction succeeds
// -------------------------------------------------------------------------

#[test]
fn observation_new_valid_without_metadata_succeeds() {
    let result = Observation::new(
        ObservationType::Discovery,
        "bash".to_string(),
        "A valid summary".to_string(),
        Some("A valid content body".to_string()),
        None,
    );
    assert!(result.is_ok());
    let obs = result.unwrap();
    assert_eq!(obs.obs_type, ObservationType::Discovery);
    assert_eq!(obs.tool_name, "bash");
    assert_eq!(obs.summary, "A valid summary");
    assert_eq!(obs.content.as_deref(), Some("A valid content body"));
    assert!(obs.metadata.is_none());
}

#[test]
fn observation_new_valid_with_metadata_succeeds() {
    let metadata = ObservationMetadata {
        files: vec!["src/lib.rs".to_string()],
        platform: "linux".to_string(),
        project_key: "rusty-brain".to_string(),
        compressed: true,
        session_id: None,
        extra: HashMap::new(),
    };
    let result = Observation::new(
        ObservationType::Feature,
        "write".to_string(),
        "Added new endpoint".to_string(),
        Some("Implemented the /store endpoint with validation".to_string()),
        Some(metadata),
    );
    assert!(result.is_ok());
    let obs = result.unwrap();
    assert!(obs.metadata.is_some());
}

#[test]
fn observation_new_auto_generates_uuid() {
    let obs1 = Observation::new(
        ObservationType::Success,
        "test".to_string(),
        "summary one".to_string(),
        Some("content one".to_string()),
        None,
    )
    .unwrap();
    let obs2 = Observation::new(
        ObservationType::Success,
        "test".to_string(),
        "summary two".to_string(),
        Some("content two".to_string()),
        None,
    )
    .unwrap();
    // Each call generates a distinct UUID
    assert_ne!(obs1.id, obs2.id);
}

#[test]
fn observation_new_auto_generates_timestamp() {
    let before = Utc::now();
    let obs = Observation::new(
        ObservationType::Pattern,
        "grep".to_string(),
        "Recurring pattern found".to_string(),
        Some("The pattern repeats across three files".to_string()),
        None,
    )
    .unwrap();
    let after = Utc::now();
    assert!(obs.timestamp >= before);
    assert!(obs.timestamp <= after);
}

// -------------------------------------------------------------------------
// T005: Observation::new() — rejects empty summary
// -------------------------------------------------------------------------

#[test]
fn observation_new_empty_summary_returns_invalid_input_error() {
    let result = Observation::new(
        ObservationType::Decision,
        "bash".to_string(),
        "".to_string(),
        Some("Valid content here".to_string()),
        None,
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, RustyBrainError::InvalidInput { .. }));
}

#[test]
fn observation_new_empty_summary_has_empty_field_error_code() {
    let result = Observation::new(
        ObservationType::Decision,
        "bash".to_string(),
        "".to_string(),
        Some("Valid content here".to_string()),
        None,
    );
    let err = result.unwrap_err();
    assert_eq!(err.code(), error_codes::E_INPUT_EMPTY_FIELD);
}

// -------------------------------------------------------------------------
// T005: Observation::new() — rejects whitespace-only summary
// -------------------------------------------------------------------------

#[test]
fn observation_new_whitespace_only_summary_returns_invalid_input_error() {
    let result = Observation::new(
        ObservationType::Warning,
        "bash".to_string(),
        "   ".to_string(),
        Some("Valid content here".to_string()),
        None,
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, RustyBrainError::InvalidInput { .. }));
}

#[test]
fn observation_new_whitespace_only_summary_has_empty_field_error_code() {
    let result = Observation::new(
        ObservationType::Warning,
        "bash".to_string(),
        "\t\n  \r".to_string(),
        Some("Valid content here".to_string()),
        None,
    );
    let err = result.unwrap_err();
    assert_eq!(err.code(), error_codes::E_INPUT_EMPTY_FIELD);
}

#[test]
fn observation_new_single_space_summary_returns_invalid_input_error() {
    let result = Observation::new(
        ObservationType::Problem,
        "read".to_string(),
        " ".to_string(),
        Some("Non-empty content".to_string()),
        None,
    );
    assert!(result.is_err());
}

// -------------------------------------------------------------------------
// T005: Observation::new() — empty/whitespace content normalized to None
// -------------------------------------------------------------------------

#[test]
fn observation_new_empty_content_normalized_to_none() {
    let obs = Observation::new(
        ObservationType::Solution,
        "bash".to_string(),
        "Valid summary".to_string(),
        Some("".to_string()),
        None,
    )
    .unwrap();
    assert!(obs.content.is_none());
}

#[test]
fn observation_new_whitespace_content_normalized_to_none() {
    let obs = Observation::new(
        ObservationType::Refactor,
        "edit".to_string(),
        "Valid summary".to_string(),
        Some("   ".to_string()),
        None,
    )
    .unwrap();
    assert!(obs.content.is_none());
}

#[test]
fn observation_new_none_content_stays_none() {
    let obs = Observation::new(
        ObservationType::Feature,
        "bash".to_string(),
        "Valid summary".to_string(),
        None,
        None,
    )
    .unwrap();
    assert!(obs.content.is_none());
}

#[test]
fn observation_new_newline_content_normalized_to_none() {
    let obs = Observation::new(
        ObservationType::Feature,
        "bash".to_string(),
        "Valid summary".to_string(),
        Some("\n".to_string()),
        None,
    )
    .unwrap();
    assert!(obs.content.is_none());
}

// -------------------------------------------------------------------------
// T005: Observation::new() — both summary and content invalid
// -------------------------------------------------------------------------

#[test]
fn observation_new_both_empty_returns_invalid_input_error() {
    let result = Observation::new(
        ObservationType::Problem,
        "bash".to_string(),
        "".to_string(),
        Some("".to_string()),
        None,
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, RustyBrainError::InvalidInput { .. }));
}

// -------------------------------------------------------------------------
// T005: Observation::new() — rejects empty tool_name
// -------------------------------------------------------------------------

#[test]
fn observation_new_empty_tool_name_returns_invalid_input_error() {
    let result = Observation::new(
        ObservationType::Discovery,
        "".to_string(),
        "Valid summary".to_string(),
        Some("Valid content".to_string()),
        None,
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, RustyBrainError::InvalidInput { .. }));
    assert_eq!(err.code(), error_codes::E_INPUT_EMPTY_FIELD);
}

#[test]
fn observation_new_whitespace_only_tool_name_returns_invalid_input_error() {
    let result = Observation::new(
        ObservationType::Discovery,
        "   \t\n".to_string(),
        "Valid summary".to_string(),
        Some("Valid content".to_string()),
        None,
    );
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, RustyBrainError::InvalidInput { .. }));
    assert_eq!(err.code(), error_codes::E_INPUT_EMPTY_FIELD);
}

// -------------------------------------------------------------------------
// T005: Observation — Clone + PartialEq traits
// -------------------------------------------------------------------------

#[test]
fn observation_implements_clone() {
    let obs = Observation::new(
        ObservationType::Discovery,
        "bash".to_string(),
        "Original summary".to_string(),
        Some("Original content".to_string()),
        None,
    )
    .unwrap();
    let cloned = obs.clone();
    assert_eq!(obs, cloned);
}

#[test]
fn observation_implements_partial_eq() {
    let id = Ulid::new();
    let timestamp = Utc::now();
    let obs1 = Observation {
        id,
        timestamp,
        obs_type: ObservationType::Success,
        tool_name: "test".to_string(),
        summary: "same summary".to_string(),
        content: Some("same content".to_string()),
        metadata: None,
    };
    let obs2 = Observation {
        id,
        timestamp,
        obs_type: ObservationType::Success,
        tool_name: "test".to_string(),
        summary: "same summary".to_string(),
        content: Some("same content".to_string()),
        metadata: None,
    };
    assert_eq!(obs1, obs2);
}

// -------------------------------------------------------------------------
// T005: ObservationMetadata — construction with defaults
// -------------------------------------------------------------------------

#[test]
fn observation_metadata_construction_with_default_values() {
    let metadata = ObservationMetadata {
        files: Vec::new(),
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra: HashMap::new(),
    };
    assert!(metadata.files.is_empty());
    assert!(metadata.platform.is_empty());
    assert!(metadata.project_key.is_empty());
    assert!(!metadata.compressed);
    assert!(metadata.session_id.is_none());
    assert!(metadata.extra.is_empty());
}

#[test]
fn observation_metadata_construction_with_all_fields() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("confidence".to_string(), serde_json::json!(0.95));
    extra.insert("tags".to_string(), serde_json::json!(["rust", "memory"]));

    let metadata = ObservationMetadata {
        files: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
        platform: "darwin".to_string(),
        project_key: "rusty-brain".to_string(),
        compressed: true,
        session_id: Some("session-abc-123".to_string()),
        extra,
    };
    assert_eq!(metadata.files.len(), 2);
    assert_eq!(metadata.platform, "darwin");
    assert_eq!(metadata.project_key, "rusty-brain");
    assert!(metadata.compressed);
    assert_eq!(metadata.session_id, Some("session-abc-123".to_string()));
    assert_eq!(metadata.extra.len(), 2);
}

// -------------------------------------------------------------------------
// T005: ObservationMetadata — extra map accepts arbitrary keys
// -------------------------------------------------------------------------

#[test]
fn observation_metadata_extra_accepts_string_value() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("custom_key".to_string(), serde_json::json!("custom_value"));
    let metadata = ObservationMetadata {
        files: Vec::new(),
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra,
    };
    assert_eq!(
        metadata.extra.get("custom_key"),
        Some(&serde_json::json!("custom_value"))
    );
}

#[test]
fn observation_metadata_extra_accepts_numeric_value() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("count".to_string(), serde_json::json!(42));
    let metadata = ObservationMetadata {
        files: Vec::new(),
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra,
    };
    assert_eq!(metadata.extra.get("count"), Some(&serde_json::json!(42)));
}

#[test]
fn observation_metadata_extra_accepts_boolean_value() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("enabled".to_string(), serde_json::json!(true));
    let metadata = ObservationMetadata {
        files: Vec::new(),
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra,
    };
    assert_eq!(
        metadata.extra.get("enabled"),
        Some(&serde_json::json!(true))
    );
}

#[test]
fn observation_metadata_extra_accepts_array_value() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert(
        "tags".to_string(),
        serde_json::json!(["rust", "async", "memory"]),
    );
    let metadata = ObservationMetadata {
        files: Vec::new(),
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra,
    };
    assert_eq!(
        metadata.extra.get("tags"),
        Some(&serde_json::json!(["rust", "async", "memory"]))
    );
}

#[test]
fn observation_metadata_extra_accepts_nested_object_value() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert(
        "diagnostics".to_string(),
        serde_json::json!({"cpu": 0.4, "mem_mb": 128}),
    );
    let metadata = ObservationMetadata {
        files: Vec::new(),
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra,
    };
    assert!(metadata.extra.contains_key("diagnostics"));
    let diag = metadata.extra.get("diagnostics").unwrap();
    assert_eq!(diag["cpu"], serde_json::json!(0.4));
    assert_eq!(diag["mem_mb"], serde_json::json!(128));
}

#[test]
fn observation_metadata_extra_accepts_null_value() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("optional_field".to_string(), serde_json::Value::Null);
    let metadata = ObservationMetadata {
        files: Vec::new(),
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra,
    };
    assert_eq!(
        metadata.extra.get("optional_field"),
        Some(&serde_json::Value::Null)
    );
}

#[test]
fn observation_metadata_extra_accepts_many_arbitrary_keys() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    for i in 0..20 {
        extra.insert(format!("key_{i}"), serde_json::json!(i));
    }
    let metadata = ObservationMetadata {
        files: Vec::new(),
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra,
    };
    assert_eq!(metadata.extra.len(), 20);
    assert_eq!(metadata.extra.get("key_0"), Some(&serde_json::json!(0)));
    assert_eq!(metadata.extra.get("key_19"), Some(&serde_json::json!(19)));
}

// -------------------------------------------------------------------------
// T005: ObservationMetadata — Clone + PartialEq traits
// -------------------------------------------------------------------------

#[test]
fn observation_metadata_implements_clone() {
    let metadata = ObservationMetadata {
        files: vec!["src/main.rs".to_string()],
        platform: "linux".to_string(),
        project_key: "proj".to_string(),
        compressed: false,
        session_id: Some("s1".to_string()),
        extra: HashMap::new(),
    };
    let cloned = metadata.clone();
    assert_eq!(metadata, cloned);
}

#[test]
fn observation_metadata_implements_partial_eq() {
    let meta1 = ObservationMetadata {
        files: vec!["a.rs".to_string()],
        platform: "darwin".to_string(),
        project_key: "p".to_string(),
        compressed: false,
        session_id: None,
        extra: HashMap::new(),
    };
    let meta2 = ObservationMetadata {
        files: vec!["a.rs".to_string()],
        platform: "darwin".to_string(),
        project_key: "p".to_string(),
        compressed: false,
        session_id: None,
        extra: HashMap::new(),
    };
    assert_eq!(meta1, meta2);
}

// -------------------------------------------------------------------------
// T005: Observation::new() — all ObservationType variants accepted
// -------------------------------------------------------------------------

#[test]
fn observation_new_accepts_all_observation_type_variants() {
    let variants = [
        ObservationType::Discovery,
        ObservationType::Decision,
        ObservationType::Problem,
        ObservationType::Solution,
        ObservationType::Pattern,
        ObservationType::Warning,
        ObservationType::Success,
        ObservationType::Refactor,
        ObservationType::Bugfix,
        ObservationType::Feature,
    ];
    for variant in variants {
        let result = Observation::new(
            variant,
            "tool".to_string(),
            "Non-empty summary".to_string(),
            Some("Non-empty content".to_string()),
            None,
        );
        assert!(
            result.is_ok(),
            "Observation::new should succeed for variant {:?}",
            variant
        );
        assert_eq!(result.unwrap().obs_type, variant);
    }
}

// -------------------------------------------------------------------------
// T016: Round-trip serialization tests
// -------------------------------------------------------------------------

#[test]
fn observation_json_round_trip_all_fields() {
    let mut extra: HashMap<String, serde_json::Value> = HashMap::new();
    extra.insert("confidence".to_string(), serde_json::json!(0.87));
    extra.insert("tags".to_string(), serde_json::json!(["rust", "memory"]));

    let metadata = ObservationMetadata {
        files: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
        platform: "darwin".to_string(),
        project_key: "rusty-brain".to_string(),
        compressed: true,
        session_id: Some("session-abc-123".to_string()),
        extra,
    };

    let id = Ulid::new();
    let timestamp = Utc::now();
    let original = Observation {
        id,
        timestamp,
        obs_type: ObservationType::Discovery,
        tool_name: "bash".to_string(),
        summary: "Found an important pattern".to_string(),
        content: Some("The pattern repeats across multiple files".to_string()),
        metadata: Some(metadata),
    };

    let json = serde_json::to_string(&original).expect("serialization must succeed");
    let deserialized: Observation =
        serde_json::from_str(&json).expect("deserialization must succeed");

    assert_eq!(
        original, deserialized,
        "round-trip must preserve all fields"
    );
}

#[test]
fn observation_json_round_trip_unicode() {
    let id = Ulid::new();
    let timestamp = Utc::now();
    let original = Observation {
        id,
        timestamp,
        obs_type: ObservationType::Pattern,
        tool_name: "grep".to_string(),
        summary: "Brain emoji 🧠 and CJK 你好世界".to_string(),
        content: Some("RTL text مرحبا and more CJK 日本語テスト".to_string()),
        metadata: None,
    };

    let json = serde_json::to_string(&original).expect("serialization must succeed");
    let deserialized: Observation =
        serde_json::from_str(&json).expect("deserialization must succeed");

    assert_eq!(
        original, deserialized,
        "round-trip must preserve unicode in summary and content"
    );
    assert!(
        deserialized.summary.contains('🧠'),
        "emoji must survive round-trip"
    );
    assert!(
        deserialized.summary.contains("你好世界"),
        "CJK must survive round-trip"
    );
    assert!(
        deserialized.content.as_ref().unwrap().contains("مرحبا"),
        "RTL text must survive round-trip"
    );
}

#[test]
fn observation_metadata_round_trip_nested_extra() {
    let extra = {
        let mut map: HashMap<String, serde_json::Value> = HashMap::new();
        map.insert(
            "level1".to_string(),
            serde_json::json!({
                "level2": {
                    "level3": {
                        "level4": {
                            "level5": "deep_value"
                        }
                    }
                }
            }),
        );
        map.insert("another_key".to_string(), serde_json::json!(42));
        map.insert("third_key".to_string(), serde_json::json!(true));
        map.insert("fourth_key".to_string(), serde_json::json!([1, 2, 3]));
        map.insert("fifth_key".to_string(), serde_json::json!(null));
        map
    };

    let original = ObservationMetadata {
        files: vec!["src/lib.rs".to_string()],
        platform: "linux".to_string(),
        project_key: "proj".to_string(),
        compressed: false,
        session_id: None,
        extra,
    };

    let json = serde_json::to_string(&original).expect("serialization must succeed");
    let deserialized: ObservationMetadata =
        serde_json::from_str(&json).expect("deserialization must succeed");

    assert_eq!(
        original, deserialized,
        "round-trip must preserve deeply nested extra fields"
    );

    let level1 = deserialized
        .extra
        .get("level1")
        .expect("level1 must be present");
    assert_eq!(
        level1["level2"]["level3"]["level4"]["level5"],
        serde_json::json!("deep_value"),
        "5 levels of nesting must survive round-trip"
    );
}

#[test]
fn observation_metadata_round_trip_empty_extra() {
    let original = ObservationMetadata {
        files: vec![],
        platform: String::new(),
        project_key: String::new(),
        compressed: false,
        session_id: None,
        extra: HashMap::new(),
    };

    let json = serde_json::to_string(&original).expect("serialization must succeed");
    let deserialized: ObservationMetadata =
        serde_json::from_str(&json).expect("deserialization must succeed");

    assert_eq!(
        original, deserialized,
        "round-trip must preserve empty extra map"
    );
    assert!(
        deserialized.extra.is_empty(),
        "empty extra map must remain empty after round-trip"
    );
}
