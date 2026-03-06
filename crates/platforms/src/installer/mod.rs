//! Agent installer subsystem.
//!
//! Provides the [`AgentInstaller`] trait, binary detection utilities,
//! path validation, and global scope path resolution.

pub mod agents;
pub mod orchestrator;
pub mod registry;
pub mod writer;

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use types::install::{AgentInfo, ConfigFile, InstallError, InstallScope};

/// Trait that each agent installer must implement.
///
/// `generate_config` MUST be a pure function — no filesystem I/O.
/// All filesystem operations go through [`writer::ConfigWriter`].
pub trait AgentInstaller: Send + Sync {
    /// Canonical lowercase agent name: "opencode", "copilot", "codex", "gemini".
    fn agent_name(&self) -> &'static str;

    /// Detect if this agent is installed on the system.
    ///
    /// Checks PATH for the agent binary and optionally runs `--version`
    /// with a 2-second timeout.
    ///
    /// Returns `None` if the agent is not found.
    fn detect(&self) -> Option<AgentInfo>;

    /// Generate configuration files for this agent.
    ///
    /// This MUST be a pure function: given scope and binary path, return
    /// a list of files to write. No filesystem I/O.
    ///
    /// `binary_path` is the absolute path to the rusty-brain binary.
    ///
    /// # Errors
    ///
    /// Returns [`InstallError`] if config generation fails (e.g., scope resolution error).
    fn generate_config(
        &self,
        scope: &InstallScope,
        binary_path: &Path,
    ) -> Result<Vec<ConfigFile>, InstallError>;

    /// Validate that the installation is working (post-install check).
    ///
    /// # Errors
    ///
    /// Returns [`InstallError`] if validation fails (e.g., config file missing or invalid).
    fn validate(&self, scope: &InstallScope) -> Result<(), InstallError>;
}

/// Hardcoded allowlist of supported agent names.
pub const SUPPORTED_AGENTS: &[&str] = &["opencode", "copilot", "codex", "gemini"];

/// Check if an agent name is in the supported allowlist.
#[must_use]
pub fn is_valid_agent(name: &str) -> bool {
    SUPPORTED_AGENTS
        .iter()
        .any(|a| a.eq_ignore_ascii_case(name))
}

/// Find a binary on the system PATH without shell execution (SEC-7).
///
/// Iterates `$PATH` entries and checks for the binary name (with `.exe`
/// extension on Windows). Returns the first match.
#[must_use]
pub fn find_binary_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        // On Windows, also check with .exe extension.
        #[cfg(target_os = "windows")]
        {
            let with_exe = dir.join(format!("{name}.exe"));
            if with_exe.is_file() {
                return Some(with_exe);
            }
            let with_cmd = dir.join(format!("{name}.cmd"));
            if with_cmd.is_file() {
                return Some(with_cmd);
            }
            let with_bat = dir.join(format!("{name}.bat"));
            if with_bat.is_file() {
                return Some(with_bat);
            }
        }
    }
    None
}

/// Detect a binary's version by running `<binary> --version` with a 2-second timeout.
///
/// Spawns the binary as a child process and kills it if the timeout fires,
/// preventing thread and process leaks (SEC-6).
///
/// Returns `None` if the binary fails to start, produces no output, or times out.
#[must_use]
pub fn detect_binary_version(binary_path: &Path) -> Option<String> {
    let mut child = Command::new(binary_path)
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
        // Enforce timeout on child lifetime too — a process that closes stdout
        // but hangs in cleanup would block child.wait() indefinitely.
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return parse_version_string(&stdout),
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => return None,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        parse_version_string(&stdout)
    } else {
        let _ = child.kill();
        let _ = child.wait();
        None
    }
}

/// Parse a version string from command output.
///
/// Handles formats like "opencode 1.2.3" or "v1.2.3" or just "1.2.3".
#[must_use]
pub fn parse_version_string(output: &str) -> Option<String> {
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

    Some(trimmed.to_string())
}

/// Validate that a JSON config file exists and is parseable.
///
/// Shared helper for agent `validate()` implementations.
///
/// # Errors
///
/// Returns [`InstallError::ConfigCorrupted`] if the file doesn't exist or contains
/// invalid JSON, or [`InstallError::IoError`] if the file can't be read.
pub fn validate_json_config(config_path: &Path) -> Result<(), InstallError> {
    if !config_path.exists() {
        return Err(InstallError::ConfigCorrupted {
            path: config_path.to_path_buf(),
        });
    }

    let content = std::fs::read_to_string(config_path).map_err(|source| InstallError::IoError {
        path: config_path.to_path_buf(),
        source,
    })?;
    serde_json::from_str::<serde_json::Value>(&content).map_err(|_| {
        InstallError::ConfigCorrupted {
            path: config_path.to_path_buf(),
        }
    })?;

    Ok(())
}

/// Validate a config path against traversal attacks (SEC-4).
///
/// Rejects paths containing `..` components. Returns the validated path on success.
///
/// # Errors
///
/// Returns [`InstallError::PathTraversal`] if the path contains `..` components.
pub fn validate_config_path(path: &Path) -> Result<PathBuf, InstallError> {
    // Check for literal ".." components in the path
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            return Err(InstallError::PathTraversal {
                path: path.to_path_buf(),
            });
        }
    }
    Ok(path.to_path_buf())
}

