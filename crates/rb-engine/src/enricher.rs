use rb_types::MemoryType;

/// Output of an [`Enricher`]. Every field is optional: an enricher fills only
/// what it is confident about, and the engine uses a value ONLY when the caller
/// left the corresponding input empty. Defaults to "no enrichment".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Enrichment {
    /// Replacement summary (else the heuristic ~150-char prefix is used).
    pub summary: Option<String>,
    /// Derived keywords (used only if the caller supplied none).
    pub keywords: Vec<String>,
    /// Derived tags (used only if the caller supplied none).
    pub tags: Vec<String>,
    /// Inferred memory type (advisory; used only if the caller did not set one).
    pub memory_type: Option<MemoryType>,
    /// Inferred importance 1..=10 (advisory; used only if the caller did not set one).
    pub importance: Option<u8>,
}

/// Opt-in enrichment over raw memory content. The default path is heuristic and
/// offline; an LLM-backed implementation is opt-in and lives in `rb-enrich`.
/// Implementations degrade gracefully: a failure returns `Err(Error::Enrichment(_))`
/// and the engine falls back to heuristics (enrichment never fails a remember).
#[async_trait::async_trait]
pub trait Enricher: Send + Sync {
    async fn enrich(&self, content: &str, context: Option<&str>) -> rb_types::Result<Enrichment>;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use rb_types::MemoryType;

    // A trivial in-test Enricher proving the trait is object-safe and awaitable.
    struct ConstEnricher;

    #[async_trait::async_trait]
    impl Enricher for ConstEnricher {
        async fn enrich(
            &self,
            _content: &str,
            _context: Option<&str>,
        ) -> rb_types::Result<Enrichment> {
            Ok(Enrichment {
                summary: Some("s".to_string()),
                keywords: vec!["k".to_string()],
                tags: vec!["t".to_string()],
                memory_type: Some(MemoryType::Insight),
                importance: Some(7),
            })
        }
    }

    #[tokio::test]
    async fn enricher_is_object_safe_and_awaitable() {
        let e: std::sync::Arc<dyn Enricher> = std::sync::Arc::new(ConstEnricher);
        let out = e.enrich("body", Some("ctx")).await.unwrap();
        assert_eq!(out.summary.as_deref(), Some("s"));
        assert_eq!(out.keywords, vec!["k".to_string()]);
        assert_eq!(out.tags, vec!["t".to_string()]);
        assert_eq!(out.memory_type, Some(MemoryType::Insight));
        assert_eq!(out.importance, Some(7));
    }

    #[test]
    fn enrichment_default_is_all_empty() {
        let d = Enrichment::default();
        assert!(d.summary.is_none());
        assert!(d.keywords.is_empty());
        assert!(d.tags.is_empty());
        assert!(d.memory_type.is_none());
        assert!(d.importance.is_none());
    }
}
