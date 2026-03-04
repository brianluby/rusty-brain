const TRUNCATION_MARKER: &str = "[...truncated...]";

/// Truncate content to approximately `max_tokens` using head/tail strategy.
///
/// - If content is under the token limit: returns as-is
/// - Otherwise: keeps first ~60% and last ~40%, inserting a truncation marker
/// - Token estimation: chars / 4
#[must_use]
pub fn head_tail_truncate(content: &str, max_tokens: usize) -> String {
    if content.is_empty() {
        return String::new();
    }

    let max_chars = max_tokens.saturating_mul(4);
    let char_count = content.chars().count();
    if char_count <= max_chars {
        return content.to_string();
    }

    let marker_len = TRUNCATION_MARKER.chars().count();
    let budget = max_chars.saturating_sub(marker_len);
    let head_chars = budget.saturating_mul(60) / 100;
    let tail_chars = budget - head_chars;

    // Use char_indices to find safe UTF-8 byte boundaries
    let head_end = content
        .char_indices()
        .nth(head_chars)
        .map_or(content.len(), |(idx, _)| idx);
    let tail_start = content
        .char_indices()
        .nth(char_count - tail_chars)
        .map_or(content.len(), |(idx, _)| idx);

    let head = &content[..head_end];
    let tail = &content[tail_start..];

    format!("{head}{TRUNCATION_MARKER}{tail}")
}
