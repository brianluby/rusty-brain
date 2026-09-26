//! The memory trust-class ladder (Vikunja #63): an EVIDENCE-derived trust
//! tier per memory, distinct from the caller-declared `confidence` prior.
//!
//! `confidence` answers "how sure did the author feel"; `trust_class` answers
//! "what backs this memory, and can that backing be re-checked when doubted".
//! Before this ladder a memory backed by a measured test result and one
//! asserted by the model ranked identically — the exact gap the ladder closes.
//!
//! # The total ranking
//!
//! Declaration order IS the ranking (derived `Ord`, strongest first):
//!
//! 1. `measured_ci` — backed by a CI-measured result. Reproducible by anyone
//!    with repository access; the strongest verification the system can name.
//! 2. `measured_local` — backed by a locally executed, machine-observed
//!    command run. Real evidence, but single-machine and not independently
//!    re-runnable.
//! 3. `human_confirmed` — vouched by the human principal on their own typed
//!    surface. Authoritative, but nothing mechanical backs it.
//! 4. `agent_attested` — a first-party assertion by the capturing agent. The
//!    default for legacy rows and ordinary hook captures.
//! 5. `inferred_activity` — derived heuristically from observed activity with
//!    no asserting author at all.
//!
//! Rationale: every CAN-SATISFY class (1–3: the claim can be substantiated
//! when challenged) outranks every CONTEXT-ONLY class (4–5: it cannot). Within
//! can-satisfy, reproducibility beats locality beats vouching. Within
//! context-only, a direct attestation beats a derived inference.
//!
//! # No self-promotion
//!
//! The trust class is DERIVED, never declared. The wire protocol has NO
//! trust-class field: a caller may only submit [`CaptureEvidence`], and the
//! daemon validates that evidence against what the writing channel can
//! substantiate ([`derive_trust_class`]). A hook — our own instrumented binary
//! observing host-runtime tool events — can prove a local command ran, but can
//! never prove a CI run happened or that a human confirmed anything. The
//! model-facing MCP and HTTP surfaces carry narrative only and cannot
//! substantiate any evidence at all.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// One rung of the trust ladder. See the module docs for the ranking and its
/// rationale; the derived `Ord` IS the ladder (a stronger class compares
/// greater) and is a TOTAL order — used directly as the recall tie-break when
/// scores tie exactly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustClass {
    // Weakest-first declaration: the DERIVED Ord treats a later variant as
    // stronger, so `MeasuredCi > MeasuredLocal > ... > InferredActivity`
    // holds without a hand-rolled ordering.
    InferredActivity,
    /// The conservative default: what a pre-ladder capture could honestly
    /// claim for itself. Legacy rows backfill to this (migration 012), and
    /// old wire payloads decode to it (`#[serde(default)]`).
    #[default]
    AgentAttested,
    HumanConfirmed,
    MeasuredLocal,
    MeasuredCi,
}

impl TrustClass {
    /// Every variant, strongest first — the canonical list that doc and
    /// round-trip tests derive from instead of hand-maintained copies.
    /// Compiler-pinned: the exhaustive match stops compiling the moment a
    /// variant is added (the `MemoryType::all` convention).
    pub fn all() -> [TrustClass; 5] {
        #[allow(clippy::needless_match)]
        const fn pin(t: TrustClass) -> TrustClass {
            match t {
                TrustClass::MeasuredCi => TrustClass::MeasuredCi,
                TrustClass::MeasuredLocal => TrustClass::MeasuredLocal,
                TrustClass::HumanConfirmed => TrustClass::HumanConfirmed,
                TrustClass::AgentAttested => TrustClass::AgentAttested,
                TrustClass::InferredActivity => TrustClass::InferredActivity,
            }
        }
        [
            pin(TrustClass::MeasuredCi),
            pin(TrustClass::MeasuredLocal),
            pin(TrustClass::HumanConfirmed),
            pin(TrustClass::AgentAttested),
            pin(TrustClass::InferredActivity),
        ]
    }

