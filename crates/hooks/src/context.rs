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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use types::observation::{Observation, ObservationType};
    use types::session::SessionSummary;

    fn make_empty_stats() -> MindStats {
        MindStats {
            total_observations: 0,
            total_sessions: 0,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes: 0,
            type_counts: HashMap::new(),
        }
    }

    fn make_stats(observations: u64, sessions: u64, file_size_bytes: u64) -> MindStats {
        MindStats {
            total_observations: observations,
            total_sessions: sessions,
            oldest_memory: None,
            newest_memory: None,
            file_size_bytes,
            type_counts: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // format_system_message — basic structure
    // -----------------------------------------------------------------------

    #[test]
    fn format_system_message_contains_header() {
        let ctx = InjectedContext::default();
        let stats = make_empty_stats();
        let path = PathBuf::from("/tmp/mind.mv2");

        let msg = format_system_message(&ctx, &stats, &path);
        assert!(
            msg.contains("# Claude Mind Active"),
            "message should contain header"
        );
    }

    #[test]
    fn format_system_message_contains_memory_path() {
        let ctx = InjectedContext::default();
        let stats = make_empty_stats();
        let path = PathBuf::from("/home/user/.agent-brain/mind.mv2");

        let msg = format_system_message(&ctx, &stats, &path);
        assert!(
            msg.contains("/home/user/.agent-brain/mind.mv2"),
            "message should contain the memory path"
        );
    }

    #[test]
    fn format_system_message_shows_file_size_in_kb() {
        let ctx = InjectedContext::default();
        let stats = make_stats(10, 2, 8192);
        let path = PathBuf::from("/tmp/mind.mv2");

        let msg = format_system_message(&ctx, &stats, &path);
        // 8192 / 1024 = 8
        assert!(
            msg.contains("8 KB"),
            "message should show file size in KB, got: {msg}"
        );
    }

    #[test]
    fn format_system_message_shows_observation_and_session_counts() {
        let ctx = InjectedContext::default();
        let stats = make_stats(42, 7, 1024);
        let path = PathBuf::from("/tmp/mind.mv2");

        let msg = format_system_message(&ctx, &stats, &path);
        assert!(
            msg.contains("Observations: 42"),
            "should show observation count"
        );
        assert!(msg.contains("Sessions: 7"), "should show session count");
    }

    #[test]
    fn format_system_message_includes_recent_observations() {
        let obs = Observation::new(
            ObservationType::Discovery,
            "Read".to_string(),
            "Read /src/main.rs".to_string(),
            None,
            None,
        )
        .expect("valid observation");

        let ctx = InjectedContext {
            recent_observations: vec![obs],
            relevant_memories: vec![],
            session_summaries: vec![],
            token_count: 0,
        };
        let stats = make_empty_stats();
        let path = PathBuf::from("/tmp/mind.mv2");

        let msg = format_system_message(&ctx, &stats, &path);
        assert!(
            msg.contains("## Recent Observations"),
            "should contain observations header"
        );
        assert!(
            msg.contains("Read /src/main.rs"),
            "should contain observation summary"
        );
    }

    #[test]
    fn format_system_message_omits_observations_section_when_empty() {
        let ctx = InjectedContext::default();
        let stats = make_empty_stats();
        let path = PathBuf::from("/tmp/mind.mv2");

        let msg = format_system_message(&ctx, &stats, &path);
        assert!(
            !msg.contains("## Recent Observations"),
            "should not contain observations header when empty"
        );
    }

    #[test]
    fn format_system_message_includes_session_summaries() {
        let summary = SessionSummary::new(
            "sess-001".to_string(),
            chrono::Utc::now(),
            chrono::Utc::now(),
            0,
            vec![],
            vec![],
            "Worked on feature X".to_string(),
        )
        .expect("valid session summary");

        let ctx = InjectedContext {
            recent_observations: vec![],
            relevant_memories: vec![],
            session_summaries: vec![summary],
            token_count: 0,
        };
        let stats = make_empty_stats();
        let path = PathBuf::from("/tmp/mind.mv2");

        let msg = format_system_message(&ctx, &stats, &path);
        assert!(
            msg.contains("## Session Summaries"),
            "should contain session summaries header"
        );
        assert!(
            msg.contains("Worked on feature X"),
            "should contain session summary text"
        );
    }

    #[test]
    fn format_system_message_includes_commands() {
        let ctx = InjectedContext::default();
        let stats = make_empty_stats();
        let path = PathBuf::from("/tmp/mind.mv2");

        let msg = format_system_message(&ctx, &stats, &path);
        assert!(msg.contains("/mind:search"), "should list search command");
        assert!(msg.contains("/mind:ask"), "should list ask command");
        assert!(msg.contains("/mind:recent"), "should list recent command");
        assert!(msg.contains("/mind:stats"), "should list stats command");
    }
}
