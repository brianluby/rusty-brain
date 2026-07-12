//! Typed code anchors (PRD 2026-07-02): a structured, queryable link from a
//! memory to a code location — a file path (+ optional line range), a commit
//! SHA, or a symbol/identifier. Anchors are caller-supplied strings, never
//! resolved AST (PRD non-goal), and multiple anchors per memory are allowed.

use crate::query::AnchorKind;
use serde::{Deserialize, Serialize};

/// One code anchor attached to a memory.
///
/// `value` is the normalized anchor payload for `kind`: a file path (`file`),
/// a commit SHA (`commit`), or a symbol name (`symbol`). `start_line`/
/// `end_line` are 1-based and only meaningful for `file` anchors.
///
/// Wire-compat: both line fields are `#[serde(default, skip_serializing_if)]`
/// so a line-less anchor serializes to `{kind, value}` only, and
/// `MemoryNote.anchors` itself is `#[serde(default)]` — pre-anchor payloads
/// (and old daemons/clients) decode to an empty list (the `contested`
/// additive-field precedent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAnchor {
    pub kind: AnchorKind,
    /// Normalized file path / commit SHA / symbol name (see
    /// [`normalize_anchor_value`]).
    pub value: String,
    /// First line of the anchored range (1-based; `file` anchors only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    /// Last line of the anchored range (inclusive, 1-based; `file` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
}

/// Normalize a raw anchor value for storage and matching: trim surrounding
/// whitespace and, for `file` anchors, strip any leading `./` segments so
/// `./src/foo.rs` and `src/foo.rs` are the same anchor. Paths are stored as
/// given otherwise (repo-relative by convention — the daemon cannot resolve a
/// repo root; path drift across machines is a documented PRD limitation).
/// May return an empty string; [`MemoryAnchor::validate`] rejects that.
#[must_use]
pub fn normalize_anchor_value(kind: AnchorKind, raw: &str) -> String {
    let mut v = raw.trim();
    if kind == AnchorKind::File {
        while let Some(rest) = v.strip_prefix("./") {
            v = rest.trim_start();
        }
    }
    v.to_string()
}

impl MemoryAnchor {
    /// Build a line-less anchor from a raw value: normalize, then validate
    /// (fail-closed on an empty value).
    pub fn new(kind: AnchorKind, raw_value: &str) -> crate::error::Result<Self> {
        let anchor = MemoryAnchor {
            kind,
            value: normalize_anchor_value(kind, raw_value),
            start_line: None,
            end_line: None,
        };
        anchor.validate()?;
        Ok(anchor)
    }

