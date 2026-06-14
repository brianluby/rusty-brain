//! Atomic, sentinel-aware JSON merge engine.
//!
//! `merge_value` deep-merges our fragment into an existing config while
//! stripping any prior sentinel-marked entries first (idempotency); `write`
//! backs up the old file to `<name>.bak` and writes atomically
//! (temp + fsync + rename + parent fsync). The user's own keys and hooks are
//! never touched.

use std::fs;
use std::path::Path;

use rb_agents::install::{ManagedFile, ManagedTextBlock, SENTINEL};
use rb_types::{Error, Result};

/// Read `path` as JSON, returning `{}` if the file is absent or empty.
///
/// # Errors
/// Returns [`Error::Io`] on read failure and [`Error::Serialization`] if the
/// file exists but is not valid JSON (fail closed — never silently clobber).
pub fn read_config(path: &Path) -> Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let text = fs::read_to_string(path).map_err(|e| Error::Io(e.to_string()))?;
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(&text).map_err(|e| Error::Serialization(e.to_string()))
}

/// True if `value` is an object/array carrying our sentinel marker (`{SENTINEL: true}`).
fn is_sentinel(value: &serde_json::Value) -> bool {
    value
        .get(SENTINEL)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Coarse structural kind used to decide whether a fragment may overwrite a
/// user's existing value. Object/Array are distinguished from each other and
/// from any scalar; all scalars (bool/number/string) share the `Scalar` kind so
/// a same-kind scalar still lets the fragment win.
#[derive(PartialEq, Eq)]
enum Kind {
    Null,
    Scalar,
    Array,
    Object,
}

fn kind_of(value: &serde_json::Value) -> Kind {
    match value {
        serde_json::Value::Null => Kind::Null,
        serde_json::Value::Array(_) => Kind::Array,
        serde_json::Value::Object(_) => Kind::Object,
        _ => Kind::Scalar,
    }
}

/// True if `value` is a non-empty container (object/array with members). Scalars
/// and null are never "empty containers". Used to decide whether a structurally
/// mismatched user value is meaningful enough to preserve.
fn is_nonempty_container(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => !o.is_empty(),
        _ => false,
    }
}

/// Deep-merge `fragment` into `base`, stripping prior sentinel entries first.
///
/// Objects merge key-by-key. Arrays are treated as hook-group lists: any
/// element already carrying the sentinel is dropped from `base`, then the
/// fragment's elements are appended — so a second merge of the same fragment is
/// a no-op (idempotent) and the user's non-sentinel elements always survive.
#[must_use]
pub fn merge_value(base: serde_json::Value, fragment: &serde_json::Value) -> serde_json::Value {
    match (base, fragment) {
        (serde_json::Value::Object(mut b), serde_json::Value::Object(f)) => {
            for (k, fv) in f {
                let merged = match b.remove(k) {
                    Some(bv) => merge_value(bv, fv),
                    None => fv.clone(),
                };
                b.insert(k.clone(), merged);
            }
            serde_json::Value::Object(b)
        }
        (serde_json::Value::Array(b), serde_json::Value::Array(f)) => {
            let mut out: Vec<serde_json::Value> =
                b.into_iter().filter(|e| !is_sentinel(e)).collect();
            out.extend(f.iter().cloned());
            serde_json::Value::Array(out)
        }
        (base, fragment) => {
            // Structural type mismatch (e.g. user `"hooks": ["x"]` vs our object).
            // Only let the fragment win when the user's base is null/absent or a
            // same-kind scalar — otherwise a meaningful, differently-typed user
            // value (a non-empty container) would be silently dropped. Preserve it.
            let same_scalar = kind_of(&base) == Kind::Scalar && kind_of(fragment) == Kind::Scalar;
            if kind_of(&base) == Kind::Null || same_scalar || !is_nonempty_container(&base) {
                fragment.clone()
            } else {
                base
            }
        }
    }
}

