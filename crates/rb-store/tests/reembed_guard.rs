//! Store-level guard behavior not dependent on provider timing.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use rb_store::{SqliteStore, Store};
use rb_types::{
    EmbeddingInputFingerprint as Fingerprint, Error, MemoryNote, MemoryType, MemoryUpdates,
    Namespace,
};

const DIM: usize = 8;

fn stale_note() -> MemoryNote {
    let mut note = MemoryNote::new(
        Namespace::Global,
        "candidate input".into(),
        MemoryType::Insight,
        5,
    );
    note.embedding_model = "deterministic".into();
    note.updated_at -= chrono::Duration::days(1);
    note
}

#[test]
fn metadata_edit_does_not_reject_valid_embedding_or_bump_updated_at() {
    let store = SqliteStore::open_in_memory(DIM).unwrap();
    let note = stale_note();
    store.insert_memory(&note, Some(&[1.0; DIM])).unwrap();
    let candidate = store
        .memories_for_reembed("deterministic", "v2-composite", 10)
        .unwrap()
        .remove(0);
    store
        .update_memory(
            &note.id,
            &MemoryUpdates {
                summary: Some("metadata-only edit".into()),
                importance: Some(9),
                confidence: Some(0.6),
                ..Default::default()
            },
        )
        .unwrap();
    let edited = store.get_memory(&note.id).unwrap().unwrap();
    assert_ne!(
        edited.updated_at, candidate.updated_at,
        "a timestamp CAS would incorrectly reject this"
    );
    assert_eq!(Fingerprint::from(&candidate), Fingerprint::from(&edited));
    store
        .update_vector(
            &note.id,
            &[0.5; DIM],
            "deterministic",
            "v2-composite",
            Fingerprint::from(&candidate),
        )
        .unwrap();
    let after = store.get_memory(&note.id).unwrap().unwrap();
    assert_eq!(after.updated_at, edited.updated_at);
    assert_eq!(after.summary, edited.summary);
    assert_eq!(after.importance, edited.importance);
    assert_eq!(after.confidence, edited.confidence);
    assert_eq!(Fingerprint::from(&after), Fingerprint::from(&candidate));
    assert!(store
        .memories_for_reembed("deterministic", "v2-composite", 10)
        .unwrap()
        .is_empty());
}

#[test]
fn stale_result_cannot_insert_a_vector_for_an_edited_vectorless_row() {
    let store = SqliteStore::open_in_memory(DIM).unwrap();
    let note = stale_note();
    store.insert_memory(&note, None).unwrap();
    let candidate = store
        .memories_for_reembed("deterministic", "v2-composite", 10)
        .unwrap()
        .remove(0);
    store
        .update_memory(
            &note.id,
            &MemoryUpdates {
                content: Some("new input".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let err = store
        .update_vector(
            &note.id,
            &[1.0; DIM],
            "deterministic",
            "v2-composite",
            Fingerprint::from(&candidate),
        )
        .unwrap_err();
    assert!(matches!(err, Error::StalePlan(_)), "{err:?}");
    assert!(store
        .vector_search(&Namespace::Global, &[1.0; DIM], 1)
        .unwrap()
        .is_empty());
    let after = store.get_memory(&note.id).unwrap().unwrap();
    assert_eq!(after.embedding_input_version, "");
    assert_eq!(
        store
            .memories_for_reembed("deterministic", "v2-composite", 10)
            .unwrap()
            .len(),
        1
    );
    store
        .update_vector(
            &note.id,
            &[0.5; DIM],
            "deterministic",
            "v2-composite",
            Fingerprint::from(&after),
        )
        .unwrap();
    assert_eq!(
        store
            .vector_search(&Namespace::Global, &[0.5; DIM], 1)
            .unwrap()[0]
            .0,
        note.id
    );
}
