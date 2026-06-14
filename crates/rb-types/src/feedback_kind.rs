use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A usefulness signal a caller reports about a recalled memory (W3.7, F37).
///
/// Distinct from `access_count`, which counts how often a memory was RETURNED,
/// not whether it was actually useful. `memory_feedback` is the explicit
/// usefulness/correctness signal the W2.2 confidence machinery and the W5c
/// per-author trust weighting need.
///
/// Serializes in `snake_case` (matching the `JobKind` precedent); the variants
/// are single lowercase words, so the wire / DB / CLI strings are
/// `helpful` | `wrong` | `stale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    /// The memory was useful in the context it was recalled into.
    Helpful,
    /// The memory is incorrect — strong evidence against its trustworthiness.
    Wrong,
    /// The memory was once correct but is now outdated.
    Stale,
}

impl FeedbackKind {
    /// Stable string form used by the CLI `--kind` argument, the MCP tool enum,
    /// and the `memory_oplog` / `memory_feedback` `kind` column. MUST stay in
    /// lockstep with the `serde(rename_all = "snake_case")` form AND the
    /// `migration 008` `CHECK (kind IN (...))` constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            FeedbackKind::Helpful => "helpful",
            FeedbackKind::Wrong => "wrong",
            FeedbackKind::Stale => "stale",
        }
    }

    /// Parse a CLI/MCP/db string into a `FeedbackKind`. Fail closed on unknown
    /// values (the `JobKind` precedent: a parse failure is `InvalidArgument`, so
    /// no dedicated `Error` variant or `error_map` arm is needed — the parse
    /// result is consumed at the CLI/MCP boundary, never travels the wire).
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "helpful" => Ok(FeedbackKind::Helpful),
            "wrong" => Ok(FeedbackKind::Wrong),
            "stale" => Ok(FeedbackKind::Stale),
            other => Err(Error::InvalidArgument(format!(
                "unknown feedback kind: {other} (expected helpful, wrong, or stale)"
            ))),
        }
    }

    /// The bounded confidence nudge this feedback applies to the target memory's
    /// trust prior (W3.7 confidence coupling — single-axis: all three kinds move
    /// `confidence`). `helpful` raises it toward full trust; `wrong`/`stale`
    /// lower it, `wrong` more sharply than `stale` (incorrect outweighs merely
    /// outdated). The store clamps `confidence + delta` to the canonical
    /// `0.0..=1.0` range, so a single nudge can never push a memory out of range
    /// and several `wrong` reports are required to fully erode trust.
    pub fn confidence_delta(&self) -> f32 {
        match self {
            FeedbackKind::Helpful => 0.05,
            FeedbackKind::Wrong => -0.30,
            FeedbackKind::Stale => -0.15,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const ALL: [FeedbackKind; 3] = [
        FeedbackKind::Helpful,
        FeedbackKind::Wrong,
        FeedbackKind::Stale,
    ];

    #[test]
    fn serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&FeedbackKind::Helpful).unwrap(),
            r#""helpful""#
        );
        assert_eq!(
            serde_json::to_string(&FeedbackKind::Wrong).unwrap(),
            r#""wrong""#
        );
        assert_eq!(
            serde_json::to_string(&FeedbackKind::Stale).unwrap(),
            r#""stale""#
        );
    }

    #[test]
    fn parse_is_inverse_of_as_str_for_all_variants() {
        for kind in ALL {
            assert_eq!(FeedbackKind::parse(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn serde_round_trips_all_variants() {
        for kind in ALL {
            let json = serde_json::to_string(&kind).unwrap();
            let back: FeedbackKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        let err = FeedbackKind::parse("useless").unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    #[test]
    fn helpful_raises_and_wrong_stale_lower_with_wrong_sharpest() {
        assert!(FeedbackKind::Helpful.confidence_delta() > 0.0);
        assert!(FeedbackKind::Wrong.confidence_delta() < 0.0);
        assert!(FeedbackKind::Stale.confidence_delta() < 0.0);
        // `wrong` (incorrect) erodes trust more sharply than `stale` (outdated).
        assert!(FeedbackKind::Wrong.confidence_delta() < FeedbackKind::Stale.confidence_delta());
    }
}