/// Merge `fragment` into the config at `path` and write it back atomically.
///
/// Backs up an existing file to `<name>.bak` first. Returns the merged value.
///
/// # Errors
/// Returns [`Error::Io`] on any filesystem failure and
/// [`Error::Serialization`] if the existing file is invalid JSON.
pub fn merge_into_file(path: &Path, fragment: &serde_json::Value) -> Result<serde_json::Value> {
    let base = read_config(path)?;
    let merged = merge_value(base, fragment);
    let body =
        serde_json::to_string_pretty(&merged).map_err(|e| Error::Serialization(e.to_string()))?;
    write(path, &body, true)?;
    Ok(merged)
}

// ---- W3.2 managed side-effects (permissions.allow, whole files, text blocks) --
//
// These live BESIDE the sentinel JSON merge, never inside it: a permission
// STRING and a Markdown block cannot carry the `{SENTINEL: true}` marker, so they
// are added/removed by exact value / by begin-end markers instead of the sentinel
// strip. Keeping them out of `merge_value`/`strip_sentinel` leaves those tested
// functions — and the install→uninstall round-trip — unchanged.

/// Read `path` as UTF-8 text, returning `""` if the file is absent.
///
/// # Errors
/// Returns [`Error::Io`] on read failure.
fn read_text(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).map_err(|e| Error::Io(e.to_string()))
}

/// The begin/end markers wrapping a [`ManagedTextBlock`] keyed by `marker_id`.
/// HTML comments so they are invisible in rendered Markdown but findable for
/// idempotent replace + clean removal.
fn block_markers(marker_id: &str) -> (String, String) {
    (
        format!(
            "<!-- BEGIN rusty-brain:{marker_id} — managed by `rusty-brain install`; \
             content between these markers is replaced on reinstall and removed on uninstall -->"
        ),
        format!("<!-- END rusty-brain:{marker_id} -->"),
    )
}

/// Locate our block in `text` as the byte range `[start_of_begin .. end]`, where
/// `end` includes the end marker and one trailing newline if present. `None`
/// when the markers are absent (or malformed: a begin with no following end).
fn find_block(text: &str, begin: &str, end: &str) -> Option<(usize, usize)> {
    let start = text.find(begin)?;
    let after_begin = start + begin.len();
    let end_at = after_begin + text.get(after_begin..)?.find(end)?;
    let mut stop = end_at + end.len();
    if text.get(stop..).is_some_and(|rest| rest.starts_with('\n')) {
        stop += 1;
    }
    Some((start, stop))
}

/// Write a whole installer-owned file (W3.2(b)), creating parent dirs. No
/// backup: the file is ours, re-written verbatim each install.
///
/// # Errors
/// Returns [`Error::Io`] on any filesystem failure.
pub fn write_managed_file(file: &ManagedFile) -> Result<()> {
    write(&file.path, &file.contents, false)
}

/// Delete an installer-owned file if present (the inverse of
/// [`write_managed_file`]). Missing path is a no-op success.
///
/// # Errors
/// Returns [`Error::Io`] on a filesystem failure other than "not found".
pub fn remove_managed_file(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).map_err(|e| Error::Io(e.to_string()))?;
    }
    Ok(())
}

/// Append (or, if already present, REPLACE in place) a marker-delimited text
/// block in `block.path` (W3.2(b)), creating the file if absent. Idempotent: a
/// second install with the same body is a no-op; a changed body replaces only
/// the bytes between the markers, leaving the user's surrounding prose intact.
///
/// # Errors
/// Returns [`Error::Io`] on read/write failure.
pub fn ensure_text_block(block: &ManagedTextBlock) -> Result<()> {
    let (begin, end) = block_markers(&block.marker_id);
    let rendered = format!("{begin}\n{}\n{end}\n", block.body.trim_end());
    let existing = read_text(&block.path)?;
    let updated = match find_block(&existing, &begin, &end) {
        Some((start, stop)) => {
            let mut s = String::with_capacity(existing.len());
            s.push_str(&existing[..start]);
            s.push_str(&rendered);
            s.push_str(&existing[stop..]);
            s
        }
        None => {
            let mut s = existing.clone();
            // Separate from prior content with exactly one blank line.
            if !s.is_empty() {
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push('\n');
            }
            s.push_str(&rendered);
            s
        }
    };
    if updated == existing {
        return Ok(());
    }
    write(&block.path, &updated, false)
}

