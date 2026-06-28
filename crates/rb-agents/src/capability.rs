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
        verified_lifecycle_source: "crates/rb-hooks/tests/fixtures/opencode/",
        limitations: &[
            "Hook adapter exists, but rb-install plugin support is deferred.",
            "session.idle maps to canonical SessionCheckpoint (fixture-backed lifecycle test; multi-fire checkpoint-safe, one-run observation fired=2, verdict ambiguous); session.deleted remains Other.",
            "Capture stays Partial: bash-command capture folds, but file edits via apply_patch are not yet captured (Gap B / apply_patch normalization pending).",
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
            "Native SessionEnd remains canonical Stop; terminus mapping is descoped (no lifecycle fixture planned).",
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

    // --- Regression / boundary tests ----------------------------------------

    #[test]
    fn agent_count_is_exactly_five() {
        // Regression: adding or removing an agent without updating related
        // logic (scorecard routing, docs table, changelog) is a common mistake.
        assert_eq!(agent_capabilities().len(), 5);
    }

    #[test]
    fn agent_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for capability in agent_capabilities() {
            assert!(
                seen.insert(capability.agent),
                "duplicate agent id: {}",
                capability.agent
            );
        }
    }

    // --- Data-integrity tests ------------------------------------------------

    #[test]
    fn all_agents_have_non_empty_limitations() {
        for capability in agent_capabilities() {
            assert!(
                !capability.limitations.is_empty(),
                "agent {} has empty limitations slice",
                capability.agent
            );
        }
    }

    #[test]
    fn all_agents_have_non_empty_verified_lifecycle_source() {
        for capability in agent_capabilities() {
            assert!(
                !capability.verified_lifecycle_source.is_empty(),
                "agent {} has empty verified_lifecycle_source",
                capability.agent
            );
        }
    }

    // --- Adapter status tests ------------------------------------------------

    #[test]
    fn experimental_agents_are_codex_opencode_gemini() {
        let experimental: Vec<_> = agent_capabilities()
            .iter()
            .filter(|c| c.adapter_status == AdapterStatus::Experimental)
            .map(|c| c.agent)
            .collect();
        assert_eq!(experimental, vec!["codex", "opencode", "gemini"]);
    }

    #[test]
    fn hermes_is_the_only_discovery_gated_agent() {
        let discovery: Vec<_> = agent_capabilities()
            .iter()
            .filter(|c| c.adapter_status == AdapterStatus::Discovery)
            .map(|c| c.agent)
            .collect();
        assert_eq!(discovery, vec!["hermes"]);
    }

    // --- Capability level exhaustiveness ------------------------------------

    #[test]
    fn claude_code_has_all_four_capabilities_supported() {
        let cc = capability_for_agent("claude-code").expect("claude-code row");
        assert_eq!(cc.capture, SupportLevel::Supported, "capture");
        assert_eq!(cc.retrieval, SupportLevel::Supported, "retrieval");
        assert_eq!(cc.config, SupportLevel::Supported, "config");
        assert_eq!(cc.scorecard, SupportLevel::Supported, "scorecard");
    }

    #[test]
    fn non_hermes_agents_have_no_unknown_support_levels() {
        for capability in agent_capabilities() {
            if capability.agent == "hermes" {
                continue;
            }
            assert_ne!(
                capability.capture,
                SupportLevel::Unknown,
                "agent {} capture must not be Unknown",
                capability.agent
            );
            assert_ne!(
                capability.retrieval,
                SupportLevel::Unknown,
                "agent {} retrieval must not be Unknown",
                capability.agent
            );
            assert_ne!(
                capability.config,
                SupportLevel::Unknown,
                "agent {} config must not be Unknown",
                capability.agent
            );
            assert_ne!(
                capability.scorecard,
                SupportLevel::Unknown,
                "agent {} scorecard must not be Unknown",
                capability.agent
            );
        }
    }

    #[test]
    fn retrieval_is_unsupported_for_all_non_claude_experimental_agents() {
        for agent in ["codex", "opencode", "gemini"] {
            let capability = capability_for_agent(agent).expect("agent row");
            assert_eq!(
                capability.retrieval,
                SupportLevel::Unsupported,
                "agent {} retrieval should be Unsupported",
                agent
            );
        }
    }

    #[test]
    fn hermes_scorecard_is_explicitly_unsupported_not_unknown() {
        // scorecard must be Unsupported (a deliberate decision), not Unknown (unverified).
        let hermes = capability_for_agent("hermes").expect("hermes row");
        assert_eq!(hermes.scorecard, SupportLevel::Unsupported);
        assert_ne!(hermes.scorecard, SupportLevel::Unknown);
    }

    // --- Lookup semantics ----------------------------------------------------

    #[test]
    fn capability_for_agent_is_case_sensitive() {
        // The agent id is a stable lower-case token; upper-case must not match.
        assert_eq!(capability_for_agent("Claude-Code"), None);
        assert_eq!(capability_for_agent("CLAUDE-CODE"), None);
        assert_eq!(capability_for_agent("Hermes"), None);
    }

    #[test]
    fn capability_for_agent_returns_none_for_empty_string() {
        assert_eq!(capability_for_agent(""), None);
    }

    // --- Debug / trait-impl smoke tests -------------------------------------

    #[test]
    fn adapter_status_debug_displays_variant_name() {
        assert_eq!(format!("{:?}", AdapterStatus::Stable), "Stable");
        assert_eq!(format!("{:?}", AdapterStatus::Experimental), "Experimental");
        assert_eq!(format!("{:?}", AdapterStatus::Discovery), "Discovery");
        assert_eq!(format!("{:?}", AdapterStatus::Unsupported), "Unsupported");
    }

    #[test]
    fn support_level_debug_displays_variant_name() {
        assert_eq!(format!("{:?}", SupportLevel::Supported), "Supported");
        assert_eq!(format!("{:?}", SupportLevel::Partial), "Partial");
        assert_eq!(format!("{:?}", SupportLevel::Unsupported), "Unsupported");
        assert_eq!(format!("{:?}", SupportLevel::Unknown), "Unknown");
    }

    #[test]
    fn adapter_status_equality_is_reflexive() {
        assert_eq!(AdapterStatus::Stable, AdapterStatus::Stable);
        assert_ne!(AdapterStatus::Stable, AdapterStatus::Experimental);
    }

    #[test]
    fn support_level_equality_is_reflexive() {
        assert_eq!(SupportLevel::Supported, SupportLevel::Supported);
        assert_ne!(SupportLevel::Supported, SupportLevel::Partial);
    }

    #[test]
    fn agent_capability_is_copy() {
        // Verifies `Copy` derive by value-copy of a capability row.
        let cap = *capability_for_agent("claude-code").expect("claude-code row");
        assert_eq!(cap.agent, "claude-code");
    }
}
