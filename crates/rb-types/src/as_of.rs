//! Recording-time point-in-time list payloads (Vikunja #102, step 1 of the
//! #88 assessment `docs/eval/2026-10-18-point-in-time-recall-assessment.md`).
//!
//! An `as_of` list answers "what did the store hold as active at instant T?"
//! from columns that already exist: membership is
//! `created_at <= T AND (archived_at IS NULL OR archived_at > T)`. That is
//! well defined because `archived_at` is write-once. It is RECORDING time,
//! not validity time, it is exact only to the whole second, metadata values
//! (summary, tags, importance, confidence) are the CURRENT ones, and purged
//! rows never appear. Every response says so through [`AsOfPage`].

use crate::MemoryId;
use serde::{Deserialize, Serialize};

/// Which clock an as-of answer is measured on. Only recording (transaction)
/// time exists today; validity/event time is deferred by #88.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalBasis {
    /// When rusty-brain wrote (or archived) the row, not when the fact held.
    Recorded,
}

/// The finest instant an as-of answer can distinguish. Timestamps are stored
/// as whole Unix seconds, so a row created and archived within the same
/// second (`created_at = archived_at`) is never a member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimePrecision {
    Second,
}

/// Which point in time the returned metadata reflects. Summary, tags,
/// importance and confidence are overwritten in place, so they are always
/// the current values, never the values as of T.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataBasis {
    Current,
}

/// Keyset cursor for an `as_of` list ordered by `(created_at, memory_id)`
/// ascending: the next page holds the rows strictly after this pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsOfCursor {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub memory_id: MemoryId,
}

impl AsOfCursor {
    /// `true` when `(created_at, id)` sorts strictly after this cursor, using
    /// the store's whole-second `created_at` and textual `memory_id` order.
    #[must_use]
    pub fn admits(&self, created_at: chrono::DateTime<chrono::Utc>, id: &MemoryId) -> bool {
        let (row, cursor) = (created_at.timestamp(), self.created_at.timestamp());
        row > cursor || (row == cursor && id.to_string() > self.memory_id.to_string())
    }

    /// The CLI token form, `<RFC 3339 created_at>/<memory_id>`.
    #[must_use]
    pub fn to_token(&self) -> String {
        format!(
            "{}/{}",
            self.created_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            self.memory_id
        )
    }

    /// Parse [`AsOfCursor::to_token`]'s form.
    pub fn parse_token(token: &str) -> crate::Result<Self> {
        let invalid = || {
            crate::Error::InvalidArgument(format!(
                "invalid as-of cursor '{token}': expected <RFC 3339 created_at>/<memory id>"
            ))
        };
        let (ts, id) = token.rsplit_once('/').ok_or_else(invalid)?;
        let created_at = chrono::DateTime::parse_from_rfc3339(ts)
            .map_err(|_| invalid())?
            .with_timezone(&chrono::Utc);
        let memory_id = id.parse::<MemoryId>().map_err(|_| invalid())?;
        Ok(Self {
            created_at,
            memory_id,
        })
    }
}

/// The as-of envelope a `list` response carries when the request set
/// `RecallFilter.as_of`. Its presence is the client's proof that the daemon
/// applied the as-of predicate rather than silently answering an ordinary
/// active-only list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsOfPage {
    /// The instant the membership predicate was evaluated at.
    pub as_of: chrono::DateTime<chrono::Utc>,
    pub temporal_basis: TemporalBasis,
    pub precision: TimePrecision,
    pub metadata: MetadataBasis,
    /// More members exist past this page (the daemon fetched one row past
    /// the effective limit).
    pub truncated: bool,
    /// Present iff `truncated`: pass it back as `RecallFilter.as_of_after`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<AsOfCursor>,
    /// Human-readable limits of this answer.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// The limitations every as-of answer states, verbatim in the output.
pub const AS_OF_NOTES: [&str; 4] = [
    "membership is as recorded (when rusty-brain wrote or archived each row), not when facts were true",
    "metadata (summary, tags, importance, confidence) is the current value, not the value as of T",
    "precision is whole seconds: a row created and archived in the same second is never a member",
    "purged rows never appear",
];

impl AsOfPage {
    /// Build the envelope for one page.
    #[must_use]
    pub fn recorded(as_of: chrono::DateTime<chrono::Utc>, next_after: Option<AsOfCursor>) -> Self {
        Self {
            as_of,
            temporal_basis: TemporalBasis::Recorded,
            precision: TimePrecision::Second,
            metadata: MetadataBasis::Current,
            truncated: next_after.is_some(),
            next_after,
            notes: AS_OF_NOTES.iter().map(|n| (*n).to_string()).collect(),
        }
    }
}

/// Recording-time membership at whole-second instant `as_of`:
/// `created_at <= T AND (archived_at IS NULL OR archived_at > T)`, compared on
/// whole seconds like the store's SQL.
#[must_use]
pub fn member_as_of(
    created_at: chrono::DateTime<chrono::Utc>,
    archived_at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: chrono::DateTime<chrono::Utc>,
) -> bool {
    let t = as_of.timestamp();
    created_at.timestamp() <= t && archived_at.is_none_or(|a| a.timestamp() > t)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use chrono::TimeZone;

    fn at(s: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc.timestamp_opt(s, 0).unwrap()
    }

    #[test]
    fn membership_boundaries() {
        // At exactly created_at: included; at exactly archived_at: excluded.
        assert!(member_as_of(at(10), Some(at(20)), at(10)));
        assert!(member_as_of(at(10), Some(at(20)), at(19)));
        assert!(!member_as_of(at(10), Some(at(20)), at(20)));
        assert!(!member_as_of(at(10), Some(at(20)), at(9)));
        assert!(member_as_of(at(10), None, at(10_000)));
        // Same-second create and archive: never a member.
        assert!(!member_as_of(at(10), Some(at(10)), at(10)));
    }

    #[test]
    fn page_serializes_basis_precision_and_cursor() {
        let id = MemoryId::new();
        let page = AsOfPage::recorded(
            at(100),
            Some(AsOfCursor {
                created_at: at(50),
                memory_id: id.clone(),
            }),
        );
        let v = serde_json::to_value(&page).unwrap();
        assert_eq!(v["temporal_basis"], "recorded");
        assert_eq!(v["precision"], "second");
        assert_eq!(v["metadata"], "current");
        assert_eq!(v["truncated"], true);
        assert_eq!(v["next_after"]["memory_id"], id.to_string());
        let back: AsOfPage = serde_json::from_value(v).unwrap();
        assert_eq!(back, page);
        let untruncated = serde_json::to_value(AsOfPage::recorded(at(1), None)).unwrap();
        assert_eq!(untruncated["truncated"], false);
        assert!(untruncated.get("next_after").is_none());
    }

    #[test]
    fn cursor_token_round_trips_and_orders_by_second_then_id() {
        let id = MemoryId::new();
        let cursor = AsOfCursor {
            created_at: at(1_700_000_000),
            memory_id: id.clone(),
        };
        assert_eq!(AsOfCursor::parse_token(&cursor.to_token()).unwrap(), cursor);
        assert!(AsOfCursor::parse_token("nope").is_err());
        assert!(AsOfCursor::parse_token("2026-01-01T00:00:00Z/not-a-uuid").is_err());
        assert!(!cursor.admits(at(1_700_000_000), &id));
        assert!(cursor.admits(at(1_700_000_001), &id));
        assert!(!cursor.admits(at(1_699_999_999), &id));
    }
}
