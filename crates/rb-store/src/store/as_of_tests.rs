//! Recording-time `as_of` list tests (Vikunja #102): the test-case table of
//! `docs/eval/2026-10-18-point-in-time-recall-assessment.md` at the store
//! layer. Envelope (`temporal_basis`, `truncated`, `next_after`) and
//! capability cases live with the engine, proto client and daemon e2e tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::store::Store as _;
use crate::SqliteStore;
use rb_types::{
    AsOfCursor, Error, MemoryId, MemoryNote, MemoryState, MemoryType, MemoryUpdates, Namespace,
    RecallFilter,
};

/// 2026-07-01T00:00:00Z — the fixtures speak in days after this.
const JUL_1: i64 = 1_782_864_000;
const DAY: i64 = 86_400;

fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(secs, 0).unwrap()
}

fn ns() -> Namespace {
    Namespace::Project("as-of".into())
}

/// Insert a row with controlled recording times (the insert writes
/// `created_at`, `archived_at` and `superseded_by` as given).
fn plant(
    store: &SqliteStore,
    ns: &Namespace,
    summary: &str,
    created: i64,
    archived: Option<i64>,
    superseded_by: Option<&MemoryId>,
) -> MemoryId {
    let mut note = MemoryNote::new(
        ns.clone(),
        format!("content for {summary}"),
        MemoryType::ArchitectureDecision,
        6,
    );
    note.summary = summary.to_string();
    note.created_at = at(created);
    note.updated_at = at(archived.unwrap_or(created));
    note.archived_at = archived.map(at);
    note.superseded_by = superseded_by.cloned();
    let id = note.id.clone();
    store.insert_memory(&note, None).unwrap();
    id
}

fn as_of(t: i64) -> RecallFilter {
    RecallFilter {
        as_of: Some(at(t)),
        ..Default::default()
    }
}

fn list(store: &SqliteStore, ns: &Namespace, filter: &RecallFilter) -> Vec<MemoryId> {
    store
        .list_filtered(ns, filter, 10_000)
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect()
}

/// The fusion decision of worked example 1: A -> B -> C.
struct Chain {
    a: MemoryId,
    b: MemoryId,
    c: MemoryId,
}

fn reversal_chain(store: &SqliteStore, ns: &Namespace) -> Chain {
    let (aug_1, sep_1) = (JUL_1 + 31 * DAY, JUL_1 + 62 * DAY);
    let c = plant(store, ns, "Back to RRF with tuned k", sep_1, None, None);
    let b = plant(
        store,
        ns,
        "Use Linear fusion (RRF regressed MRR)",
        aug_1,
        Some(sep_1),
        Some(&c),
    );
    let a = plant(store, ns, "Use RRF fusion", JUL_1, Some(aug_1), Some(&b));
    Chain { a, b, c }
}

/// Test 1: the reversal chain answers each instant with exactly one version.
#[test]
fn reversal_chain_has_one_member_per_instant() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let chain = reversal_chain(&store, &ns());
    assert!(list(&store, &ns(), &as_of(JUL_1 - DAY)).is_empty());
    assert_eq!(list(&store, &ns(), &as_of(JUL_1 + 14 * DAY)), vec![chain.a]);
    assert_eq!(list(&store, &ns(), &as_of(JUL_1 + 45 * DAY)), vec![chain.b]);
    assert_eq!(
        list(&store, &ns(), &as_of(JUL_1 + 100 * DAY)),
        vec![chain.c.clone()]
    );
    // The pre-#102 predicates the assessment calls wrong: `until` + All
    // returns A and B on Aug 15; the active-only default returns nothing.
    let until_all = RecallFilter {
        until: Some(at(JUL_1 + 45 * DAY)),
        state: MemoryState::All,
        ..Default::default()
    };
    assert_eq!(list(&store, &ns(), &until_all).len(), 2);
    let until_active = RecallFilter {
        until: Some(at(JUL_1 + 45 * DAY)),
        ..Default::default()
    };
    assert!(list(&store, &ns(), &until_active).is_empty());
}

