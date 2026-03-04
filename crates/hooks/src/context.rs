use std::path::Path;

use types::{InjectedContext, MindStats};

/// Format an `InjectedContext` and `MindStats` into a system message string
/// suitable for injection into the agent's prompt.
#[must_use]
pub fn format_system_message(
    context: &InjectedContext,
    stats: &MindStats,
    memory_path: &Path,
) -> String {
    let mut parts = Vec::new();

    parts.push("# Claude Mind Active\n".to_string());
    parts.push(format!(
        "Memory: `{}` ({} KB)\n",
        memory_path.display(),
        stats.file_size_bytes / 1024
    ));

    // Stats
    parts.push(format!(
        "Observations: {} | Sessions: {}\n",
        stats.total_observations, stats.total_sessions
    ));

    // Recent observations
    if !context.recent_observations.is_empty() {
        parts.push("\n## Recent Observations\n".to_string());
        for obs in &context.recent_observations {
            parts.push(format!(
                "- [{}] {}: {}\n",
                obs.obs_type, obs.tool_name, obs.summary
            ));
        }
    }

    // Session summaries
    if !context.session_summaries.is_empty() {
        parts.push("\n## Session Summaries\n".to_string());
        for summary in &context.session_summaries {
            parts.push(format!("- {}\n", summary.summary));
        }
    }

    // Available commands
    parts.push("\n**Commands:**\n".to_string());
    parts.push("- `/mind:search <query>` - Search memories\n".to_string());
    parts.push("- `/mind:ask <question>` - Ask your memory\n".to_string());
    parts.push("- `/mind:recent` - View timeline\n".to_string());
    parts.push("- `/mind:stats` - View statistics\n".to_string());

    parts.join("")
}
