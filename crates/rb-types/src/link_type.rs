use crate::error::Error;
use crate::error::Result;
use serde::{Deserialize, Serialize};

/// Relationship between two memories. `as_str` values match the SQL CHECK exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkType {
    Extends,
    Contradicts,
    Implements,
    References,
    Supersedes,
}

impl LinkType {
    /// Stable db string. MUST stay in lockstep with the SQL CHECK constraint.
    pub fn as_str(&self) -> &'static str {
        match self {
            LinkType::Extends => "extends",
            LinkType::Contradicts => "contradicts",
            LinkType::Implements => "implements",
            LinkType::References => "references",
            LinkType::Supersedes => "supersedes",
        }
    }

    /// Parse a db string into a `LinkType`. Fail closed on unknown values.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "extends" => Ok(LinkType::Extends),
            "contradicts" => Ok(LinkType::Contradicts),
            "implements" => Ok(LinkType::Implements),
            "references" => Ok(LinkType::References),
            "supersedes" => Ok(LinkType::Supersedes),
            other => Err(Error::InvalidLinkType(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::Error;

    const ALL: [LinkType; 5] = [
        LinkType::Extends,
        LinkType::Contradicts,
        LinkType::Implements,
        LinkType::References,
        LinkType::Supersedes,
    ];

    #[test]
    fn as_str_matches_sql_check_values() {
        assert_eq!(LinkType::Extends.as_str(), "extends");
        assert_eq!(LinkType::Contradicts.as_str(), "contradicts");
        assert_eq!(LinkType::Implements.as_str(), "implements");
        assert_eq!(LinkType::References.as_str(), "references");
        assert_eq!(LinkType::Supersedes.as_str(), "supersedes");
    }

    #[test]
    fn parse_is_inverse_of_as_str_for_all_variants() {
        for lt in ALL {
            assert_eq!(LinkType::parse(lt.as_str()).unwrap(), lt);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        let err = LinkType::parse("depends_on").unwrap_err();
        assert!(matches!(err, Error::InvalidLinkType(_)));
    }

    #[test]
    fn serde_json_round_trip() {
        let lt = LinkType::Supersedes;
        let json = serde_json::to_string(&lt).unwrap();
        let back: LinkType = serde_json::from_str(&json).unwrap();
        assert_eq!(lt, back);
    }
}
