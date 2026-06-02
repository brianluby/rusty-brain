use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Which background maintenance ("evolution") job to run. Shared by `rb-proto`
/// (wire) and `rb-daemon` (`jobs` module) so neither needs to depend on the
/// other. Serializes in `snake_case` to match the TOML config section names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    LinkDecay,
    Consolidation,
    ImportanceRecalibration,
}

impl JobKind {
    /// Stable string form used by the CLI `evolve <job>` argument and logs.
    /// MUST stay in lockstep with the `serde(rename_all = "snake_case")` form.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobKind::LinkDecay => "link_decay",
            JobKind::Consolidation => "consolidation",
            JobKind::ImportanceRecalibration => "importance_recalibration",
        }
    }

    /// Parse the CLI/db string into a `JobKind`. Fail closed on unknown values.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "link_decay" => Ok(JobKind::LinkDecay),
            "consolidation" => Ok(JobKind::Consolidation),
            "importance_recalibration" => Ok(JobKind::ImportanceRecalibration),
            other => Err(Error::InvalidArgument(format!("unknown job: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const ALL: [JobKind; 3] = [
        JobKind::LinkDecay,
        JobKind::Consolidation,
        JobKind::ImportanceRecalibration,
    ];

    #[test]
    fn serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&JobKind::LinkDecay).unwrap(),
            r#""link_decay""#
        );
        assert_eq!(
            serde_json::to_string(&JobKind::Consolidation).unwrap(),
            r#""consolidation""#
        );
        assert_eq!(
            serde_json::to_string(&JobKind::ImportanceRecalibration).unwrap(),
            r#""importance_recalibration""#
        );
    }

    #[test]
    fn serde_round_trips_all_variants() {
        for kind in ALL {
            let json = serde_json::to_string(&kind).unwrap();
            let back: JobKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn parse_is_inverse_of_as_str() {
        for kind in ALL {
            assert_eq!(JobKind::parse(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        let err = JobKind::parse("garbage").unwrap_err();
        assert!(matches!(err, crate::Error::InvalidArgument(_)));
    }

    #[test]
    fn copy_and_eq_hold() {
        let k = JobKind::LinkDecay;
        let copied = k; // Copy
        assert_eq!(k, copied);
    }
}
