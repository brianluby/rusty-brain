//! Composite embedding input (P5 Feature A).
//!
//! A pure, deterministic function that composes the *document* representation
//! fed to the embedder from a note's enriched fields, plus the stamp string
//! recorded per-row so a corpus can be selectively re-embedded when the
//! composition changes.

use rb_types::MemoryNote;

/// Stamp written to `memories.embedding_input_version` and the global
/// `meta.embedding_input_version` for every memory embedded with the composite
/// representation below. Bump this string (and add a migration that re-seeds
/// the meta key) whenever [`embedding_input`] changes its composition so the
/// `reembed` batch can detect and converge stale rows.
pub const EMBEDDING_INPUT_VERSION: &str = "v2-composite";

/// Compose the text embedded for a note's *document* vector.
///
/// Documented order: `content`, then `keywords` (space-joined), then `tags`
/// (space-joined), then `context` — each on its own line, joined with `\n`.
/// Empty fields (empty string / empty list) are skipped entirely so a memory
/// with no enrichment embeds exactly its content (no leading/trailing blank
/// lines), matching the pre-P5 behavior for content-only notes.
///
/// Pure and deterministic: the same note always yields the same string. The
/// query is NOT composed this way — only the document representation changes
/// (A-MEM does the same; query symmetry is not required).
pub fn embedding_input(note: &MemoryNote) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(4);

    // Push raw content (NOT trimmed): pre-P5 embedded `note.content` verbatim, so a
    // content-only note must reproduce that exact representation — otherwise
    // re-embedding silently shifts its vector, and two notes differing only by
    // surrounding whitespace would collide. Skip only when content is truly blank.
    if !note.content.trim().is_empty() {
        parts.push(note.content.clone());
    }

    let keywords = join_non_empty(&note.keywords);
    if !keywords.is_empty() {
        parts.push(keywords);
    }

    let tags = join_non_empty(&note.tags);
    if !tags.is_empty() {
        parts.push(tags);
    }

    let context = note.context.trim();
    if !context.is_empty() {
        parts.push(context.to_string());
    }

    parts.join("\n")
}

/// Join a slice of strings with a single space, dropping empty/blank entries so
/// a stray empty keyword/tag cannot inject double spaces or a blank segment.
fn join_non_empty(items: &[String]) -> String {
    items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    fn note(content: &str) -> MemoryNote {
        MemoryNote::new(
            Namespace::Project("rb".into()),
            content.to_string(),
            MemoryType::Insight,
            5,
        )
    }

    #[test]
    fn version_constant_is_the_composite_stamp() {
        assert_eq!(EMBEDDING_INPUT_VERSION, "v2-composite");
    }

    #[test]
    fn content_only_note_embeds_just_its_content() {
        // A note with no enrichment must embed exactly its content (no blank
        // lines), so a never-enriched memory matches the pre-P5 representation.
        let n = note("single writer over sqlite wal");
        assert_eq!(embedding_input(&n), "single writer over sqlite wal");
    }

    #[test]
    fn content_only_note_preserves_raw_whitespace_for_v1_parity() {
        // Pre-P5 embedded note.content verbatim. A content-only note must embed the
        // RAW string (including surrounding whitespace) so re-embedding is a true
        // no-op for it and whitespace-variant notes do not collide on one vector.
        let n = note("  spaced out  ");
        assert_eq!(embedding_input(&n), "  spaced out  ");
    }

    #[test]
    fn composes_content_keywords_tags_context_in_documented_order() {
        let mut n = note("the body");
        n.keywords = vec!["sqlite".into(), "wal".into()];
        n.tags = vec!["db".into(), "search".into()];
        n.context = "in the storage layer".into();
        // content \n keywords \n tags \n context, each space-joined.
        assert_eq!(
            embedding_input(&n),
            "the body\nsqlite wal\ndb search\nin the storage layer"
        );
    }

    #[test]
    fn empty_fields_are_skipped_no_blank_lines() {
        let mut n = note("body here");
        // keywords empty, tags present, context empty.
        n.keywords = Vec::new();
        n.tags = vec!["tag1".into()];
        n.context = String::new();
        assert_eq!(embedding_input(&n), "body here\ntag1");
    }

    #[test]
    fn blank_keyword_and_tag_entries_do_not_inject_blank_segments() {
        let mut n = note("body");
        n.keywords = vec!["".into(), "kept".into(), "   ".into()];
        n.tags = vec!["   ".into()];
        n.context = "  ".into();
        // Only the real keyword survives; blank tag list and context drop out.
        assert_eq!(embedding_input(&n), "body\nkept");
    }

    #[test]
    fn is_deterministic_for_the_same_note() {
        let mut n = note("repeat me");
        n.keywords = vec!["alpha".into()];
        n.tags = vec!["beta".into()];
        n.context = "gamma".into();
        assert_eq!(embedding_input(&n), embedding_input(&n));
    }

    #[test]
    fn ordering_is_stable_and_field_order_matters() {
        let mut a = note("x");
        a.keywords = vec!["k".into()];
        a.tags = vec!["t".into()];
        let mut b = note("x");
        // Same fields but keyword/tag swapped: different composition.
        b.keywords = vec!["t".into()];
        b.tags = vec!["k".into()];
        assert_ne!(embedding_input(&a), embedding_input(&b));
    }
}