/// Test 2: at exactly `archived_at` a row is excluded; at exactly
/// `created_at` it is included.
#[test]
fn boundary_instants_exclude_archived_at_and_include_created_at() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let chain = reversal_chain(&store, &ns());
    let aug_1 = JUL_1 + 31 * DAY;
    assert_eq!(list(&store, &ns(), &as_of(JUL_1)), vec![chain.a.clone()]);
    assert_eq!(list(&store, &ns(), &as_of(aug_1 - 1)), vec![chain.a]);
    assert_eq!(list(&store, &ns(), &as_of(aug_1)), vec![chain.b]);
}

/// Test 2b: a same-second create-and-archive row is never a member, its
/// same-second replacement is a member from that second on, and a
/// sub-second `as_of` is rejected rather than truncated.
#[test]
fn same_second_create_and_archive_is_never_a_member() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let t = JUL_1 + 7;
    let replacement = plant(&store, &ns(), "replacement", t, None, None);
    let _flash = plant(&store, &ns(), "flash", t, Some(t), Some(&replacement));
    assert!(list(&store, &ns(), &as_of(t - 1)).is_empty());
    assert_eq!(list(&store, &ns(), &as_of(t)), vec![replacement.clone()]);
    assert_eq!(list(&store, &ns(), &as_of(t + 1)), vec![replacement]);

    let sub_second = RecallFilter {
        as_of: Some(at(t) + chrono::Duration::milliseconds(500)),
        ..Default::default()
    };
    let err = store.list_filtered(&ns(), &sub_second, 10).unwrap_err();
    assert!(
        matches!(&err, Error::InvalidArgument(m) if m.contains("sub-second")),
        "{err:?}"
    );
}

/// Test 3: a row archived WITHOUT a successor (the delete path, a real
/// archive) is present before its `archived_at` and absent from it on, and
/// `history` reports that `archived_at`.
#[test]
fn archive_without_successor_ends_membership_and_history_shows_when() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let created = chrono::Utc::now().timestamp() - 3_600;
    let id = plant(&store, &ns(), "retired", created, None, None);
    store.archive_memory(&id).unwrap();
    let archived_at = store
        .get_memory(&id)
        .unwrap()
        .unwrap()
        .archived_at
        .expect("archive stamps archived_at");
    let archived = archived_at.timestamp();
    assert_eq!(list(&store, &ns(), &as_of(created)), vec![id.clone()]);
    assert_eq!(list(&store, &ns(), &as_of(archived - 1)), vec![id.clone()]);
    assert!(list(&store, &ns(), &as_of(archived)).is_empty());

    let history = store.memory_history(&ns(), &id, 4, 16, 16).unwrap();
    let entry = &history.chain[0];
    assert!(entry.archived);
    assert_eq!(entry.archived_at, Some(archived_at));
    assert!(entry.superseded_by.is_none());
}

/// Test 3 (history side): every archived chain member carries its
/// `archived_at`; the current one carries none.
#[test]
fn history_chain_carries_archived_at_per_member() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let chain = reversal_chain(&store, &ns());
    let history = store.memory_history(&ns(), &chain.c, 4, 16, 16).unwrap();
    let ends: Vec<_> = history
        .chain
        .iter()
        .map(|e| (e.id.clone(), e.archived_at.map(|t| t.timestamp())))
        .collect();
    assert_eq!(
        ends,
        vec![
            (chain.a, Some(JUL_1 + 31 * DAY)),
            (chain.b, Some(JUL_1 + 62 * DAY)),
            (chain.c, None),
        ]
    );
}

/// Test 4: fan-in — two near-duplicates superseded (for real) into one
/// survivor are both members before the supersede, only the survivor after.
#[test]
fn fan_in_supersede_leaves_only_the_survivor() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let now = chrono::Utc::now().timestamp();
    let d1 = plant(&store, &ns(), "dup one", now - 300, None, None);
    let d2 = plant(&store, &ns(), "dup two", now - 200, None, None);
    let survivor = plant(&store, &ns(), "survivor", now - 100, None, None);
    store.supersede(&d1, &survivor).unwrap();
    store.supersede(&d2, &survivor).unwrap();
    let superseded_at = [&d1, &d2]
        .iter()
        .map(|id| {
            store
                .get_memory(id)
                .unwrap()
                .unwrap()
                .archived_at
                .unwrap()
                .timestamp()
        })
        .min()
        .unwrap();
    assert_eq!(
        list(&store, &ns(), &as_of(now - 150)),
        vec![d1.clone(), d2.clone()]
    );
    assert_eq!(
        list(&store, &ns(), &as_of(superseded_at - 1)),
        vec![d1, d2, survivor.clone()]
    );
    assert_eq!(
        list(&store, &ns(), &as_of(superseded_at + 1)),
        vec![survivor]
    );
}

