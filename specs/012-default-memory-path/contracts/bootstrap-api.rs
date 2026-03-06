// Contract: Updated bootstrap.rs API
// Feature: 012-default-memory-path

use std::path::Path;

/// Diagnostic produced by legacy path detection.
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
}

pub enum DiagnosticLevel {
    Warning,
    Info,
}

/// Detect legacy memory paths and produce diagnostics.
///
/// Checks for `.agent-brain/mind.mv2` and `.claude/mind.mv2` relative to
/// `project_root`. Returns diagnostics with migration instructions pointing
/// to `.rusty-brain/mind.mv2`.
///
/// Returns `Vec<Diagnostic>` (changed from `Option<Diagnostic>`) to support
/// multiple legacy paths detected simultaneously.
#[must_use]
pub fn detect_legacy_paths(project_root: &Path) -> Vec<Diagnostic> {
    todo!()
}

/// Resolve the effective memory path, falling back to `.agent-brain/` if
/// `.rusty-brain/` doesn't exist yet.
///
/// Resolution order:
/// 1. `.rusty-brain/mind.mv2` — used if the file or directory exists
/// 2. `.agent-brain/mind.mv2` — used if exists and `.rusty-brain/` doesn't
/// 3. `.rusty-brain/mind.mv2` — returned as default for new installations
///
/// Does NOT create directories. Returns the path to use; caller decides
/// whether to create on write.
#[must_use]
pub fn resolve_effective_path(project_root: &Path) -> std::path::PathBuf {
    todo!()
}
