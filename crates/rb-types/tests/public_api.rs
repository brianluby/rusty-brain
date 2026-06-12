#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_types::{
    Error, LinkType, MemoryId, MemoryLink, MemoryNote, MemoryType, MemoryUpdates, Namespace,
    Result, SearchQuery, SearchResult,
};
use std::str::FromStr;

#[test]
fn all_public_types_are_reachable_from_crate_root() {
    // Error + Result
    let r: Result<u8> = Err(Error::Storage("x".into()));
    assert!(r.is_err());

    // MemoryId
    let id = MemoryId::new();
    let id2 = MemoryId::from_str(&id.to_string()).unwrap();
    assert_eq!(id, id2);

    // Namespace
    let ns = Namespace::Project("rusty-brain".into());
    assert_eq!(ns.as_db_string(), "project:rusty-brain");

    // MemoryType + LinkType
    assert_eq!(MemoryType::BugFix.as_str(), "bug_fix");
    assert_eq!(LinkType::Extends.as_str(), "extends");

    // MemoryNote built via the constructor
    let note = MemoryNote::new(ns.clone(), "body".into(), MemoryType::Insight, 5);
    assert_eq!(note.namespace, ns);

    // MemoryLink
    let link = MemoryLink {
        source_id: MemoryId::new(),
        target_id: MemoryId::new(),
        link_type: LinkType::References,
        strength: 0.5,
        reason: "r".into(),
        created_at: chrono::Utc::now(),
    };
    assert_eq!(link.link_type, LinkType::References);

    // SearchQuery / SearchResult / MemoryUpdates
    let q = SearchQuery {
        query: "q".into(),
        limit: 3,
        ..Default::default()
    };
    assert_eq!(q.limit, 3);
    let res = SearchResult {
        memory: note,
        score: 1.0,
        channels: rb_types::ChannelHits::default(),
    };
    assert!((res.score - 1.0).abs() < f32::EPSILON);
    assert!(!res.channels.fts && !res.channels.vector && !res.channels.graph);
    let upd = MemoryUpdates {
        importance: Some(9),
        ..Default::default()
    };
    assert_eq!(upd.importance, Some(9));
}
