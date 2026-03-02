//! Project context and identity types for the platform adapter system.
//!
//! [`ProjectContext`] captures information about which project a session belongs
//! to — platform project ID, canonical path, and current working directory. All
//! fields are optional because different platforms provide different subsets.
//!
//! [`ProjectIdentity`] is the resolved unique key for a project, derived from
//! the project context during identity resolution. [`IdentitySource`] records
//! which resolution method produced the key.

use serde::{Deserialize, Serialize};

/// Information about which project a session belongs to.
///
/// All fields are optional because different platforms provide different
/// subsets of project information. Populated from hook input during
/// normalization (FR-003).
///
/// Serialized with camelCase keys.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContext {
    /// Platform-specific project identifier (e.g., repository ID).
    pub platform_project_id: Option<String>,
    /// Canonical filesystem path to the project root.
    pub canonical_path: Option<String>,
    /// Current working directory at the time of the hook invocation.
    pub cwd: Option<String>,
}

/// The resolved unique key for a project, derived from project context.
///
/// Identity resolution produces this by selecting the best available
/// identifier from a [`ProjectContext`]. The `key` is `None` when
/// resolution fails (source will be [`IdentitySource::Unresolved`]).
///
/// Serialized with camelCase keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdentity {
    /// The resolved identity string, or `None` if unresolved (FR-010).
    pub key: Option<String>,
    /// Which resolution method was used (FR-011).
    pub source: IdentitySource,
}

/// Which resolution method was used to derive the project identity.
///
/// Marked `#[non_exhaustive]` to allow future resolution strategies
/// without breaking downstream matches.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentitySource {
    /// Identity was derived from a platform-specific project ID.
    PlatformProjectId,
    /// Identity was derived from the canonical filesystem path.
    CanonicalPath,
    /// Identity was derived from the current working directory.
    Cwd,
    /// No identity could be resolved.
    Unresolved,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // T005: Unit tests for ProjectContext, ProjectIdentity, IdentitySource
    //
    // Tests written FIRST (RED phase), then struct/enum definitions added above
    // to make them compile and pass (GREEN phase).
    // -------------------------------------------------------------------------

    // -------------------------------------------------------------------------
    // 1. ProjectContext::default() has all fields None
    // -------------------------------------------------------------------------

    #[test]
    fn project_context_default_has_all_none() {
        let ctx = ProjectContext::default();
        assert!(
            ctx.platform_project_id.is_none(),
            "platform_project_id should be None by default"
        );
        assert!(
            ctx.canonical_path.is_none(),
            "canonical_path should be None by default"
        );
        assert!(ctx.cwd.is_none(), "cwd should be None by default");
    }

    // -------------------------------------------------------------------------
    // 2. ProjectContext serde round-trip with all fields Some
    // -------------------------------------------------------------------------

    #[test]
    fn project_context_serde_round_trip() {
        let original = ProjectContext {
            platform_project_id: Some("proj-42".to_string()),
            canonical_path: Some("/home/user/my-project".to_string()),
            cwd: Some("/home/user/my-project/src".to_string()),
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: ProjectContext =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve all ProjectContext fields"
        );

        // Verify camelCase field names in JSON output.
        assert!(
            json.contains("platformProjectId"),
            "JSON should use camelCase key 'platformProjectId', got: {json}"
        );
        assert!(
            json.contains("canonicalPath"),
            "JSON should use camelCase key 'canonicalPath', got: {json}"
        );
    }

    // -------------------------------------------------------------------------
    // 3. ProjectContext serde with all None fields
    // -------------------------------------------------------------------------

    #[test]
    fn project_context_serde_with_none_fields() {
        let original = ProjectContext {
            platform_project_id: None,
            canonical_path: None,
            cwd: None,
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: ProjectContext =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve None fields"
        );

        // None fields serialize as null in JSON.
        assert!(
            json.contains("null"),
            "None fields should serialize as null, got: {json}"
        );
    }

    // -------------------------------------------------------------------------
    // 4. ProjectIdentity serde round-trip (key=Some, source=PlatformProjectId)
    // -------------------------------------------------------------------------

    #[test]
    fn project_identity_serde_round_trip() {
        let original = ProjectIdentity {
            key: Some("proj-42".to_string()),
            source: IdentitySource::PlatformProjectId,
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: ProjectIdentity =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve ProjectIdentity fields"
        );

        // Verify snake_case variant name in JSON output.
        assert!(
            json.contains("platform_project_id"),
            "IdentitySource::PlatformProjectId should serialize as 'platform_project_id', got: {json}"
        );
    }

    // -------------------------------------------------------------------------
    // 5. ProjectIdentity unresolved serde round-trip (key=None, source=Unresolved)
    // -------------------------------------------------------------------------

    #[test]
    fn project_identity_unresolved_serde_round_trip() {
        let original = ProjectIdentity {
            key: None,
            source: IdentitySource::Unresolved,
        };

        let json = serde_json::to_string(&original).expect("serialization must succeed");
        let deserialized: ProjectIdentity =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert_eq!(
            original, deserialized,
            "round-trip must preserve unresolved identity"
        );

        // Verify snake_case variant name in JSON output.
        assert!(
            json.contains("unresolved"),
            "IdentitySource::Unresolved should serialize as 'unresolved', got: {json}"
        );
    }

    // -------------------------------------------------------------------------
    // 6. All IdentitySource variants can be constructed
    // -------------------------------------------------------------------------

    #[test]
    fn identity_source_all_variants_constructable() {
        let platform = IdentitySource::PlatformProjectId;
        let canonical = IdentitySource::CanonicalPath;
        let cwd = IdentitySource::Cwd;
        let unresolved = IdentitySource::Unresolved;

        // Verify each variant is distinct.
        assert_ne!(platform, canonical);
        assert_ne!(platform, cwd);
        assert_ne!(platform, unresolved);
        assert_ne!(canonical, cwd);
        assert_ne!(canonical, unresolved);
        assert_ne!(cwd, unresolved);

        // Verify serde round-trip for each variant independently.
        for variant in [&platform, &canonical, &cwd, &unresolved] {
            let json = serde_json::to_string(variant).expect("serialization must succeed");
            let deserialized: IdentitySource =
                serde_json::from_str(&json).expect("deserialization must succeed");
            assert_eq!(
                variant, &deserialized,
                "variant round-trip failed for {json}"
            );
        }
    }

    // -------------------------------------------------------------------------
    // 7. ProjectContext derives Debug and Clone
    // -------------------------------------------------------------------------

    #[test]
    fn project_context_derives_debug_clone() {
        let ctx = ProjectContext {
            platform_project_id: Some("proj-99".to_string()),
            canonical_path: Some("/tmp/test".to_string()),
            cwd: Some("/tmp/test/sub".to_string()),
        };

        // Verify Debug.
        let debug_str = format!("{ctx:?}");
        assert!(
            debug_str.contains("ProjectContext"),
            "Debug output should contain 'ProjectContext', got: {debug_str}"
        );

        // Verify Clone produces an equal copy.
        let cloned = ctx.clone();
        assert_eq!(ctx, cloned, "cloned ProjectContext must equal original");
    }
}
