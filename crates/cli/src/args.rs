//! CLI argument definitions using clap derive macros.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use types::ObservationType;

#[derive(Parser)]
#[command(
    name = "rusty-brain",
    about = "Query your AI agent's memory",
    version,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Path to memory file (overrides auto-detection)
    #[arg(long, global = true)]
    pub memory_path: Option<PathBuf>,

    /// Enable verbose debug output to stderr
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Search memories by text pattern
    Find {
        /// Search pattern
        #[arg(value_parser = parse_pattern)]
        pattern: String,
        /// Maximum results
        #[arg(long, default_value_t = 10, value_parser = parse_limit)]
        limit: usize,
        /// Filter by observation type
        #[arg(long, value_parser = parse_obs_type)]
        r#type: Option<ObservationType>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Ask a question about your memory
    Ask {
        /// Natural language question
        #[arg(value_parser = parse_question)]
        question: String,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// View memory statistics
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// View chronological timeline
    Timeline {
        /// Maximum entries
        #[arg(long, default_value_t = 10, value_parser = parse_limit)]
        limit: usize,
        /// Filter by observation type
        #[arg(long, value_parser = parse_obs_type)]
        r#type: Option<ObservationType>,
        /// Show oldest entries first
        #[arg(long)]
        oldest_first: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// `OpenCode` editor adapter subcommands
    #[command(subcommand)]
    Opencode(OpenCodeCommand),
    /// Configure rusty-brain for external AI agents
    Install {
        /// Comma-separated list of agents to configure (e.g., opencode,copilot)
        #[arg(long, value_delimiter = ',')]
        agents: Option<Vec<String>>,
        /// Install config relative to current working directory
        #[arg(long, group = "scope")]
        project: bool,
        /// Install config in user-level directories
        #[arg(long, group = "scope")]
        global: bool,
        /// Force JSON output
        #[arg(long)]
        json: bool,
        /// Regenerate config files (backup existing)
        #[arg(long)]
        reconfigure: bool,
    },
}

/// `OpenCode`-specific subcommands for editor integration.
#[derive(Subcommand)]
pub enum OpenCodeCommand {
    /// Process a chat hook event (reads `HookInput` JSON from stdin)
    ChatHook,
    /// Process a tool hook event (reads `HookInput` JSON from stdin)
    ToolHook,
    /// Process a mind tool invocation (reads `MindToolInput` JSON from stdin)
    Mind,
    /// Process a session cleanup event (reads `HookInput` JSON from stdin)
    SessionCleanup,
    /// Process a session start event with orphan cleanup (reads `HookInput` JSON from stdin)
    SessionStart,
}

impl Command {
    /// Whether `--json` output was requested for this subcommand.
    pub fn json(&self) -> bool {
        match self {
            Self::Find { json, .. }
            | Self::Ask { json, .. }
            | Self::Stats { json, .. }
            | Self::Timeline { json, .. }
            | Self::Install { json, .. } => *json,
            // OpenCode subcommands always output JSON
            Self::Opencode(_) => true,
        }
    }
}

/// Parse and validate search pattern (must be non-empty after trimming).
fn parse_pattern(s: &str) -> Result<String, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("pattern must not be empty".to_string());
    }
    Ok(trimmed.to_string())
}

/// Parse and validate question (must be non-empty after trimming).
fn parse_question(s: &str) -> Result<String, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("question must not be empty".to_string());
    }
    Ok(trimmed.to_string())
}

/// Parse limit as a positive integer (>= 1).
fn parse_limit(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| format!("invalid value '{s}' for '--limit': not a valid integer"))?;
    if n == 0 {
        return Err("invalid value '0' for '--limit': must be at least 1".to_string());
    }
    Ok(n)
}

