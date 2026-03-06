use hooks::dedup::DedupCache;

#[test]
fn is_duplicate_returns_false_for_new_entry() {
    let dir = tempfile::tempdir().unwrap();
    let rusty_brain_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_brain_dir).unwrap();
    let cache = DedupCache::new(dir.path());
    assert!(!cache.is_duplicate("Read", "Read src/main.rs"));
}

#[test]
fn is_duplicate_returns_true_within_window() {
    let dir = tempfile::tempdir().unwrap();
    let rusty_brain_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_brain_dir).unwrap();
    let cache = DedupCache::new(dir.path());

    cache.record("Read", "Read src/main.rs").unwrap();
    assert!(cache.is_duplicate("Read", "Read src/main.rs"));
}

#[test]
fn is_duplicate_returns_false_for_different_entry() {
    let dir = tempfile::tempdir().unwrap();
    let rusty_brain_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_brain_dir).unwrap();
    let cache = DedupCache::new(dir.path());

    cache.record("Read", "Read src/main.rs").unwrap();
    assert!(!cache.is_duplicate("Read", "Read src/lib.rs"));
}

#[test]
fn record_creates_cache_file() {
    let dir = tempfile::tempdir().unwrap();
    let rusty_brain_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_brain_dir).unwrap();
    let cache = DedupCache::new(dir.path());

    cache.record("Edit", "Edited foo.rs").unwrap();
    assert!(rusty_brain_dir.join(".dedup-cache.json").exists());
}

#[test]
fn corrupt_cache_file_treated_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    let rusty_brain_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_brain_dir).unwrap();
    std::fs::write(rusty_brain_dir.join(".dedup-cache.json"), "not json!!!").unwrap();

    let cache = DedupCache::new(dir.path());
    // Should not panic or error — treated as empty (fail-open)
    assert!(!cache.is_duplicate("Read", "Read foo.rs"));
}

#[test]
fn record_uses_atomic_write() {
    let dir = tempfile::tempdir().unwrap();
    let rusty_brain_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_brain_dir).unwrap();
    let cache = DedupCache::new(dir.path());

    cache.record("Read", "Read main.rs").unwrap();
    // File should be valid JSON after write
    let content = std::fs::read_to_string(rusty_brain_dir.join(".dedup-cache.json")).unwrap();
    let _: serde_json::Value =
        serde_json::from_str(&content).expect("cache file must be valid JSON after atomic write");
}

#[test]
fn dedup_cache_stores_hashes_not_content() {
    let dir = tempfile::tempdir().unwrap();
    let rusty_brain_dir = dir.path().join(".rusty-brain");
    std::fs::create_dir_all(&rusty_brain_dir).unwrap();
    let cache = DedupCache::new(dir.path());

    cache.record("Read", "Read secrets/api-key.txt").unwrap();
    let content = std::fs::read_to_string(rusty_brain_dir.join(".dedup-cache.json")).unwrap();
    // SEC-2: cache must NOT contain the actual tool name or summary text
    assert!(
        !content.contains("secrets/api-key.txt"),
        "cache must store hashes, not content"
    );
    assert!(
        !content.contains("Read secrets"),
        "cache must store hashes, not content"
    );
}
