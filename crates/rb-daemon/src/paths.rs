use std::path::PathBuf;

use rb_types::{Error, Result};

/// Default Unix-domain-socket path.
pub fn default_socket_path() -> Result<PathBuf> {
    if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
        let rt = PathBuf::from(rt);
        if !rt.as_os_str().is_empty() {
            return Ok(rt.join("rusty-brain").join("sock"));
        }
    }

    let dirs = directories::ProjectDirs::from("dev", "rusty-brain", "rusty-brain")
        .ok_or_else(|| Error::Io("cannot determine a runtime directory".to_string()))?;
    let base = dirs
        .runtime_dir()
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs.cache_dir().to_path_buf());
    Ok(base.join("rusty-brain").join("sock"))
}

/// Default database path.
pub fn default_db_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "rusty-brain", "rusty-brain")
        .ok_or_else(|| Error::Io("cannot determine a data directory".to_string()))?;
    Ok(dirs.data_dir().join("memory.db"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn socket_path_is_under_a_rusty_brain_dir_named_sock() {
        let p = default_socket_path().unwrap();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("sock"));
        assert!(
            p.parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                == Some("rusty-brain"),
            "socket lives in a rusty-brain directory: {p:?}"
        );
    }

    #[test]
    fn db_path_ends_with_rusty_brain_db_file() {
        let p = default_db_path().unwrap();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("memory.db"));
    }

    #[test]
    fn xdg_runtime_dir_is_honored_for_socket() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        let p = default_socket_path().unwrap();
        assert!(
            p.starts_with(dir.path()),
            "socket must live under XDG_RUNTIME_DIR when set: {p:?}"
        );
        std::env::remove_var("XDG_RUNTIME_DIR");
    }
}