/// Remove our marker-delimited block from `path` (the inverse of
/// [`ensure_text_block`]), plus one blank-line separator immediately before it.
/// The user's surrounding content is preserved; a file that held only our block
/// is left empty rather than deleted (it may be a user-created `CLAUDE.md`).
/// Missing path / absent block is a no-op success.
///
/// # Errors
/// Returns [`Error::Io`] on read/write failure.
pub fn remove_text_block(path: &Path, marker_id: &str) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let existing = read_text(path)?;
    let (begin, end) = block_markers(marker_id);
    let Some((start, stop)) = find_block(&existing, &begin, &end) else {
        return Ok(());
    };
    // Consume one blank-line separator we inserted before the block, if present.
    let real_start = if existing[..start].ends_with("\n\n") {
        start - 1
    } else {
        start
    };
    let mut updated = String::with_capacity(existing.len());
    updated.push_str(&existing[..real_start]);
    updated.push_str(&existing[stop..]);
    if updated == existing {
        return Ok(());
    }
    write(path, &updated, false)
}

/// Union `entries` into the config's `permissions.allow` array (W3.2(c)),
/// creating `permissions`/`allow` as needed and skipping entries already
/// present (idempotent — re-install adds nothing). Writes WITHOUT a backup so
/// the `.bak` left by the preceding hooks merge keeps the true pre-install
/// original. A malformed non-object `permissions` or non-array `allow` is left
/// untouched rather than clobbered.
///
/// # Errors
/// Returns [`Error::Io`]/[`Error::Serialization`] on read/parse/write failure.
pub fn ensure_allow_entries(path: &Path, entries: &[String]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut root = match read_config(path)? {
        serde_json::Value::Object(m) => m,
        serde_json::Value::Null => serde_json::Map::new(),
        // A non-object settings file is malformed; do not clobber it.
        _ => return Ok(()),
    };
    let permissions = root
        .entry("permissions")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let serde_json::Value::Object(perms) = permissions else {
        return Ok(());
    };
    let allow = perms
        .entry("allow")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let serde_json::Value::Array(arr) = allow else {
        return Ok(());
    };
    let mut changed = false;
    for e in entries {
        let ev = serde_json::Value::String(e.clone());
        if !arr.contains(&ev) {
            arr.push(ev);
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .map_err(|e| Error::Serialization(e.to_string()))?;
    write(path, &body, false)
}

/// Remove `entries` from the config's `permissions.allow` (the inverse of
/// [`ensure_allow_entries`]), pruning an `allow`/`permissions` we emptied while
/// leaving any user entries. Missing path / absent entries is a no-op success.
///
/// # Errors
/// Returns [`Error::Io`]/[`Error::Serialization`] on read/parse/write failure.
pub fn remove_allow_entries(path: &Path, entries: &[String]) -> Result<()> {
    if entries.is_empty() || !path.exists() {
        return Ok(());
    }
    let mut root = match read_config(path)? {
        serde_json::Value::Object(m) => m,
        _ => return Ok(()),
    };
    let Some(serde_json::Value::Object(perms)) = root.get_mut("permissions") else {
        return Ok(());
    };
    let Some(serde_json::Value::Array(allow)) = perms.get_mut("allow") else {
        return Ok(());
    };
    let before = allow.len();
    allow.retain(|v| {
        !entries
            .iter()
            .any(|e| v == &serde_json::Value::String(e.clone()))
    });
    if allow.len() == before {
        return Ok(()); // none of ours were present
    }
    if allow.is_empty() {
        perms.remove("allow");
    }
    if perms.is_empty() {
        root.remove("permissions");
    }
    let body = serde_json::to_string_pretty(&serde_json::Value::Object(root))
        .map_err(|e| Error::Serialization(e.to_string()))?;
    write(path, &body, false)
}

/// Write `body` to `path` atomically; back up an existing file to `<name>.bak`.
///
/// temp-in-same-dir → fsync → rename → parent-dir fsync (Unix). Creates parent
/// directories as needed.
///
/// # Errors
/// Returns [`Error::Io`] on any filesystem failure.
pub fn write(path: &Path, body: &str, backup: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::Io(e.to_string()))?;
    }
    if backup && path.exists() {
        let bak = backup_path(path);
        fs::copy(path, &bak).map_err(|e| Error::Io(e.to_string()))?;
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let (temp, file) = create_tempfile_in(parent)?;
    write_and_sync(&file, body)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o644);
        fs::set_permissions(&temp, perms).map_err(|e| Error::Io(e.to_string()))?;
    }

    // Drop the handle before the rename so the file is fully closed first.
    drop(file);

    rename_replacing(&temp, path)?;

    #[cfg(unix)]
    {
        let dir = fs::File::open(parent).map_err(|e| Error::Io(e.to_string()))?;
        dir.sync_all().map_err(|e| Error::Io(e.to_string()))?;
    }
    Ok(())
}

