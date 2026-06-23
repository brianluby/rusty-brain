//! Static support matrix for the agent surface.
//!
//! This module is deliberately descriptive: it does not invent hook names or
//! lifecycle events. It gives callers and docs one stable place to report which
//! agents are verified, partial, discovery-gated, or unsupported.

/// Adapter maturity for an agent target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterStatus {
    /// Verified against committed lifecycle fixtures.
    Stable,
    /// Adapter code exists, but lifecycle behavior is not fully fixture-proven.
    Experimental,
    /// Discovery is still needed before implementation can safely begin.
    Discovery,
    /// No adapter is implemented.
    Unsupported,
}

/// Coarse support level for one capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportLevel {
    /// Capability is implemented and verified for ordinary use.
    Supported,
    /// Capability exists, but has documented gaps or fixture gates.
    Partial,
    /// Capability is known to be absent.
    Unsupported,
    /// Capability has not been verified.
    Unknown,
}

/// User-facing capability status for one agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCapability {
    /// Stable agent id, matching hook/install flags where the agent is wired.
    pub agent: &'static str,
    /// Adapter maturity.
    pub adapter_status: AdapterStatus,
    /// Automatic capture support.
    pub capture: SupportLevel,
    /// Prompt-time retrieval/injection support.
    pub retrieval: SupportLevel,
    /// Installer/config support.
    pub config: SupportLevel,
    /// Memory-value scorecard support.
    pub scorecard: SupportLevel,
    /// Fixture, doc, or discovery note that justifies the status.
    pub verified_lifecycle_source: &'static str,
    /// Short constraints to show in docs/status output.
    pub limitations: &'static [&'static str],
}

const CAPABILITIES: &[AgentCapability] = &[
    AgentCapability {
        agent: "claude-code",
        adapter_status: AdapterStatus::Stable,
        capture: SupportLevel::Supported,
        retrieval: SupportLevel::Supported,
        config: SupportLevel::Supported,
        scorecard: SupportLevel::Supported,
        verified_lifecycle_source: "crates/rb-hooks/tests/fixtures/claude_code/",
        limitations: &[
            "SessionEnd lifecycle is fixture-backed.",
            "Scorecard runner uses the Claude Code headless CLI.",
        ],
    },
    AgentCapability {
        agent: "codex",
        adapter_status: AdapterStatus::Experimental,
        capture: SupportLevel::Partial,
        retrieval: SupportLevel::Unsupported,
        config: SupportLevel::Partial,
        scorecard: SupportLevel::Unsupported,
        verified_lifecycle_source: "crates/rb-hooks/tests/fixtures/codex/README.md",
        limitations: &[
            "Native Stop remains canonical Stop until a real lifecycle fixture proves a checkpoint or terminus boundary.",
            "UserPromptSubmit retrieval and apply_patch capture are fixture-gated.",
        ],
    },
    AgentCapability {
        agent: "opencode",
        adapter_status: AdapterStatus::Experimental,
        capture: SupportLevel::Partial,
        retrieval: SupportLevel::Unsupported,
        config: SupportLevel::Unsupported,
        scorecard: SupportLevel::Unsupported,
        verified_lifecycle_source: "crates/rb-hooks/tests/fixtures/opencode/README.md",
        limitations: &[
            "Hook adapter exists, but rb-install plugin support is deferred.",
            "session.idle remains canonical Stop and session.deleted remains Other until fixtures prove otherwise.",
        ],
    },
    AgentCapability {
        agent: "gemini",
        adapter_status: AdapterStatus::Experimental,
        capture: SupportLevel::Partial,
        retrieval: SupportLevel::Unsupported,
        config: SupportLevel::Partial,
        scorecard: SupportLevel::Unsupported,
        verified_lifecycle_source: "crates/rb-hooks/tests/fixtures/gemini/README.md",
        limitations: &[
            "Native SessionEnd remains canonical Stop until a real lifecycle fixture proves a checkpoint or terminus boundary.",
            "Prompt-time retrieval parity has not been verified.",
        ],
    },
    AgentCapability {
        agent: "hermes",
        adapter_status: AdapterStatus::Discovery,
        capture: SupportLevel::Unknown,
        retrieval: SupportLevel::Unknown,
        config: SupportLevel::Unknown,
        scorecard: SupportLevel::Unsupported,
        verified_lifecycle_source: "docs/follow-ups/2026-06-23-hermes-discovery.md",
        limitations: &[
            "Discovery-gated: no hook names, config paths, or lifecycle semantics are hard-coded.",
        ],
    },
];

/// Return the static agent capability matrix.
#[must_use]
pub fn agent_capabilities() -> &'static [AgentCapability] {
    CAPABILITIES
}

/// Look up one agent capability row by stable id.
#[must_use]
pub fn capability_for_agent(agent: &str) -> Option<&'static AgentCapability> {
    CAPABILITIES
        .iter()
        .find(|capability| capability.agent == agent)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn capability_matrix_lists_current_targets() {
        let agents: Vec<_> = agent_capabilities()
            .iter()
            .map(|capability| capability.agent)
            .collect();
        assert_eq!(
            agents,
            vec!["claude-code", "codex", "opencode", "gemini", "hermes"]
        );
    }

    #[test]
    fn claude_code_is_the_only_supported_scorecard_agent() {
        let supported: Vec<_> = agent_capabilities()
            .iter()
            .filter(|capability| capability.scorecard == SupportLevel::Supported)
            .map(|capability| capability.agent)
            .collect();
        assert_eq!(supported, vec!["claude-code"]);
    }

    #[test]
    fn non_claude_capture_is_not_claimed_as_full_parity() {
        for agent in ["codex", "opencode", "gemini"] {
            let capability = capability_for_agent(agent).expect("agent row");
            assert_eq!(capability.capture, SupportLevel::Partial);
            assert_ne!(capability.capture, SupportLevel::Supported);
        }
    }

    #[test]
    fn hermes_is_discovery_gated_without_capability_claims() {
        let capability = capability_for_agent("hermes").expect("hermes row");
        assert_eq!(capability.adapter_status, AdapterStatus::Discovery);
        assert_eq!(capability.capture, SupportLevel::Unknown);
        assert_eq!(capability.retrieval, SupportLevel::Unknown);
        assert_eq!(capability.config, SupportLevel::Unknown);
    }

    #[test]
    fn unknown_agent_has_no_capability_row() {
        assert_eq!(capability_for_agent("copilot"), None);
    }
}
