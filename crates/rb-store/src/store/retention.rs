//! Retention/forget sweep (retention PRD): candidate planning and the
//! bounded, guarded sweep over the archive/purge primitives.
//!
//! One candidate query is the single source of truth: the dry-run plan
//! renders it (read-only) and `retention_sweep` re-runs the SAME function on
//! the single-writer connection immediately before mutating, so the preview
//! and the mutation cannot drift structurally (no TOCTOU window exists on the
//! writer thread — nothing interleaves between planning and mutating).
//!
//! Eligibility guards, all absolute (PRD RET-1/RET-3):
//! - age: `created_at` older than the mode's horizon (apply: the earlier of
//!   `archive_after_days`/`max_age_days`; hard: `max_age_days` only — the
//!   archive horizon must never authorize a purge);
//! - importance floor over BOTH the effective importance and the author
//!   prior (`COALESCE(base_importance, importance)`) — the W1.9 author-intent
//!   clamp productized: a signal-degraded effective value can never defeat
//!   author intent, so an authored importance-10 memory is never eligible;
//! - protected tags (any match exempts);
//! - contested exclusion: the SQL predicate below is the third expression of
//!   the ONE `active_contradicts` semantics (with `list_filtered` and the
//!   stats aggregate), pinned by
//!   `contested_memory_survives_sweep_and_agrees_with_active_contradicts`.

use super::internal::*;
use super::*;
use crate::error::storage_err;
use rb_types::{
    ForgetCandidate, ForgetMode, ForgetOutcome, ForgetPlan, ForgetRule, RetentionPolicy,
};

/// Ids a sweep actually touched, for the daemon to publish change events.
/// Store-internal (not a wire shape): the wire carries [`ForgetOutcome`].
#[derive(Debug, Clone, Default)]
pub struct RetentionSweepEffects {
    pub archived_ids: Vec<MemoryId>,
    pub purged_ids: Vec<MemoryId>,
}

/// A built WHERE fragment: the SQL text plus its positional params.
type WhereClause = (String, Vec<Box<dyn rusqlite::ToSql>>);

/// The WHERE clause + params shared by the plan SELECT, the total COUNT, and
/// the stats eligibility gauge — one builder so they cannot diverge.
/// `Ok(None)` means the policy has no age rule for this mode ⇒ empty set
/// (apply with neither horizon set); hard mode without `max_age_days` is a
/// fail-closed error instead (an absent forget horizon must never default).
fn retention_where(
    ns: &Namespace,
    policy: &RetentionPolicy,
    mode: ForgetMode,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<WhereClause>> {
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(ns.as_db_string())];
    let mut sql = String::from("m.namespace = ?1");

    // Horizon cutoffs. `days as i64 * 86_400 <= u32::MAX * 86_400 ≈ 3.7e14`
    // and `now.timestamp() ≈ 1.8e9`, so the subtraction cannot overflow i64
    // (a huge horizon just yields a far-negative cutoff that matches nothing).
    let cutoff = |days: u32| now.timestamp() - i64::from(days) * 86_400;
    let forget_cutoff = policy.max_age_days.map(cutoff);
    match mode {
        ForgetMode::Apply => {
            // Apply archives ACTIVE rows past the earlier horizon (validation
            // guarantees archive_after_days <= max_age_days when both exist).
            let horizon = policy.archive_after_days.or(policy.max_age_days);
            let Some(days) = horizon else {
                return Ok(None);
            };
            params.push(Box::new(cutoff(days)));
            sql.push_str(&format!(
                " AND m.archived_at IS NULL AND m.created_at < ?{}",
                params.len()
            ));
        }
        ForgetMode::Hard => {
            let Some(bound) = forget_cutoff else {
                return Err(Error::InvalidArgument(
                    "hard forget requires retention.max_age_days: the archive horizon \
                     never authorizes a purge"
                        .to_string(),
                ));
            };
            // Active AND already-archived rows: hard is the terminal stage of
            // the archive-then-purge lifecycle.
            params.push(Box::new(bound));
            sql.push_str(&format!(" AND m.created_at < ?{}", params.len()));
        }
    }

    // Importance floor over the effective value AND the author prior (W1.9).
    params.push(Box::new(i64::from(policy.importance_floor)));
    let floor_param = params.len();
    sql.push_str(&format!(
        " AND m.importance < ?{floor_param} \
         AND COALESCE(m.base_importance, m.importance) < ?{floor_param}"
    ));

    // Protected tags: any match exempts the memory entirely.
    if !policy.protected_tags.is_empty() {
        let start = params.len() + 1;
        let placeholders: Vec<String> = (0..policy.protected_tags.len())
            .map(|i| format!("?{}", start + i))
            .collect();
        sql.push_str(&format!(
            " AND NOT EXISTS (SELECT 1 FROM json_each(m.tags) \
             WHERE json_each.value IN ({}))",
            placeholders.join(", ")
        ));
        for tag in &policy.protected_tags {
            params.push(Box::new(tag.clone()));
        }
    }

    // Contested exclusion — the `list_filtered` predicate verbatim (an active
    // `contradicts` edge whose far endpoint is active AND in-namespace, local
    // endpoint active). An archived row is never contested, so `NOT (...)`
    // correctly admits archived rows in hard mode. Kept in lockstep with
    // `active_contradicts` by the sweep drift test.
    params.push(Box::new(ns.as_db_string()));
    let ns_param = params.len();
    sql.push_str(&format!(
        " AND NOT (m.archived_at IS NULL AND (EXISTS (
             SELECT 1 FROM memory_links l
               JOIN memories far ON far.memory_id = l.target_id
             WHERE l.link_type = 'contradicts'
               AND l.source_id = m.memory_id
               AND far.archived_at IS NULL
               AND far.namespace = ?{ns_param}
         ) OR EXISTS (
             SELECT 1 FROM memory_links l
               JOIN memories far ON far.memory_id = l.source_id
             WHERE l.link_type = 'contradicts'
               AND l.target_id = m.memory_id
               AND far.archived_at IS NULL
               AND far.namespace = ?{ns_param}
         )))"
    ));

    Ok(Some((sql, params)))
}