/// Test 5: a purged row never appears at any instant (its row, vectors and
/// oplog are gone by design).
#[test]
fn purged_row_is_never_present() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let kept = plant(&store, &ns(), "kept", JUL_1, None, None);
    let purged = plant(&store, &ns(), "purged", JUL_1, Some(JUL_1 + DAY), None);
    assert!(store
        .purge_memory_for_retention(&purged, r#"{"cause":"test"}"#)
        .unwrap());
    for t in [JUL_1, JUL_1 + DAY / 2, JUL_1 + DAY, JUL_1 + 400 * DAY] {
        assert_eq!(list(&store, &ns(), &as_of(t)), vec![kept.clone()], "at {t}");
    }
}

/// Test 6: the late-recorded event. P (recorded Oct 10, about Sep 1)
/// supersedes M on Oct 10, so as of Sep 15 the store held M — correct about
/// the store, wrong about the project; the envelope labels the basis.
#[test]
fn late_recorded_event_returns_what_the_store_held() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let (jun_1, sep_15, oct_10) = (JUL_1 - 30 * DAY, JUL_1 + 76 * DAY, JUL_1 + 101 * DAY);
    let p = plant(
        &store,
        &ns(),
        "Production migrated from MySQL to Postgres on Sep 1",
        oct_10,
        None,
        None,
    );
    let m = plant(
        &store,
        &ns(),
        "Production runs MySQL",
        jun_1,
        Some(oct_10),
        Some(&p),
    );
    assert_eq!(list(&store, &ns(), &as_of(sep_15)), vec![m]);
    assert_eq!(list(&store, &ns(), &as_of(oct_10)), vec![p]);
}

/// Test 7: `as_of` with `state` archived/all is a validation error; with the
/// default `active` state it applies the as-of predicate, not the active
/// one, so a row archived after T is still returned.
#[test]
fn as_of_replaces_the_state_predicate() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let chain = reversal_chain(&store, &ns());
    for state in [MemoryState::Archived, MemoryState::All] {
        let filter = RecallFilter {
            state,
            ..as_of(JUL_1 + DAY)
        };
        let err = store.list_filtered(&ns(), &filter, 10).unwrap_err();
        assert!(
            matches!(err, Error::InvalidArgument(_)),
            "{state:?}: {err:?}"
        );
    }
    // A is archived now, but was the member at Jul 2.
    assert_eq!(list(&store, &ns(), &as_of(JUL_1 + DAY)), vec![chain.a]);
    // A cursor without as_of is a caller mistake.
    let orphan = RecallFilter {
        as_of_after: Some(AsOfCursor {
            created_at: at(JUL_1),
            memory_id: MemoryId::new(),
        }),
        ..Default::default()
    };
    assert!(matches!(
        store.list_filtered(&ns(), &orphan, 10),
        Err(Error::InvalidArgument(_))
    ));
}

/// Test 8 (store side): ranked recall channels refuse `as_of` instead of
/// silently losing the vector channel; with no `as_of` set the list order
/// is the unchanged newest-first one.
#[test]
fn ranked_channels_reject_as_of_and_plain_list_is_unchanged() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let chain = reversal_chain(&store, &ns());
    let filter = as_of(JUL_1 + DAY);
    assert!(matches!(
        store.keyword_search_filtered(&ns(), "fusion", 5, &filter),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        store.vector_search_filtered(&ns(), &[0.1; 8], 5, &filter),
        Err(Error::InvalidArgument(_))
    ));
    let plain = RecallFilter {
        state: MemoryState::All,
        ..Default::default()
    };
    assert_eq!(list(&store, &ns(), &plain), vec![chain.c, chain.b, chain.a]);
}