/// Compute the `<name>.bak` sibling path for `path`.
#[must_use]
pub fn backup_path(path: &Path) -> std::path::PathBuf {
    match path.extension() {
        Some(ext) => path.with_extension(format!("{}.bak", ext.to_string_lossy())),
        None => path.with_extension("bak"),
    }
}

/// Atomically create a fresh temp file inside `dir` and return its path + handle.
///
/// Uses `create_new` (`O_EXCL`): the open fails if the path already exists, so
/// there is no TOCTOU/symlink race between an `exists()` check and the create.
/// On the rare name collision we retry with a fresh `pid.nanos.counter` name a
/// bounded number of times, then fail (the installer is fail-CLOSED — surfacing
/// an IO error here is correct; only the hook path is fail-open).
///
/// # Errors
/// Returns [`Error::Io`] if no unique temp file can be created within the bounded
/// retries, or on any other create failure.
fn create_tempfile_in(dir: &Path) -> Result<(std::path::PathBuf, fs::File)> {
    use std::io::ErrorKind;

    const MAX_ATTEMPTS: u32 = 16;
    let pid = std::process::id();
    for attempt in 0..MAX_ATTEMPTS {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let name = format!(".rusty-brain-install.{pid}.{nanos}.{attempt}.tmp");
        let candidate = dir.join(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            // Collision (or a pre-existing symlink at the path): try a fresh name.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::Io(e.to_string())),
        }
    }
    Err(Error::Io(
        "could not create a unique temp file after bounded retries".to_string(),
    ))
}

/// Write `body` to an already-open temp file handle and fsync it durably.
///
/// # Errors
/// Returns [`Error::Io`] on any write or fsync failure.
fn write_and_sync(mut file: &fs::File, body: &str) -> Result<()> {
    use std::io::Write as _;
    file.write_all(body.as_bytes())
        .map_err(|e| Error::Io(e.to_string()))?;
    file.sync_all().map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}

