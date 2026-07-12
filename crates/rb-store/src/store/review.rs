//! Review-queue generation and snooze persistence (PRD 2026-07-02
//! contradiction/dedup review).
//!
//! Read side: [`SqliteStore::review_queue`] surfaces, in priority order,
//! active contradiction pairs, near-duplicate pairs (via the ONE similarity
//! definition, [`SqliteStore::near_duplicates`]), and low-confidence /
//! stale-never-recalled singles. Write side: small helpers that persist
//! snoozes in the additive `review_state` table (migration 010) and append
//! the `review_resolve` / `review_sweep` oplog rows (REV-4 audit).

use super::internal::{immediate_tx, parse_id};
use super::*;
use crate::error::storage_err;
use rb_types::{
    review_item_key, ReviewItem, ReviewMember, ReviewPlan, ReviewReason, ReviewTotals,
    REVIEW_LOW_CONFIDENCE_BOUND, REVIEW_MAX_LIMIT, REVIEW_MAX_SNOOZE_DAYS, REVIEW_MIN_THRESHOLD,
    REVIEW_STALE_DAYS,
};
use std::collections::HashSet;

/// Bound on the near-dup anchor scan per sweep (the consolidation
/// `batch_limit` idea): each anchor costs one vec0 KNN probe, so the scan is
/// O(budget) probes no matter how large the namespace. Anchors outside the
/// window are picked up by later sweeps (deterministic oldest-first order).
const NEAR_DUP_SCAN_BUDGET: usize = 200;
/// Max near-duplicates fetched per anchor (the write-time suppression bound).
const NEAR_DUP_PER_ANCHOR: usize = 8;

/// Parameters for one review-queue generation pass (REV-1/REV-3).
#[derive(Debug, Clone, Copy)]
pub struct ReviewQueueParams {
    /// Raw cosine-similarity bound for the near-dup tier (the
    /// `near_duplicates()` unit). Validated to
    /// `REVIEW_MIN_THRESHOLD..=1.0` fail-closed.
    pub threshold: f32,
    /// Max queue items emitted (1..=`REVIEW_MAX_LIMIT`); totals still report
    /// the full per-tier counts.
    pub limit: usize,
    /// When set, only items with a member touched by an oplog row with
    /// `seq > since` qualify (REV-3 `--since`).
    pub since: Option<u64>,
}

/// The snooze-exclusion probe, resolved INSIDE each bounded tier query (the
/// PR #58 contested-filter lesson: post-filtering a fetch window silently
/// under-fills the limit). `{key}` is the SQL expression computing the
/// canonical item key for the row.
fn not_snoozed_sql(key_expr: &str, ns_param: usize, now_param: usize) -> String {
    format!(
        "({key_expr}) NOT IN (SELECT item_key FROM review_state
             WHERE namespace = ?{ns_param}
               AND snooze_until IS NOT NULL AND snooze_until > ?{now_param})"
    )
}

/// The `--since` probe: the memory aliased `{alias}` was touched by an oplog
/// row after the cursor.
fn touched_since_sql(alias: &str, since_param: usize) -> String {
    format!(
        "EXISTS (SELECT 1 FROM memory_oplog o WHERE o.memory_id = {alias}.memory_id
             AND o.seq > ?{since_param})"
    )
}

impl SqliteStore {
    /// Generate the review queue for `ns` (REV-1), priority-ordered:
    /// contradiction pairs, near-duplicate pairs, then low-confidence and
    /// stale-never-recalled singles. Read-only — this IS the dry-run, and the
    /// daemon's apply pass derives its actions from this same queue plus the
    /// pure `ReviewPolicy::plan_action`, so preview and mutation cannot
    /// diverge structurally.
    ///
    /// Sources of truth, deliberately not re-invented:
    /// - contradictions: the pair SQL below is the pairwise expression of
    ///   `active_contradicts` (both endpoints active AND in `ns`), pinned by
    ///   the `contradiction_items_agree_with_active_contradicts` drift test;
    /// - near-duplicates: [`SqliteStore::near_duplicates`] — the ONE
    ///   similarity definition (write-time suppression and the consolidation
    ///   job use the same function);
    /// - stale: the stats `never_recalled_live` predicate
    ///   (`access_count = 0`) plus the [`REVIEW_STALE_DAYS`] age bound.
    ///
    /// Snoozed items (unexpired `review_state.snooze_until`) are excluded
    /// inside each bounded query; `totals` count the actionable (unsnoozed)
    /// sets, with `totals.snoozed` reporting what is currently hidden.
    pub fn review_queue(
        &self,
        ns: &Namespace,
        params: &ReviewQueueParams,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ReviewPlan> {
        if !params.threshold.is_finite()
            || params.threshold < REVIEW_MIN_THRESHOLD
            || params.threshold > 1.0
        {
            return Err(Error::InvalidArgument(format!(
                "review threshold {} is out of range {REVIEW_MIN_THRESHOLD}..=1.0",
                params.threshold
            )));
        }
        if params.limit == 0 || params.limit > REVIEW_MAX_LIMIT as usize {
            return Err(Error::InvalidArgument(format!(
                "review limit {} is out of range 1..={REVIEW_MAX_LIMIT}",
                params.limit
            )));
        }
        let ns_str = ns.as_db_string();
        let now_ts = now.timestamp();
        let mut totals = ReviewTotals::default();
        let mut items: Vec<ReviewItem> = Vec::new();
        let mut truncated = false;

        // Currently-hidden items (unexpired snoozes): the gauge plus the
        // Rust-side probe for near-dup pairs (whose keys are computed here,
        // not in SQL).
        let snoozed_keys: HashSet<String> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT item_key FROM review_state
                     WHERE namespace = ?1 AND snooze_until IS NOT NULL AND snooze_until > ?2",
                )
                .map_err(storage_err)?;
            let rows = stmt
                .query_map(rusqlite::params![ns_str, now_ts], |r| r.get::<_, String>(0))
                .map_err(storage_err)?;
            let mut out = HashSet::new();
            for r in rows {
                out.insert(r.map_err(storage_err)?);
            }
            out
        };
        totals.snoozed = snoozed_keys.len() as u64;

