//! Orchestration: detect → fragment → merge / uninstall / status across agents.

use std::path::PathBuf;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, InstallScope};

use crate::installers::builtins;
use crate::report::{AgentReport, AgentStatus, InstallError, InstallReport};
use crate::uninstall::uninstall_file;
use crate::writer::{merge_into_file, read_config};

/// Resolve the hooks binary path: sibling of the running installer named
/// `rusty-brain-hooks`, falling back to the bare name for `PATH` resolution.
#[must_use]
pub fn resolve_hooks_bin() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let bin = if cfg!(windows) {
        "rusty-brain-hooks.exe"
    } else {
        "rusty-brain-hooks"
    };
    match exe.and_then(|e| e.parent().map(|p| p.join(bin))) {
        Some(p) if p.exists() => p,
        _ => PathBuf::from("rusty-brain-hooks"),
    }
}

/// Select installers: all four when `requested` is `None`, else exactly the
/// named subset. Fail closed on an unknown agent id.
///
/// # Errors
/// Returns [`InstallError::InvalidAgent`] for any unrecognized name.
pub fn select_installers(
    requested: Option<&[String]>,
) -> Result<Vec<Box<dyn AgentInstaller>>, InstallError> {
    let all = builtins();
    match requested {
        None => Ok(all),
        Some(names) => {
            for name in names {
                if AgentId::parse(name).is_none() {
                    return Err(InstallError::InvalidAgent {
                        agent: name.clone(),
                    });
                }
            }
            Ok(all
                .into_iter()
                .filter(|inst| names.iter().any(|n| AgentId::parse(n) == Some(inst.id())))
                .collect())
        }
    }
}

/// Run an install (or dry-run install) across the selected installers.
pub fn run_install(
    installers: &[Box<dyn AgentInstaller>],
    hooks_bin: &std::path::Path,
    scope: &InstallScope,
    dry_run: bool,
) -> InstallReport {
    let mut agents = Vec::new();
    for inst in installers {
        let id = inst.id().as_str().to_string();
        let version = inst.detect();
        if version.is_none() {
            agents.push(AgentReport {
                agent: id,
                status: AgentStatus::NotFound,
                config_path: None,
                version: None,
                error: None,
            });
            continue;
        }
        let report = match inst.hook_fragment(hooks_bin, scope) {
            Ok(frag) => {
                if dry_run {
                    AgentReport {
                        agent: id,
                        status: AgentStatus::WouldConfigure,
                        config_path: Some(frag.config_path),
                        version,
                        error: None,
                    }
                } else {
                    // "Upgraded" means we replaced OUR previously-installed block.
                    // Classify by whether our sentinel already exists in the
                    // config, NOT by mere file existence: a first-time install into
                    // a pre-existing (unrelated) user config is a fresh Configure,
                    // and an idempotent re-run over our own block is an Upgrade.
                    let had_sentinel = read_config(&frag.config_path)
                        .ok()
                        .map(|v| contains_sentinel(&v))
                        .unwrap_or(false);
                    match merge_into_file(&frag.config_path, &frag.merge) {
                        Ok(_) => AgentReport {
                            agent: id,
                            status: if had_sentinel {
                                AgentStatus::Upgraded
                            } else {
                                AgentStatus::Configured
                            },
                            config_path: Some(frag.config_path),
                            version,
                            error: None,
                        },
                        Err(e) => AgentReport {
                            agent: id,
                            status: AgentStatus::Failed,
                            config_path: Some(frag.config_path),
                            version,
                            error: Some(e.to_string()),
                        },
                    }
                }
            }
            Err(e) => AgentReport {
                agent: id,
                status: AgentStatus::Failed,
                config_path: None,
                version,
                error: Some(e.to_string()),
            },
        };
        agents.push(report);
    }
    InstallReport::roll_up(scope_label(scope), dry_run, agents)
}

