// Contract: Updated path_policy.rs API
// Feature: 012-default-memory-path

/// New canonical memory directory name.
pub const DEFAULT_MEMORY_DIR: &str = ".rusty-brain";

/// Default memory file path relative to project root (new canonical).
const DEFAULT_MEMORY_PATH: &str = ".rusty-brain/mind.mv2";

/// Legacy memory file path from the agent-brain era.
pub const LEGACY_AGENT_BRAIN_PATH: &str = ".agent-brain/mind.mv2";

/// Oldest legacy memory file path from early Claude Code integrations.
pub const LEGACY_CLAUDE_MEMORY_PATH: &str = ".claude/mind.mv2";

/// All legacy paths in detection order (newest legacy first).
pub const LEGACY_PATHS: &[&str] = &[LEGACY_AGENT_BRAIN_PATH, LEGACY_CLAUDE_MEMORY_PATH];

/// Format migration instructions for a detected legacy path.
///
/// Returns actionable shell commands the user can run to migrate.
#[must_use]
pub fn format_migration_instructions(legacy_path: &str, canonical_path: &std::path::Path) -> String {
    todo!()
}
