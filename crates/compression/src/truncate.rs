//! Budget enforcement via truncation.

/// Enforce the character budget on a string.
///
/// If `text.chars().count() <= budget`, returns text unchanged.
/// Otherwise, truncates from the end preserving the head, and appends
/// a `[...truncated to N chars]` marker. The marker itself counts
/// toward the budget.
///
/// # Invariant
///
/// Return value satisfies: `result.chars().count() <= budget`
pub fn enforce_budget(text: &str, budget: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= budget {
        return text.to_string();
    }

    if budget == 0 {
        return String::new();
    }

    // Build marker: "[...truncated to N chars]"
    let marker = format!("[...truncated to {budget} chars]");
    let marker_len = marker.chars().count();

    if marker_len >= budget {
        // Budget too small for marker + any content — just hard-truncate
        return text.chars().take(budget).collect();
    }

    // Keep as much of the head as fits alongside the marker
    let head_budget = budget - marker_len;
    let head: String = text.chars().take(head_budget).collect();
    format!("{head}{marker}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_within_budget_returned_unchanged() {
        let text = "hello world";
        assert_eq!(enforce_budget(text, 100), text);
    }

    #[test]
    fn empty_string_returned_unchanged() {
        assert_eq!(enforce_budget("", 100), "");
    }

    #[test]
    fn text_exactly_at_budget_returned_unchanged() {
        let text = "12345";
        assert_eq!(enforce_budget(text, 5), text);
    }

    #[test]
    fn text_exceeding_budget_has_truncation_marker() {
        let text = "a".repeat(100);
        let result = enforce_budget(&text, 50);
        assert!(result.contains("[...truncated to 50 chars]"));
        assert!(result.chars().count() <= 50);
    }

    #[test]
    fn truncated_result_within_budget() {
        let text = "a".repeat(10_000);
        let result = enforce_budget(&text, 2_000);
        assert!(result.chars().count() <= 2_000);
    }

    #[test]
    fn unicode_multibyte_counted_by_chars() {
        // Each emoji is 1 char but multiple bytes
        let text = "🎉".repeat(10);
        assert_eq!(text.chars().count(), 10);
        let result = enforce_budget(&text, 5);
        assert!(result.chars().count() <= 5);
    }

    #[test]
    fn marker_fits_within_budget() {
        let text = "a".repeat(200);
        let budget = 40;
        let result = enforce_budget(&text, budget);
        assert!(result.chars().count() <= budget);
        assert!(result.contains("[...truncated to 40 chars]"));
    }

    #[test]
    fn budget_of_zero() {
        let text = "some text";
        let result = enforce_budget(text, 0);
        assert!(result.is_empty());
    }
}
