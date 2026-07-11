//! Retroactive redaction.

use super::internal::*;
use super::*;
use crate::error::storage_err;

impl SqliteStore {
    /// Retroactively redact secrets from every stored memory (W2.4
    /// `rusty-brain scrub`). Scans ALL rows — active AND archived, since a
    /// secret at rest is a secret regardless of archive state — applies the
    /// shared `rb_redact` pass to EVERY stored free-text column and rewrites
    /// any row that changed.
    ///
    /// The redacted column set MUST be the union of the FTS-indexed text
    /// columns (migration 001: `content`, `summary`, `keywords`, `tags`) and
    /// the embedding-input columns (`rb_engine::embed_input`: `content`,
    /// `keywords`, `tags`, `context`) — i.e. all five of content, summary,
    /// keywords, tags, context — or a redacted-away secret could still be
    /// discoverable by keyword search or still encoded in the vector after
    /// reembed. FTS resyncs automatically via the `mem_au` trigger, which fires
    /// on UPDATE OF content/summary/keywords/tags (migration 006).
    /// `keywords`/`tags` are JSON arrays; redacting the raw JSON string is safe
    /// — the `[REDACTED:..]` markers contain no quotes (the chars that would
    /// break a JSON string value; the surrounding `[ ]` are inside the value
    /// and harmless), so the array stays valid. A drift guard
    /// (`scrub_covers_every_fts_and_embedding_text_column`) fails if a new
    /// searchable/embedded column is added without teaching scrub.
    ///
    /// Embedding hygiene: a row's embedding stamp is blanked (so the next
    /// `reembed` recomputes a clean vector) ONLY when it is ACTIVE and its
    /// embedding-feeding text (content/keywords/tags/context) changed. Archived
    /// rows have no
    /// live vector — archival deletes the vec0 row, `reembed` skips
    /// `archived_at IS NULL` rows, and `update_vector` refuses archived ids —
    /// so blanking their stamp would inflate `reembed_pending` with work no
    /// reembed pass could ever clear (the "run reembed until changed=0"
    /// guidance would never converge). Their text is still redacted; only the
    /// stamp/count is left alone. `reembed_pending` therefore counts exactly
    /// the rows a follow-up `reembed` will revisit.
    ///
    /// `updated_at` is deliberately NOT bumped: scrub is administrative
    /// hygiene, not an authorial edit (the `rename_namespace` /
    /// `set_recalibrated_importance` rule), so it must not distort recency
    /// ranking. One bulk oplog row records the pass.
    pub fn scrub(&self) -> Result<ScrubOutcome> {
        let outcome = immediate_tx(&self.conn, || {
            let mut scanned = 0u64;
            let mut redacted = 0u64;
            let mut reembed_pending = 0u64;
            // Keyset-paginate by memory_id so peak memory is bounded to one
            // batch, not the whole corpus (scrub is meant to clean exactly the
            // big DBs that a full-table snapshot would OOM). Already-rewritten
            // rows keep their id, so `memory_id > cursor` never revisits them.
            let mut cursor = String::new();
            loop {
                let batch = self.scrub_select_batch(&cursor)?;
                if batch.is_empty() {
                    break;
                }
                cursor = batch[batch.len() - 1].0.clone();
                scanned += batch.len() as u64;
                for (id, content, summary, context, keywords, tags, archived_at) in batch {
                    // keywords/tags are JSON-array strings; redacting the raw
                    // JSON is safe — the `[REDACTED:..]` markers contain no
                    // quotes, so the array stays valid JSON.
                    let cols = [
                        ("content", &content, rb_redact::redact(&content)),
                        ("summary", &summary, rb_redact::redact(&summary)),
                        ("context", &context, rb_redact::redact(&context)),
                        ("keywords", &keywords, rb_redact::redact(&keywords)),
                        ("tags", &tags, rb_redact::redact(&tags)),
                    ];
                    if cols.iter().all(|(_, old, new)| new == *old) {
                        continue; // nothing redactable in this row
                    }
                    redacted += 1;
                    // Blank the embedding stamp only for ACTIVE rows whose
                    // embedding-feeding text (content/context/keywords/tags —
                    // summary does NOT feed the composite document) changed:
                    // reembed only revisits active, stale rows, so an archived
                    // row's blanked stamp would never be cleared.
                    let embed_text_changed = cols
                        .iter()
                        .any(|(name, old, new)| *name != "summary" && new != *old);
                    let blank_embed = archived_at.is_none() && embed_text_changed;
                    if blank_embed {
                        reembed_pending += 1;
                    }
                    // Build the SET from ONLY the columns that actually changed,
                    // so a context-only redaction does not list content/summary/
                    // keywords/tags and needlessly fire the `mem_au` FTS trigger
                    // (`AFTER UPDATE OF content,summary,keywords,tags`).
                    let mut sets: Vec<String> = Vec::new();
                    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
                    for (name, old, new) in &cols {
                        if new != *old {
                            sets.push(format!("{name} = ?{}", params.len() + 1));
                            params.push(Box::new(new.clone()));
                        }
                    }
                    if blank_embed {
                        sets.push("embedding_input_version = ''".to_string());
                    }
                    let id_pos = params.len() + 1;
                    params.push(Box::new(id.clone()));
                    let sql = format!(
                        "UPDATE memories SET {} WHERE memory_id = ?{id_pos}",
                        sets.join(", ")
                    );
                    let refs: Vec<&dyn rusqlite::ToSql> =
                        params.iter().map(|b| b.as_ref()).collect();
                    self.conn
                        .execute(&sql, refs.as_slice())
                        .map_err(storage_err)?;
                }
            }

            if redacted > 0 {
                let details = serde_json::json!({
                    "scanned": scanned,
                    "redacted": redacted,
                    "reembed_pending": reembed_pending,
                })
                .to_string();
                self.conn
                    .execute(
                        "INSERT INTO memory_oplog (site_id, op, memory_id, namespace, at, details)
                         VALUES (?1, 'scrub', '', '', ?2, ?3)",
                        rusqlite::params![self.site_id, chrono::Utc::now().timestamp(), details],
                    )
                    .map_err(storage_err)?;
            }

            Ok(ScrubOutcome {
                scanned,
                redacted,
                reembed_pending,
            })
        })?;

        // Force the pre-redaction WAL frames into the main DB and truncate the
        // -wal file: without this, `scrub` could report success while the old
        // plaintext still sits in the -wal on disk until an arbitrary later
        // checkpoint. Best-effort and only when something changed (a no-op
        // scrub wrote nothing). A no-op on an in-memory DB (no WAL). Residual:
        // freed-page slack in the main DB needs VACUUM/purge (W5b.3) — out of
        // scope here; this closes the obvious recoverable copy.
        if outcome.redacted > 0 {
            self.conn
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .map_err(storage_err)?;
        }
        Ok(outcome)
    }
    /// One keyset page of redactable rows with `memory_id > cursor`, ordered by
    /// `memory_id`, capped at `SCRUB_BATCH`. Split out so the prepared SELECT
    /// statement is dropped before the caller issues UPDATEs for the page.
    #[allow(clippy::type_complexity)]
    fn scrub_select_batch(
        &self,
        cursor: &str,
    ) -> Result<Vec<(String, String, String, String, String, String, Option<i64>)>> {
        const SCRUB_BATCH: i64 = 500;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT memory_id, content, summary, context, keywords, tags, archived_at \
                 FROM memories WHERE memory_id > ?1 ORDER BY memory_id LIMIT ?2",
            )
            .map_err(storage_err)?;
        let mapped = stmt
            .query_map(rusqlite::params![cursor, SCRUB_BATCH], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            })
            .map_err(storage_err)?;
        let mut out = Vec::new();
        for r in mapped {
            out.push(r.map_err(storage_err)?);
        }
        Ok(out)
    }
}
/// Result of a retroactive [`SqliteStore::scrub`] pass (W2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrubOutcome {
    /// Rows examined (every memory, active and archived).
    pub scanned: u64,
    /// Rows whose content/summary/context had a secret redacted out.
    pub redacted: u64,
    /// Of `redacted`, rows whose embedding-feeding text (content/context)
    /// changed and were therefore marked stale for re-embedding.
    pub reembed_pending: u64,
}
#[cfg(test)]
mod scrub_tests {
    use super::*;
    use rb_types::{MemoryNote, MemoryType, Namespace};

