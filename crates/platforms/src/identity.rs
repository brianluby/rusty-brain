//! Project identity resolution from project context.
//!
//! Resolves a unique project key from the available fields in a
//! [`ProjectContext`], following the priority order defined in FR-010:
//! platform project ID > canonical path > cwd > unresolved.
//!
//! No filesystem I/O is performed — path strings are used as provided.

use types::{IdentitySource, ProjectContext, ProjectIdentity};

/// Resolve a project identity from a project context.
///
/// Priority (FR-010):
/// 1. `platform_project_id` (if present, non-empty, non-whitespace)
/// 2. `canonical_path` (if present, non-empty)
/// 3. `cwd` (if present, non-empty)
/// 4. Unresolved (`key=None`)
///
/// No filesystem I/O — path strings are used as provided.
#[must_use]
pub fn resolve_project_identity(context: &ProjectContext) -> ProjectIdentity {
    // 1. Check platform_project_id (trimmed; empty/whitespace-only treated as absent).
    if let Some(ref id) = context.platform_project_id {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            return ProjectIdentity {
                key: Some(trimmed.to_string()),
                source: IdentitySource::PlatformProjectId,
            };
        }
    }

    // 2. Check canonical_path.
    if let Some(ref path) = context.canonical_path {
        if !path.is_empty() {
            return ProjectIdentity {
                key: Some(path.clone()),
                source: IdentitySource::CanonicalPath,
            };
        }
    }

    // 3. Check cwd.
    if let Some(ref cwd) = context.cwd {
        if !cwd.is_empty() {
            return ProjectIdentity {
                key: Some(cwd.clone()),
                source: IdentitySource::Cwd,
            };
        }
    }

    // 4. Unresolved.
    ProjectIdentity {
        key: None,
        source: IdentitySource::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // T019: Failing tests for resolve_project_identity (RED phase)
    // -------------------------------------------------------------------------

    // 1. platform_project_id present → key=Some("proj-42"), source=PlatformProjectId
    #[test]
    fn platform_project_id_present() {
        let ctx = ProjectContext {
            platform_project_id: Some("proj-42".to_string()),
            canonical_path: None,
            cwd: None,
        };

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, Some("proj-42".to_string()));
        assert_eq!(identity.source, IdentitySource::PlatformProjectId);
    }

    // 2. canonical_path fallback when platform_project_id is None
    #[test]
    fn canonical_path_fallback() {
        let ctx = ProjectContext {
            platform_project_id: None,
            canonical_path: Some("/home/user/project".to_string()),
            cwd: None,
        };

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, Some("/home/user/project".to_string()));
        assert_eq!(identity.source, IdentitySource::CanonicalPath);
    }

    // 3. cwd fallback when both platform_project_id and canonical_path are None
    #[test]
    fn cwd_fallback() {
        let ctx = ProjectContext {
            platform_project_id: None,
            canonical_path: None,
            cwd: Some("/home/user/project".to_string()),
        };

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, Some("/home/user/project".to_string()));
        assert_eq!(identity.source, IdentitySource::Cwd);
    }

    // 4. All fields None → key=None, source=Unresolved
    #[test]
    fn nothing_present() {
        let ctx = ProjectContext::default();

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, None);
        assert_eq!(identity.source, IdentitySource::Unresolved);
    }

    // 5. All three fields set → platform_project_id takes priority
    #[test]
    fn platform_project_id_takes_priority() {
        let ctx = ProjectContext {
            platform_project_id: Some("proj-42".to_string()),
            canonical_path: Some("/home/user/project".to_string()),
            cwd: Some("/tmp/other".to_string()),
        };

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, Some("proj-42".to_string()));
        assert_eq!(identity.source, IdentitySource::PlatformProjectId);
    }

    // 6. canonical_path and cwd both set → canonical_path takes priority over cwd
    #[test]
    fn canonical_path_takes_priority_over_cwd() {
        let ctx = ProjectContext {
            platform_project_id: None,
            canonical_path: Some("/home/user/project".to_string()),
            cwd: Some("/tmp/other".to_string()),
        };

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, Some("/home/user/project".to_string()));
        assert_eq!(identity.source, IdentitySource::CanonicalPath);
    }

    // 7. Empty string platform_project_id treated as absent → falls through
    #[test]
    fn empty_string_project_id_treated_as_absent() {
        let ctx = ProjectContext {
            platform_project_id: Some(String::new()),
            canonical_path: Some("/home/user/project".to_string()),
            cwd: None,
        };

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, Some("/home/user/project".to_string()));
        assert_eq!(identity.source, IdentitySource::CanonicalPath);
    }

    // 8. Whitespace-only platform_project_id treated as absent → falls through
    #[test]
    fn whitespace_only_project_id_treated_as_absent() {
        let ctx = ProjectContext {
            platform_project_id: Some("   ".to_string()),
            canonical_path: Some("/home/user/project".to_string()),
            cwd: None,
        };

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, Some("/home/user/project".to_string()));
        assert_eq!(identity.source, IdentitySource::CanonicalPath);
    }

    // 9. Nonexistent cwd used as-is (no filesystem I/O)
    #[test]
    fn nonexistent_cwd_used_as_is() {
        let ctx = ProjectContext {
            platform_project_id: None,
            canonical_path: None,
            cwd: Some("/nonexistent/path/abc123".to_string()),
        };

        let identity = resolve_project_identity(&ctx);

        assert_eq!(identity.key, Some("/nonexistent/path/abc123".to_string()));
        assert_eq!(identity.source, IdentitySource::Cwd);
    }
}
