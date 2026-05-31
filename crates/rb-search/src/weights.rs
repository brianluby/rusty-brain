/// Relative contribution of each ranking signal to the final score.
///
/// The `Default` weights are tuned for hybrid recall and **sum to 1.0**, so the
/// weighted score of any single candidate stays in `[0.0, 1.0]`:
///
/// | signal     | weight |
/// |------------|--------|
/// | vector     | 0.45   |
/// | keyword    | 0.30   |
/// | graph      | 0.10   |
/// | importance | 0.10   |
/// | recency    | 0.05   |
#[derive(Clone, Copy, Debug)]
pub struct Weights {
    pub vector: f32,
    pub keyword: f32,
    pub graph: f32,
    pub importance: f32,
    pub recency: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            vector: 0.45,
            keyword: 0.30,
            graph: 0.10,
            importance: 0.10,
            recency: 0.05,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn default_weights_match_documented_values() {
        let w = Weights::default();
        assert!((w.vector - 0.45).abs() < f32::EPSILON);
        assert!((w.keyword - 0.30).abs() < f32::EPSILON);
        assert!((w.graph - 0.10).abs() < f32::EPSILON);
        assert!((w.importance - 0.10).abs() < f32::EPSILON);
        assert!((w.recency - 0.05).abs() < f32::EPSILON);
    }

    #[test]
    fn default_weights_sum_to_one() {
        let w = Weights::default();
        let sum = w.vector + w.keyword + w.graph + w.importance + w.recency;
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "weights must sum to 1.0, got {sum}"
        );
    }

    #[test]
    fn weights_is_copy_and_clone() {
        let w = Weights::default();
        let copied = w; // Copy
                        // Invoke the Clone impl explicitly: `w.clone()` would trip clippy's
                        // `clone_on_copy` lint (denied by -D warnings) because Weights is Copy.
        let cloned = Clone::clone(&w);
        assert!((copied.vector - cloned.vector).abs() < f32::EPSILON);
        // original still usable after the copy (proves Copy, not move)
        assert!((w.vector - 0.45).abs() < f32::EPSILON);
    }
}