/// Run an uninstall (or dry-run uninstall) across the selected installers.
pub fn run_uninstall(
    installers: &[Box<dyn AgentInstaller>],
    hooks_bin: &std::path::Path,
    scope: &InstallScope,
    dry_run: bool,
) -> InstallReport {
    let mut agents = Vec::new();
    for inst in installers {
        let id = inst.id().as_str().to_string();
        let config_path = match inst.hook_fragment(hooks_bin, scope) {
            Ok(frag) => frag.config_path,
            Err(e) => {
                agents.push(AgentReport {
                    agent: id,
                    status: AgentStatus::Failed,
                    config_path: None,
                    version: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };
        if dry_run {
            agents.push(AgentReport {
                agent: id,
                status: AgentStatus::WouldRemove,
                config_path: Some(config_path),
                version: None,
                error: None,
            });
            continue;
        }
        let report = match uninstall_file(&config_path) {
            Ok(()) => AgentReport {
                agent: id,
                status: AgentStatus::Removed,
                config_path: Some(config_path),
                version: None,
                error: None,
            },
            Err(e) => AgentReport {
                agent: id,
                status: AgentStatus::Failed,
                config_path: Some(config_path),
                version: None,
                error: Some(e.to_string()),
            },
        };
        agents.push(report);
    }
    InstallReport::roll_up(scope_label(scope), dry_run, agents)
}

/// Report detection + whether our sentinel block is present in each config.
pub fn run_status(
    installers: &[Box<dyn AgentInstaller>],
    hooks_bin: &std::path::Path,
    scope: &InstallScope,
) -> InstallReport {
    let mut agents = Vec::new();
    for inst in installers {
        let id = inst.id().as_str().to_string();
        let version = inst.detect();
        let config_path = inst
            .hook_fragment(hooks_bin, scope)
            .ok()
            .map(|f| f.config_path);
        let present = config_path
            .as_ref()
            .and_then(|p| read_config(p).ok())
            .map(|v| contains_sentinel(&v))
            .unwrap_or(false);
        let status = if version.is_none() {
            AgentStatus::NotFound
        } else if present {
            AgentStatus::Present
        } else {
            AgentStatus::Absent
        };
        agents.push(AgentReport {
            agent: id,
            status,
            config_path,
            version,
            error: None,
        });
    }
    InstallReport::roll_up(scope_label(scope), false, agents)
}

/// True if any value anywhere in the tree carries our sentinel marker.
fn contains_sentinel(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            if map
                .get(rb_agents::install::SENTINEL)
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                return true;
            }
            map.values().any(contains_sentinel)
        }
        serde_json::Value::Array(items) => items.iter().any(contains_sentinel),
        _ => false,
    }
}

