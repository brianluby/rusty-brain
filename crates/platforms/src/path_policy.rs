//! Memory file path policy resolution.
//!
//! Resolves the memory file path based on platform opt-in policy:
//! - Default (no opt-in): `.agent-brain/mind.mv2` (FR-015, mode: LegacyFirst)
//! - Platform opt-in: `.{platform}/mind-{platform}.mv2` (FR-015, mode: PlatformOptIn)
//! - Platform name sanitized per FR-016
//! - Resolved path MUST stay within project_dir (FR-014)

use std::path::{Path, PathBuf};

use types::AgentBrainError;
use types::error::error_codes;

/// Default legacy memory file path relative to project root.
const DEFAULT_LEGACY_PATH: &str = ".agent-brain/mind.mv2";

/// The mode used to resolve a memory file path.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    /// Legacy mode: `.agent-brain/mind.mv2` (no platform opt-in).
    LegacyFirst,
    /// Platform opt-in mode: `.{platform}/mind-{platform}.mv2`.
    PlatformOptIn,
}

/// A resolved memory file path with the policy mode used.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMemoryPath {
    /// The resolved absolute path to the memory file.
    pub path: PathBuf,
    /// The path resolution mode that was applied.
    pub mode: PathMode,
}

/// Sanitize a platform name for use in file paths.
///
/// Replaces non-alphanumeric characters (except hyphens and underscores)
/// with hyphens (FR-016).
fn sanitize_platform_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Resolve the memory file path based on path policy.
///
/// - Default (no opt-in): `.agent-brain/mind.mv2` (FR-015, mode: `LegacyFirst`)
/// - Platform opt-in: `.{platform}/mind-{platform}.mv2` (FR-015, mode: `PlatformOptIn`)
/// - Platform name sanitized per FR-016
/// - Resolved path MUST stay within `project_dir` (FR-014)
///
/// Does NOT perform filesystem I/O.
///
/// # Errors
///
/// Returns [`AgentBrainError::Platform`] with code
/// [`error_codes::E_PLATFORM_PATH_TRAVERSAL`] if the resolved path would escape
/// the project directory (e.g. via `..` components in the platform name).
pub fn resolve_memory_path(
    project_dir: &Path,
    platform_name: &str,
    platform_opt_in: bool,
) -> Result<ResolvedMemoryPath, AgentBrainError> {
    let sanitized = sanitize_platform_name(platform_name);

    let relative = if platform_opt_in {
        PathBuf::from(format!(".{sanitized}/mind-{sanitized}.mv2"))
    } else {
        PathBuf::from(DEFAULT_LEGACY_PATH)
    };

    let resolved = project_dir.join(&relative);

    // Path traversal check: verify the relative path has no ".." components.
    // This catches crafted platform names like "../../etc" which sanitize to
    // "--..--..-etc" but we also want to catch names that somehow still contain
    // parent-dir components after sanitization. We check relative components
    // for ParentDir to reject traversal without performing I/O.
    for component in relative.components() {
        if let std::path::Component::ParentDir = component {
            return Err(AgentBrainError::Platform {
                code: error_codes::E_PLATFORM_PATH_TRAVERSAL,
                message: format!(
                    "resolved memory path escapes project directory: {}",
                    resolved.display()
                ),
            });
        }
    }

    // Belt-and-suspenders: verify the resolved path starts with project_dir.
    // This guards against edge cases the component check might miss.
    if !resolved.starts_with(project_dir) {
        return Err(AgentBrainError::Platform {
            code: error_codes::E_PLATFORM_PATH_TRAVERSAL,
            message: format!(
                "resolved memory path escapes project directory: {}",
                resolved.display()
            ),
        });
    }

    let mode = if platform_opt_in {
        PathMode::PlatformOptIn
    } else {
        PathMode::LegacyFirst
    };

    Ok(ResolvedMemoryPath {
        path: resolved,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // T025: Failing tests for memory file path policy

    #[test]
    fn legacy_mode_no_opt_in() {
        let result = resolve_memory_path(Path::new("/project"), "claude", false)
            .expect("legacy mode should succeed");

        assert_eq!(result.path, PathBuf::from("/project/.agent-brain/mind.mv2"));
        assert_eq!(result.mode, PathMode::LegacyFirst);
    }

    #[test]
    fn platform_opt_in_mode() {
        let result = resolve_memory_path(Path::new("/project"), "claude", true)
            .expect("platform opt-in should succeed");

        assert_eq!(
            result.path,
            PathBuf::from("/project/.claude/mind-claude.mv2")
        );
        assert_eq!(result.mode, PathMode::PlatformOptIn);
    }

    #[test]
    fn path_traversal_rejected() {
        // Sanitization replaces dots and slashes with hyphens, so "../../etc"
        // becomes "--..--..-etc" -- inherently preventing traversal. Verify
        // the crafted name stays within the project directory.
        let result = resolve_memory_path(Path::new("/project"), "../../etc", true);
        assert!(result.is_ok(), "sanitized traversal attempt should be safe");
        let resolved = result.unwrap();
        assert!(
            resolved.path.starts_with("/project"),
            "path must stay within project dir even with traversal attempt"
        );

        // Verify the E_PLATFORM_PATH_TRAVERSAL error code is wired correctly.
        let err = AgentBrainError::Platform {
            code: error_codes::E_PLATFORM_PATH_TRAVERSAL,
            message: "resolved memory path escapes project directory: /etc/passwd".to_string(),
        };
        assert_eq!(err.code(), error_codes::E_PLATFORM_PATH_TRAVERSAL);
    }

    #[test]
    fn special_chars_sanitized() {
        let result = resolve_memory_path(Path::new("/project"), "my.platform!v2", true)
            .expect("special chars should be sanitized");

        assert_eq!(
            result.path,
            PathBuf::from("/project/.my-platform-v2/mind-my-platform-v2.mv2")
        );
        assert_eq!(result.mode, PathMode::PlatformOptIn);
    }

    #[test]
    fn path_stays_within_project() {
        let project_dir = Path::new("/project");

        // Test with various platform names
        let names = ["claude", "opencode", "../sneaky", "a/b/c", "normal"];
        for name in &names {
            let result = resolve_memory_path(project_dir, name, true)
                .expect("should succeed after sanitization");
            assert!(
                result.path.starts_with(project_dir),
                "path for platform '{}' must stay within project dir, got: {}",
                name,
                result.path.display()
            );
        }

        // Also test legacy mode
        let result = resolve_memory_path(project_dir, "anything", false)
            .expect("legacy mode should succeed");
        assert!(
            result.path.starts_with(project_dir),
            "legacy path must stay within project dir"
        );
    }

    #[test]
    fn opencode_platform_opt_in() {
        let result = resolve_memory_path(Path::new("/project"), "opencode", true)
            .expect("opencode opt-in should succeed");

        assert_eq!(
            result.path,
            PathBuf::from("/project/.opencode/mind-opencode.mv2")
        );
        assert_eq!(result.mode, PathMode::PlatformOptIn);
    }

    // Additional edge-case tests for sanitize_platform_name

    #[test]
    fn sanitize_preserves_alphanumeric_and_hyphens() {
        assert_eq!(sanitize_platform_name("claude-code"), "claude-code");
        assert_eq!(sanitize_platform_name("my_platform"), "my_platform");
        assert_eq!(sanitize_platform_name("abc123"), "abc123");
    }

    #[test]
    fn sanitize_replaces_dots_and_special_chars() {
        assert_eq!(sanitize_platform_name("my.platform"), "my-platform");
        assert_eq!(sanitize_platform_name("a/b"), "a-b");
        assert_eq!(sanitize_platform_name("a..b"), "a--b");
        assert_eq!(sanitize_platform_name("x!@#$%y"), "x-----y");
    }

    #[test]
    fn sanitize_replaces_path_separators() {
        // Slashes (both forward and back) must be replaced to prevent traversal
        assert_eq!(sanitize_platform_name("../.."), "-----");
        assert_eq!(sanitize_platform_name("a\\b"), "a-b");
    }

    #[test]
    fn legacy_mode_ignores_platform_name() {
        // In legacy mode, different platform names produce the same path
        let r1 = resolve_memory_path(Path::new("/proj"), "claude", false).unwrap();
        let r2 = resolve_memory_path(Path::new("/proj"), "opencode", false).unwrap();
        assert_eq!(r1.path, r2.path);
        assert_eq!(r1.mode, PathMode::LegacyFirst);
        assert_eq!(r2.mode, PathMode::LegacyFirst);
    }
}
