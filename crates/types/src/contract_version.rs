//! Contract version validation result.
//!
//! [`ContractValidationResult`] captures the outcome of checking an event's
//! contract version against the version supported by the current adapter.
//! It is a simple pass/fail with an optional machine-readable reason string.

use serde::{Deserialize, Serialize};

/// The outcome of checking an event's contract version against the supported version.
///
/// When `compatible` is `true`, the event's contract version is accepted and
/// `reason` is `None`. When `compatible` is `false`, `reason` carries a
/// machine-readable explanation such as `"incompatible_contract_major"` or
/// `"invalid_contract_version"`.
///
/// Serialized with camelCase keys to match the project's JSON wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractValidationResult {
    /// Whether the event's contract version is compatible.
    pub compatible: bool,
    /// Machine-readable reason when incompatible, or `None` when compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Serde round-trip tests
    // -------------------------------------------------------------------------

    #[test]
    fn compatible_result_serde_round_trip() {
        let original = ContractValidationResult {
            compatible: true,
            reason: None,
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: ContractValidationResult =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve compatible result"
        );

        // Verify camelCase key naming and that reason is absent when None.
        assert!(
            json.contains("\"compatible\""),
            "JSON must contain key 'compatible', got: {json}"
        );
        assert!(
            !json.contains("\"reason\""),
            "JSON must NOT contain 'reason' when None (skip_serializing_if), got: {json}"
        );
    }

    #[test]
    fn incompatible_result_serde_round_trip() {
        let original = ContractValidationResult {
            compatible: false,
            reason: Some("incompatible_contract_major".to_string()),
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: ContractValidationResult =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve incompatible result with reason"
        );

        // Verify the reason value survives serialization.
        assert!(
            json.contains("\"incompatible_contract_major\""),
            "JSON must contain the reason string, got: {json}"
        );
    }

    #[test]
    fn invalid_version_result_serde_round_trip() {
        let original = ContractValidationResult {
            compatible: false,
            reason: Some("invalid_contract_version".to_string()),
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: ContractValidationResult =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve invalid-version result with reason"
        );

        // Verify the reason value survives serialization.
        assert!(
            json.contains("\"invalid_contract_version\""),
            "JSON must contain the reason string, got: {json}"
        );
    }

    // -------------------------------------------------------------------------
    // Derive verification
    // -------------------------------------------------------------------------

    #[test]
    fn derives_debug_clone_partialeq() {
        let result = ContractValidationResult {
            compatible: true,
            reason: None,
        };

        // Debug: format string must not panic.
        let debug_str = format!("{result:?}");
        assert!(
            debug_str.contains("ContractValidationResult"),
            "Debug output must contain type name, got: {debug_str}"
        );

        // Clone: cloned value must equal original.
        let cloned = result.clone();
        assert_eq!(result, cloned, "cloned value must equal original");

        // PartialEq: distinct instances with same data must be equal.
        let same = ContractValidationResult {
            compatible: true,
            reason: None,
        };
        assert_eq!(result, same, "identical values must be equal");

        // PartialEq: different data must not be equal.
        let different = ContractValidationResult {
            compatible: false,
            reason: Some("different".to_string()),
        };
        assert_ne!(result, different, "different values must not be equal");
    }
}
