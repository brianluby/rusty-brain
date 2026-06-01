use std::path::PathBuf;

use rb_types::{Error, Result};

fn nonempty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> Result<PathBuf> {
    nonempty_env_path("HOME")
        .or_else(|| nonempty_env_path("USERPROFILE"))
        .ok_or_else(|| Error::Io("cannot determine a home directory".to_string()))
}

fn cache_base_dir() -> Result<PathBuf> {
    if let Some(path) = nonempty_env_path("XDG_CACHE_HOME") {
        return Ok(path);
    }

    #[cfg(target_os = "macos")]
    {
        Ok(home_dir()?.join("Library").join("Caches"))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(path) = nonempty_env_path("LOCALAPPDATA") {
            Ok(path)
        } else {
            Ok(home_dir()?.join("AppData").join("Local"))
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(home_dir()?.join(".cache"))
    }
}

fn data_base_dir() -> Result<PathBuf> {
    if let Some(path) = nonempty_env_path("XDG_DATA_HOME") {
        return Ok(path);
    }

    #[cfg(target_os = "macos")]
    {
        Ok(home_dir()?.join("Library").join("Application Support"))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(path) = nonempty_env_path("APPDATA") {
            Ok(path)
        } else {
            Ok(home_dir()?.join("AppData").join("Roaming"))
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(home_dir()?.join(".local").join("share"))
    }
}

/// Default Unix-domain-socket path.
pub fn default_socket_path() -> Result<PathBuf> {
    if let Some(rt) = nonempty_env_path("XDG_RUNTIME_DIR") {
        return Ok(rt.join("rusty-brain").join("sock"));
    }

    Ok(cache_base_dir()?.join("rusty-brain").join("sock"))
}

/// Default database path.
pub fn default_db_path() -> Result<PathBuf> {
    Ok(data_base_dir()?.join("rusty-brain").join("memory.db"))
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
