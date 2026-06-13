use async_trait::async_trait;
use rb_engine::{Enricher, Enrichment};

/// Maximum characters retained in a heuristic summary.
const SUMMARY_MAX_CHARS: usize = 150;
/// Maximum number of derived keywords.
const MAX_KEYWORDS: usize = 5;
/// Minimum token length (in characters) kept as a keyword.
const MIN_KEYWORD_LEN: usize = 4;

/// Offline, deterministic enricher: a trimmed ~150-char summary and up to five
/// lowercased keyword tokens. Sets no tags/type/importance. Never errors.
#[derive(Debug, Default, Clone)]
pub struct HeuristicEnricher;

fn default_summary(content: &str) -> String {
    content.trim().chars().take(SUMMARY_MAX_CHARS).collect()
}

fn derive_keywords(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in content.split(|c: char| !c.is_alphanumeric()) {
        if raw.chars().count() < MIN_KEYWORD_LEN {
            continue;
        }
        let token = raw.to_lowercase();
        if !out.iter().any(|existing| existing == &token) {
            out.push(token);
        }
        if out.len() == MAX_KEYWORDS {
            break;
        }
    }
    out
}

#[async_trait]
impl Enricher for HeuristicEnricher {
    async fn enrich(&self, content: &str, _context: Option<&str>) -> rb_types::Result<Enrichment> {
        Ok(Enrichment {
            summary: Some(default_summary(content)),
            keywords: derive_keywords(content),
            tags: Vec::new(),
            memory_type: None,
            importance: None,
            // The heuristic has no basis for a trust judgment; the caller's
            // prior (or the full-trust default) stands.
            confidence: None,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_engine::Enricher;

    #[tokio::test]
    async fn summary_is_trimmed_prefix_for_short_content() {
        let e = HeuristicEnricher;
        let out = e.enrich("  one DB one transaction  ", None).await.unwrap();
        assert_eq!(out.summary.as_deref(), Some("one DB one transaction"));
    }

    #[tokio::test]
    async fn summary_truncates_to_150_chars_on_char_boundary() {
        let e = HeuristicEnricher;
        let content = "é".repeat(200); // 2 bytes each; must not split a code point
        let out = e.enrich(&content, None).await.unwrap();
        let summary = out.summary.unwrap();
        assert_eq!(summary.chars().count(), 150);
        assert!(summary.chars().all(|c| c == 'é'));
    }

    #[tokio::test]
    async fn keywords_lowercased_deduped_capped_at_five() {
        let e = HeuristicEnricher;
        let out = e
            .enrich(
                "SQLite WAL mode enables concurrent SQLITE readers and writers safely",
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            out.keywords,
            vec!["sqlite", "mode", "enables", "concurrent", "readers"]
        );
    }

    #[tokio::test]
    async fn sets_no_tags_type_or_importance() {
        let e = HeuristicEnricher;
        let out = e.enrich("body text here", Some("ctx")).await.unwrap();
        assert!(out.tags.is_empty());
        assert!(out.memory_type.is_none());
        assert!(out.importance.is_none());
    }

    #[tokio::test]
    async fn is_deterministic_same_input_same_output() {
        let e = HeuristicEnricher;
        let a = e
            .enrich("repeatable content for hashing", None)
            .await
            .unwrap();
        let b = e
            .enrich("repeatable content for hashing", None)
            .await
            .unwrap();
        assert_eq!(a, b);
    }
}