impl SqliteStore {
    /// The forget plan: exactly the candidate set one sweep pass in `mode`
    /// would touch (bounded by `batch_limit`, oldest first) plus the total
    /// eligible count. Read-only — this IS the dry-run (RET-2), and
    /// [`Self::retention_sweep`] calls it for the execute path, so preview
    /// and mutation share one query. Allowed for a disabled policy (previews
    /// are read-only); the policy is still validated fail-closed.
    pub fn retention_candidates(
        &self,
        ns: &Namespace,
        policy: &RetentionPolicy,
        mode: ForgetMode,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ForgetPlan> {
        policy.validate()?;
        let Some((where_sql, params)) = retention_where(ns, policy, mode, now)? else {
            return Ok(ForgetPlan {
                mode,
                ..ForgetPlan::default()
            });
        };
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();

        let total: i64 = self
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM memories m WHERE {where_sql}"),
                refs.as_slice(),
                |r| r.get(0),
            )
            .map_err(storage_err)?;

        let forget_cutoff = policy
            .max_age_days
            .map(|days| now.timestamp() - i64::from(days) * 86_400);
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT m.memory_id, m.summary, m.created_at, m.importance,
                        COALESCE(m.base_importance, m.importance),
                        m.last_accessed_at, m.archived_at IS NOT NULL
                 FROM memories m WHERE {where_sql}
                 ORDER BY m.created_at ASC LIMIT {}",
                i64::from(policy.batch_limit)
            ))
            .map_err(storage_err)?;
        let mut rows = stmt.query(refs.as_slice()).map_err(storage_err)?;
        let mut candidates = Vec::new();
        while let Some(row) = rows.next().map_err(storage_err)? {
            let id: String = row.get(0).map_err(storage_err)?;
            let summary: String = row.get(1).map_err(storage_err)?;
            let created_at: i64 = row.get(2).map_err(storage_err)?;
            let importance: i64 = row.get(3).map_err(storage_err)?;
            let base_importance: i64 = row.get(4).map_err(storage_err)?;
            let last_accessed_at: Option<i64> = row.get(5).map_err(storage_err)?;
            let archived: bool = row.get(6).map_err(storage_err)?;
            let rule = match forget_cutoff {
                Some(bound) if created_at < bound => ForgetRule::MaxAge,
                _ => ForgetRule::ArchiveAge,
            };
            candidates.push(ForgetCandidate {
                id: parse_id(&id)?,
                summary,
                created_at,
                age_days: u32::try_from((now.timestamp() - created_at) / 86_400).unwrap_or(0),
                importance: u8::try_from(importance).unwrap_or(u8::MAX),
                base_importance: u8::try_from(base_importance).unwrap_or(u8::MAX),
                last_accessed_at,
                archived,
                rule,
            });
        }
        Ok(ForgetPlan {
            mode,
            candidates,
            total_eligible: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Execute one bounded forget pass (RET-2/RET-3): recompute the candidate
    /// set with the same query the dry-run showed, then archive (apply) or
    /// purge (hard) each candidate — one transaction per memory, so a failure
    /// mid-pass leaves every earlier item durably committed and the pass
    /// re-runnable. Refuses a disabled policy outright (off-by-default is
    /// enforced here as the last line of defense; the daemon refuses earlier).
    /// Appends one bulk `retention_sweep` oplog row (namespace-stamped, empty
    /// memory-id sentinel so replay skips it) and, after any purge,
    /// checkpoint-truncates the WAL so purged content does not survive at
    /// rest in the sidecar (the scrub precedent).
    pub fn retention_sweep(
        &self,
        ns: &Namespace,
        policy: &RetentionPolicy,
        mode: ForgetMode,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(ForgetOutcome, RetentionSweepEffects)> {
        policy.validate()?;
        if !policy.enabled {
            return Err(Error::InvalidArgument(
                "retention is not enabled (retention.enabled = false): refusing to \
                 mutate; use a dry-run to preview, or enable it in [retention]"
                    .to_string(),
            ));
        }
        let plan = self.retention_candidates(ns, policy, mode, now)?;
        // Hard mode zeroes freed bytes as pages are released: a plain DELETE
        // leaves the old row content readable in free-page slack (the W5b.3
        // gap the scrub TODO documents), which the raw-bytes drill test
        // catches. Scoped to the purge loop and always reset — the daemon's
        // writer connection is long-lived.
        if mode == ForgetMode::Hard {
            self.conn
                .execute_batch("PRAGMA secure_delete = ON;")
                .map_err(storage_err)?;
        }
        let loop_result = (|| {
            let mut effects = RetentionSweepEffects::default();
            for cand in &plan.candidates {
                let details = serde_json::json!({
                    "cause": "retention",
                    "mode": mode.as_str(),
                    "rule": cand.rule,
                })
                .to_string();
                match mode {
                    ForgetMode::Apply => {
                        if archive_with_details(&self.conn, &self.site_id, &cand.id, &details)? {
                            effects.archived_ids.push(cand.id.clone());
                        }
                    }
                    ForgetMode::Hard => {
                        if self.purge_memory_for_retention(&cand.id, &details)? {
                            effects.purged_ids.push(cand.id.clone());
                        }
                    }
                }
            }
            Ok(effects)
        })();
        if mode == ForgetMode::Hard {
            // Always restore the default, even when the loop failed midway.
            self.conn
                .execute_batch("PRAGMA secure_delete = OFF;")
                .map_err(storage_err)?;
        }
        let effects = loop_result?;
        let archived = effects.archived_ids.len() as u64;
        let purged = effects.purged_ids.len() as u64;
        if archived + purged > 0 {
            let details = serde_json::json!({
                "mode": mode.as_str(),
                "archived": archived,
                "purged": purged,
            })
            .to_string();
            self.conn
                .execute(
                    "INSERT INTO memory_oplog (site_id, op, memory_id, namespace, at, details)
                     VALUES (?1, 'retention_sweep', '', ?2, ?3, ?4)",
                    rusqlite::params![self.site_id, ns.as_db_string(), now.timestamp(), details],
                )
                .map_err(storage_err)?;
        }
        if purged > 0 {
            // Purged content must not survive at rest in the -wal sidecar
            // (the scrub drill rule). Best-effort no-op on in-memory DBs.
            self.conn
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(storage_err)?;
        }
        Ok((
            ForgetOutcome {
                mode,
                archived,
                purged,
                total_eligible: plan.total_eligible,
                // The sweep runs on the single-writer connection, so nothing
                // interleaves between plan and execute: unprocessed = the
                // overflow past batch_limit.
                remaining: plan.total_eligible.saturating_sub(archived + purged),
            },
            effects,
        ))
    }

    /// Latest oplog seq recorded for `id` — the row the sweep just wrote for
    /// it. Lets the daemon stamp each per-memory change event with its OWN
    /// replay cursor instead of the batch-final `last_oplog_seq()`: stamping
    /// a whole burst with the final seq would let a subscriber that cursors
    /// past it mid-burst skip replaying a later item it never received.
    pub fn latest_oplog_seq_for(&self, id: &MemoryId) -> Result<Option<u64>> {
        let seq: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(seq) FROM memory_oplog WHERE memory_id = ?1",
                rusqlite::params![id.to_string()],
                |r| r.get(0),
            )
            .map_err(storage_err)?;
        Ok(seq.and_then(|s| u64::try_from(s).ok()))
    }

    /// Every namespace that still has memory rows — the scheduled retention
    /// job iterates these, applying the one user-global policy per namespace
    /// (the sweep itself is strictly namespace-scoped). Unparseable namespace
    /// strings fail closed rather than being silently skipped.
    pub fn retention_namespaces(&self) -> Result<Vec<Namespace>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT namespace FROM memories ORDER BY namespace")
            .map_err(storage_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for row in rows {
            let s = row.map_err(storage_err)?;
            out.push(Namespace::parse_db_string(&s)?);
        }
        Ok(out)
    }

    /// Hard-purge one memory and every sidecar in ONE transaction: links
    /// (both directions — the FK has no ON DELETE), dangling `superseded_by`
    /// references, the vec0 row, feedback rows, the memory's own oplog
    /// history, and the row itself (the `mem_ad` trigger cascades the FTS
    /// delete). Leaves exactly one namespace-stamped `purge` oplog marker so
    /// history can explain the disappearance (RET-4). A missing id is an
    /// `Ok(false)` no-op that logs nothing.
    pub(crate) fn purge_memory_for_retention(&self, id: &MemoryId, details: &str) -> Result<bool> {
        immediate_tx(&self.conn, || {
            let ns: Option<String> = self
                .conn
                .query_row(
                    "SELECT namespace FROM memories WHERE memory_id = ?1",
                    rusqlite::params![id.to_string()],
                    |r| r.get(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(storage_err(other)),
                })?;
            let Some(ns) = ns else {
                return Ok(false);
            };
            let id_str = id.to_string();
            self.conn
                .execute(
                    "DELETE FROM memory_links WHERE source_id = ?1 OR target_id = ?1",
                    rusqlite::params![id_str],
                )
                .map_err(storage_err)?;
            // Rows archived-by-supersession point at their successor; a
            // purged successor must not leave an FK-dangling pointer.
            self.conn
                .execute(
                    "UPDATE memories SET superseded_by = NULL WHERE superseded_by = ?1",
                    rusqlite::params![id_str],
                )
                .map_err(storage_err)?;
            self.conn
                .execute(
                    "DELETE FROM memory_vectors WHERE memory_id = ?1",
                    rusqlite::params![id_str],
                )
                .map_err(storage_err)?;
            self.conn
                .execute(
                    "DELETE FROM memory_feedback WHERE memory_id = ?1",
                    rusqlite::params![id_str],
                )
                .map_err(storage_err)?;
            self.conn
                .execute(
                    "DELETE FROM memory_oplog WHERE memory_id = ?1",
                    rusqlite::params![id_str],
                )
                .map_err(storage_err)?;
            self.conn
                .execute(
                    "DELETE FROM memories WHERE memory_id = ?1",
                    rusqlite::params![id_str],
                )
                .map_err(storage_err)?;
            self.conn
                .execute(
                    "INSERT INTO memory_oplog (site_id, op, memory_id, namespace, at, details)
                     VALUES (?1, 'purge', ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        self.site_id,
                        id_str,
                        ns,
                        chrono::Utc::now().timestamp(),
                        details
                    ],
                )
                .map_err(storage_err)?;
            Ok(true)
        })
    }
}

#[cfg(test)]
mod retention_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::super::*;
    use rb_types::{
        ForgetMode, ForgetRule, MemoryId, MemoryNote, MemoryType, Namespace, RetentionPolicy,
    };

    fn ns() -> Namespace {
        Namespace::Project("retention".into())
    }

    fn policy() -> RetentionPolicy {
        RetentionPolicy {
            enabled: true,
            max_age_days: Some(30),
            ..RetentionPolicy::default()
        }
    }

    fn insert_aged(
        store: &SqliteStore,
        days: i64,
        importance: u8,
        f: impl FnOnce(&mut MemoryNote),
    ) -> MemoryId {
        let mut m = MemoryNote::new(
            ns(),
            format!("retention note aged {days}d importance {importance}"),
            MemoryType::Insight,
            importance,
        );
        m.created_at = chrono::Utc::now() - chrono::Duration::days(days);
        f(&mut m);
        let id = m.id.clone();
        store.insert_memory(&m, None).unwrap();
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

    fn candidate_ids(
        store: &SqliteStore,
        policy: &RetentionPolicy,
        mode: ForgetMode,
    ) -> Vec<MemoryId> {
        store
            .retention_candidates(&ns(), policy, mode, chrono::Utc::now())
            .unwrap()
            .candidates
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    fn is_archived(store: &SqliteStore, id: &MemoryId) -> bool {
        store
            .get_memory(id)
            .unwrap()
            .expect("memory exists")
            .archived_at
            .is_some()
    }

    // Baseline eligibility: old + below-floor is swept; young or at/above the
    // floor SURVIVES the same sweep.
    #[test]
    fn sweep_archives_only_old_low_importance_memories() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old_low = insert_aged(&store, 60, 3, |_| {});
        let young_low = insert_aged(&store, 5, 3, |_| {});
        let old_high = insert_aged(&store, 60, 6, |_| {}); // == floor: protected

        let (outcome, _) = store
            .retention_sweep(&ns(), &policy(), ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(outcome.archived, 1);
        assert_eq!(outcome.purged, 0, "apply never purges");
        assert!(is_archived(&store, &old_low));
        assert!(!is_archived(&store, &young_low), "young memory survives");
        assert!(
            !is_archived(&store, &old_high),
            "importance at the floor survives regardless of age"
        );
    }

    // W1.9 author-intent clamp productized: eligibility gates on the AUTHOR
    // prior as well as the effective importance, so a signal-degraded
    // effective value can never defeat author intent.
    #[test]
    fn author_high_importance_survives_even_when_effective_drops_below_floor() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        // Author said 10; recalibration (clamped to prior-2 by W1.9) dragged
        // the effective importance to 8.
        let authored_ten = insert_aged(&store, 400, 10, |_| {});
        store.set_recalibrated_importance(&authored_ten, 8).unwrap();
        // Control: authored low, effective low — eligible.
        let authored_low = insert_aged(&store, 400, 3, |_| {});

        let mut p = policy();
        p.importance_floor = 9; // would take effective-8 if only effective gated
        let (outcome, _) = store
            .retention_sweep(&ns(), &p, ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert!(
            !is_archived(&store, &authored_ten),
            "author-set importance 10 must survive even with effective 8 < floor 9"
        );
        assert!(is_archived(&store, &authored_low), "control was swept");
        assert_eq!(outcome.archived, 1);

        // Acceptance criterion: an importance-10 memory is NEVER eligible,
        // regardless of age, for every legal floor.
        for floor in 1..=10u8 {
            let mut p = policy();
            p.importance_floor = floor;
            assert!(
                !candidate_ids(&store, &p, ForgetMode::Apply).contains(&authored_ten),
                "importance-10 memory must never be eligible (floor {floor})"
            );
            assert!(
                !candidate_ids(&store, &p, ForgetMode::Hard).contains(&authored_ten),
                "importance-10 memory must never be hard-eligible (floor {floor})"
            );
        }
    }

    // Protected tags are absolute: same age/importance twin without the tag
    // is swept, the tagged one SURVIVES.
    #[test]
    fn protected_tag_survives_sweep() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let protected = insert_aged(&store, 90, 2, |m| {
            m.tags = vec!["architecture_decision".to_string()];
        });
        let twin = insert_aged(&store, 90, 2, |m| {
            m.tags = vec!["scratch".to_string()];
        });

        let mut p = policy();
        p.protected_tags = vec!["architecture_decision".to_string()];
        let (outcome, _) = store
            .retention_sweep(&ns(), &p, ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(outcome.archived, 1);
        assert!(
            !is_archived(&store, &protected),
            "protected tag must survive the sweep"
        );
        assert!(is_archived(&store, &twin), "unprotected twin was swept");
    }

    // Contested exclusion: a contested memory must never be swept. The fixture
    // mirrors `contested_filter_agrees_with_active_contradicts` (far-archived
    // and cross-namespace edges do NOT protect) so the candidate query stays
    // in lockstep with `active_contradicts` — the drift-test pattern.
    #[test]
    fn contested_memory_survives_sweep_and_agrees_with_active_contradicts() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let a = insert_aged(&store, 60, 2, |_| {});
        let b = insert_aged(&store, 61, 2, |_| {});
        let c = insert_aged(&store, 62, 2, |_| {});
        let gone = insert_aged(&store, 63, 2, |_| {});
        let mut foreign =
            MemoryNote::new(Namespace::Global, "foreign".into(), MemoryType::Insight, 2);
        foreign.created_at = chrono::Utc::now() - chrono::Duration::days(64);
        let foreign_id = foreign.id.clone();
        store.insert_memory(&foreign, None).unwrap();

        contradict(&store, &a, &b); // both active, in-ns -> contested, protected
        contradict(&store, &c, &gone); // far endpoint archived -> NOT contested
        contradict(&store, &c, &foreign_id); // far endpoint out-of-ns -> NOT contested
        store.archive_memory(&gone).unwrap();

        let all = vec![a.clone(), b.clone(), c.clone()];
        let expected_contested = store.active_contradicts(&ns(), &all).unwrap();
        assert_eq!(
            expected_contested,
            [a.clone(), b.clone()].into_iter().collect(),
            "precondition: active_contradicts flags exactly a and b"
        );

        // Drift guard: candidates = eligible-by-age/floor MINUS the contested
        // set, exactly as active_contradicts defines it.
        let got = candidate_ids(&store, &policy(), ForgetMode::Apply);
        assert_eq!(
            got,
            vec![c.clone()],
            "only the uncontested memory is eligible"
        );

        let (outcome, _) = store
            .retention_sweep(&ns(), &policy(), ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(outcome.archived, 1);
        assert!(!is_archived(&store, &a), "contested memory a survives");
        assert!(!is_archived(&store, &b), "contested memory b survives");
        assert!(is_archived(&store, &c), "uncontested c was swept");
    }

    // Dry-run fidelity: the plan IS the set apply touches — same query, no
    // writes on the plan path.
    #[test]
    fn dry_run_plan_matches_apply_sweep_exactly() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let e1 = insert_aged(&store, 40, 2, |_| {});
        let e2 = insert_aged(&store, 50, 4, |_| {});
        insert_aged(&store, 5, 2, |_| {}); // young: not a candidate
        insert_aged(&store, 50, 7, |_| {}); // above floor: not a candidate

        let plan = store
            .retention_candidates(&ns(), &policy(), ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        let planned: Vec<MemoryId> = plan.candidates.iter().map(|c| c.id.clone()).collect();
        assert_eq!(plan.total_eligible, 2);
        assert_eq!(planned.len(), 2);
        assert!(planned.contains(&e1) && planned.contains(&e2));
        // Planning wrote nothing.
        assert!(!is_archived(&store, &e1) && !is_archived(&store, &e2));

        let (outcome, effect_ids) = store
            .retention_sweep(&ns(), &policy(), ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(outcome.archived, 2);
        assert_eq!(
            effect_ids
                .archived_ids
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>(),
            planned
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
            "apply touched exactly the planned set"
        );
    }

    // Off-by-default: a disabled policy refuses to mutate (the daemon refuses
    // earlier too — this is the last line of defense).
    #[test]
    fn sweep_refuses_disabled_policy_and_mutates_nothing() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old_low = insert_aged(&store, 60, 2, |_| {});
        let mut p = policy();
        p.enabled = false;
        let err = store
            .retention_sweep(&ns(), &p, ForgetMode::Apply, chrono::Utc::now())
            .unwrap_err();
        assert!(
            err.to_string().contains("enabled"),
            "error must say retention is disabled: {err}"
        );
        assert!(!is_archived(&store, &old_low), "nothing was mutated");

        // Dry-run previews are allowed for a disabled policy (read-only).
        assert_eq!(candidate_ids(&store, &p, ForgetMode::Apply), vec![old_low]);
    }

    // Hard mode needs an explicit forget horizon; archive_after_days alone
    // must never authorize a purge.
    #[test]
    fn hard_mode_requires_max_age_days() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        insert_aged(&store, 60, 2, |_| {});
        let p = RetentionPolicy {
            enabled: true,
            archive_after_days: Some(30),
            max_age_days: None,
            ..RetentionPolicy::default()
        };
        let err = store
            .retention_candidates(&ns(), &p, ForgetMode::Hard, chrono::Utc::now())
            .unwrap_err();
        assert!(err.to_string().contains("max_age_days"), "{err}");
        let err = store
            .retention_sweep(&ns(), &p, ForgetMode::Hard, chrono::Utc::now())
            .unwrap_err();
        assert!(err.to_string().contains("max_age_days"), "{err}");
    }

    // The two horizons report distinct reasons, and only the forget horizon
    // feeds hard mode.
    #[test]
    fn archive_and_forget_horizons_report_rules_and_gate_hard_mode() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let archive_aged = insert_aged(&store, 40, 2, |_| {});
        let forget_aged = insert_aged(&store, 100, 2, |_| {});
        let p = RetentionPolicy {
            enabled: true,
            archive_after_days: Some(30),
            max_age_days: Some(90),
            ..RetentionPolicy::default()
        };

        let plan = store
            .retention_candidates(&ns(), &p, ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(plan.candidates.len(), 2);
        let rule_of = |id: &MemoryId| {
            plan.candidates
                .iter()
                .find(|c| &c.id == id)
                .expect("candidate present")
                .rule
        };
        assert_eq!(rule_of(&archive_aged), ForgetRule::ArchiveAge);
        assert_eq!(rule_of(&forget_aged), ForgetRule::MaxAge);

        // Hard mode: only past the forget horizon.
        let hard = candidate_ids(&store, &p, ForgetMode::Hard);
        assert_eq!(hard, vec![forget_aged]);
    }

    // Hard sweep purges the row and every sidecar: vectors, links, feedback,
    // FTS, and the memory's own oplog history — leaving exactly one `purge`
    // marker so history can explain the disappearance (RET-4).
    #[test]
    fn hard_sweep_purges_row_vectors_links_feedback_fts_and_history() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let victim = insert_aged(&store, 100, 2, |m| {
            m.content = "purge-target zebra-quixotic-token".to_string();
        });
        // Give it every sidecar: a vector, a link, feedback, oplog rows.
        let neighbor = insert_aged(&store, 1, 8, |_| {});
        store
            .add_link(&rb_types::MemoryLink {
                source_id: victim.clone(),
                target_id: neighbor.clone(),
                link_type: rb_types::LinkType::References,
                strength: 0.5,
                reason: "sidecar".to_string(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        store
            .record_feedback(&victim, rb_types::FeedbackKind::Helpful, None)
            .unwrap();

        // Re-insert with an embedding is awkward; instead assert vectors via a
        // second victim stored WITH an embedding.
        let embedded = insert_aged_with_embedding(&store, 100, 2);

        let count = |sql: &str, id: &MemoryId| -> i64 {
            store
                .conn
                .query_row(sql, rusqlite::params![id.to_string()], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            count(
                "SELECT COUNT(*) FROM memory_vectors WHERE memory_id = ?1",
                &embedded
            ),
            1,
            "precondition: embedded victim has a vector row"
        );

        let (outcome, effects) = store
            .retention_sweep(&ns(), &policy(), ForgetMode::Hard, chrono::Utc::now())
            .unwrap();
        assert_eq!(outcome.purged, 2);
        assert_eq!(outcome.archived, 0);
        assert!(effects.purged_ids.contains(&victim));

        // Row gone (FTS row cascades via the mem_ad trigger).
        assert!(store.get_memory(&victim).unwrap().is_none());
        assert!(store
            .keyword_search(&ns(), "zebra-quixotic-token", 10)
            .unwrap()
            .is_empty());
        // Sidecars gone.
        for (sql, label) in [
            (
                "SELECT COUNT(*) FROM memory_links WHERE source_id = ?1 OR target_id = ?1",
                "links",
            ),
            (
                "SELECT COUNT(*) FROM memory_vectors WHERE memory_id = ?1",
                "vectors",
            ),
            (
                "SELECT COUNT(*) FROM memory_feedback WHERE memory_id = ?1",
                "feedback",
            ),
        ] {
            assert_eq!(count(sql, &victim), 0, "{label} must be purged");
        }
        assert_eq!(
            count(
                "SELECT COUNT(*) FROM memory_vectors WHERE memory_id = ?1",
                &embedded
            ),
            0,
            "vector row must be purged"
        );
        // History: the memory's own rows are gone; exactly one purge marker
        // remains, namespace-stamped.
        assert_eq!(
            count(
                "SELECT COUNT(*) FROM memory_oplog WHERE memory_id = ?1 AND op <> 'purge'",
                &victim
            ),
            0,
            "pre-purge history must be gone"
        );
        assert_eq!(
            count(
                "SELECT COUNT(*) FROM memory_oplog WHERE memory_id = ?1 AND op = 'purge'",
                &victim
            ),
            1,
            "exactly one purge marker remains"
        );
        let marker_ns: String = store
            .conn
            .query_row(
                "SELECT namespace FROM memory_oplog WHERE memory_id = ?1 AND op = 'purge'",
                rusqlite::params![victim.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(marker_ns, ns().as_db_string());
        // One bulk sweep row records the pass (drives stats last_forget_at).
        let bulk: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_oplog WHERE op = 'retention_sweep' AND namespace = ?1",
                rusqlite::params![ns().as_db_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bulk, 1);
        // The untouched neighbor survives with its history.
        assert!(store.get_memory(&neighbor).unwrap().is_some());
    }

    fn insert_aged_with_embedding(store: &SqliteStore, days: i64, importance: u8) -> MemoryId {
        let mut m = MemoryNote::new(
            ns(),
            format!("embedded retention note aged {days}d"),
            MemoryType::Insight,
            importance,
        );
        m.created_at = chrono::Utc::now() - chrono::Duration::days(days);
        let id = m.id.clone();
        store.insert_memory(&m, Some(&[0.5f32; 8])).unwrap();
        id
    }

    // Raw-bytes drill (the scrub precedent): after a hard sweep + checkpoint,
    // the purged content no longer exists anywhere in the db/-wal files.
    #[test]
    fn hard_sweep_removes_content_from_db_bytes_at_rest() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("retention.db");
        let token = "kraken-obsidian-canary-77";
        {
            let store = SqliteStore::open(&db, 8).unwrap();
            let mut m = MemoryNote::new(
                ns(),
                format!("secret pattern {token} lives here"),
                MemoryType::Insight,
                2,
            );
            m.created_at = chrono::Utc::now() - chrono::Duration::days(100);
            store.insert_memory(&m, Some(&[0.25f32; 8])).unwrap();

            let bytes_before = read_db_bytes(&db);
            assert!(
                contains_token(&bytes_before, token),
                "precondition: token present at rest"
            );

            let (outcome, _) = store
                .retention_sweep(&ns(), &policy(), ForgetMode::Hard, chrono::Utc::now())
                .unwrap();
            assert_eq!(outcome.purged, 1);

            let bytes_after = read_db_bytes(&db);
            assert!(
                !contains_token(&bytes_after, token),
                "purged content must not survive at rest"
            );
        }
    }

    fn read_db_bytes(db: &std::path::Path) -> Vec<u8> {
        let mut bytes = std::fs::read(db).unwrap();
        for suffix in ["-wal", "-shm"] {
            let side = std::path::PathBuf::from(format!("{}{suffix}", db.display()));
            if let Ok(more) = std::fs::read(side) {
                bytes.extend(more);
            }
        }
        bytes
    }

    fn contains_token(haystack: &[u8], token: &str) -> bool {
        haystack.windows(token.len()).any(|w| w == token.as_bytes())
    }

    // Already-archived rows: hard mode may purge them (the archive-then-purge
    // lifecycle), apply mode has nothing left to do with them.
    #[test]
    fn hard_includes_already_archived_rows_but_apply_does_not() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let archived_old = insert_aged(&store, 100, 2, |_| {});
        store.archive_memory(&archived_old).unwrap();

        assert!(
            candidate_ids(&store, &policy(), ForgetMode::Apply).is_empty(),
            "apply has nothing to do with an archived row"
        );
        let hard_plan = store
            .retention_candidates(&ns(), &policy(), ForgetMode::Hard, chrono::Utc::now())
            .unwrap();
        assert_eq!(hard_plan.candidates.len(), 1);
        assert!(
            hard_plan.candidates[0].archived,
            "flagged as already archived"
        );

        let (outcome, _) = store
            .retention_sweep(&ns(), &policy(), ForgetMode::Hard, chrono::Utc::now())
            .unwrap();
        assert_eq!(outcome.purged, 1);
        assert!(store.get_memory(&archived_old).unwrap().is_none());
    }

    // Bounded per pass and re-runnable (PRD RET-2).
    #[test]
    fn sweep_is_bounded_by_batch_limit_and_rerunnable() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        for i in 0..5 {
            insert_aged(&store, 60 + i, 2, |_| {});
        }
        let mut p = policy();
        p.batch_limit = 2;

        let (first, _) = store
            .retention_sweep(&ns(), &p, ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(
            (first.archived, first.total_eligible, first.remaining),
            (2, 5, 3)
        );

        let (second, _) = store
            .retention_sweep(&ns(), &p, ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(
            (second.archived, second.total_eligible, second.remaining),
            (2, 3, 1)
        );

        let (third, _) = store
            .retention_sweep(&ns(), &p, ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(
            (third.archived, third.total_eligible, third.remaining),
            (1, 1, 0)
        );

        let plan = store
            .retention_candidates(&ns(), &p, ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(plan.total_eligible, 0, "nothing left after three passes");
    }

    // Namespace isolation: a sweep never crosses namespaces.
    #[test]
    fn sweep_is_namespace_scoped() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        insert_aged(&store, 60, 2, |_| {});
        let mut foreign = MemoryNote::new(
            Namespace::Global,
            "old foreign".into(),
            MemoryType::Insight,
            2,
        );
        foreign.created_at = chrono::Utc::now() - chrono::Duration::days(60);
        let foreign_id = foreign.id.clone();
        store.insert_memory(&foreign, None).unwrap();

        let (outcome, _) = store
            .retention_sweep(&ns(), &policy(), ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        assert_eq!(outcome.archived, 1);
        assert!(
            store
                .get_memory(&foreign_id)
                .unwrap()
                .unwrap()
                .archived_at
                .is_none(),
            "other-namespace memory untouched"
        );
    }

    // Archive-mode sweeps stamp a retention cause in the oplog details so
    // history explains WHY the memory disappeared (RET-4), and the sweep
    // appends one bulk row.
    #[test]
    fn apply_sweep_records_cause_and_bulk_row_in_oplog() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old_low = insert_aged(&store, 60, 2, |_| {});
        store
            .retention_sweep(&ns(), &policy(), ForgetMode::Apply, chrono::Utc::now())
            .unwrap();
        let details: String = store
            .conn
            .query_row(
                "SELECT details FROM memory_oplog WHERE memory_id = ?1 AND op = 'archive'",
                rusqlite::params![old_low.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            details.contains("retention"),
            "archive row must carry the retention cause: {details}"
        );
        let bulk_details: String = store
            .conn
            .query_row(
                "SELECT details FROM memory_oplog WHERE op = 'retention_sweep' AND namespace = ?1",
                rusqlite::params![ns().as_db_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert!(bulk_details.contains("\"archived\":1"), "{bulk_details}");
    }

    // Replay maps a purge to Archived (a disappearance, not an update), and
    // the bulk sweep row is skipped by the empty-id sentinel.
    #[test]
    fn purge_replays_as_archived_and_bulk_row_is_skipped() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let victim = insert_aged(&store, 100, 2, |_| {});
        store
            .retention_sweep(&ns(), &policy(), ForgetMode::Hard, chrono::Utc::now())
            .unwrap();
        let page = store.oplog_changes_since(&ns(), 0, 100).unwrap();
        let purge_events: Vec<_> = page.changes.iter().filter(|c| c.id == victim).collect();
        // The victim's insert row was purged with its history; the only
        // surviving event for it is the purge marker, replayed as Archived.
        assert_eq!(purge_events.len(), 1);
        assert_eq!(purge_events[0].kind, rb_types::ChangeKind::Archived);
    }

    // The scheduled job sweeps per namespace: the enumeration lists every
    // namespace that still has rows.
    #[test]
    fn retention_namespaces_lists_distinct_namespaces() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        insert_aged(&store, 1, 5, |_| {});
        insert_aged(&store, 2, 5, |_| {});
        let foreign = MemoryNote::new(Namespace::Global, "g".into(), MemoryType::Insight, 5);
        store.insert_memory(&foreign, None).unwrap();

        let mut got = store.retention_namespaces().unwrap();
        got.sort_by_key(|n| n.as_db_string());
        assert_eq!(got, vec![Namespace::Global, ns()]);
    }

    // Purging a memory that superseded another must not leave the old row's
    // `superseded_by` FK dangling (the purge nulls it in the same tx).
    #[test]
    fn purge_clears_dangling_superseded_by_references() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let old = insert_aged(&store, 200, 2, |_| {});
        let successor = insert_aged(&store, 100, 2, |_| {});
        store.supersede(&old, &successor).unwrap();

        // Both are eligible for hard (old is archived-by-supersession).
        let (outcome, _) = store
            .retention_sweep(&ns(), &policy(), ForgetMode::Hard, chrono::Utc::now())
            .unwrap();
        assert_eq!(outcome.purged, 2, "both rows purged without an FK abort");
        assert!(store.get_memory(&old).unwrap().is_none());
        assert!(store.get_memory(&successor).unwrap().is_none());
    }

    // A stale plan entry (row vanished between plan and execute) is a no-op,
    // not an abort: the sweep is re-runnable and per-item atomic.
    #[test]
    fn purge_of_missing_id_is_a_noop_that_logs_nothing() {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let ghost = MemoryId::new();
        assert!(!store.purge_memory_for_retention(&ghost, "{}").unwrap());
        let rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM memory_oplog", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "a no-op purge logs nothing");
    }
}
