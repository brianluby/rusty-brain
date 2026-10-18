//! HIDDEN acceptance test for kanban #83 run 1 (batched relation loading).
//! Not shown to the implementer; applied after the run as the oracle.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_store::{SqliteStore, Store};
use rb_types::{
    AnchorKind, LinkType, MemoryAnchor, MemoryId, MemoryLink, MemoryNote, MemoryType, Namespace,
    RecallFilter,
};
use std::collections::HashMap;

const DIM: usize = 4;
const TYPES: [LinkType; 3] = [LinkType::Extends, LinkType::Implements, LinkType::References];

struct Seeded {
    ids: Vec<MemoryId>,
    links: HashMap<MemoryId, Vec<MemoryLink>>,
    anchors: HashMap<MemoryId, Vec<MemoryAnchor>>,
}

fn seed(store: &SqliteStore, ns: &Namespace, n: usize, tag: &str) -> Seeded {
    let mut ids = Vec::new();
    let mut anchors = HashMap::new();
    for i in 0..n {
        let mut note = MemoryNote::new(ns.clone(), format!("{tag} note {i}"), MemoryType::Insight, 5);
        note.summary = format!("{tag} {i}");
        note.anchors = (0..(i % 4))
            .map(|k| match k {
                0 => MemoryAnchor::parse_file_spec(&format!("src/{tag}/f{i}.rs:{}-{}", k + 1, k + 9)).unwrap(),
                1 => MemoryAnchor::new(AnchorKind::Commit, &format!("abc{i:04}def")).unwrap(),
                _ => MemoryAnchor::new(AnchorKind::Symbol, &format!("Sym{i}::k{k}")).unwrap(),
            })
            .collect();
        anchors.insert(note.id.clone(), note.anchors.clone());
        ids.push(note.id.clone());
        store.insert_memory(&note, Some(&[1.0, 0.0, 0.0, 0.0])).unwrap();
    }
    let mut links: HashMap<MemoryId, Vec<MemoryLink>> = ids.iter().map(|id| (id.clone(), Vec::new())).collect();
    for i in 1..n {
        // Note 0 has no outgoing links; others have 1..=3, some sharing a target.
        for (k, step) in [1usize, 2, 1].iter().enumerate().take(1 + i % 3) {
            let link = MemoryLink {
                source_id: ids[i].clone(),
                target_id: ids[(i + step) % n].clone(),
                link_type: TYPES[k],
                strength: 0.25 + k as f32 * 0.25,
                reason: format!("{tag} {i}->{k}"),
                created_at: chrono::DateTime::from_timestamp(1_790_000_000 + i as i64, 0).unwrap(),
            };
            store.add_link(&link).unwrap();
            links.get_mut(&ids[i]).unwrap().push(link);
        }
    }
    for v in links.values_mut() {
        v.sort_by(|a, b| {
            (a.target_id.to_string(), a.link_type.as_str()).cmp(&(b.target_id.to_string(), b.link_type.as_str()))
        });
    }
    Seeded { ids, links, anchors }
}

fn assert_relations(seeded: &Seeded, notes: &[MemoryNote], what: &str) {
    for note in notes {
        let want_links = &seeded.links[&note.id];
        assert_eq!(note.links.len(), want_links.len(), "{what}: link count for {}", note.id);
        for (got, want) in note.links.iter().zip(want_links) {
            assert_eq!(got.source_id, want.source_id, "{what}: source");
            assert_eq!(got.target_id, want.target_id, "{what}: target order");
            assert_eq!(got.link_type, want.link_type, "{what}: type order");
            assert_eq!(got.reason, want.reason, "{what}: reason");
            assert!((got.strength - want.strength).abs() < 1e-6, "{what}: strength");
            assert_eq!(got.created_at, want.created_at, "{what}: created_at");
        }
        assert_eq!(note.anchors, seeded.anchors[&note.id], "{what}: anchors (insertion order)");
    }
}

#[test]
fn get_many_list_and_reembed_return_identical_relations() {
    let store = SqliteStore::open_in_memory(DIM).unwrap();
    let ns = Namespace::Project("plan-c".to_string());
    let other = Namespace::Project("other".to_string());
    let seeded = seed(&store, &ns, 60, "a");
    let _noise = seed(&store, &other, 20, "b");

    // Request order, duplicates, and missing ids are honoured.
    let mut req: Vec<MemoryId> = seeded.ids.iter().rev().cloned().collect();
    req.insert(3, seeded.ids[5].clone());
    req.insert(7, MemoryId::new());
    let got = store.get_many(&ns, &req).unwrap();
    let want: Vec<MemoryId> = req.iter().filter(|id| seeded.links.contains_key(*id)).cloned().collect();
    assert_eq!(got.iter().map(|n| n.id.clone()).collect::<Vec<_>>(), want, "get_many order");
    assert_relations(&seeded, &got, "get_many");

    // Cross-namespace ids are not returned.
    let b_only = store.get_many(&ns, &_noise.ids).unwrap();
    assert!(b_only.is_empty(), "get_many must stay namespace-scoped");

    assert!(store.get_many(&ns, &[]).unwrap().is_empty());

    let listed = store.list_filtered(&ns, &RecallFilter::default(), 1000).unwrap();
    assert_eq!(listed.len(), 60, "list_filtered returns every ns row");
    assert_relations(&seeded, &listed, "list_filtered");

    let stale = store.memories_for_reembed("no-such-model", "v0", 1000).unwrap();
    assert_eq!(stale.len(), 80, "reembed scans all namespaces");
    let in_ns: Vec<MemoryNote> = stale.into_iter().filter(|n| n.namespace == ns).collect();
    assert_relations(&seeded, &in_ns, "memories_for_reembed");
}

#[test]
fn get_many_accepts_more_ids_than_sqlite_bind_limit() {
    let store = SqliteStore::open_in_memory(DIM).unwrap();
    let ns = Namespace::Project("plan-c".to_string());
    let seeded = seed(&store, &ns, 30, "c");
    let mut req: Vec<MemoryId> = (0..40_000).map(|_| MemoryId::new()).collect();
    for (k, id) in seeded.ids.iter().enumerate() {
        req.insert(k * 1_000, id.clone());
    }
    let got = store.get_many(&ns, &req).unwrap();
    assert_eq!(got.iter().map(|n| n.id.clone()).collect::<Vec<_>>(), seeded.ids, "order across chunks");
    assert_relations(&seeded, &got, "get_many 40k");
}
