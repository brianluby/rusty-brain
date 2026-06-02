//! Atomic, sentinel-aware JSON merge engine.
//!
//! `merge_value` deep-merges our fragment into an existing config while
//! stripping any prior sentinel-marked entries first (idempotency); `write`
//! backs up the old file to `<name>.bak` and writes atomically
//! (temp + fsync + rename + parent fsync). The user's own keys and hooks are
//! never touched.

use std::fs;
use std::path::Path;

use rb_agents::install::SENTINEL;
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

    fs::rename(&temp, path).map_err(|e| Error::Io(e.to_string()))?;

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
}