/// Rename `temp` onto `dest`, replacing any existing target.
///
/// On unix `rename` already replaces an existing `dest` atomically. On Windows
/// `rename` FAILS if `dest` exists, so we remove an existing target immediately
/// before the rename. The Windows branch is the only behavioral change; unix is
/// untouched.
///
/// # Errors
/// Returns [`Error::Io`] on any filesystem failure.
fn rename_replacing(temp: &Path, dest: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        if dest.exists() {
            fs::remove_file(dest).map_err(|e| Error::Io(e.to_string()))?;
        }
    }
    fs::rename(temp, dest).map_err(|e| Error::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::installers::ClaudeCodeInstaller;
    use rb_agents::cli::AgentId;
    use rb_agents::install::{AgentInstaller, InstallScope};
    use std::path::PathBuf;

    fn claude_fragment(root: &Path) -> serde_json::Value {
        ClaudeCodeInstaller
            .hook_fragment(
                Path::new("/usr/local/bin/rusty-brain-hooks"),
                &InstallScope::Project(root.to_path_buf()),
            )
            .unwrap()
            .merge
    }

    #[test]
    fn merge_preserves_unrelated_keys_and_user_hooks() {
        let existing = serde_json::json!({
            "model": "claude-opus",
            "hooks": {
                "SessionStart": [
                    { "hooks": [ { "type": "command", "command": "user-tool" } ] }
                ]
            }
        });
        let frag = claude_fragment(Path::new("/tmp/p"));
        let merged = merge_value(existing, &frag);

        // Unrelated top-level key survives.
        assert_eq!(
            merged.get("model").unwrap(),
            &serde_json::json!("claude-opus")
        );
        // The user's SessionStart hook survives AND ours is appended.
        let ss = merged
            .get("hooks")
            .unwrap()
            .get("SessionStart")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(ss.len(), 2);
        let user = ss.iter().find(|g| {
            g.get("hooks")
                .and_then(|h| h.as_array())
                .map(|a| {
                    a.iter()
                        .any(|e| e.get("command").and_then(|c| c.as_str()) == Some("user-tool"))
                })
                .unwrap_or(false)
        });
        assert!(user.is_some(), "user hook must survive merge");
        let ours = ss.iter().find(|g| g.get(SENTINEL).is_some());
        assert!(ours.is_some(), "our sentinel group must be present");
    }

    #[test]
    fn merge_preserves_user_value_on_structural_type_mismatch() {
        // User has `hooks` as a non-empty ARRAY; our fragment merges an OBJECT at
        // the same key. The mismatched, meaningful user value must NOT be dropped.
        let existing = serde_json::json!({ "hooks": ["x"] });
        let frag = serde_json::json!({ "hooks": { "SessionStart": [] } });
        let merged = merge_value(existing, &frag);
        assert_eq!(
            merged.get("hooks").unwrap(),
            &serde_json::json!(["x"]),
            "a non-empty differently-typed user value must be preserved, not clobbered"
        );
    }

    #[test]
    fn merge_lets_fragment_win_over_null_and_same_scalar_and_empty_container() {
        // null base -> fragment wins.
        let m = merge_value(
            serde_json::json!({ "k": serde_json::Value::Null }),
            &serde_json::json!({ "k": 1 }),
        );
        assert_eq!(m.get("k").unwrap(), &serde_json::json!(1));
        // same-kind scalar base -> fragment wins.
        let m = merge_value(
            serde_json::json!({ "k": "old" }),
            &serde_json::json!({ "k": "new" }),
        );
        assert_eq!(m.get("k").unwrap(), &serde_json::json!("new"));
        // empty container of a different kind -> fragment wins (nothing to lose).
        let m = merge_value(
            serde_json::json!({ "k": [] }),
            &serde_json::json!({ "k": { "a": 1 } }),
        );
        assert_eq!(m.get("k").unwrap(), &serde_json::json!({ "a": 1 }));
    }

    #[test]
    fn merge_is_idempotent() {
        let frag = claude_fragment(Path::new("/tmp/p"));
        let once = merge_value(serde_json::json!({}), &frag);
        let twice = merge_value(once.clone(), &frag);
        // Re-merging must not duplicate our entries.
        for event in ["SessionStart", "PostToolUse", "Stop", "PreCompact"] {
            let a = once
                .get("hooks")
                .unwrap()
                .get(event)
                .unwrap()
                .as_array()
                .unwrap();
            let b = twice
                .get("hooks")
                .unwrap()
                .get(event)
                .unwrap()
                .as_array()
                .unwrap();
            assert_eq!(a.len(), 1, "single entry after first merge");
            assert_eq!(b.len(), 1, "still single entry after second merge");
        }
        assert_eq!(once, twice);
    }

    #[test]
    fn write_backs_up_and_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write(&path, r#"{"original":true}"#, false).unwrap();
        let frag = claude_fragment(dir.path());
        let merged = merge_into_file(&path, &frag).unwrap();

        // .bak holds the original.
        let bak = dir.path().join("settings.json.bak");
        assert!(bak.exists());
        let bak_text = std::fs::read_to_string(&bak).unwrap();
        assert!(bak_text.contains("\"original\":true"));

        // file holds the merged result.
        let on_disk: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, merged);
        assert!(on_disk.get("hooks").is_some());
    }

    #[test]
    fn create_tempfile_in_makes_a_fresh_exclusive_file() {
        // The temp file is created with create_new (O_EXCL): the path must not
        // pre-exist, and a second create at the SAME path must fail with
        // AlreadyExists — proving the exclusive create, not a TOCTOU exists()+open.
        let dir = tempfile::tempdir().unwrap();
        let (path, _file) = create_tempfile_in(dir.path()).unwrap();
        assert!(path.exists(), "temp file must have been created");
        let second = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path);
        assert!(
            matches!(second, Err(ref e) if e.kind() == std::io::ErrorKind::AlreadyExists),
            "a second exclusive create at the same path must fail AlreadyExists"
        );
    }

    #[test]
    fn repeated_writes_succeed_and_preserve_latest_content() {
        // Rapid successive writes exercise unique temp creation; each must land
        // atomically with the latest content (no temp-name collision failure).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for i in 0..25 {
            let body = format!(r#"{{"n":{i}}}"#);
            write(&path, &body, false).unwrap();
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert_eq!(on_disk, body, "write {i} must produce the latest content");
        }
        // No stray temp files left behind in the directory.
        let leftover = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".rusty-brain-install.")
            });
        assert!(!leftover, "no temp files should remain after writes");
    }

    #[test]
    fn read_config_returns_empty_object_for_missing_file() {
        let p = PathBuf::from("/tmp/__rb_install_definitely_missing__.json");
        assert_eq!(read_config(&p).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn read_config_fails_closed_on_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let err = read_config(&path).unwrap_err();
        assert!(matches!(err, Error::Serialization(_)));
    }

    #[test]
    fn backup_path_handles_extension_and_none() {
        assert_eq!(
            backup_path(Path::new("/a/settings.json")),
            PathBuf::from("/a/settings.json.bak")
        );
        assert_eq!(
            backup_path(Path::new("/a/hooks")),
            PathBuf::from("/a/hooks.bak")
        );
    }

    #[test]
    fn agent_id_round_trips_for_all_four() {
        for id in [
            AgentId::ClaudeCode,
            AgentId::OpenCode,
            AgentId::Gemini,
            AgentId::Codex,
        ] {
            assert_eq!(AgentId::parse(id.as_str()), Some(id));
        }
    }

    // ---- W3.2 managed side-effects --------------------------------------

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn ensure_allow_entries_unions_preserves_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "permissions": { "allow": ["Bash(ls)"], "deny": ["Read(./.env)"] }
            }))
            .unwrap(),
        )
        .unwrap();
        let entries = vec!["mcp__rusty-brain__*".to_string()];
        ensure_allow_entries(&path, &entries).unwrap();
        let v = read_json(&path);
        let allow = v["permissions"]["allow"].as_array().unwrap();
        assert!(
            allow.contains(&serde_json::json!("Bash(ls)")),
            "user entry preserved"
        );
        assert!(
            allow.contains(&serde_json::json!("mcp__rusty-brain__*")),
            "our entry added"
        );
        assert_eq!(
            v["permissions"]["deny"],
            serde_json::json!(["Read(./.env)"]),
            "deny untouched"
        );
        // Idempotent: re-install adds no duplicate.
        ensure_allow_entries(&path, &entries).unwrap();
        assert_eq!(
            read_json(&path)["permissions"]["allow"]
                .as_array()
                .unwrap()
                .len(),
            2,
            "no duplicate entry on re-install"
        );
    }

    #[test]
    fn ensure_allow_entries_creates_permissions_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({ "model": "x" })).unwrap(),
        )
        .unwrap();
        ensure_allow_entries(&path, &["mcp__rusty-brain__*".to_string()]).unwrap();
        let v = read_json(&path);
        assert_eq!(
            v["permissions"]["allow"],
            serde_json::json!(["mcp__rusty-brain__*"])
        );
        assert_eq!(v["model"], serde_json::json!("x"), "user keys preserved");
    }

    #[test]
    fn ensure_then_remove_allow_entries_round_trips_to_user_block() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let original = serde_json::json!({ "permissions": { "allow": ["Bash(ls)"], "deny": [] } });
        fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();
        let entries = vec!["mcp__rusty-brain__*".to_string()];
        ensure_allow_entries(&path, &entries).unwrap();
        remove_allow_entries(&path, &entries).unwrap();
        assert_eq!(
            read_json(&path),
            original,
            "install + uninstall of our permission restores the user's block"
        );
    }

    #[test]
    fn remove_allow_entries_prunes_only_emptied_containers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        // Ours was the sole allow entry: allow AND permissions are pruned.
        fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({
                "model": "x", "permissions": { "allow": ["mcp__rusty-brain__*"] }
            }))
            .unwrap(),
        )
        .unwrap();
        remove_allow_entries(&path, &["mcp__rusty-brain__*".to_string()]).unwrap();
        let v = read_json(&path);
        assert!(
            v.get("permissions").is_none(),
            "emptied permissions pruned: {v}"
        );
        assert_eq!(v["model"], serde_json::json!("x"));
    }

    #[test]
    fn ensure_text_block_appends_replaces_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        fs::write(&path, "# Project\n\nUser prose.\n").unwrap();
        let block = |body: &str| ManagedTextBlock {
            path: path.clone(),
            marker_id: "memory-policy".to_string(),
            body: body.to_string(),
        };
        ensure_text_block(&block("policy v1")).unwrap();
        let s = fs::read_to_string(&path).unwrap();
        assert!(s.contains("User prose."), "user prose preserved");
        assert!(s.contains("policy v1") && s.contains("BEGIN rusty-brain:memory-policy"));
        // Same body => no-op.
        ensure_text_block(&block("policy v1")).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            s,
            "same body is a no-op"
        );
        // Changed body => replaced in place (still exactly one block).
        ensure_text_block(&block("policy v2")).unwrap();
        let s2 = fs::read_to_string(&path).unwrap();
        assert!(s2.contains("policy v2") && !s2.contains("policy v1"));
        assert_eq!(
            s2.matches("BEGIN rusty-brain:memory-policy").count(),
            1,
            "exactly one block after replace"
        );
        assert!(s2.contains("User prose."));
    }

    #[test]
    fn ensure_then_remove_text_block_restores_prose() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        let original = "# Project\n\nUser prose.\n";
        fs::write(&path, original).unwrap();
        let block = ManagedTextBlock {
            path: path.clone(),
            marker_id: "memory-policy".to_string(),
            body: "policy".to_string(),
        };
        ensure_text_block(&block).unwrap();
        assert_ne!(fs::read_to_string(&path).unwrap(), original);
        remove_text_block(&path, "memory-policy").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            original,
            "uninstall restores CLAUDE.md verbatim"
        );
    }

    #[test]
    fn ensure_text_block_creates_file_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("CLAUDE.md");
        ensure_text_block(&ManagedTextBlock {
            path: path.clone(),
            marker_id: "m".to_string(),
            body: "hi".to_string(),
        })
        .unwrap();
        let s = fs::read_to_string(&path).unwrap();
        assert!(s.contains("hi") && s.contains("BEGIN rusty-brain:m"));
    }

    #[test]
    fn managed_file_write_then_remove_is_reversible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("skills")
            .join("rusty-brain-memory")
            .join("SKILL.md");
        let f = ManagedFile {
            path: path.clone(),
            contents: "# skill".to_string(),
        };
        write_managed_file(&f).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# skill");
        remove_managed_file(&path).unwrap();
        assert!(!path.exists());
        // Removing an absent file is a no-op success.
        remove_managed_file(&path).unwrap();
    }
}
