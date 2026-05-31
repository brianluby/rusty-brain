use crate::error::Error;
use crate::error::Result;
use serde::{Deserialize, Serialize};

/// Scope a memory belongs to. DB forms (exact):
/// `global` | `project:{name}` | `session:{project}:{session_id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Namespace {
    Global,
    Project(String),
    Session { project: String, session_id: String },
}

impl Namespace {
    /// Lower value = narrower/higher precedence. Session=0, Project=1, Global=2.
    pub fn priority(&self) -> u8 {
        match self {
            Namespace::Session { .. } => 0,
            Namespace::Project(_) => 1,
            Namespace::Global => 2,
        }
    }

    /// Serialize to the exact string stored in the `namespace` column.
    pub fn as_db_string(&self) -> String {
        match self {
            Namespace::Global => "global".to_string(),
            Namespace::Project(name) => format!("project:{name}"),
            Namespace::Session {
                project,
                session_id,
            } => format!("session:{project}:{session_id}"),
        }
    }

    /// Parse a db string back into a `Namespace`. Fail closed on anything unrecognized.
    pub fn parse_db_string(s: &str) -> Result<Self> {
        if s == "global" {
            return Ok(Namespace::Global);
        }
        if let Some(name) = s.strip_prefix("project:") {
            if name.is_empty() {
                return Err(Error::InvalidNamespace(s.to_string()));
            }
            return Ok(Namespace::Project(name.to_string()));
        }
        if let Some(rest) = s.strip_prefix("session:") {
            // Split into project and session_id on the FIRST colon only;
            // session_id may itself contain colons.
            if let Some((project, session_id)) = rest.split_once(':') {
                if !project.is_empty() && !session_id.is_empty() {
                    return Ok(Namespace::Session {
                        project: project.to_string(),
                        session_id: session_id.to_string(),
                    });
                }
            }
            return Err(Error::InvalidNamespace(s.to_string()));
        }
        Err(Error::InvalidNamespace(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::Error;

    #[test]
    fn priority_orders_session_project_global() {
        assert_eq!(Namespace::Global.priority(), 2);
        assert_eq!(Namespace::Project("p".into()).priority(), 1);
        assert_eq!(
            Namespace::Session {
                project: "p".into(),
                session_id: "s".into(),
            }
            .priority(),
            0
        );
        // session is highest priority (smallest number)
        let mut ns = [
            Namespace::Global,
            Namespace::Session {
                project: "p".into(),
                session_id: "s".into(),
            },
            Namespace::Project("p".into()),
        ];
        ns.sort_by_key(|n| n.priority());
        assert!(matches!(ns[0], Namespace::Session { .. }));
        assert!(matches!(ns[1], Namespace::Project(_)));
        assert!(matches!(ns[2], Namespace::Global));
    }

    #[test]
    fn db_strings_match_exact_forms() {
        assert_eq!(Namespace::Global.as_db_string(), "global");
        assert_eq!(
            Namespace::Project("rusty-brain".into()).as_db_string(),
            "project:rusty-brain"
        );
        assert_eq!(
            Namespace::Session {
                project: "rusty-brain".into(),
                session_id: "abc123".into(),
            }
            .as_db_string(),
            "session:rusty-brain:abc123"
        );
    }

    #[test]
    fn parse_db_string_round_trips_all_variants() {
        for ns in [
            Namespace::Global,
            Namespace::Project("rusty-brain".into()),
            Namespace::Session {
                project: "rusty-brain".into(),
                session_id: "abc123".into(),
            },
        ] {
            let s = ns.as_db_string();
            let back = Namespace::parse_db_string(&s).unwrap();
            assert_eq!(ns, back);
        }
    }

    #[test]
    fn parse_session_keeps_session_id_with_colons() {
        // session_id may itself contain colons; only the first two colons delimit.
        let ns = Namespace::parse_db_string("session:proj:sid:with:colons").unwrap();
        assert_eq!(
            ns,
            Namespace::Session {
                project: "proj".into(),
                session_id: "sid:with:colons".into(),
            }
        );
    }

    #[test]
    fn parse_rejects_unknown_and_empty() {
        assert!(matches!(
            Namespace::parse_db_string("bogus").unwrap_err(),
            Error::InvalidNamespace(_)
        ));
        assert!(matches!(
            Namespace::parse_db_string("project:").unwrap_err(),
            Error::InvalidNamespace(_)
        ));
        assert!(matches!(
            Namespace::parse_db_string("session:onlyproject").unwrap_err(),
            Error::InvalidNamespace(_)
        ));
    }

    #[test]
    fn serde_json_round_trip() {
        let ns = Namespace::Session {
            project: "p".into(),
            session_id: "s".into(),
        };
        let json = serde_json::to_string(&ns).unwrap();
        let back: Namespace = serde_json::from_str(&json).unwrap();
        assert_eq!(ns, back);
    }
}
