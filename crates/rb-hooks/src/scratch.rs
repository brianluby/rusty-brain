//! Per-session capture scratch file (W3.1 capture inversion).
//!
//! PostToolUse writes ZERO memories; it appends a compact observation (a file
//! touched, a command run, a failure seen) to a small per-session JSON file
//! keyed by `session_id`. SessionEnd folds that scratch (plus the transcript)
//! into ONE summary memory and resets it. Like the dedup cache, the file lives
//! under the shared XDG cache dir, namespaced per project AND per session, with
//! atomic writes and strict fail-open semantics: every error (unreadable /
//! corrupt / unwritable) degrades to an empty/absent scratch so capture never
//! blocks the CLI and never loses the agent's turn.
//!
//! SECURITY: callers MUST redact before appending — the scratch persists raw
//! observation text to disk (unlike the dedup cache, which stores only hashes),
//! so a secret reaching it would be a leak. The capture flow applies the shared
//! `rb-redact` pass to every value before it gets here (the same discipline the
//! store path uses).

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use rb_types::Namespace;

use crate::cache;

/// Max distinct entries kept per category (bounds the on-disk file and the
/// folded summary). First-seen order is preserved; once full, further DISTINCT
/// entries are dropped — a session's earliest signal is the most
/// context-setting, and repeats always coalesce.
const MAX_FILES: usize = 100;
const MAX_COMMANDS: usize = 100;
const MAX_FAILURES: usize = 50;
/// Max characters retained per entry, so one pathological command/line cannot
/// bloat the file. Head-kept with no marker — the scratch is internal, never
/// shown to the model; the folded summary is what carries forward.
const MAX_ENTRY_CHARS: usize = 500;
/// Stale-scratch TTL: a scratch file untouched for this long belongs to an
/// abandoned/crashed session whose SessionEnd never fired; pruned opportunistically.
const STALE_TTL_SECONDS: u64 = 24 * 60 * 60;

/// The fold-once-per-session capture buffer, persisted as JSON.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScratchData {
    /// Files created/edited this session (deduped, first-seen order).
    #[serde(default)]
    pub files: Vec<String>,
    /// Distinct shell commands run this session (deduped, first-seen order).
    #[serde(default)]
    pub commands: Vec<String>,
    /// Distinct failures/errors observed this session (deduped).
    #[serde(default)]
    pub failures: Vec<String>,
    /// The id of the summary memory last written for THIS session, if any.
    /// SessionEnd supersedes it with the next summary (W3.1 update-as-supersede)
    /// so a resumed-then-re-ended session keeps ONE live summary, not two.
    #[serde(default)]
    pub prior_summary_id: Option<String>,
}

impl ScratchData {
    /// True when nothing worth folding has been recorded (no files, commands,
    /// or failures). A scratch carrying only a `prior_summary_id` is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.commands.is_empty() && self.failures.is_empty()
    }
}

/// Which category an observation appends to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Command,
    Failure,
}