    /// Parse a CLI/MCP `--file` capture spec: `PATH`, `PATH:LINE`, or
    /// `PATH:START-END` (1-based, inclusive). The suffix after the LAST `:`
    /// is treated as a line range ONLY when it starts with an ASCII digit —
    /// and then it MUST parse (garbage like `foo.rs:12-` is a clean error,
    /// never silently part of the path). Paths whose last segment does not
    /// start with a digit keep their colons (`C:\proj\a.rs` stays a path).
    pub fn parse_file_spec(spec: &str) -> crate::error::Result<Self> {
        if let Some((path, suffix)) = spec.rsplit_once(':') {
            if suffix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                let (start, end) = parse_line_range(suffix)?;
                let mut anchor = Self::new(AnchorKind::File, path)?;
                anchor.start_line = Some(start);
                anchor.end_line = Some(end);
                anchor.validate()?;
                return Ok(anchor);
            }
        }
        Self::new(AnchorKind::File, spec)
    }

    /// Fail-fast boundary validation: a non-empty normalized value; line
    /// fields only on `file` anchors, 1-based, with `start <= end` and never
    /// an end without a start.
    pub fn validate(&self) -> crate::error::Result<()> {
        let invalid = |msg: String| crate::error::Error::InvalidArgument(msg);
        if self.value.trim().is_empty() {
            return Err(invalid(format!(
                "{} anchor value must not be empty",
                kind_str(self.kind)
            )));
        }
        if self.kind != AnchorKind::File && (self.start_line.is_some() || self.end_line.is_some()) {
            return Err(invalid(format!(
                "line ranges are only valid on file anchors, not {}",
                kind_str(self.kind)
            )));
        }
        if self.end_line.is_some() && self.start_line.is_none() {
            return Err(invalid("anchor end_line requires a start_line".to_string()));
        }
        for line in [self.start_line, self.end_line].into_iter().flatten() {
            if line == 0 {
                return Err(invalid(
                    "anchor lines are 1-based; 0 is invalid".to_string(),
                ));
            }
        }
        if let (Some(start), Some(end)) = (self.start_line, self.end_line) {
            if start > end {
                return Err(invalid(format!(
                    "anchor line range {start}-{end} is inverted"
                )));
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for MemoryAnchor {
    /// Compact human label: `file:src/a.rs:12-40`, `file:src/a.rs:12`,
    /// `commit:abc123`, `symbol:Foo::bar`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", kind_str(self.kind), self.value)?;
        match (self.start_line, self.end_line) {
            (Some(s), Some(e)) if s == e => write!(f, ":{s}"),
            (Some(s), Some(e)) => write!(f, ":{s}-{e}"),
            (Some(s), None) => write!(f, ":{s}"),
            _ => Ok(()),
        }
    }
}

/// Parse a recall/list `--file` FILTER value into an [`crate::AnchorFilter`]
/// (shared by the CLI and MCP so the surfaces agree). Filters match by PATH
/// only in v1, so a `:LINE`/`:START-END` suffix is a clean error — silently
/// stripping it would widen the query behind the caller's back, and treating
/// it as part of the path would silently match nothing.
pub fn parse_file_filter(spec: &str) -> crate::error::Result<crate::AnchorFilter> {
    let anchor = MemoryAnchor::parse_file_spec(spec)?;
    if anchor.start_line.is_some() || anchor.end_line.is_some() {
        return Err(crate::error::Error::InvalidArgument(format!(
            "file filters match by path only; drop the line range from '{spec}' \
             (line ranges are for capture, e.g. `remember --file {spec}`)"
        )));
    }
    let filter = crate::AnchorFilter {
        kind: AnchorKind::File,
        value: anchor.value,
    };
    filter.validate()?;
    Ok(filter)
}

/// The canonical db/display string for an anchor kind (mirrors the serde
/// `snake_case` rename on [`AnchorKind`]; pinned by a test).
#[must_use]
pub fn kind_str(kind: AnchorKind) -> &'static str {
    match kind {
        AnchorKind::File => "file",
        AnchorKind::Commit => "commit",
        AnchorKind::Symbol => "symbol",
    }
}

/// Parse `kind_str` back to an [`AnchorKind`] (store decode path).
pub fn parse_kind(s: &str) -> crate::error::Result<AnchorKind> {
    match s {
        "file" => Ok(AnchorKind::File),
        "commit" => Ok(AnchorKind::Commit),
        "symbol" => Ok(AnchorKind::Symbol),
        other => Err(crate::error::Error::InvalidArgument(format!(
            "unknown anchor kind '{other}': expected file, commit, or symbol"
        ))),
    }
}

/// Parse the `LINE` / `START-END` suffix of a file spec. The caller has
/// already established the suffix starts with a digit, so anything that does
/// not fully parse is a hard error (fail fast, never a silent path).
fn parse_line_range(suffix: &str) -> crate::error::Result<(u32, u32)> {
    let invalid = || {
        crate::error::Error::InvalidArgument(format!(
            "invalid line range '{suffix}': expected LINE or START-END (1-based)"
        ))
    };
    let (start_raw, end_raw) = match suffix.split_once('-') {
        Some((s, e)) => (s, e),
        None => (suffix, suffix),
    };
    let start: u32 = start_raw.parse().map_err(|_| invalid())?;
    let end: u32 = end_raw.parse().map_err(|_| invalid())?;
    if start == 0 || end == 0 {
        return Err(crate::error::Error::InvalidArgument(format!(
            "invalid line range '{suffix}': lines are 1-based"
        )));
    }
    if start > end {
        return Err(crate::error::Error::InvalidArgument(format!(
            "invalid line range '{suffix}': start exceeds end"
        )));
    }
    Ok((start, end))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn new_normalizes_and_validates() {
        let a = MemoryAnchor::new(AnchorKind::File, "  ./src/foo.rs  ").unwrap();
        assert_eq!(a.value, "src/foo.rs");
        assert_eq!(a.start_line, None);
        assert_eq!(a.end_line, None);

        let c = MemoryAnchor::new(AnchorKind::Commit, " abc123 ").unwrap();
        assert_eq!(c.value, "abc123");

        let s = MemoryAnchor::new(AnchorKind::Symbol, "Foo::bar").unwrap();
        assert_eq!(s.value, "Foo::bar");
    }

    #[test]
    fn new_rejects_empty_values() {
        for kind in [AnchorKind::File, AnchorKind::Commit, AnchorKind::Symbol] {
            for raw in ["", "   ", "./", "././"] {
                let res = MemoryAnchor::new(kind, raw);
                if kind == AnchorKind::File || raw.trim().is_empty() {
                    assert!(res.is_err(), "{kind:?} anchor {raw:?} must be rejected");
                }
            }
        }
    }

    #[test]
    fn normalize_strips_leading_dot_slash_for_files_only() {
        assert_eq!(
            normalize_anchor_value(AnchorKind::File, "././src/a.rs"),
            "src/a.rs"
        );
        // Non-file kinds keep the raw (trimmed) value.
        assert_eq!(
            normalize_anchor_value(AnchorKind::Symbol, "./looks::weird"),
            "./looks::weird"
        );
    }

    #[test]
    fn parse_file_spec_plain_path() {
        let a = MemoryAnchor::parse_file_spec("src/server.rs").unwrap();
        assert_eq!(a.value, "src/server.rs");
        assert_eq!((a.start_line, a.end_line), (None, None));
    }

    #[test]
    fn parse_file_spec_single_line_and_range() {
        let single = MemoryAnchor::parse_file_spec("src/a.rs:12").unwrap();
        assert_eq!(single.value, "src/a.rs");
        assert_eq!((single.start_line, single.end_line), (Some(12), Some(12)));

        let range = MemoryAnchor::parse_file_spec("src/a.rs:12-40").unwrap();
        assert_eq!((range.start_line, range.end_line), (Some(12), Some(40)));
    }

    #[test]
    fn parse_file_spec_keeps_non_numeric_colon_suffixes_as_path() {
        // A Windows-style or otherwise colon-bearing path whose final segment
        // does not start with a digit stays a path.
        let a = MemoryAnchor::parse_file_spec("C:\\proj\\a.rs").unwrap();
        assert_eq!(a.value, "C:\\proj\\a.rs");
        assert_eq!((a.start_line, a.end_line), (None, None));
    }

    #[test]
    fn parse_file_spec_rejects_garbage_ranges_cleanly() {
        // Once the suffix starts with a digit it MUST be a valid range —
        // trailing dashes, inverted ranges, zero lines, overflow, and junk are
        // clean errors (never a panic, never silently part of the path).
        for bad in [
            "a.rs:12-",
            "a.rs:12-9",
            "a.rs:0",
            "a.rs:0-4",
            "a.rs:12-40-2",
            "a.rs:12x",
            "a.rs:99999999999999999999",
            "a.rs:1-99999999999999999999",
            ":12",
        ] {
            let res = MemoryAnchor::parse_file_spec(bad);
            assert!(res.is_err(), "{bad:?} must be rejected, got {res:?}");
            assert!(
                matches!(res, Err(crate::error::Error::InvalidArgument(_))),
                "{bad:?} must be InvalidArgument"
            );
        }
    }

    #[test]
    fn validate_rejects_lines_on_non_file_kinds_and_bad_ranges() {
        let mut commit = MemoryAnchor::new(AnchorKind::Commit, "abc").unwrap();
        commit.start_line = Some(1);
        assert!(commit.validate().is_err(), "lines on a commit anchor");

        let mut file = MemoryAnchor::new(AnchorKind::File, "a.rs").unwrap();
        file.end_line = Some(4);
        assert!(file.validate().is_err(), "end without start");

        file.start_line = Some(9);
        assert!(file.validate().is_err(), "inverted range");

        file.start_line = Some(0);
        file.end_line = None;
        assert!(file.validate().is_err(), "0 is not a 1-based line");
    }

    #[test]
    fn open_ended_start_line_is_valid() {
        let mut file = MemoryAnchor::new(AnchorKind::File, "a.rs").unwrap();
        file.start_line = Some(3);
        assert!(file.validate().is_ok(), "start-only range is allowed");
    }

    #[test]
    fn serde_round_trip_and_lineless_shape_is_minimal() {
        let a = MemoryAnchor::parse_file_spec("src/a.rs:3-9").unwrap();
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["kind"], "file");
        assert_eq!(json["value"], "src/a.rs");
        assert_eq!(json["start_line"], 3);
        assert_eq!(json["end_line"], 9);
        let back: MemoryAnchor = serde_json::from_value(json).unwrap();
        assert_eq!(back, a);

        // A line-less anchor serializes WITHOUT the line keys (additive shape).
        let c = MemoryAnchor::new(AnchorKind::Commit, "abc123").unwrap();
        let json = serde_json::to_value(&c).unwrap();
        let obj = json.as_object().unwrap();
        assert!(!obj.contains_key("start_line"), "{json}");
        assert!(!obj.contains_key("end_line"), "{json}");

        // A payload without the line keys decodes (serde defaults).
        let old = serde_json::json!({ "kind": "symbol", "value": "Foo::bar" });
        let back: MemoryAnchor = serde_json::from_value(old).unwrap();
        assert_eq!(back.start_line, None);
        assert_eq!(back.end_line, None);
    }

    #[test]
    fn parse_file_filter_takes_paths_and_rejects_line_ranges() {
        let f = parse_file_filter("./src/a.rs").unwrap();
        assert_eq!(f.kind, AnchorKind::File);
        assert_eq!(f.value, "src/a.rs", "filter values normalize like anchors");

        for bad in ["src/a.rs:12", "src/a.rs:12-40", "", "  ", "a.rs:0"] {
            let res = parse_file_filter(bad);
            assert!(res.is_err(), "{bad:?} must be rejected, got {res:?}");
        }
    }

    #[test]
    fn kind_str_round_trips_and_matches_serde_snake_case() {
        for kind in [AnchorKind::File, AnchorKind::Commit, AnchorKind::Symbol] {
            let s = kind_str(kind);
            assert_eq!(parse_kind(s).unwrap(), kind);
            // The db string must equal the serde wire string so the two
            // representations can never disagree.
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(s));
        }
        assert!(parse_kind("branch").is_err());
    }

    #[test]
    fn display_is_compact() {
        assert_eq!(
            MemoryAnchor::parse_file_spec("src/a.rs:12-40")
                .unwrap()
                .to_string(),
            "file:src/a.rs:12-40"
        );
        assert_eq!(
            MemoryAnchor::parse_file_spec("src/a.rs:12")
                .unwrap()
                .to_string(),
            "file:src/a.rs:12"
        );
        assert_eq!(
            MemoryAnchor::new(AnchorKind::Commit, "abc123")
                .unwrap()
                .to_string(),
            "commit:abc123"
        );
        assert_eq!(
            MemoryAnchor::new(AnchorKind::Symbol, "Foo::bar")
                .unwrap()
                .to_string(),
            "symbol:Foo::bar"
        );
    }
}
