use crate::error::Error;
use crate::error::Result;
use serde::{Deserialize, Serialize};

/// Category of a memory. `as_str` values match the `memory_type` SQL CHECK exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryType {
    ArchitectureDecision,
    CodePattern,
    BugFix,
    Configuration,
    Constraint,
    Entity,
    Insight,
    Reference,
    Preference,
}

impl MemoryType {
    /// Every variant, in declaration order — the canonical list that
    /// doc-anchoring tests (e.g. `rb-agents/tests/cognition_docs.rs`) and
    /// round-trip tests derive from instead of hand-maintaining copies.
    ///
    /// Compiler-pinned: every element is produced through `pin`, whose
    /// exhaustive match stops compiling the moment a variant is added — the
    /// error lands HERE, and the fix is to extend the match, the returned
    /// array, and the array length in this signature in the same edit.
    pub fn all() -> [MemoryType; 9] {
        // The identity match is the point: `t` would compile through a new
        // variant silently, the exhaustive match cannot.
        #[allow(clippy::needless_match)]
        const fn pin(t: MemoryType) -> MemoryType {
            match t {
                MemoryType::ArchitectureDecision => MemoryType::ArchitectureDecision,
                MemoryType::CodePattern => MemoryType::CodePattern,
                MemoryType::BugFix => MemoryType::BugFix,
                MemoryType::Configuration => MemoryType::Configuration,
                MemoryType::Constraint => MemoryType::Constraint,
                MemoryType::Entity => MemoryType::Entity,
                MemoryType::Insight => MemoryType::Insight,
                MemoryType::Reference => MemoryType::Reference,
                MemoryType::Preference => MemoryType::Preference,
            }
        }
        [
            pin(MemoryType::ArchitectureDecision),
            pin(MemoryType::CodePattern),
            pin(MemoryType::BugFix),
            pin(MemoryType::Configuration),
            pin(MemoryType::Constraint),
            pin(MemoryType::Entity),
            pin(MemoryType::Insight),
            pin(MemoryType::Reference),
            pin(MemoryType::Preference),
        ]
    }

    /// Stable db string. MUST stay in lockstep with the SQL CHECK constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::ArchitectureDecision => "architecture_decision",
            MemoryType::CodePattern => "code_pattern",
            MemoryType::BugFix => "bug_fix",
            MemoryType::Configuration => "configuration",
            MemoryType::Constraint => "constraint",
            MemoryType::Entity => "entity",
            MemoryType::Insight => "insight",
            MemoryType::Reference => "reference",
            MemoryType::Preference => "preference",
        }
    }

    /// Parse a db string into a `MemoryType`. Fail closed on unknown values.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "architecture_decision" => Ok(MemoryType::ArchitectureDecision),
            "code_pattern" => Ok(MemoryType::CodePattern),
            "bug_fix" => Ok(MemoryType::BugFix),
            "configuration" => Ok(MemoryType::Configuration),
            "constraint" => Ok(MemoryType::Constraint),
            "entity" => Ok(MemoryType::Entity),
            "insight" => Ok(MemoryType::Insight),
            "reference" => Ok(MemoryType::Reference),
            "preference" => Ok(MemoryType::Preference),
            other => Err(Error::InvalidMemoryType(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::Error;

    #[test]
    fn all_is_duplicate_free() {
        // `all()` is compiler-pinned for MISSING variants (the exhaustive
        // `pin` match); this pins the other failure shape — the same variant
        // listed twice while another is dropped from the returned array.
        let all = MemoryType::all();
        let distinct: std::collections::HashSet<&str> = all.iter().map(|t| t.as_str()).collect();
        assert_eq!(
            distinct.len(),
            all.len(),
            "MemoryType::all() repeats a variant"
        );
    }

    #[test]
    fn as_str_matches_sql_check_values() {
        assert_eq!(
            MemoryType::ArchitectureDecision.as_str(),
            "architecture_decision"
        );
        assert_eq!(MemoryType::CodePattern.as_str(), "code_pattern");
        assert_eq!(MemoryType::BugFix.as_str(), "bug_fix");
        assert_eq!(MemoryType::Configuration.as_str(), "configuration");
        assert_eq!(MemoryType::Constraint.as_str(), "constraint");
        assert_eq!(MemoryType::Entity.as_str(), "entity");
        assert_eq!(MemoryType::Insight.as_str(), "insight");
        assert_eq!(MemoryType::Reference.as_str(), "reference");
        assert_eq!(MemoryType::Preference.as_str(), "preference");
    }

    #[test]
    fn parse_is_inverse_of_as_str_for_all_variants() {
        for mt in MemoryType::all() {
            assert_eq!(MemoryType::parse(mt.as_str()).unwrap(), mt);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        let err = MemoryType::parse("nonsense").unwrap_err();
        assert!(matches!(err, Error::InvalidMemoryType(_)));
    }

    #[test]
    fn serde_json_round_trip() {
        let mt = MemoryType::BugFix;
        let json = serde_json::to_string(&mt).unwrap();
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(mt, back);
    }
}