/// A file-backed, per-(namespace, session) capture scratch.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Resolve the scratch file for `(namespace, session_id)` under the shared
    /// hook cache dir. Same slug + FNV-1a disambiguation as the dedup cache,
    /// with the session id mixed in so two sessions in one project never share
    /// a scratch (and the same session in two projects stays separate).
    pub fn for_session(namespace: &Namespace, session_id: &str) -> Self {
        let raw = namespace.as_db_string();
        let file = format!(
            "scratch-{}-{:08x}-{:08x}.json",
            cache::namespace_slug(namespace),
            cache::fnv1a(raw.as_bytes()),
            cache::fnv1a(session_id.as_bytes()),
        );
        Self {
            path: cache::cache_dir().join(file),
        }
    }

    /// Construct a scratch at an explicit path (tests).
    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Append one observation of `kind`. Repeats coalesce; the per-category cap
    /// bounds growth; an over-long entry is head-truncated. Best-effort: any
    /// error is swallowed (fail-open). The caller is responsible for redaction.
    pub fn append(&self, kind: Kind, value: &str) {
        self.append_many([(kind, value.to_string())]);
    }

    /// Append a batch of observations in ONE read-modify-write round. A
    /// multi-file `apply_patch` event would otherwise pay a full
    /// read-redact-serialize-write-rename per path, multiplying disk I/O and
    /// widening the lost-update window across overlapping hook processes (the
    /// same accepted last-writer-wins posture as [`Scratch::append`], just a
    /// single window per event instead of one per path). Per-entry semantics
    /// match repeated `append` calls exactly: trim, blank-skip, head-truncate,
    /// coalesce, per-category cap, first-seen order, fail-open. A batch with no
    /// effective entries performs no write.
    pub fn append_many(&self, entries: impl IntoIterator<Item = (Kind, String)>) {
        let mut data = self.read();
        let mut changed = false;
        for (kind, value) in entries {
            let value = truncate(value.trim());
            if value.is_empty() {
                continue;
            }
            let (list, cap) = match kind {
                Kind::File => (&mut data.files, MAX_FILES),
                Kind::Command => (&mut data.commands, MAX_COMMANDS),
                Kind::Failure => (&mut data.failures, MAX_FAILURES),
            };
            if list.len() < cap && !list.iter().any(|e| e == &value) {
                list.push(value);
                changed = true;
            }
        }
        if changed {
            self.write(&data);
        }
    }

    /// Read the scratch (empty on any error — fail-open).
    pub fn read(&self) -> ScratchData {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => ScratchData::default(),
        }
    }

    /// Reset the scratch after SessionEnd folds it: drop the buffered
    /// observations but RETAIN `summary_id` as the new `prior_summary_id`, so a
    /// resumed session supersedes this summary instead of duplicating it. A
    /// `None` summary id (the fold produced no memory) still clears the buffer.
    pub fn mark_folded(&self, summary_id: Option<&str>) {
        let data = ScratchData {
            prior_summary_id: summary_id.map(str::to_string),
            ..Default::default()
        };
        self.write(&data);
    }

    /// Record the latest checkpoint summary id while retaining observations.
    /// Used by non-Claude fallback boundaries where no true session terminus is
    /// available: later checkpoints supersede the live summary without losing
    /// early-session scratch entries.
    ///
    /// Like [`Scratch::append`], this is a best-effort, non-atomic
    /// read-modify-write (last writer wins): the per-event hook binary is
    /// single-process, so the only race is two overlapping hook processes for the
    /// same session. A checkpoint racing a concurrent `append` could drop the
    /// just-appended entry — an accepted file-level posture for coarse-grained,
    /// best-effort capture, not a correctness guarantee.
    pub fn mark_checkpointed(&self, summary_id: &str) {
        let mut data = self.read();
        data.prior_summary_id = Some(summary_id.to_string());
        self.write(&data);
    }

    /// Atomically write (temp file 0600 + rename), creating the parent 0700.
    /// Best-effort. The scratch holds best-effort-redacted plaintext, so it gets
    /// the same at-rest posture as the DB and socket: `docs/THREAT_MODEL.md`
    /// names file permissions (0600) as THE backstop for secrets that slip
    /// redaction, and the rename preserves the temp file's 0600 mode.
    fn write(&self, data: &ScratchData) {
        if let Some(parent) = self.path.parent() {
            if create_private_dir(parent).is_err() {
                return;
            }
        }
        let Ok(json) = serde_json::to_string(data) else {
            return;
        };
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        if write_private_file(&tmp, json.as_bytes()).is_err() {
            return;
        }
        if std::fs::rename(&tmp, &self.path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Create `dir` (and missing parents) private to the user (0700 on unix). An
/// already-existing dir is left as-is — the file's own 0600 is the binding
/// protection. Mirrors the 0700 the daemon enforces on its runtime dir.
fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Write `bytes` to `path` mode 0600 (unix), truncating. The 0600 is set at
/// create time and survives the subsequent rename onto the real scratch path,
/// mirroring rb-store's 0600 DB enforcement.
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

/// Head-truncate an entry to [`MAX_ENTRY_CHARS`], UTF-8 safe (boundary taken on
/// `char_indices`, never a raw byte offset).
fn truncate(value: &str) -> String {
    match value.char_indices().nth(MAX_ENTRY_CHARS) {
        Some((idx, _)) => value[..idx].to_string(),
        None => value.to_string(),
    }
}

/// Opportunistically delete scratch files untouched for longer than the stale
/// TTL — abandoned/crashed sessions whose SessionEnd never fired. Best-effort:
/// any error (no cache dir, unreadable entry) is ignored. Keyed on file mtime,
/// so even a corrupt scratch is reclaimed.
pub fn prune_stale() {
    let Ok(entries) = std::fs::read_dir(cache::cache_dir()) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("scratch-") || !name.ends_with(".json") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age.as_secs() > STALE_TTL_SECONDS);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn scratch() -> (tempfile::TempDir, Scratch) {
        let dir = tempfile::tempdir().unwrap();
        let s = Scratch::at(dir.path().join("scratch.json"));
        (dir, s)
    }

    #[test]
    fn fresh_scratch_reads_empty() {
        let (_d, s) = scratch();
        let data = s.read();
        assert!(data.is_empty());
        assert!(data.files.is_empty() && data.commands.is_empty() && data.failures.is_empty());
        assert_eq!(data.prior_summary_id, None);
    }

    #[test]
    fn append_records_each_category() {
        let (_d, s) = scratch();
        s.append(Kind::File, "src/main.rs");
        s.append(Kind::Command, "cargo test");
        s.append(Kind::Failure, "assertion failed: x == y");
        let data = s.read();
        assert_eq!(data.files, vec!["src/main.rs"]);
        assert_eq!(data.commands, vec!["cargo test"]);
        assert_eq!(data.failures, vec!["assertion failed: x == y"]);
        assert!(!data.is_empty());
    }

    #[test]
    fn append_coalesces_repeats_and_preserves_order() {
        let (_d, s) = scratch();
        for f in ["a.rs", "b.rs", "a.rs", "c.rs", "b.rs"] {
            s.append(Kind::File, f);
        }
        // Distinct, first-seen order — a file edited repeatedly appears once.
        assert_eq!(s.read().files, vec!["a.rs", "b.rs", "c.rs"]);
    }

    #[test]
    fn append_ignores_blank_values() {
        let (_d, s) = scratch();
        s.append(Kind::Command, "   ");
        s.append(Kind::File, "");
        assert!(s.read().is_empty());
    }

    #[test]
    fn append_many_records_batch_with_single_append_semantics() {
        // One multi-file apply_patch event batches all its observations into ONE
        // read-modify-write round; per-entry semantics (trim, blank-skip,
        // coalesce, first-seen order) match repeated `append` calls exactly.
        let (_d, s) = scratch();
        s.append(Kind::File, "pre.rs");
        s.append_many([
            (Kind::File, "a.rs".to_string()),
            (Kind::File, "b.rs".to_string()),
            (Kind::File, "a.rs".to_string()), // in-batch repeat coalesces
            (Kind::File, "pre.rs".to_string()), // on-disk repeat coalesces
            (Kind::File, "   ".to_string()),  // blank skipped
            (Kind::Command, "cargo test".to_string()),
        ]);
        let data = s.read();
        assert_eq!(data.files, vec!["pre.rs", "a.rs", "b.rs"]);
        assert_eq!(data.commands, vec!["cargo test"]);
    }

    #[test]
    fn append_many_respects_category_cap_across_the_batch() {
        // MAX_FILES semantics are preserved within a single batch: first-seen
        // entries win, overflow is dropped, exactly as repeated appends would.
        let (_d, s) = scratch();
        let batch: Vec<(Kind, String)> = (0..(MAX_FILES + 25))
            .map(|i| (Kind::File, format!("f{i}.rs")))
            .collect();
        s.append_many(batch);
        let data = s.read();
        assert_eq!(data.files.len(), MAX_FILES, "files capped within a batch");
        assert_eq!(data.files[0], "f0.rs");
        assert!(!data.files.contains(&format!("f{}.rs", MAX_FILES + 10)));
    }

    #[test]
    fn append_many_with_no_effective_entries_writes_nothing() {
        // An all-blank/empty batch performs no write: no scratch file appears.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scratch.json");
        let s = Scratch::at(path.clone());
        s.append_many([]);
        s.append_many([(Kind::File, "   ".to_string())]);
        assert!(!path.exists(), "no-op batches must not create the file");
    }

    #[test]
    fn append_respects_category_cap() {
        let (_d, s) = scratch();
        for i in 0..(MAX_FILES + 25) {
            s.append(Kind::File, &format!("f{i}.rs"));
        }
        let data = s.read();
        assert_eq!(data.files.len(), MAX_FILES, "files capped");
        // The cap keeps the FIRST-seen entries (earliest context).
        assert_eq!(data.files[0], "f0.rs");
        assert!(!data.files.contains(&format!("f{}.rs", MAX_FILES + 10)));
    }

    #[test]
    fn append_truncates_overlong_entry() {
        let (_d, s) = scratch();
        s.append(Kind::Command, &"x".repeat(MAX_ENTRY_CHARS + 500));
        assert_eq!(s.read().commands[0].chars().count(), MAX_ENTRY_CHARS);
    }

    #[test]
    fn mark_folded_clears_buffer_but_keeps_summary_id() {
        let (_d, s) = scratch();
        s.append(Kind::File, "src/lib.rs");
        s.append(Kind::Command, "cargo build");
        s.mark_folded(Some("mem-123"));
        let data = s.read();
        assert!(data.is_empty(), "buffer cleared after fold");
        assert_eq!(data.prior_summary_id.as_deref(), Some("mem-123"));

        // A later append accumulates afresh while the prior id is retained.
        s.append(Kind::File, "src/new.rs");
        let data = s.read();
        assert_eq!(data.files, vec!["src/new.rs"]);
        assert_eq!(
            data.prior_summary_id.as_deref(),
            Some("mem-123"),
            "prior summary id survives a post-fold append"
        );
    }

    #[test]
    fn mark_checkpointed_keeps_buffer_and_updates_summary_id() {
        let (_d, s) = scratch();
        s.append(Kind::File, "src/lib.rs");
        s.append(Kind::Command, "cargo test");
        s.mark_checkpointed("mem-123");
        let data = s.read();
        assert_eq!(data.files, vec!["src/lib.rs"]);
        assert_eq!(data.commands, vec!["cargo test"]);
        assert_eq!(data.prior_summary_id.as_deref(), Some("mem-123"));

        s.append(Kind::File, "src/main.rs");
        s.mark_checkpointed("mem-456");
        let data = s.read();
        assert_eq!(data.files, vec!["src/lib.rs", "src/main.rs"]);
        assert_eq!(data.prior_summary_id.as_deref(), Some("mem-456"));
    }

    #[test]
    fn mark_checkpointed_on_fresh_scratch_sets_id_without_creating_stray_entries() {
        // mark_checkpointed on a completely empty scratch (no appends, no prior
        // id) must only set prior_summary_id — the observation buffer stays empty
        // (is_empty considers the buffers, not the id).
        let (_d, s) = scratch();
        s.mark_checkpointed("mem-000");
        let data = s.read();
        assert!(data.files.is_empty());
        assert!(data.commands.is_empty());
        assert!(data.failures.is_empty());
        assert_eq!(data.prior_summary_id.as_deref(), Some("mem-000"));
        assert!(data.is_empty(), "no observations recorded");
    }

    #[test]
    fn mark_checkpointed_after_mark_folded_updates_the_id_without_restoring_buffer() {
        // A SessionEnd (mark_folded) followed by a SessionCheckpoint
        // (mark_checkpointed) with no new work: the buffer stays clear and only
        // the id advances.
        let (_d, s) = scratch();
        s.append(Kind::File, "src/first.rs");
        s.mark_folded(Some("end-id-1"));
        let d1 = s.read();
        assert!(d1.is_empty());
        assert_eq!(d1.prior_summary_id.as_deref(), Some("end-id-1"));

        s.mark_checkpointed("cp-id-2");
        let d2 = s.read();
        assert!(
            d2.is_empty(),
            "buffer stays empty after mark_checkpointed with no new appends"
        );
        assert_eq!(d2.prior_summary_id.as_deref(), Some("cp-id-2"));
    }

    #[test]
    fn mark_checkpointed_supersedes_prior_id_from_mark_folded_with_new_appends() {
        // Resume-after-fold: mark_folded clears + sets the id; a new append then a
        // checkpoint must keep BOTH the post-fold entry AND the latest id, while
        // the pre-fold entries stay cleared.
        let (_d, s) = scratch();
        s.append(Kind::Command, "cargo build");
        s.mark_folded(Some("old-end-id"));
        s.append(Kind::File, "src/new.rs");
        s.mark_checkpointed("cp-id-after-resume");
        let data = s.read();
        assert_eq!(
            data.files,
            vec!["src/new.rs"],
            "the post-fold append is retained"
        );
        assert!(
            data.commands.is_empty(),
            "pre-fold commands were cleared by mark_folded"
        );
        assert_eq!(
            data.prior_summary_id.as_deref(),
            Some("cp-id-after-resume"),
            "mark_checkpointed advances the id from the prior mark_folded value"
        );
    }

    #[test]
    fn mark_folded_with_none_clears_buffer() {
        let (_d, s) = scratch();
        s.append(Kind::Failure, "boom");
        s.mark_folded(None);
        let data = s.read();
        assert!(data.is_empty());
        assert_eq!(data.prior_summary_id, None);
    }

    #[test]
    fn corrupt_scratch_fails_open_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scratch.json");
        std::fs::write(&path, b"{ not valid json").unwrap();
        let s = Scratch::at(path);
        assert!(s.read().is_empty());
        // And a subsequent append still works (overwrites the garbage).
        s.append(Kind::File, "ok.rs");
        assert_eq!(s.read().files, vec!["ok.rs"]);
    }

    #[cfg(unix)]
    #[test]
    fn scratch_file_and_dir_are_private() {
        // THREAT_MODEL backstop: the scratch persists best-effort-redacted
        // plaintext, so the file is 0600 and the dir it creates is 0700.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("rusty-brain");
        let s = Scratch::at(sub.join("scratch.json"));
        s.append(Kind::Command, "echo hi");
        let file_mode = std::fs::metadata(sub.join("scratch.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "scratch file must be 0600, got {file_mode:o}"
        );
        let dir_mode = std::fs::metadata(&sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "scratch dir must be 0700, got {dir_mode:o}"
        );
    }

    #[test]
    fn for_session_is_injective_across_sessions_and_namespaces() {
        let ns = Namespace::Project("proj".into());
        let a = Scratch::for_session(&ns, "session-1").path;
        let b = Scratch::for_session(&ns, "session-2").path;
        assert_ne!(a, b, "distinct sessions map to distinct files");

        let other_ns = Namespace::Project("other".into());
        let c = Scratch::for_session(&other_ns, "session-1").path;
        assert_ne!(a, c, "same session in distinct namespaces stays separate");
    }
}