        // --- tier 1: active contradiction pairs -----------------------------
        // Canonical pair form (lo < hi, TEXT/BINARY order — the same order
        // `review_item_key` produces) collapses a<->b and b<->a edges into one
        // row. Both endpoints must be ACTIVE and IN `ns`: the pairwise
        // expression of `active_contradicts` (see the drift test).
        let since_frag = match params.since {
            // ?3 below is the since cursor in both pair-side probes.
            Some(_) => format!(
                " AND ({} OR {})",
                touched_since_sql("a", 3),
                touched_since_sql("b", 3)
            ),
            None => String::new(),
        };
        let pairs_cte = format!(
            "SELECT DISTINCT
                CASE WHEN l.source_id < l.target_id THEN l.source_id ELSE l.target_id END AS lo,
                CASE WHEN l.source_id < l.target_id THEN l.target_id ELSE l.source_id END AS hi
             FROM memory_links l
               JOIN memories a ON a.memory_id = l.source_id
               JOIN memories b ON b.memory_id = l.target_id
             WHERE l.link_type = 'contradicts'
               AND a.archived_at IS NULL AND b.archived_at IS NULL
               AND a.namespace = ?1 AND b.namespace = ?1{since_frag}"
        );
        let not_snoozed = not_snoozed_sql("'contradiction:' || p.lo || ':' || p.hi", 1, 2);
        let mut pair_params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(ns_str.clone()), Box::new(now_ts)];
        if let Some(since) = params.since {
            pair_params.push(Box::new(i64::try_from(since).unwrap_or(i64::MAX)));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = pair_params.iter().map(|b| b.as_ref()).collect();
        let count: i64 = self
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM ({pairs_cte}) p WHERE {not_snoozed}"),
                refs.as_slice(),
                |r| r.get(0),
            )
            .map_err(storage_err)?;
        totals.contradictions = u64::try_from(count).unwrap_or(0);
        {
            let remaining = params.limit.saturating_sub(items.len());
            let mut stmt = self
                .conn
                .prepare(&format!(
                    "SELECT p.lo, p.hi FROM ({pairs_cte}) p WHERE {not_snoozed}
                     ORDER BY p.lo, p.hi LIMIT {remaining}"
                ))
                .map_err(storage_err)?;
            let mut rows = stmt.query(refs.as_slice()).map_err(storage_err)?;
            while let Some(row) = rows.next().map_err(storage_err)? {
                let lo: String = row.get(0).map_err(storage_err)?;
                let hi: String = row.get(1).map_err(storage_err)?;
                let lo = parse_id(&lo)?;
                let hi = parse_id(&hi)?;
                items.push(ReviewItem {
                    key: review_item_key(ReviewReason::Contradiction, &[lo.clone(), hi.clone()]),
                    reason: ReviewReason::Contradiction,
                    members: vec![
                        self.review_member(&lo, now_ts)?,
                        self.review_member(&hi, now_ts)?,
                    ],
                    similarity: None,
                });
            }
        }
        truncated |= totals.contradictions > items.len() as u64;

        // --- tier 2: near-duplicate pairs ------------------------------------
        // Bounded anchor scan (deterministic oldest-first, the consolidation
        // order); each anchor probes `near_duplicates` — the ONE similarity
        // definition. A member joins at most one pair per sweep so a single
        // apply pass can never plan two mutations over the same memory;
        // remaining cluster members re-pair on later sweeps (convergent, the
        // consolidation argument).
        let tier1_len = items.len();
        let anchor_since = match params.since {
            Some(_) => format!(" AND {}", touched_since_sql("m", 2)),
            None => String::new(),
        };
        let anchors_sql = format!(
            "SELECT m.memory_id FROM memories m
             WHERE m.namespace = ?1 AND m.archived_at IS NULL{anchor_since}
             ORDER BY m.created_at ASC, m.memory_id ASC LIMIT {NEAR_DUP_SCAN_BUDGET}"
        );
        let mut anchor_params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(ns_str.clone())];
        if let Some(since) = params.since {
            anchor_params.push(Box::new(i64::try_from(since).unwrap_or(i64::MAX)));
        }
        let anchor_refs: Vec<&dyn rusqlite::ToSql> =
            anchor_params.iter().map(|b| b.as_ref()).collect();
        let anchors: Vec<MemoryId> = {
            let mut stmt = self.conn.prepare(&anchors_sql).map_err(storage_err)?;
            let rows = stmt
                .query_map(anchor_refs.as_slice(), |r| r.get::<_, String>(0))
                .map_err(storage_err)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(parse_id(&r.map_err(storage_err)?)?);
            }
            out
        };
        let mut consumed: HashSet<String> = HashSet::new();
        for anchor in &anchors {
            if consumed.contains(&anchor.to_string()) {
                continue;
            }
            let dups = self.near_duplicates(ns, anchor, params.threshold, NEAR_DUP_PER_ANCHOR)?;
            for (dup, similarity) in dups {
                if consumed.contains(&dup.to_string()) {
                    continue;
                }
                let key =
                    review_item_key(ReviewReason::NearDuplicate, &[anchor.clone(), dup.clone()]);
                if snoozed_keys.contains(&key) {
                    // A snoozed pair neither surfaces nor consumes its
                    // members — the anchor may still pair with another twin.
                    continue;
                }
                totals.near_duplicates += 1;
                consumed.insert(anchor.to_string());
                consumed.insert(dup.to_string());
                if items.len() < params.limit {
                    items.push(ReviewItem {
                        key,
                        reason: ReviewReason::NearDuplicate,
                        members: vec![
                            self.review_member(anchor, now_ts)?,
                            self.review_member(&dup, now_ts)?,
                        ],
                        similarity: Some(similarity),
                    });
                }
                break; // one pair per anchor per sweep
            }
        }
        truncated |= totals.near_duplicates > (items.len() - tier1_len) as u64;

        // --- tier 3: low-confidence, then stale never-recalled ----------------
        let single_since = match params.since {
            Some(_) => format!(" AND {}", touched_since_sql("m", 4)),
            None => String::new(),
        };
        let mut single_params: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(ns_str.clone()),
            Box::new(now_ts),
            Box::new(f64::from(REVIEW_LOW_CONFIDENCE_BOUND)),
        ];
        if let Some(since) = params.since {
            single_params.push(Box::new(i64::try_from(since).unwrap_or(i64::MAX)));
        }
        let single_refs: Vec<&dyn rusqlite::ToSql> =
            single_params.iter().map(|b| b.as_ref()).collect();

        let low_where = format!(
            "m.namespace = ?1 AND m.archived_at IS NULL AND m.confidence < ?3{single_since}
             AND {}",
            not_snoozed_sql("'low_confidence:' || m.memory_id", 1, 2)
        );
        let (low_count, low_ids) = self.single_tier(
            &low_where,
            "m.confidence ASC, m.created_at ASC, m.memory_id ASC",
            single_refs.as_slice(),
            params.limit.saturating_sub(items.len()),
        )?;
        totals.low_confidence = low_count;
        for id in &low_ids {
            items.push(ReviewItem {
                key: review_item_key(ReviewReason::LowConfidence, std::slice::from_ref(id)),
                reason: ReviewReason::LowConfidence,
                members: vec![self.review_member(id, now_ts)?],
                similarity: None,
            });
        }
        truncated |= low_count > low_ids.len() as u64;

        // Stale: the stats never-recalled predicate (`access_count = 0`) plus
        // the age bound; low-confidence rows are excluded so a memory is
        // listed once, under its higher tier.
        let stale_cutoff = now_ts - i64::from(REVIEW_STALE_DAYS) * 86_400;
        let mut stale_params: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(ns_str.clone()),
            Box::new(now_ts),
            Box::new(f64::from(REVIEW_LOW_CONFIDENCE_BOUND)),
        ];
        if let Some(since) = params.since {
            stale_params.push(Box::new(i64::try_from(since).unwrap_or(i64::MAX)));
        }
        stale_params.push(Box::new(stale_cutoff));
        let stale_cutoff_param = stale_params.len();
        let stale_refs: Vec<&dyn rusqlite::ToSql> =
            stale_params.iter().map(|b| b.as_ref()).collect();
        let stale_where = format!(
            "m.namespace = ?1 AND m.archived_at IS NULL AND m.access_count = 0
             AND m.created_at < ?{stale_cutoff_param} AND m.confidence >= ?3{single_since}
             AND {}",
            not_snoozed_sql("'stale_never_recalled:' || m.memory_id", 1, 2)
        );
        let (stale_count, stale_ids) = self.single_tier(
            &stale_where,
            "m.created_at ASC, m.memory_id ASC",
            stale_refs.as_slice(),
            params.limit.saturating_sub(items.len()),
        )?;
        totals.stale_never_recalled = stale_count;
        for id in &stale_ids {
            items.push(ReviewItem {
                key: review_item_key(ReviewReason::StaleNeverRecalled, std::slice::from_ref(id)),
                reason: ReviewReason::StaleNeverRecalled,
                members: vec![self.review_member(id, now_ts)?],
                similarity: None,
            });
        }
        truncated |= stale_count > stale_ids.len() as u64;

        Ok(ReviewPlan {
            items,
            totals,
            policy: None,
            planned: Vec::new(),
            truncated,
        })
    }

    /// Count + bounded fetch for one tier-3 predicate — count and rows share
    /// the WHERE so the gauge can never disagree with the listing.
    fn single_tier(
        &self,
        where_sql: &str,
        order_sql: &str,
        refs: &[&dyn rusqlite::ToSql],
        limit: usize,
    ) -> Result<(u64, Vec<MemoryId>)> {
        let count: i64 = self
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM memories m WHERE {where_sql}"),
                refs,
                |r| r.get(0),
            )
            .map_err(storage_err)?;
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT m.memory_id FROM memories m WHERE {where_sql}
                 ORDER BY {order_sql} LIMIT {limit}"
            ))
            .map_err(storage_err)?;
        let rows = stmt
            .query_map(refs, |r| r.get::<_, String>(0))
            .map_err(storage_err)?;
        let mut ids = Vec::new();
        for r in rows {
            ids.push(parse_id(&r.map_err(storage_err)?)?);
        }
        Ok((u64::try_from(count).unwrap_or(0), ids))
    }

    /// Load the display fields one review member needs (REV-1: summary,
    /// importance, confidence, age, provenance). Counts, ids and the STORED
    /// summary only — renderers redact before display.
    fn review_member(&self, id: &MemoryId, now_ts: i64) -> Result<ReviewMember> {
        self.conn
            .query_row(
                "SELECT summary, importance, confidence, created_at, last_accessed_at,
                        origin_source, origin_user
                 FROM memories WHERE memory_id = ?1",
                rusqlite::params![id.to_string()],
                |r| {
                    Ok(ReviewMember {
                        id: id.clone(),
                        summary: r.get(0)?,
                        importance: u8::try_from(r.get::<_, i64>(1)?).unwrap_or(u8::MAX),
                        confidence: r.get::<_, f64>(2)? as f32,
                        created_at: r.get(3)?,
                        age_days: u32::try_from((now_ts - r.get::<_, i64>(3)?) / 86_400)
                            .unwrap_or(0),
                        last_accessed_at: r.get(4)?,
                        origin_source: r.get(5)?,
                        origin_user: r.get(6)?,
                    })
                },
            )
            .map_err(storage_err)
    }

    /// Persist a snooze (REV-2): upsert the item's `review_state` row with
    /// `reviewed_at = now` and `snooze_until = now + days`, and append one
    /// `review_resolve` oplog row (REV-4) — one transaction. Returns the
    /// snooze expiry (unix seconds).
    pub fn snooze_review_item(
        &self,
        ns: &Namespace,
        key: &str,
        days: u32,
        details: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<i64> {
        if key.trim().is_empty() {
            return Err(Error::InvalidArgument(
                "review item key must not be empty".to_string(),
            ));
        }
        if days == 0 || days > REVIEW_MAX_SNOOZE_DAYS {
            return Err(Error::InvalidArgument(format!(
                "snooze days {days} is out of range 1..={REVIEW_MAX_SNOOZE_DAYS}"
            )));
        }
        let until = now.timestamp() + i64::from(days) * 86_400;
        immediate_tx(&self.conn, || {
            self.upsert_review_state(ns, key, now.timestamp(), Some(until))?;
            self.append_review_oplog(ns, "review_resolve", details, now)
        })?;
        Ok(until)
    }

    /// Record a non-snooze resolution (REV-4): stamp `reviewed_at` (clearing
    /// any snooze — the user acted on the item) and append one
    /// `review_resolve` oplog row — one transaction. The mutation itself
    /// (supersede/archive/update) runs through the existing primitives, each
    /// with its own oplog row.
    pub fn record_review_resolution(
        &self,
        ns: &Namespace,
        key: &str,
        details: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        if key.trim().is_empty() {
            return Err(Error::InvalidArgument(
                "review item key must not be empty".to_string(),
            ));
        }
        immediate_tx(&self.conn, || {
            self.upsert_review_state(ns, key, now.timestamp(), None)?;
            self.append_review_oplog(ns, "review_resolve", details, now)
        })
    }

    /// Append the bulk `review_sweep` oplog row (namespace-stamped, empty
    /// memory-id sentinel so replay skips it — the `retention_sweep`
    /// precedent). Written unconditionally after a policy apply pass, so a
    /// zero-change or partial pass is still a durable, auditable run.
    pub fn record_review_sweep(
        &self,
        ns: &Namespace,
        details: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        self.append_review_oplog(ns, "review_sweep", details, now)
    }

    fn upsert_review_state(
        &self,
        ns: &Namespace,
        key: &str,
        reviewed_at: i64,
        snooze_until: Option<i64>,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO review_state (item_key, namespace, reviewed_at, snooze_until)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(item_key) DO UPDATE SET
                   namespace = excluded.namespace,
                   reviewed_at = excluded.reviewed_at,
                   snooze_until = excluded.snooze_until",
                rusqlite::params![key, ns.as_db_string(), reviewed_at, snooze_until],
            )
            .map_err(storage_err)?;
        Ok(())
    }

    fn append_review_oplog(
        &self,
        ns: &Namespace,
        op: &str,
        details: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO memory_oplog (site_id, op, memory_id, namespace, at, details)
                 VALUES (?1, ?2, '', ?3, ?4, ?5)",
                rusqlite::params![
                    self.site_id,
                    op,
                    ns.as_db_string(),
                    now.timestamp(),
                    details
                ],
            )
            .map_err(storage_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod review_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::super::*;
    use rb_types::{
        review_item_key, MemoryId, MemoryNote, MemoryType, Namespace, ReviewReason,
        REVIEW_DEFAULT_SNOOZE_DAYS,
    };

    fn ns() -> Namespace {
        Namespace::Project("review".into())
    }

    fn params() -> ReviewQueueParams {
        ReviewQueueParams {
            threshold: 0.95,
            limit: 50,
            since: None,
        }
    }

    fn insert(
        store: &SqliteStore,
        namespace: &Namespace,
        content: &str,
        f: impl FnOnce(&mut MemoryNote),
    ) -> MemoryId {
        let mut m = MemoryNote::new(namespace.clone(), content.into(), MemoryType::Insight, 5);
        m.summary = content.to_string();
        f(&mut m);
        let id = m.id.clone();
        store.insert_memory(&m, None).unwrap();
        id
    }

    fn insert_vec(
        store: &SqliteStore,
        namespace: &Namespace,
        content: &str,
        v: [f32; 8],
    ) -> MemoryId {
        let mut m = MemoryNote::new(namespace.clone(), content.into(), MemoryType::Insight, 5);
        m.summary = content.to_string();
        let id = m.id.clone();
        store.insert_memory(&m, Some(&v)).unwrap();
        id
    }

    fn contradict(store: &SqliteStore, a: &MemoryId, b: &MemoryId) {
        store
            .add_link(&rb_types::MemoryLink {
                source_id: a.clone(),
                target_id: b.clone(),
                link_type: rb_types::LinkType::Contradicts,
                strength: 1.0,
                reason: "test contradiction".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
    }

    fn queue(store: &SqliteStore, p: &ReviewQueueParams) -> rb_types::ReviewPlan {
        store.review_queue(&ns(), p, chrono::Utc::now()).unwrap()
    }

    // --- contradictions (priority 1) -----------------------------------------

    /// DRIFT TEST: the queue's contradiction items must resolve contested
    /// state through the same semantics as `active_contradicts` (the source
    /// of truth `contested` filtering also agrees with). Change the pair SQL
    /// without this invariant and this test fails.
    #[test]
    fn contradiction_items_agree_with_active_contradicts() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let other = Namespace::Project("elsewhere".into());

        // An active in-namespace pair: BOTH endpoints contested.
        let a = insert(&store, &ns(), "a", |_| {});
        let b = insert(&store, &ns(), "b", |_| {});
        contradict(&store, &a, &b);

        // A pair whose far endpoint is archived: NOT contested.
        let c = insert(&store, &ns(), "c", |_| {});
        let dead = insert(&store, &ns(), "dead", |_| {});
        contradict(&store, &c, &dead);
        store.archive_memory(&dead).unwrap();

        // A cross-namespace edge: NEVER contested in `ns`.
        let d = insert(&store, &ns(), "d", |_| {});
        let foreign = insert(&store, &other, "foreign", |_| {});
        contradict(&store, &d, &foreign);

        let all = vec![a.clone(), b.clone(), c.clone(), dead.clone(), d.clone()];
        let truth = store.active_contradicts(&ns(), &all).unwrap();

        let plan = queue(&store, &params());
        let mut member_ids: Vec<MemoryId> = plan
            .items
            .iter()
            .filter(|i| i.reason == ReviewReason::Contradiction)
            .flat_map(|i| i.members.iter().map(|m| m.id.clone()))
            .collect();
        member_ids.sort_by_key(std::string::ToString::to_string);
        let mut expected: Vec<MemoryId> = truth.into_iter().collect();
        expected.sort_by_key(std::string::ToString::to_string);
        assert_eq!(
            member_ids, expected,
            "contradiction items must contain exactly the active_contradicts set"
        );
        assert_eq!(plan.totals.contradictions, 1, "one actionable pair");
    }

    #[test]
    fn a_contradiction_pair_is_one_item_with_a_canonical_key() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = insert(&store, &ns(), "a", |_| {});
        let b = insert(&store, &ns(), "b", |_| {});
        // Both directions on the same pair must still yield ONE item.
        contradict(&store, &a, &b);
        contradict(&store, &b, &a);

        let plan = queue(&store, &params());
        let items: Vec<_> = plan
            .items
            .iter()
            .filter(|i| i.reason == ReviewReason::Contradiction)
            .collect();
        assert_eq!(items.len(), 1, "one pair, one item: {items:?}");
        assert_eq!(
            items[0].key,
            review_item_key(ReviewReason::Contradiction, &[a.clone(), b.clone()])
        );
        assert_eq!(items[0].members.len(), 2);
        // Members carry what the human decision needs.
        assert!(!items[0].members[0].summary.is_empty());
    }

    // --- near-duplicates (priority 2) -----------------------------------------

    #[test]
    fn near_dup_items_come_from_near_duplicates_with_similarity() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let v = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let a = insert_vec(&store, &ns(), "twin one", v);
        let b = insert_vec(&store, &ns(), "twin two", v);
        // Orthogonal: similarity ~0, never a dup at any sane threshold.
        let c = insert_vec(
            &store,
            &ns(),
            "different",
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );

        let plan = queue(&store, &params());
        let dups: Vec<_> = plan
            .items
            .iter()
            .filter(|i| i.reason == ReviewReason::NearDuplicate)
            .collect();
        assert_eq!(dups.len(), 1, "exactly the twin pair: {dups:?}");
        let item = dups[0];
        assert_eq!(
            item.key,
            review_item_key(ReviewReason::NearDuplicate, &[a.clone(), b.clone()])
        );
        assert!(
            item.similarity.unwrap() >= 0.95,
            "similarity is the raw near_duplicates() unit: {:?}",
            item.similarity
        );
        let ids: Vec<&MemoryId> = item.members.iter().map(|m| &m.id).collect();
        assert!(!ids.contains(&&c), "the orthogonal memory is not a dup");
    }

    #[test]
    fn near_dup_members_join_at_most_one_pair_per_sweep() {
        // Three near-identical memories: ONE pair this sweep (the remaining
        // member re-pairs on a later sweep once the pair is resolved) — a
        // member appearing in two merge plans could double-supersede.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let v = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        insert_vec(&store, &ns(), "one", v);
        insert_vec(&store, &ns(), "two", v);
        insert_vec(&store, &ns(), "three", v);

        let plan = queue(&store, &params());
        let dups: Vec<_> = plan
            .items
            .iter()
            .filter(|i| i.reason == ReviewReason::NearDuplicate)
            .collect();
        assert_eq!(dups.len(), 1, "one pair per member per sweep: {dups:?}");
        let mut seen = std::collections::HashSet::new();
        for m in dups.iter().flat_map(|i| i.members.iter()) {
            assert!(
                seen.insert(m.id.to_string()),
                "member repeated across items"
            );
        }
    }

    #[test]
    fn near_dup_threshold_is_honored() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // Two similar-but-not-identical vectors: cosine similarity ~0.894.
        insert_vec(
            &store,
            &ns(),
            "close one",
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        insert_vec(
            &store,
            &ns(),
            "close two",
            [1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        );
        let strict = queue(&store, &params());
        assert!(
            !strict
                .items
                .iter()
                .any(|i| i.reason == ReviewReason::NearDuplicate),
            "0.894 similarity is below the 0.95 threshold"
        );
        let loose = store
            .review_queue(
                &ns(),
                &ReviewQueueParams {
                    threshold: 0.85,
                    ..params()
                },
                chrono::Utc::now(),
            )
            .unwrap();
        assert!(
            loose
                .items
                .iter()
                .any(|i| i.reason == ReviewReason::NearDuplicate),
            "a looser threshold admits the pair"
        );
    }

    // --- tier 3: low confidence + stale never-recalled -------------------------

    #[test]
    fn low_confidence_and_stale_items_fill_tier_three() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let low = insert(&store, &ns(), "low confidence", |m| m.confidence = 0.2);
        let _confident = insert(&store, &ns(), "confident", |m| m.confidence = 0.9);
        let stale = insert(&store, &ns(), "stale never recalled", |m| {
            m.confidence = 0.9;
            m.created_at = chrono::Utc::now() - chrono::Duration::days(90);
        });
        // Old but recalled: not stale.
        let recalled = insert(&store, &ns(), "old but recalled", |m| {
            m.confidence = 0.9;
            m.created_at = chrono::Utc::now() - chrono::Duration::days(90);
        });
        store.record_access(&recalled).unwrap();
        // Low confidence AND stale: listed once, as low-confidence (the
        // higher tier), never twice.
        let both = insert(&store, &ns(), "low and stale", |m| {
            m.confidence = 0.1;
            m.created_at = chrono::Utc::now() - chrono::Duration::days(90);
        });
        // Archived low-confidence: never listed.
        let dead = insert(&store, &ns(), "archived low", |m| m.confidence = 0.1);
        store.archive_memory(&dead).unwrap();

        let plan = queue(&store, &params());
        let low_items: Vec<&MemoryId> = plan
            .items
            .iter()
            .filter(|i| i.reason == ReviewReason::LowConfidence)
            .map(|i| &i.members[0].id)
            .collect();
        let stale_items: Vec<&MemoryId> = plan
            .items
            .iter()
            .filter(|i| i.reason == ReviewReason::StaleNeverRecalled)
            .map(|i| &i.members[0].id)
            .collect();
        assert!(low_items.contains(&&low));
        assert!(low_items.contains(&&both));
        assert!(!low_items.contains(&&dead), "archived rows never enter");
        assert_eq!(
            stale_items,
            vec![&stale],
            "never-recalled + old, not {stale_items:?}"
        );
        assert_eq!(plan.totals.low_confidence, 2);
        assert_eq!(plan.totals.stale_never_recalled, 1);
        // Priority order within tier 3: low-confidence before stale.
        let reasons: Vec<ReviewReason> = plan.items.iter().map(|i| i.reason).collect();
        let low_pos = reasons
            .iter()
            .position(|r| *r == ReviewReason::LowConfidence)
            .unwrap();
        let stale_pos = reasons
            .iter()
            .position(|r| *r == ReviewReason::StaleNeverRecalled)
            .unwrap();
        assert!(low_pos < stale_pos);
    }

    /// DRIFT TEST: the stale tier's "never recalled" predicate must agree
    /// with the stats surface (`never_recalled_live`: active rows with
    /// `access_count = 0`) — REV-1 sources tier 3 "from the stats surface".
    #[test]
    fn stale_never_recalled_agrees_with_the_stats_predicate() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        for i in 0..3 {
            insert(&store, &ns(), &format!("old {i}"), |m| {
                m.created_at = chrono::Utc::now() - chrono::Duration::days(90);
            });
        }
        let recalled = insert(&store, &ns(), "old recalled", |m| {
            m.created_at = chrono::Utc::now() - chrono::Duration::days(90);
        });
        store.record_access(&recalled).unwrap();

        let stats = store.namespace_stats(&ns(), 30, "", "", 5, None).unwrap();
        let plan = queue(&store, &params());
        let stale_count = plan
            .items
            .iter()
            .filter(|i| i.reason == ReviewReason::StaleNeverRecalled)
            .count() as u64;
        // Every memory here is older than the stale horizon, so the stale
        // tier must equal the stats gauge exactly.
        assert_eq!(stale_count, stats.never_recalled_live);
        assert_eq!(plan.totals.stale_never_recalled, stats.never_recalled_live);
    }

    // --- ordering, bounds, isolation -------------------------------------------

    #[test]
    fn queue_is_priority_ordered_and_bounded_with_truncation_flag() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // Tier 3 first so insertion order cannot fake priority order.
        insert(&store, &ns(), "low", |m| m.confidence = 0.2);
        let v = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        insert_vec(&store, &ns(), "twin one", v);
        insert_vec(&store, &ns(), "twin two", v);
        let a = insert(&store, &ns(), "contra a", |_| {});
        let b = insert(&store, &ns(), "contra b", |_| {});
        contradict(&store, &a, &b);

        let plan = queue(&store, &params());
        let reasons: Vec<ReviewReason> = plan.items.iter().map(|i| i.reason).collect();
        assert_eq!(
            reasons,
            vec![
                ReviewReason::Contradiction,
                ReviewReason::NearDuplicate,
                ReviewReason::LowConfidence,
            ]
        );
        assert!(!plan.truncated);

        // A limit of 1 keeps only the highest-priority item and flags the cut.
        let bounded = store
            .review_queue(
                &ns(),
                &ReviewQueueParams {
                    limit: 1,
                    ..params()
                },
                chrono::Utc::now(),
            )
            .unwrap();
        assert_eq!(bounded.items.len(), 1);
        assert_eq!(bounded.items[0].reason, ReviewReason::Contradiction);
        assert!(bounded.truncated);
        // Totals still report the full picture.
        assert_eq!(bounded.totals.contradictions, 1);
        assert_eq!(bounded.totals.low_confidence, 1);
    }

    #[test]
    fn queue_is_namespace_isolated() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let other = Namespace::Project("elsewhere".into());
        let a = insert(&store, &other, "foreign a", |m| m.confidence = 0.1);
        let b = insert(&store, &other, "foreign b", |_| {});
        contradict(&store, &a, &b);
        let v = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        insert_vec(&store, &other, "foreign twin 1", v);
        insert_vec(&store, &other, "foreign twin 2", v);

        let plan = queue(&store, &params());
        assert!(
            plan.items.is_empty(),
            "foreign items leaked: {:?}",
            plan.items
        );
        assert_eq!(plan.totals, rb_types::ReviewTotals::default());
    }

    #[test]
    fn queue_rejects_out_of_range_params_fail_closed() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        for bad in [
            ReviewQueueParams {
                threshold: 0.5,
                ..params()
            },
            ReviewQueueParams {
                threshold: 1.5,
                ..params()
            },
            ReviewQueueParams {
                threshold: f32::NAN,
                ..params()
            },
            ReviewQueueParams {
                limit: 0,
                ..params()
            },
            ReviewQueueParams {
                limit: (rb_types::REVIEW_MAX_LIMIT + 1) as usize,
                ..params()
            },
        ] {
            let err = store
                .review_queue(&ns(), &bad, chrono::Utc::now())
                .unwrap_err();
            assert!(matches!(err, Error::InvalidArgument(_)), "{bad:?}: {err:?}");
        }
    }

    // --- snooze -----------------------------------------------------------------

    #[test]
    fn snoozed_item_stays_hidden_until_the_window_elapses() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = insert(&store, &ns(), "a", |_| {});
        let b = insert(&store, &ns(), "b", |_| {});
        contradict(&store, &a, &b);
        let key = review_item_key(ReviewReason::Contradiction, &[a, b]);

        let now = chrono::Utc::now();
        let until = store
            .snooze_review_item(&ns(), &key, REVIEW_DEFAULT_SNOOZE_DAYS, "{}", now)
            .unwrap();
        assert_eq!(
            until,
            now.timestamp() + i64::from(REVIEW_DEFAULT_SNOOZE_DAYS) * 86_400
        );

        let hidden = store.review_queue(&ns(), &params(), now).unwrap();
        assert!(
            hidden.items.is_empty(),
            "snoozed item resurfaced early: {:?}",
            hidden.items
        );
        assert_eq!(hidden.totals.snoozed, 1);
        assert_eq!(hidden.totals.contradictions, 0, "snoozed = not actionable");

        // One second past the window it reappears (REV acceptance).
        let later = now
            + chrono::Duration::days(i64::from(REVIEW_DEFAULT_SNOOZE_DAYS))
            + chrono::Duration::seconds(1);
        let back = store.review_queue(&ns(), &params(), later).unwrap();
        assert_eq!(back.items.len(), 1, "expired snooze must resurface");
        assert_eq!(back.totals.snoozed, 0, "expired snoozes don't count");
    }

    #[test]
    fn snooze_exclusion_fills_the_limit_beyond_snoozed_rows() {
        // The PR #58 lesson: exclusion must be resolved INSIDE the bounded
        // query — a snoozed row must not consume the limit budget.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let first = insert(&store, &ns(), "low one", |m| {
            m.confidence = 0.1;
            m.created_at = chrono::Utc::now() - chrono::Duration::days(2);
        });
        let second = insert(&store, &ns(), "low two", |m| {
            m.confidence = 0.2;
            m.created_at = chrono::Utc::now() - chrono::Duration::days(1);
        });
        let key = review_item_key(ReviewReason::LowConfidence, std::slice::from_ref(&first));
        store
            .snooze_review_item(&ns(), &key, 7, "{}", chrono::Utc::now())
            .unwrap();

        let plan = store
            .review_queue(
                &ns(),
                &ReviewQueueParams {
                    limit: 1,
                    ..params()
                },
                chrono::Utc::now(),
            )
            .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(
            plan.items[0].members[0].id, second,
            "the unsnoozed row must fill the bounded window"
        );
    }

    #[test]
    fn snooze_upserts_and_appends_a_review_resolve_oplog_row() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let key = "contradiction:x:y";
        let now = chrono::Utc::now();
        store
            .snooze_review_item(&ns(), key, 7, r#"{"action":"snooze"}"#, now)
            .unwrap();
        // Snoozing again REPLACES the row (one row per item, bounded).
        store
            .snooze_review_item(&ns(), key, 14, r#"{"action":"snooze"}"#, now)
            .unwrap();
        let (count, until): (i64, i64) = store
            .conn
            .query_row(
                "SELECT COUNT(*), MAX(snooze_until) FROM review_state WHERE item_key = ?1",
                rusqlite::params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "upsert, not append");
        assert_eq!(until, now.timestamp() + 14 * 86_400);

        let ops: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_oplog WHERE op = 'review_resolve' AND namespace = ?1",
                rusqlite::params![ns().as_db_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ops, 2, "every snooze is oplog-recorded (REV-4)");
    }

    #[test]
    fn record_review_resolution_marks_reviewed_at_and_appends_oplog() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let key = "near_duplicate:x:y";
        let now = chrono::Utc::now();
        store
            .record_review_resolution(&ns(), key, r#"{"action":"keep"}"#, now)
            .unwrap();
        let (reviewed_at, snooze_until): (i64, Option<i64>) = store
            .conn
            .query_row(
                "SELECT reviewed_at, snooze_until FROM review_state WHERE item_key = ?1",
                rusqlite::params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(reviewed_at, now.timestamp());
        assert_eq!(snooze_until, None, "a non-snooze resolution never hides");
        let details: String = store
            .conn
            .query_row(
                "SELECT details FROM memory_oplog WHERE op = 'review_resolve'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(details.contains("keep"), "{details}");
        // A later snooze on the same key must not lose the row (upsert).
        store.snooze_review_item(&ns(), key, 7, "{}", now).unwrap();
        // And a later plain resolution CLEARS the snooze (the user acted on it).
        store
            .record_review_resolution(&ns(), key, r#"{"action":"demote"}"#, now)
            .unwrap();
        let snooze_until: Option<i64> = store
            .conn
            .query_row(
                "SELECT snooze_until FROM review_state WHERE item_key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(snooze_until, None, "acting on an item clears its snooze");
    }

    #[test]
    fn record_review_sweep_appends_a_namespace_stamped_bulk_row() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        store
            .record_review_sweep(
                &ns(),
                r#"{"policy":"auto-merge-dups","merged":2}"#,
                chrono::Utc::now(),
            )
            .unwrap();
        let (op, memory_id, namespace, details): (String, String, String, String) = store
            .conn
            .query_row(
                "SELECT op, memory_id, namespace, details FROM memory_oplog",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(op, "review_sweep");
        assert_eq!(
            memory_id, "",
            "bulk rows use the empty sentinel (replay skips)"
        );
        assert_eq!(namespace, ns().as_db_string());
        assert!(details.contains("auto-merge-dups"));
    }

    // --- since ------------------------------------------------------------------

    #[test]
    fn since_limits_the_queue_to_recently_touched_memories() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = insert(&store, &ns(), "old low", |m| m.confidence = 0.1);
        let cutoff = store.latest_oplog_seq_for(&a).unwrap().unwrap();
        let b = insert(&store, &ns(), "new low", |m| m.confidence = 0.2);

        let plan = store
            .review_queue(
                &ns(),
                &ReviewQueueParams {
                    since: Some(cutoff),
                    ..params()
                },
                chrono::Utc::now(),
            )
            .unwrap();
        let ids: Vec<&MemoryId> = plan.items.iter().map(|i| &i.members[0].id).collect();
        assert_eq!(ids, vec![&b], "only the memory touched after the cursor");

        // A pair qualifies when EITHER side was touched after the cursor.
        let c = insert(&store, &ns(), "contra c", |_| {});
        let cutoff2 = store.latest_oplog_seq_for(&c).unwrap().unwrap();
        contradict(&store, &a, &c);
        let plan = store
            .review_queue(
                &ns(),
                &ReviewQueueParams {
                    since: Some(cutoff2),
                    ..params()
                },
                chrono::Utc::now(),
            )
            .unwrap();
        assert!(
            plan.items
                .iter()
                .any(|i| i.reason == ReviewReason::Contradiction),
            "the link touched a side after the cursor: {:?}",
            plan.items
        );
    }
}