    /// Stable db string. MUST stay in lockstep with the migration 012 SQL
    /// CHECK constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            TrustClass::MeasuredCi => "measured_ci",
            TrustClass::MeasuredLocal => "measured_local",
            TrustClass::HumanConfirmed => "human_confirmed",
            TrustClass::AgentAttested => "agent_attested",
            TrustClass::InferredActivity => "inferred_activity",
        }
    }

    /// Parse a db string into a `TrustClass`. Fail closed on unknown values.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "measured_ci" => Ok(TrustClass::MeasuredCi),
            "measured_local" => Ok(TrustClass::MeasuredLocal),
            "human_confirmed" => Ok(TrustClass::HumanConfirmed),
            "agent_attested" => Ok(TrustClass::AgentAttested),
            "inferred_activity" => Ok(TrustClass::InferredActivity),
            other => Err(Error::InvalidArgument(format!(
                "unknown trust class '{other}': expected one of {}",
                TrustClass::all()
                    .iter()
                    .map(|t| t.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// Whether a memory of this class CAN present substantiating evidence when
    /// its claim is doubted (the can-satisfy vs context-only split).
    #[must_use]
    pub fn can_satisfy(&self) -> bool {
        matches!(
            self,
            TrustClass::MeasuredCi | TrustClass::MeasuredLocal | TrustClass::HumanConfirmed
        )
    }

    /// The recall ranking prior (Vikunja #63): a multiplicative score
    /// dampener applied like the session-provenance prior — it REORDERS, and
    /// the admission floor is scaled by the same factor so the prior never
    /// silently raises the admission bar (the session-multiplier precedent).
    ///
    /// Calibration: the top of the ladder keeps the untouched score (no class
    /// outranks its own evidence); each rung below steps down 5%, because
    /// adjacent rungs differ by provenance quality, not truth. `agent_attested`
    /// sits at 0.80 — enough that an otherwise-identical measured memory always
    /// outranks an attested one, small enough that attested memories remain
    /// strongly competitive with today's behavior (uniform-class corpora scale
    /// scores and floors identically, so ordering and admissions are
    /// unchanged). `inferred_activity` at 0.70 mirrors the extra distance from
    /// any asserting author.
    #[must_use]
    pub fn score_multiplier(&self) -> f32 {
        match self {
            TrustClass::MeasuredCi => 1.00,
            TrustClass::MeasuredLocal => 0.95,
            TrustClass::HumanConfirmed => 0.90,
            TrustClass::AgentAttested => 0.80,
            TrustClass::InferredActivity => 0.70,
        }
    }

    /// The context-only default for a write that carries NO evidence: what the
    /// writing channel can honestly claim on its own. Internal maintenance
    /// jobs produce derived rows nobody attested (`inferred_activity`);
    /// every authoring surface — hook, mcp, cli, http, or unknown — is at
    /// least a first-party attestation (`agent_attested`).
    #[must_use]
    pub fn context_default(origin_source: Option<&str>) -> TrustClass {
        match origin_source {
            Some("job") => TrustClass::InferredActivity,
            _ => TrustClass::AgentAttested,
        }
    }
}

/// Verifiable evidence a capture path claims to hold (Vikunja #63). This is
/// the ONLY trust-related thing a caller may put on the wire — never a class.
/// The daemon gates each kind against the channel that can substantiate it
/// ([`derive_trust_class`]); an unsubstantiatable claim is a hard
/// `InvalidArgument`, never a silent downgrade (a silent downgrade would
/// teach callers to always claim the maximum).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureEvidence {
    /// Commands the WRITING HOOK itself observed executing on this machine
    /// (already redacted at capture). Substantiable by the `hook` channel
    /// only: the hook is the instrument that recorded the host runtime's
    /// tool events, so a machine-observed exit exists behind every entry.
    MeasuredLocal { commands: Vec<String> },
    /// A reference to the CI run that produced the result (URL, run id, or
    /// check name). Substantiable by the `cli` channel only — the human's
    /// typed surface on a same-uid UDS connection (the credential proves the
    /// user; the surface is the client's honest self-report) — and ONLY
    /// alongside a commit anchor, which pins the claim to an auditable
    /// point in history.
    MeasuredCi { run_ref: String },
    /// The human principal confirmed the statement itself on their own typed
    /// surface (`cli`). Optional free-form `by` records who.
    HumanConfirmed { by: Option<String> },
}

impl CaptureEvidence {
    /// Shape validation, independent of channel: the fields that carry the
    /// claim's substance must be non-empty (an empty command list or run
    /// reference substantiates nothing).
    pub fn validate(&self) -> Result<()> {
        match self {
            CaptureEvidence::MeasuredLocal { commands } => {
                if commands.is_empty() || commands.iter().any(|c| c.trim().is_empty()) {
                    return Err(Error::InvalidArgument(
                        "measured_local evidence requires at least one non-empty observed command"
                            .to_string(),
                    ));
                }
            }
            CaptureEvidence::MeasuredCi { run_ref } => {
                if run_ref.trim().is_empty() {
                    return Err(Error::InvalidArgument(
                        "measured_ci evidence requires a non-empty CI run reference".to_string(),
                    ));
                }
            }
            CaptureEvidence::HumanConfirmed { .. } => {}
        }
        Ok(())
    }
}

/// Derive the trust class SERVER-SIDE from what the writing channel can
/// substantiate (Vikunja #63, the no-self-promotion rule). Called by the
/// daemon on every `Remember`; the result is the ONLY way a trust class
/// enters the store.
///
/// Gates, and why each holds:
///
/// - `hook` — our own instrumented binary. It machine-records host-runtime
///   tool events, so `MeasuredLocal` is substantiated. It never observes CI
///   machinery (only local text claiming CI ran) and never a human's
///   confirmation: both are rejected.
/// - `cli` — the human principal's typed surface over a same-uid UDS
///   connection (the credential proves the user; the surface string is the
///   honest client's self-report — same-user honesty, not binary
///   verification). Can carry `HumanConfirmed`, a human-reported
///   `MeasuredLocal`, or `MeasuredCi` — the CI claim only WITH a commit
///   anchor (`has_commit_anchor`), because an unpinned CI reference is
///   unfalsifiable.
/// - `mcp`, `http`, anything else — model-narrative or unattributed
///   channels: NO evidence kind is substantiatable, all rejected.
/// - `None` (evidence) — the honest context-only default per channel
///   ([`TrustClass::context_default`]).
///
/// Threat-model scope (docs/THREAT_MODEL.md): a same-user process can lie
/// about its channel and could write the SQLite file directly anyway; these
/// gates separate honest capture paths (the model self-attesting "tests pass"
/// vs the hook that observed the run), they are not a defense against hostile
/// same-user code.
pub fn derive_trust_class(
    origin_source: Option<&str>,
    evidence: Option<&CaptureEvidence>,
    has_commit_anchor: bool,
) -> Result<TrustClass> {
    let Some(evidence) = evidence else {
        return Ok(TrustClass::context_default(origin_source));
    };
    evidence.validate()?;
    match (origin_source, evidence) {
        (Some("hook"), CaptureEvidence::MeasuredLocal { .. }) => Ok(TrustClass::MeasuredLocal),
        (Some("cli"), CaptureEvidence::MeasuredLocal { .. }) => Ok(TrustClass::MeasuredLocal),
        (Some("cli"), CaptureEvidence::MeasuredCi { .. }) => {
            if has_commit_anchor {
                Ok(TrustClass::MeasuredCi)
            } else {
                Err(Error::InvalidArgument(
                    "measured_ci evidence requires a commit anchor pinning the CI run to a \
                     point in history; add --commit <sha>"
                        .to_string(),
                ))
            }
        }
        (Some("cli"), CaptureEvidence::HumanConfirmed { .. }) => Ok(TrustClass::HumanConfirmed),
        (Some(channel), _) => Err(Error::InvalidArgument(format!(
            "the '{channel}' capture path cannot substantiate this evidence kind; only the \
             hook channel can claim a local measurement and only the cli channel can claim a \
             CI run or a human confirmation"
        ))),
        (None, _) => Err(Error::InvalidArgument(
            "evidence requires an identified capture channel".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn all_is_duplicate_free_and_strongest_first() {
        let all = TrustClass::all();
        let mut seen = std::collections::HashSet::new();
        for t in all {
            assert!(seen.insert(t.as_str()), "duplicate variant {}", t.as_str());
        }
        assert_eq!(all.first(), Some(&TrustClass::MeasuredCi));
        assert_eq!(all.last(), Some(&TrustClass::InferredActivity));
    }

    #[test]
    fn declaration_order_is_the_documented_total_ranking() {
        // can-satisfy rungs outrank every context-only rung; within each
        // group the documented order holds.
        assert!(TrustClass::MeasuredCi > TrustClass::MeasuredLocal);
        assert!(TrustClass::MeasuredLocal > TrustClass::HumanConfirmed);
        assert!(TrustClass::HumanConfirmed > TrustClass::AgentAttested);
        assert!(TrustClass::AgentAttested > TrustClass::InferredActivity);
        for t in TrustClass::all() {
            assert_eq!(t.can_satisfy(), t > TrustClass::AgentAttested);
        }
    }

    #[test]
    fn as_str_matches_sql_check_values() {
        // Lockstep with migration 012's CHECK constraint.
        for t in TrustClass::all() {
            assert_eq!(TrustClass::parse(t.as_str()).unwrap(), t);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(TrustClass::parse("measured").is_err());
        assert!(TrustClass::parse("").is_err());
        assert!(TrustClass::parse("MEASURED_CI").is_err());
    }

    #[test]
    fn serde_snake_case_round_trip() {
        for t in TrustClass::all() {
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(json, format!("\"{}\"", t.as_str()));
            assert_eq!(serde_json::from_str::<TrustClass>(&json).unwrap(), t);
        }
    }

    #[test]
    fn default_is_agent_attested() {
        assert_eq!(TrustClass::default(), TrustClass::AgentAttested);
    }

    #[test]
    fn multipliers_are_strictly_ordered_and_positive() {
        let all = TrustClass::all();
        for pair in all.windows(2) {
            assert!(
                pair[0].score_multiplier() > pair[1].score_multiplier(),
                "multipliers must strictly descend with the ladder"
            );
        }
        assert_eq!(TrustClass::MeasuredCi.score_multiplier(), 1.0);
        assert!(TrustClass::InferredActivity.score_multiplier() > 0.0);
    }

    #[test]
    fn context_default_jobs_infer_everything_else_attests() {
        assert_eq!(
            TrustClass::context_default(Some("job")),
            TrustClass::InferredActivity
        );
        for channel in ["hook", "mcp", "cli", "http", "weird"] {
            assert_eq!(
                TrustClass::context_default(Some(channel)),
                TrustClass::AgentAttested,
                "channel {channel}"
            );
        }
        assert_eq!(TrustClass::context_default(None), TrustClass::AgentAttested);
    }

    #[test]
    fn evidence_shape_validation_fails_closed() {
        assert!(CaptureEvidence::MeasuredLocal {
            commands: vec!["cargo test".to_string()]
        }
        .validate()
        .is_ok());
        assert!(CaptureEvidence::MeasuredLocal { commands: vec![] }
            .validate()
            .is_err());
        assert!(CaptureEvidence::MeasuredLocal {
            commands: vec!["  ".to_string()]
        }
        .validate()
        .is_err());
        assert!(CaptureEvidence::MeasuredCi {
            run_ref: "actions/runs/42".to_string()
        }
        .validate()
        .is_ok());
        assert!(CaptureEvidence::MeasuredCi {
            run_ref: String::new()
        }
        .validate()
        .is_err());
        assert!(CaptureEvidence::HumanConfirmed { by: None }
            .validate()
            .is_ok());
    }

    #[test]
    fn derive_without_evidence_uses_context_default() {
        for channel in [Some("hook"), Some("cli"), Some("job"), None] {
            assert_eq!(
                derive_trust_class(channel, None, false).unwrap(),
                TrustClass::context_default(channel)
            );
        }
    }

    #[test]
    fn derive_accepts_hook_local_measurement_only() {
        let local = CaptureEvidence::MeasuredLocal {
            commands: vec!["cargo test -p rb-types".to_string()],
        };
        assert_eq!(
            derive_trust_class(Some("hook"), Some(&local), false).unwrap(),
            TrustClass::MeasuredLocal
        );
        let ci = CaptureEvidence::MeasuredCi {
            run_ref: "runs/42".to_string(),
        };
        assert!(derive_trust_class(Some("hook"), Some(&ci), true).is_err());
        let human = CaptureEvidence::HumanConfirmed { by: None };
        assert!(derive_trust_class(Some("hook"), Some(&human), false).is_err());
    }

    #[test]
    fn derive_cli_can_substantiate_all_three_ci_needs_a_commit_anchor() {
        let local = CaptureEvidence::MeasuredLocal {
            commands: vec!["cargo test".to_string()],
        };
        let ci = CaptureEvidence::MeasuredCi {
            run_ref: "runs/42".to_string(),
        };
        let human = CaptureEvidence::HumanConfirmed { by: None };
        assert_eq!(
            derive_trust_class(Some("cli"), Some(&local), false).unwrap(),
            TrustClass::MeasuredLocal
        );
        assert_eq!(
            derive_trust_class(Some("cli"), Some(&human), false).unwrap(),
            TrustClass::HumanConfirmed
        );
        assert!(derive_trust_class(Some("cli"), Some(&ci), false).is_err());
        assert_eq!(
            derive_trust_class(Some("cli"), Some(&ci), true).unwrap(),
            TrustClass::MeasuredCi
        );
    }

    #[test]
    fn derive_rejects_narrative_channels_entirely() {
        let local = CaptureEvidence::MeasuredLocal {
            commands: vec!["cargo test".to_string()],
        };
        for channel in ["mcp", "http", "job", "agent"] {
            assert!(
                derive_trust_class(Some(channel), Some(&local), true).is_err(),
                "channel {channel} must not substantiate evidence"
            );
        }
        assert!(derive_trust_class(None, Some(&local), true).is_err());
    }

    #[test]
    fn derive_rejects_malformed_evidence_first() {
        let empty = CaptureEvidence::MeasuredLocal { commands: vec![] };
        assert!(derive_trust_class(Some("hook"), Some(&empty), false).is_err());
    }
}