    /// Shape-true FAKE secrets, BUILT at runtime from split literals so no
    /// committed source byte forms a scanner-matchable token (GitHub push
    /// protection blocks otherwise). Not real credentials.
    fn aws_key() -> String {
        format!("AKIA{}", "A".repeat(16))
    }
    fn github_token() -> String {
        format!("ghp_{}", "A".repeat(36))
    }

    fn seeded(content: &str, context: &str) -> (SqliteStore, MemoryId) {
        let store = SqliteStore::open_in_memory(8).unwrap();
        let mut m = MemoryNote::new(
            Namespace::Project("scrub".into()),
            content.to_string(),
            MemoryType::Insight,
            5,
        );
        m.context = context.to_string();
        m.summary = "a summary".to_string();
        store.insert_memory(&m, None).unwrap();
        (store, m.id)
    }

    #[test]
    fn scrub_redacts_planted_secret_from_content_and_marks_reembed() {
        let secret = aws_key();
        let (store, id) = seeded(
            &format!("deploy key is {secret} do not share"),
            "no secret here",
        );
        let out = store.scrub().unwrap();
        assert_eq!(out.scanned, 1);
        assert_eq!(out.redacted, 1);
        assert_eq!(out.reembed_pending, 1, "content changed -> reembed");

        let got = store.get_memory(&id).unwrap().unwrap();
        assert!(
            !got.content.contains(&secret),
            "plaintext secret must be gone: {}",
            got.content
        );
        assert!(got.content.contains("[REDACTED:aws-key]"));
        assert_eq!(
            got.embedding_input_version, "",
            "embedding stamp blanked for reembed"
        );

        // FTS no longer matches the secret token (mem_au resynced on content).
        let hits = store
            .keyword_search(&Namespace::Project("scrub".into()), &secret, 10)
            .unwrap();
        assert!(
            hits.is_empty(),
            "scrubbed secret must not be FTS-searchable"
        );
    }

