use rb_types::{EmbeddingInputFingerprint as Fingerprint, MemoryNote, MemoryType, Namespace};

fn note() -> MemoryNote {
    let mut note = MemoryNote::new(Namespace::Global, "content".into(), MemoryType::Insight, 5);
    note.keywords = vec!["key".into()];
    note.tags = vec!["tag".into()];
    note.context = "context".into();
    note
}

#[test]
fn every_embedded_field_participates_in_the_fingerprint() {
    let original = note();
    let expected = Fingerprint::from(&original);
    assert_eq!(expected, Fingerprint::from(&original.clone()));
    let mut changed = original.clone();
    changed.content.push('!');
    assert_ne!(expected, Fingerprint::from(&changed));
    changed = original.clone();
    changed.keywords.push("more".into());
    assert_ne!(expected, Fingerprint::from(&changed));
    changed = original.clone();
    changed.tags.push("more".into());
    assert_ne!(expected, Fingerprint::from(&changed));
    changed = original;
    changed.context.push('!');
    assert_ne!(expected, Fingerprint::from(&changed));
}

#[test]
fn metadata_and_maintenance_stamps_do_not_change_the_fingerprint() {
    let mut changed = note();
    let expected = Fingerprint::from(&changed);
    changed.summary = "not embedded".into();
    changed.importance = 8;
    changed.confidence = 0.5;
    changed.updated_at += chrono::Duration::seconds(1);
    changed.access_count += 1;
    changed.embedding_model = "new-model".into();
    changed.embedding_input_version = "new-composition".into();
    assert_eq!(expected, Fingerprint::from(&changed));
}

#[test]
fn boundaries_and_list_order_are_unambiguous() {
    let mut a = note();
    let mut b = a.clone();
    a.keywords = vec!["ab".into(), "c".into()];
    b.keywords = vec!["a".into(), "bc".into()];
    assert_ne!(Fingerprint::from(&a), Fingerprint::from(&b));
    b = a.clone();
    b.keywords.reverse();
    assert_ne!(Fingerprint::from(&a), Fingerprint::from(&b));
    b = a.clone();
    b.tags = b.keywords.clone();
    b.keywords = a.tags.clone();
    assert_ne!(Fingerprint::from(&a), Fingerprint::from(&b));
    b = a.clone();
    b.content = a.context.clone();
    b.context = a.content.clone();
    assert_ne!(Fingerprint::from(&a), Fingerprint::from(&b));
}
