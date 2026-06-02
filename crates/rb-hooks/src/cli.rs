//! Command-line arguments for the hook binary.
//!
//! The only argument is `--agent <id>`, selecting which JSON-protocol CLI's
//! stdin/stdout shapes to use. The lifecycle event itself is NOT a CLI arg — it
//! is read from the stdin JSON via `AgentCli::parse_input`.

use rb_agents::AgentId;

/// Parsed hook invocation arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// Which agent CLI's JSON shapes to use.
    pub agent: AgentId,
}

impl Args {
    /// Parse from an argv-like iterator. Accepts `--agent <id>` and
    /// `--agent=<id>`. Returns `Err(message)` on a missing/unknown agent or an
    /// unexpected argument — the caller treats any error as fail-open.
    pub fn parse_from<I, S>(argv: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut agent: Option<AgentId> = None;
        let mut iter = argv.into_iter();
        // Skip argv[0] (program name).
        let _ = iter.next();
        while let Some(arg) = iter.next() {
            let arg = arg.as_ref();
            if let Some(value) = arg.strip_prefix("--agent=") {
                agent = Some(Self::parse_agent(value)?);
            } else if arg == "--agent" {
                let value = iter
                    .next()
                    .ok_or_else(|| "missing value for --agent".to_string())?;
                agent = Some(Self::parse_agent(value.as_ref())?);
            } else {
                return Err(format!("unexpected argument: {arg}"));
            }
        }
        let agent = agent.ok_or_else(|| "missing required --agent <id>".to_string())?;
        Ok(Args { agent })
    }

    fn parse_agent(value: &str) -> Result<AgentId, String> {
        AgentId::parse(value).ok_or_else(|| format!("unknown agent: {value}"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_agents::AgentId;

    #[test]
    fn parses_claude_code_agent() {
        let args = Args::parse_from(["rusty-brain-hooks", "--agent", "claude-code"]).unwrap();
        assert_eq!(args.agent, AgentId::ClaudeCode);
    }

    #[test]
    fn parses_each_agent_id() {
        for (raw, expected) in [
            ("claude-code", AgentId::ClaudeCode),
            ("opencode", AgentId::OpenCode),
            ("gemini", AgentId::Gemini),
            ("codex", AgentId::Codex),
        ] {
            let args = Args::parse_from(["rusty-brain-hooks", "--agent", raw]).unwrap();
            assert_eq!(args.agent, expected, "agent {raw}");
        }
    }

    #[test]
    fn missing_agent_is_error() {
        let err = Args::parse_from(["rusty-brain-hooks"]);
        assert!(err.is_err(), "missing --agent must error");
    }

    #[test]
    fn unknown_agent_is_error() {
        let err = Args::parse_from(["rusty-brain-hooks", "--agent", "bogus"]);
        assert!(err.is_err(), "unknown agent must error");
    }

    #[test]
    fn equals_form_is_accepted() {
        let args = Args::parse_from(["rusty-brain-hooks", "--agent=gemini"]).unwrap();
        assert_eq!(args.agent, AgentId::Gemini);
    }
}