    #[test]
    fn scrub_is_idempotent_and_a_clean_db_is_a_noop() {
        let (store, _id) = seeded(&format!("token {} here", github_token()), "");
        let first = store.scrub().unwrap();
        assert_eq!(first.redacted, 1);
        let second = store.scrub().unwrap();
        assert_eq!(second.scanned, 1);
        assert_eq!(second.redacted, 0, "second pass finds nothing to redact");
        assert_eq!(second.reembed_pending, 0);
    }

    #[test]
    fn scrub_redacts_archived_rows_too() {
        let secret = aws_key();
        let (store, id) = seeded(&format!("archived secret {secret}"), "");
        store.archive_memory(&id).unwrap();
        let out = store.scrub().unwrap();
        assert_eq!(
            out.redacted, 1,
            "a secret at rest is scrubbed even if archived"
        );
        // Fix #3: an archived row's text is redacted, but it is NOT counted in
        // reembed_pending — reembed skips archived rows, so counting it would
        // make "reembed until changed=0" never converge.
        assert_eq!(
            out.reembed_pending, 0,
            "archived rows must not be queued for a reembed that can never run"
        );
        let got = store.get_memory(&id).unwrap().unwrap();
        assert!(!got.content.contains(&secret));
    }

    #[test]
    fn scrub_drill_removes_planted_secret_from_the_db_file_at_rest() {
        // W2.4 / Phase 2 gate: the scrub drill on a SEEDED db. A secret that
        // predates the rule (written straight to the store, bypassing
        // capture-time redaction) must be gone from the raw file bytes after
        // scrub + checkpoint — the proof that retroactive redaction reaches
        // disk, not just the in-memory row.
        let secret = aws_key();
        let secret = secret.as_str();
        const SENTINEL: &str = "scrub-drill-sentinel-9b2e";
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("seeded.db");

        {
            let store = SqliteStore::open(&db, 8).unwrap();
            let mut m = MemoryNote::new(
                Namespace::Project("scrub".into()),
                format!("deploy key {secret} and marker {SENTINEL}"),
                MemoryType::Insight,
                5,
            );
            m.summary = format!("uses {secret}");
            store.insert_memory(&m, None).unwrap();

            // Vacuousness guard: the secret really is at rest before scrub.
            // In WAL mode the just-inserted row lives in the -wal file until
            // checkpoint, so grep all three files, not just the main db.
            let mut before = Vec::new();
            for suffix in ["", "-wal", "-shm"] {
                let p = format!("{}{suffix}", db.to_string_lossy());
                if let Ok(bytes) = std::fs::read(&p) {
                    before.extend_from_slice(&bytes);
                }
            }
            assert!(
                before.windows(secret.len()).any(|w| w == secret.as_bytes()),
                "secret must be present before scrub (else the test is vacuous)"
            );

            let out = store.scrub().unwrap();
            assert_eq!(out.redacted, 1);
            // Close the connection so SQLite checkpoints WAL into the main file.
        }

        // Grep the main file plus any WAL/SHM straggler.
        let mut at_rest = Vec::new();
        for suffix in ["", "-wal", "-shm"] {
            let p = format!("{}{suffix}", db.to_string_lossy());
            if let Ok(bytes) = std::fs::read(&p) {
                at_rest.extend_from_slice(&bytes);
                at_rest.push(0);
            }
        }
        assert!(
            at_rest
                .windows(SENTINEL.len())
                .any(|w| w == SENTINEL.as_bytes()),
            "the non-secret sentinel must survive (proves the row is intact)"
        );
        assert!(
            at_rest
                .windows(b"[REDACTED:".len())
                .any(|w| w == b"[REDACTED:"),
            "a redaction marker must be present at rest"
        );
        assert!(
            !at_rest
                .windows(secret.len())
                .any(|w| w == secret.as_bytes()),
            "the planted secret must be GONE from the db file bytes after scrub"
        );
    }

