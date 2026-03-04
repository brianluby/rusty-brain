use hooks::truncate::head_tail_truncate;

#[test]
fn under_limit_returns_as_is() {
    let content = "short text";
    let result = head_tail_truncate(content, 500);
    assert_eq!(result, content);
}

#[test]
fn over_limit_preserves_head_and_tail() {
    // 500 tokens ~ 2000 chars. Create a string well over that.
    let content: String = "a".repeat(4000);
    let result = head_tail_truncate(&content, 500);
    assert!(result.len() < content.len(), "truncated should be shorter");
    assert!(
        result.contains("[...truncated...]"),
        "must contain truncation marker"
    );
    // Head portion should start with 'a's
    assert!(result.starts_with('a'));
    // Tail portion should end with 'a's
    assert!(result.ends_with('a'));
}

#[test]
fn empty_string_returns_empty() {
    let result = head_tail_truncate("", 500);
    assert_eq!(result, "");
}

#[test]
fn exact_boundary_returns_as_is() {
    // 500 tokens * 4 chars/token = 2000 chars
    let content: String = "x".repeat(2000);
    let result = head_tail_truncate(&content, 500);
    assert_eq!(result, content, "exactly at boundary should not truncate");
}

#[test]
fn single_char_content_returns_as_is() {
    let result = head_tail_truncate("a", 500);
    assert_eq!(result, "a");
}

#[test]
fn truncated_preserves_60_40_ratio() {
    // Create content well over limit
    let content: String = (0..5000)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    let result = head_tail_truncate(&content, 500);
    let marker = "[...truncated...]";
    let marker_pos = result.find(marker).expect("marker must exist");
    let head_len = marker_pos;
    let tail_len = result.len() - marker_pos - marker.len();
    // Head should be roughly 60% and tail 40% of the token budget
    // With 500 tokens * 4 chars = 2000 chars budget
    // Head ~1200 chars, tail ~800 chars (approximate)
    let ratio = head_len as f64 / (head_len + tail_len) as f64;
    assert!(
        ratio > 0.5 && ratio < 0.7,
        "head/tail ratio should be approximately 60/40, got {ratio:.2}"
    );
}