/// Resolve the global config directory for an agent, per platform.
///
/// - macOS: `~/Library/Application Support/<agent>/`
/// - Linux: `~/.config/<agent>/`
/// - Windows: `%APPDATA%/<agent>/`
///
/// # Errors
///
/// Returns [`InstallError::IoError`] if the required environment variable
/// (`HOME`, `APPDATA`) is not set.
pub fn resolve_global_config_dir(agent_name: &str) -> Result<PathBuf, InstallError> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").map_err(|_| InstallError::IoError {
            path: PathBuf::from("~"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME environment variable not set",
            ),
        })?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(agent_name))
    }

    #[cfg(target_os = "linux")]
    {
        // Respect XDG_CONFIG_HOME if set, otherwise fall back to ~/.config
        let config_dir = match std::env::var("XDG_CONFIG_HOME") {
            Ok(xdg) => xdg,
            Err(_) => {
                let home = std::env::var("HOME").map_err(|_| InstallError::IoError {
                    path: PathBuf::from("~"),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "HOME environment variable not set",
                    ),
                })?;
                format!("{home}/.config")
            }
        };
        Ok(PathBuf::from(config_dir).join(agent_name))
    }

    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").map_err(|_| InstallError::IoError {
            path: PathBuf::from("%APPDATA%"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "APPDATA environment variable not set",
            ),
        })?;
        Ok(PathBuf::from(appdata).join(agent_name))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let home = std::env::var("HOME").map_err(|_| InstallError::IoError {
            path: PathBuf::from("~"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME environment variable not set",
            ),
        })?;
        Ok(PathBuf::from(home).join(".config").join(agent_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // T013: AgentInstaller trait is object-safe and can be stored in Box
    #[test]
    fn agent_installer_trait_is_object_safe() {
        // This test verifies the trait can be used as a trait object.
        // Compilation alone proves object safety; we verify Box<dyn> works.
        struct DummyInstaller;
        impl AgentInstaller for DummyInstaller {
            fn agent_name(&self) -> &'static str {
                "dummy"
            }
            fn detect(&self) -> Option<AgentInfo> {
                None
            }
            fn generate_config(
                &self,
                _scope: &InstallScope,
                _binary_path: &Path,
            ) -> Result<Vec<ConfigFile>, InstallError> {
                Ok(vec![])
            }
            fn validate(&self, _scope: &InstallScope) -> Result<(), InstallError> {
                Ok(())
            }
        }

        let installer: Box<dyn AgentInstaller> = Box::new(DummyInstaller);
        assert_eq!(installer.agent_name(), "dummy");
        assert!(installer.detect().is_none());
    }

    // T015: find_binary_on_path tests
    #[test]
    fn find_binary_on_path_finds_existing_binary() {
        // "sh" should exist on any Unix system
        #[cfg(unix)]
        {
            let result = find_binary_on_path("sh");
            assert!(result.is_some(), "sh should be found on PATH");
            assert!(result.unwrap().is_file());
        }
    }

    #[test]
    fn find_binary_on_path_returns_none_for_nonexistent() {
        let result = find_binary_on_path("__rusty_brain_nonexistent_binary_12345__");
        assert!(result.is_none());
    }

    #[test]
    fn find_binary_returns_none_for_nonexistent_binary() {
        // Even with a valid PATH, a truly nonexistent binary returns None
        let result = find_binary_on_path("__absolutely_nonexistent_binary_xyz_99__");
        assert!(result.is_none());
    }

    // T017: validate_config_path tests
    #[test]
    fn validate_config_path_accepts_valid_path() {
        let path = Path::new("/tmp/project/.opencode/plugins");
        let result = validate_config_path(path);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_config_path_rejects_traversal() {
        let path = Path::new("/tmp/project/../etc/shadow");
        let result = validate_config_path(path);
        assert!(result.is_err());
        match result.unwrap_err() {
            InstallError::PathTraversal { .. } => {}
            other => panic!("Expected PathTraversal, got: {other:?}"),
        }
    }

    #[test]
    fn validate_config_path_accepts_absolute_path() {
        let path = Path::new("/usr/local/bin/rusty-brain");
        assert!(validate_config_path(path).is_ok());
    }

    // T018b: resolve_global_config_dir tests
    #[test]
    fn resolve_global_config_dir_returns_path_for_agent() {
        let result = resolve_global_config_dir("opencode");
        assert!(result.is_ok());
        let path = result.unwrap();
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("opencode"),
            "path should contain agent name: {path_str}"
        );
    }

    // is_valid_agent tests
    #[test]
    fn is_valid_agent_accepts_known_agents() {
        assert!(is_valid_agent("opencode"));
        assert!(is_valid_agent("copilot"));
        assert!(is_valid_agent("codex"));
        assert!(is_valid_agent("gemini"));
    }

    #[test]
    fn is_valid_agent_case_insensitive() {
        assert!(is_valid_agent("OpenCode"));
        assert!(is_valid_agent("COPILOT"));
    }

    #[test]
    fn is_valid_agent_rejects_unknown() {
        assert!(!is_valid_agent("vscode"));
        assert!(!is_valid_agent("cursor"));
        assert!(!is_valid_agent(""));
    }
}