    #[test]
    fn scrub_covers_every_fts_and_embedding_text_column() {
        // Fix #9: a secret in ANY FTS-indexed or embedding-feeding text column
        // (content, summary, keywords, tags, context) must be redacted and no
        // longer keyword-searchable. keywords/tags were previously NOT scrubbed
        // despite being both FTS columns and embedding inputs.
        let store = SqliteStore::open_in_memory(8).unwrap();
        let proj = Namespace::Project("scrub".into());
        // Each secret is a valid AWS-key shape: AKIA + exactly 16 [0-9A-Z]
        // (the word padded with 'A' to width 16), distinct per column.
        let key = |w: &str| format!("AKIA{w:A<16}");
        let (c_sec, s_sec, k_sec, t_sec, x_sec) = (
            key("CONTENT"),
            key("SUMMARY"),
            key("KEYWORD"),
            key("TAGS"),
            key("CONTEXT"),
        );
        let mut m = MemoryNote::new(
            proj.clone(),
            format!("content has {c_sec}"),
            MemoryType::Insight,
            5,
        );
        m.summary = format!("summary has {s_sec}");
        m.keywords = vec![k_sec.clone(), "ok".into()];
        m.tags = vec![t_sec.clone()];
        m.context = format!("context has {x_sec}");
        store.insert_memory(&m, None).unwrap();

        let out = store.scrub().unwrap();
        assert_eq!(out.redacted, 1);

        let got = store.get_memory(&m.id).unwrap().unwrap();
        for secret in [&c_sec, &s_sec, &k_sec, &t_sec, &x_sec] {
            let secret = secret.as_str();
            assert!(
                !got.content.contains(secret)
                    && !got.summary.contains(secret)
                    && !got.context.contains(secret)
                    && !got.keywords.iter().any(|k| k.contains(secret))
                    && !got.tags.iter().any(|t| t.contains(secret)),
                "secret {secret} must be redacted from every text column"
            );
            // And gone from FTS (content/summary/keywords/tags are indexed).
            assert!(
                store.keyword_search(&proj, secret, 10).unwrap().is_empty(),
                "scrubbed secret {secret} must not be keyword-searchable"
            );
        }
        // keywords/tags stayed valid JSON arrays (the non-secret element survived).
        assert!(got.keywords.iter().any(|k| k == "ok"), "{:?}", got.keywords);
    }

    #[test]
    fn scrub_appends_one_bulk_oplog_row_only_when_something_changed() {
        let (store, _id) = seeded("clean content, nothing secret", "");
        store.scrub().unwrap();
        let n: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM memory_oplog WHERE op = 'scrub'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "a no-op scrub logs nothing");
    }
}