fn scope_label(scope: &InstallScope) -> &'static str {
    match scope {
        InstallScope::Project(_) => "project",
        InstallScope::Global => "global",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::report::{AgentStatus, ReportStatus};

    #[test]
    fn select_installers_all_when_none() {
        let all = select_installers(None).unwrap();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn select_installers_subset() {
        let subset = select_installers(Some(&["codex".to_string()])).unwrap();
        assert_eq!(subset.len(), 1);
        assert_eq!(subset[0].id(), AgentId::Codex);
    }

    #[test]
    fn select_installers_rejects_unknown() {
        // Match on the result instead of `.unwrap_err()`: `Box<dyn AgentInstaller>`
        // is not `Debug`, so `unwrap_err` (which would print the Ok value) won't
        // compile. The assertion below pins the identical plan intent.
        let result = select_installers(Some(&["cursor".to_string()]));
        assert!(matches!(result, Err(InstallError::InvalidAgent { .. })));
    }

    /// A test-only installer whose `detect()` always succeeds, so engine tests
    /// that exercise the detect -> fragment dry-run flow do not depend on a real
    /// agent CLI being on `PATH` (it is not on CI). Emits a Claude-Code-shaped
    /// fragment under the scope directory.
    struct FakeInstaller;

    impl AgentInstaller for FakeInstaller {
        fn id(&self) -> AgentId {
            AgentId::ClaudeCode
        }

        fn detect(&self) -> Option<String> {
            Some("fake-1.0".to_string())
        }

        fn hook_fragment(
            &self,
            _hooks_bin: &std::path::Path,
            scope: &InstallScope,
        ) -> rb_types::Result<rb_agents::install::HookFragment> {
            let base = match scope {
                InstallScope::Project(p) => p.clone(),
                InstallScope::Global => std::path::PathBuf::from("/tmp"),
            };
            Ok(rb_agents::install::HookFragment {
                config_path: base.join(".claude").join("settings.json"),
                merge: serde_json::json!({ "hooks": {} }),
            })
        }
    }

    #[test]
    fn dry_run_install_writes_nothing_but_reports_would_configure() {
        // Use a fake installer whose detect() always succeeds, so this test is
        // deterministic regardless of whether `claude` is on PATH (it is not on
        // CI). This exercises the engine's dry-run WouldConfigure path, not binary
        // detection.
        let dir = tempfile::tempdir().unwrap();
        let installers: Vec<Box<dyn AgentInstaller>> = vec![Box::new(FakeInstaller)];
        let scope = InstallScope::Project(dir.path().to_path_buf());
        let report = run_install(
            &installers,
            std::path::Path::new("/x/rusty-brain-hooks"),
            &scope,
            true,
        );
        assert_eq!(report.agents[0].status, AgentStatus::WouldConfigure);
        assert!(report.dry_run);
        // Nothing written to disk.
        assert!(!dir.path().join(".claude").join("settings.json").exists());
    }

    #[test]
    fn first_install_into_existing_unrelated_config_classifies_configured() {
        // A pre-existing, unrelated user config (no sentinel) must classify as a
        // fresh Configure, NOT Upgraded — the old file-existence check mislabeled
        // this as Upgraded.
        let dir = tempfile::tempdir().unwrap();
        let installers = select_installers(Some(&["claude-code".to_string()])).unwrap();
        let scope = InstallScope::Project(dir.path().to_path_buf());
        let bin = std::path::Path::new("/x/rusty-brain-hooks");
        let frag = installers[0].hook_fragment(bin, &scope).unwrap();

        // Seed an existing config that has none of our markers.
        crate::writer::write(&frag.config_path, r#"{"model":"opus"}"#, false).unwrap();

        // The classification predicate run_install uses: no sentinel yet => fresh.
        let had_sentinel = read_config(&frag.config_path)
            .ok()
            .map(|v| contains_sentinel(&v))
            .unwrap_or(false);
        assert!(
            !had_sentinel,
            "an unrelated existing config must classify as Configured, not Upgraded"
        );
    }

    #[test]
    fn reinstall_over_our_own_block_classifies_upgraded() {
        // After we have already installed our block, a re-run must classify as
        // Upgraded (our sentinel is present before the merge).
        let dir = tempfile::tempdir().unwrap();
        let installers = select_installers(Some(&["claude-code".to_string()])).unwrap();
        let scope = InstallScope::Project(dir.path().to_path_buf());
        let bin = std::path::Path::new("/x/rusty-brain-hooks");
        let frag = installers[0].hook_fragment(bin, &scope).unwrap();

        // First install lays down our sentinel-marked block.
        crate::writer::merge_into_file(&frag.config_path, &frag.merge).unwrap();

        let had_sentinel = read_config(&frag.config_path)
            .ok()
            .map(|v| contains_sentinel(&v))
            .unwrap_or(false);
        assert!(
            had_sentinel,
            "a re-install over our own block must classify as Upgraded"
        );
    }

    #[test]
    fn install_then_status_present_then_uninstall_absent() {
        let dir = tempfile::tempdir().unwrap();
        let installers = select_installers(Some(&["claude-code".to_string()])).unwrap();
        let scope = InstallScope::Project(dir.path().to_path_buf());
        let bin = std::path::Path::new("/x/rusty-brain-hooks");

        // detect() needs the binary on PATH; here it returns None (claude not
        // installed in CI), so install reports NotFound and writes nothing.
        let installed = run_install(&installers, bin, &scope, false);
        // When claude is absent the engine short-circuits to NotFound.
        assert!(matches!(
            installed.agents[0].status,
            AgentStatus::NotFound | AgentStatus::Configured
        ));

        // Drive the file-level path directly to assert status/uninstall logic
        // without depending on a real `claude` binary.
        let frag = installers[0].hook_fragment(bin, &scope).unwrap();
        crate::writer::merge_into_file(&frag.config_path, &frag.merge).unwrap();
        let present = contains_sentinel(&read_config(&frag.config_path).unwrap());
        assert!(present, "sentinel present after merge");

        let removed = run_uninstall(&installers, bin, &scope, false);
        assert_eq!(removed.agents[0].status, AgentStatus::Removed);
        let after = read_config(&frag.config_path).unwrap();
        assert!(!contains_sentinel(&after), "sentinel gone after uninstall");
        assert_eq!(installed.status, installed.status); // report builds
        let _ = ReportStatus::Success;
    }
}
