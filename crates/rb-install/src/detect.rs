//! No-shell binary detection used by every `AgentInstaller::detect()`.
//!
//! `find_binary_on_path` scans `$PATH` directly (never spawns a shell);
//! `version_of` runs `<bin> --version` under a hard 2-second timeout via a
//! reader thread + `recv_timeout`, killing the child on timeout so neither a
//! thread nor a process can leak.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Scan `$PATH` for an executable named `name`. Returns the first match.
///
/// A candidate must be a *runnable* file, not merely present: on unix it must be
/// a regular file with at least one execute bit set (a non-executable shadow file
/// named `claude` must NOT count as an install). On Windows the extension itself
/// is the executability signal, so we probe `name.exe`/`name.cmd`/`name.bat`
/// (and accept a bare `name` if it is already a file). No shell is ever spawned
/// (a literal directory join + metadata check only).
#[must_use]
pub fn find_binary_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
        #[cfg(target_os = "windows")]
        {
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = dir.join(format!("{name}.{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}

/// True if `path` is a regular file that the OS would treat as executable.
///
/// On unix this requires at least one execute bit (`mode & 0o111 != 0`) so a
/// non-executable file is rejected. On Windows executability is signalled by the
/// file extension (handled by the caller's extension probe), so a plain
/// `is_file()` check suffices here.
#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

/// See the unix variant: on Windows the extension carries the executability
/// signal, so a regular-file check is sufficient.
#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// Run `<binary> --version` with a 2-second timeout and parse a version token.
///
/// Returns `None` if the binary fails to start, emits no output, times out, or
/// prints no semver-ish token. Kills the child on timeout to avoid leaks.
#[must_use]
pub fn version_of(binary: &Path) -> Option<String> {
    let mut child = Command::new(binary)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdout_pipe = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_pipe.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });

    let deadline = Instant::now() + Duration::from_secs(2);
    if let Ok(stdout) = rx.recv_timeout(Duration::from_secs(2)) {
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return parse_version(&stdout),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => return None,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        parse_version(&stdout)
    } else {
        let _ = child.kill();
        let _ = child.wait();
        None
    }
}

/// Extract the first semver-ish token (e.g. `1.2.3`) from `--version` output.
///
/// Strips a leading `v`; requires a leading ASCII digit and at least one `.`.
#[must_use]
pub fn parse_version(output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return None;
    }
    for word in trimmed.split_whitespace() {
        let cleaned = word.trim_start_matches('v');
        if cleaned.chars().next().is_some_and(|c| c.is_ascii_digit()) && cleaned.contains('.') {
            return Some(cleaned.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::sync::Mutex;

    // Serializes the few tests that mutate the process-global PATH so parallel
    // test threads never observe a half-modified PATH (mirrors rb-daemon).
    #[cfg(unix)]
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_version_handles_common_shapes() {
        assert_eq!(parse_version("claude 1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(parse_version("v0.9.0"), Some("0.9.0".to_string()));
        assert_eq!(parse_version("2.0.1"), Some("2.0.1".to_string()));
    }

    #[test]
    fn parse_version_rejects_non_semver() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("   "), None);
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version("License: MIT"), None);
    }

    #[test]
    fn find_binary_returns_none_for_nonexistent() {
        assert!(find_binary_on_path("__rb_install_nonexistent_binary_98765__").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn find_binary_finds_executable_in_fake_path_dir() {
        // Hold the lock across the whole PATH mutation + read + restore so no
        // other test sees a mutated PATH. On edition 2021, set_var/remove_var
        // are safe (no `unsafe` block — that would be `unused_unsafe`).
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("rb-fake-cli");
        fs::write(&bin, "#!/bin/sh\necho rb-fake 4.5.6\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();

        // Prepend the fake dir onto a copy of PATH for this process.
        let old = std::env::var_os("PATH");
        let mut paths = vec![dir.path().to_path_buf()];
        if let Some(ref p) = old {
            paths.extend(std::env::split_paths(p));
        }
        let joined = std::env::join_paths(paths).unwrap();
        std::env::set_var("PATH", &joined);

        let found = find_binary_on_path("rb-fake-cli");

        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }

        assert_eq!(found, Some(bin));
    }

    #[cfg(unix)]
    #[test]
    fn non_executable_shadow_file_is_not_detected_but_executable_is() {
        // A file named like a CLI but WITHOUT an execute bit must not be treated as
        // installed (it cannot actually run); chmod 0755 makes the same name count.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("faketool");
        fs::write(&bin, "#!/bin/sh\necho faketool 1.0.0\n").unwrap();
        // Non-executable (0o644): present on PATH but not runnable.
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o644)).unwrap();

        let old = std::env::var_os("PATH");
        let mut paths = vec![dir.path().to_path_buf()];
        if let Some(ref p) = old {
            paths.extend(std::env::split_paths(p));
        }
        let joined = std::env::join_paths(paths).unwrap();
        std::env::set_var("PATH", &joined);

        let before = find_binary_on_path("faketool");

        // Now make it executable; it must be found.
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let after = find_binary_on_path("faketool");

        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }

        assert_eq!(
            before, None,
            "a non-executable shadow file must NOT be detected"
        );
        assert_eq!(after, Some(bin), "a chmod 0755 file must be detected");
    }
}