/// Parse observation type from string (case-insensitive).
/// On error, lists all valid type names.
fn parse_obs_type(s: &str) -> Result<ObservationType, String> {
    s.parse::<ObservationType>().map_err(|_| {
        format!(
            "invalid observation type '{s}'; valid types: discovery, decision, problem, \
             solution, pattern, warning, success, refactor, bugfix, feature"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // -------------------------------------------------------------------------
    // T030: Subcommand routing and flag validation
    // -------------------------------------------------------------------------

    #[test]
    fn parse_find_subcommand_basic() {
        let cli = Cli::try_parse_from(["rusty-brain", "find", "hello"]).unwrap();
        match cli.command {
            Command::Find {
                pattern,
                limit,
                r#type,
                json,
            } => {
                assert_eq!(pattern, "hello");
                assert_eq!(limit, 10, "default limit must be 10");
                assert!(r#type.is_none());
                assert!(!json);
            }
            _ => panic!("expected Find command"),
        }
    }

    #[test]
    fn parse_find_with_limit_and_json() {
        let cli =
            Cli::try_parse_from(["rusty-brain", "find", "test", "--limit", "5", "--json"]).unwrap();
        match cli.command {
            Command::Find {
                pattern,
                limit,
                json,
                ..
            } => {
                assert_eq!(pattern, "test");
                assert_eq!(limit, 5);
                assert!(json);
            }
            _ => panic!("expected Find command"),
        }
    }

    #[test]
    fn parse_find_with_type_filter() {
        let cli =
            Cli::try_parse_from(["rusty-brain", "find", "test", "--type", "discovery"]).unwrap();
        match cli.command {
            Command::Find { r#type, .. } => {
                assert_eq!(r#type, Some(ObservationType::Discovery));
            }
            _ => panic!("expected Find command"),
        }
    }

    #[test]
    fn parse_ask_subcommand() {
        let cli = Cli::try_parse_from(["rusty-brain", "ask", "What happened?"]).unwrap();
        match cli.command {
            Command::Ask { question, json } => {
                assert_eq!(question, "What happened?");
                assert!(!json);
            }
            _ => panic!("expected Ask command"),
        }
    }

    #[test]
    fn parse_ask_with_json() {
        let cli = Cli::try_parse_from(["rusty-brain", "ask", "question", "--json"]).unwrap();
        match cli.command {
            Command::Ask { json, .. } => assert!(json),
            _ => panic!("expected Ask command"),
        }
    }

    #[test]
    fn parse_stats_subcommand() {
        let cli = Cli::try_parse_from(["rusty-brain", "stats"]).unwrap();
        match cli.command {
            Command::Stats { json } => assert!(!json),
            _ => panic!("expected Stats command"),
        }
    }

    #[test]
    fn parse_stats_with_json() {
        let cli = Cli::try_parse_from(["rusty-brain", "stats", "--json"]).unwrap();
        match cli.command {
            Command::Stats { json } => assert!(json),
            _ => panic!("expected Stats command"),
        }
    }

    #[test]
    fn parse_timeline_defaults() {
        let cli = Cli::try_parse_from(["rusty-brain", "timeline"]).unwrap();
        match cli.command {
            Command::Timeline {
                limit,
                r#type,
                oldest_first,
                json,
            } => {
                assert_eq!(limit, 10);
                assert!(r#type.is_none());
                assert!(!oldest_first);
                assert!(!json);
            }
            _ => panic!("expected Timeline command"),
        }
    }

    #[test]
    fn parse_timeline_with_all_flags() {
        let cli = Cli::try_parse_from([
            "rusty-brain",
            "timeline",
            "--limit",
            "20",
            "--type",
            "bugfix",
            "--oldest-first",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Command::Timeline {
                limit,
                r#type,
                oldest_first,
                json,
            } => {
                assert_eq!(limit, 20);
                assert_eq!(r#type, Some(ObservationType::Bugfix));
                assert!(oldest_first);
                assert!(json);
            }
            _ => panic!("expected Timeline command"),
        }
    }

    #[test]
    fn parse_global_verbose_flag() {
        let cli = Cli::try_parse_from(["rusty-brain", "--verbose", "stats"]).unwrap();
        assert!(cli.verbose);
    }

    #[test]
    fn parse_global_memory_path_flag() {
        let cli = Cli::try_parse_from(["rusty-brain", "--memory-path", "/tmp/test.mv2", "stats"])
            .unwrap();
        assert_eq!(cli.memory_path, Some(PathBuf::from("/tmp/test.mv2")));
    }

    #[test]
    fn parse_opencode_chat_hook() {
        let cli = Cli::try_parse_from(["rusty-brain", "opencode", "chat-hook"]).unwrap();
        match cli.command {
            Command::Opencode(OpenCodeCommand::ChatHook) => {}
            _ => panic!("expected Opencode ChatHook"),
        }
    }

    #[test]
    fn parse_opencode_tool_hook() {
        let cli = Cli::try_parse_from(["rusty-brain", "opencode", "tool-hook"]).unwrap();
        match cli.command {
            Command::Opencode(OpenCodeCommand::ToolHook) => {}
            _ => panic!("expected Opencode ToolHook"),
        }
    }

    #[test]
    fn parse_opencode_mind() {
        let cli = Cli::try_parse_from(["rusty-brain", "opencode", "mind"]).unwrap();
        match cli.command {
            Command::Opencode(OpenCodeCommand::Mind) => {}
            _ => panic!("expected Opencode Mind"),
        }
    }

    #[test]
    fn parse_opencode_session_cleanup() {
        let cli = Cli::try_parse_from(["rusty-brain", "opencode", "session-cleanup"]).unwrap();
        match cli.command {
            Command::Opencode(OpenCodeCommand::SessionCleanup) => {}
            _ => panic!("expected Opencode SessionCleanup"),
        }
    }

    #[test]
    fn parse_opencode_session_start() {
        let cli = Cli::try_parse_from(["rusty-brain", "opencode", "session-start"]).unwrap();
        match cli.command {
            Command::Opencode(OpenCodeCommand::SessionStart) => {}
            _ => panic!("expected Opencode SessionStart"),
        }
    }

    // -------------------------------------------------------------------------
    // Validation tests (parse_pattern, parse_question, parse_limit, parse_obs_type)
    // -------------------------------------------------------------------------

    #[test]
    fn empty_pattern_rejected() {
        let result = Cli::try_parse_from(["rusty-brain", "find", ""]);
        assert!(result.is_err());
    }

    #[test]
    fn whitespace_only_pattern_rejected() {
        let result = Cli::try_parse_from(["rusty-brain", "find", "   "]);
        assert!(result.is_err());
    }

    #[test]
    fn pattern_is_trimmed() {
        let cli = Cli::try_parse_from(["rusty-brain", "find", "  hello  "]).unwrap();
        match cli.command {
            Command::Find { pattern, .. } => assert_eq!(pattern, "hello"),
            _ => panic!("expected Find"),
        }
    }

    #[test]
    fn empty_question_rejected() {
        let result = Cli::try_parse_from(["rusty-brain", "ask", ""]);
        assert!(result.is_err());
    }

    #[test]
    fn whitespace_only_question_rejected() {
        let result = Cli::try_parse_from(["rusty-brain", "ask", "   "]);
        assert!(result.is_err());
    }

    #[test]
    fn limit_zero_rejected() {
        let result = Cli::try_parse_from(["rusty-brain", "find", "test", "--limit", "0"]);
        assert!(result.is_err());
    }

    #[test]
    fn limit_non_integer_rejected() {
        let result = Cli::try_parse_from(["rusty-brain", "find", "test", "--limit", "abc"]);
        assert!(result.is_err());
    }

    #[test]
    fn limit_one_accepted() {
        let cli = Cli::try_parse_from(["rusty-brain", "find", "test", "--limit", "1"]).unwrap();
        match cli.command {
            Command::Find { limit, .. } => assert_eq!(limit, 1),
            _ => panic!("expected Find"),
        }
    }

    #[test]
    fn invalid_obs_type_rejected() {
        let result = Cli::try_parse_from(["rusty-brain", "find", "test", "--type", "invalid"]);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------------
    // Command::json() helper
    // -------------------------------------------------------------------------

    #[test]
    fn json_method_find_default_false() {
        let cli = Cli::try_parse_from(["rusty-brain", "find", "test"]).unwrap();
        assert!(!cli.command.json());
    }

    #[test]
    fn json_method_find_with_flag_true() {
        let cli = Cli::try_parse_from(["rusty-brain", "find", "test", "--json"]).unwrap();
        assert!(cli.command.json());
    }

    #[test]
    fn json_method_opencode_always_true() {
        let cli = Cli::try_parse_from(["rusty-brain", "opencode", "chat-hook"]).unwrap();
        assert!(cli.command.json());
    }

    #[test]
    fn no_subcommand_shows_help() {
        let result = Cli::try_parse_from(["rusty-brain"]);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------------
    // T027: Install subcommand arg parsing tests
    // -------------------------------------------------------------------------

    #[test]
    fn parse_install_project_scope() {
        let cli = Cli::try_parse_from(["rusty-brain", "install", "--project"]).unwrap();
        match cli.command {
            Command::Install {
                project, global, ..
            } => {
                assert!(project);
                assert!(!global);
            }
            _ => panic!("expected Install command"),
        }
    }

    #[test]
    fn parse_install_global_scope() {
        let cli = Cli::try_parse_from(["rusty-brain", "install", "--global"]).unwrap();
        match cli.command {
            Command::Install {
                project, global, ..
            } => {
                assert!(!project);
                assert!(global);
            }
            _ => panic!("expected Install command"),
        }
    }

    #[test]
    fn parse_install_project_and_global_mutually_exclusive() {
        let result = Cli::try_parse_from(["rusty-brain", "install", "--project", "--global"]);
        assert!(
            result.is_err(),
            "--project and --global should be mutually exclusive"
        );
    }

    #[test]
    fn parse_install_agents_comma_delimited() {
        let cli = Cli::try_parse_from([
            "rusty-brain",
            "install",
            "--agents",
            "opencode,copilot",
            "--project",
        ])
        .unwrap();
        match cli.command {
            Command::Install { agents, .. } => {
                let agents = agents.unwrap();
                assert_eq!(agents, vec!["opencode", "copilot"]);
            }
            _ => panic!("expected Install command"),
        }
    }

    #[test]
    fn parse_install_json_flag() {
        let cli = Cli::try_parse_from(["rusty-brain", "install", "--project", "--json"]).unwrap();
        match cli.command {
            Command::Install { json, .. } => assert!(json),
            _ => panic!("expected Install command"),
        }
    }

    #[test]
    fn parse_install_reconfigure_flag() {
        let cli =
            Cli::try_parse_from(["rusty-brain", "install", "--project", "--reconfigure"]).unwrap();
        match cli.command {
            Command::Install { reconfigure, .. } => assert!(reconfigure),
            _ => panic!("expected Install command"),
        }
    }

    #[test]
    fn parse_install_no_scope_allowed() {
        // Neither --project nor --global should be accepted (scope required at runtime)
        let cli = Cli::try_parse_from(["rusty-brain", "install"]).unwrap();
        match cli.command {
            Command::Install {
                project, global, ..
            } => {
                assert!(!project);
                assert!(!global);
            }
            _ => panic!("expected Install command"),
        }
    }

    #[test]
    fn json_method_install_default_false() {
        let cli = Cli::try_parse_from(["rusty-brain", "install", "--project"]).unwrap();
        assert!(!cli.command.json());
    }

    #[test]
    fn json_method_install_with_flag_true() {
        let cli = Cli::try_parse_from(["rusty-brain", "install", "--project", "--json"]).unwrap();
        assert!(cli.command.json());
    }
}