/// Test 9: `as_of` never returns rows from another namespace, even with
/// identical recording times.
#[test]
fn as_of_is_namespace_isolated() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let mine = reversal_chain(&store, &ns());
    let other = Namespace::Project("as-of-other".into());
    let theirs = reversal_chain(&store, &other);
    for t in [JUL_1, JUL_1 + 45 * DAY, JUL_1 + 100 * DAY] {
        let got = list(&store, &ns(), &as_of(t));
        for id in [&theirs.a, &theirs.b, &theirs.c] {
            assert!(!got.contains(id), "foreign row at {t}");
        }
    }
    assert_eq!(
        list(&store, &other, &as_of(JUL_1 + 45 * DAY)),
        vec![theirs.b]
    );
    assert_eq!(list(&store, &ns(), &as_of(JUL_1 + 45 * DAY)), vec![mine.b]);
}

/// Test 10: returned importance and confidence are the CURRENT values, not
/// the values as of T (they are overwritten in place).
#[test]
fn as_of_returns_current_metadata() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let chain = reversal_chain(&store, &ns());
    store
        .update_memory(
            &chain.c,
            &MemoryUpdates {
                importance: Some(9),
                confidence: Some(0.25),
                ..Default::default()
            },
        )
        .unwrap();
    let rows = store
        .list_filtered(&ns(), &as_of(JUL_1 + 70 * DAY), 10)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, chain.c);
    assert_eq!(rows[0].importance, 9);
    assert!((rows[0].confidence - 0.25).abs() < f32::EPSILON);
}

/// Test 13 (store side): `(created_at, memory_id)` keyset paging returns
/// every member exactly once, in order, across equal `created_at` values.
#[test]
fn keyset_cursor_pages_through_equal_created_at_without_gaps() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    let mut expected = Vec::new();
    for i in 0..45 {
        // Three distinct seconds, 15 rows each: ties dominate.
        expected.push(plant(
            &store,
            &ns(),
            &format!("row {i}"),
            JUL_1 + i / 15,
            None,
            None,
        ));
    }
    // Non-members at T are skipped by every page.
    plant(&store, &ns(), "later", JUL_1 + DAY, None, None);
    plant(&store, &ns(), "gone", JUL_1, Some(JUL_1 + 1), None);
    expected.sort_by_key(|id| {
        let n = store.get_memory(id).unwrap().unwrap();
        (n.created_at.timestamp(), id.to_string())
    });

    let mut seen = Vec::new();
    let mut filter = as_of(JUL_1 + 60);
    loop {
        let page = store.list_filtered(&ns(), &filter, 20).unwrap();
        let Some(last) = page.last() else { break };
        filter.as_of_after = Some(AsOfCursor {
            created_at: last.created_at,
            memory_id: last.id.clone(),
        });
        seen.extend(page.into_iter().map(|n| n.id));
    }
    assert_eq!(seen, expected);
}

/// Drift test: the SQL predicate (membership, state replacement, cursor) and
/// `RecallFilter::matches` agree on every row at every probed instant.
#[test]
fn as_of_filter_agrees_with_recall_filter_matches() {
    let store = SqliteStore::open_in_memory(8).unwrap();
    reversal_chain(&store, &ns());
    let t = JUL_1 + 7;
    let r = plant(&store, &ns(), "replacement", t, None, None);
    plant(&store, &ns(), "flash", t, Some(t), Some(&r));
    let all = store
        .list_filtered(
            &ns(),
            &RecallFilter {
                state: MemoryState::All,
                ..Default::default()
            },
            100,
        )
        .unwrap();
    let probes = [
        JUL_1 - 1,
        JUL_1,
        t,
        JUL_1 + 31 * DAY - 1,
        JUL_1 + 31 * DAY,
        JUL_1 + 62 * DAY,
        JUL_1 + 900 * DAY,
    ];
    for probe in probes {
        for cursor in [None, Some((JUL_1, MemoryId::new()))] {
            let mut filter = as_of(probe);
            filter.as_of_after = cursor.map(|(c, id)| AsOfCursor {
                created_at: at(c),
                memory_id: id,
            });
            let mut want: Vec<_> = all
                .iter()
                .filter(|n| filter.matches(n))
                .map(|n| (n.created_at.timestamp(), n.id.to_string()))
                .collect();
            want.sort();
            let got: Vec<_> = store
                .list_filtered(&ns(), &filter, 100)
                .unwrap()
                .iter()
                .map(|n| (n.created_at.timestamp(), n.id.to_string()))
                .collect();
            assert_eq!(got, want, "as_of {probe}");
        }
    }
}
