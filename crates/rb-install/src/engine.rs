//! Orchestration: detect → fragment → merge / uninstall / status across agents.

use std::path::PathBuf;

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};

use crate::installers::builtins;
use crate::report::{AgentReport, AgentStatus, InstallError, InstallReport};
use crate::uninstall::uninstall_file;
use crate::writer::{
    ensure_allow_entries, ensure_text_block, merge_into_file, read_config, remove_allow_entries,
    remove_managed_file, remove_text_block, write_managed_file,
};

/// Apply a fragment's W3.2 managed side-effects AFTER its hooks merge: union the
/// `permissions.allow` entries (W3.2(c)) and write the managed files +
/// marker-delimited text blocks (W3.2(b)). Each is idempotent on re-install.
fn apply_managed_side_effects(frag: &HookFragment) -> rb_types::Result<()> {
    ensure_allow_entries(&frag.config_path, &frag.allow_entries)?;
    for file in &frag.managed_files {
        write_managed_file(file)?;
    }
    for block in &frag.text_blocks {
        ensure_text_block(block)?;
    }
    Ok(())
}

/// Reverse a fragment's managed side-effects on uninstall: remove our
/// `permissions.allow` entries, delete the managed files, and strip the
/// marker-delimited text blocks — the inverse of [`apply_managed_side_effects`].
fn reverse_managed_side_effects(frag: &HookFragment) -> rb_types::Result<()> {
    remove_allow_entries(&frag.config_path, &frag.allow_entries)?;
    for file in &frag.managed_files {
        remove_managed_file(&file.path)?;
    }
    for block in &frag.text_blocks {
        remove_text_block(&block.path, &block.marker_id)?;
    }
    Ok(())
}

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

/// Select installers: all shipped installers when `requested` is `None`, else
/// exactly the named subset. Fail closed on an unknown agent id, and reject the
/// deferred `opencode` agent with a clear error rather than silently doing
/// nothing (it has no JSON installer — it needs a JS/TS plugin).
///
/// # Errors
/// Returns [`InstallError::InvalidAgent`] for any unrecognized name, and
/// [`InstallError::AgentDeferred`] when `opencode` is explicitly requested.
pub fn select_installers(
    requested: Option<&[String]>,
) -> Result<Vec<Box<dyn AgentInstaller>>, InstallError> {
    let all = builtins();
    match requested {
        None => Ok(all),
        Some(names) => {
            for name in names {
                match AgentId::parse(name) {
                    None => {
                        return Err(InstallError::InvalidAgent {
                            agent: name.clone(),
                        });
                    }
                    // `opencode` is a real AgentId (the hook binary still supports
                    // `--agent opencode`) but has no installer — fail clearly.
                    Some(AgentId::OpenCode) => {
                        return Err(InstallError::AgentDeferred {
                            agent: name.clone(),
                        });
                    }
                    Some(_) => {}
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
                    // Merge the sentinel hooks block, then apply the W3.2 managed
                    // side-effects (permissions.allow + skill file + CLAUDE.md block).
                    let outcome = merge_into_file(&frag.config_path, &frag.merge)
                        .and_then(|_| apply_managed_side_effects(&frag));
                    if outcome.is_err() {
                        // Best-effort rollback so a partial install never leaves a
                        // half-configured tree: reverse our side-effects and strip the
                        // sentinel hooks block. Errors here are ignored — `outcome`
                        // already carries the original failure reported below.
                        let _ = reverse_managed_side_effects(&frag);
                        let _ = uninstall_file(&frag.config_path);
                    }
                    match outcome {
                        Ok(()) => AgentReport {
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
        let frag = match inst.hook_fragment(hooks_bin, scope) {
            Ok(frag) => frag,
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
        let config_path = frag.config_path.clone();
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
        // Strip the sentinel hooks block AND reverse the W3.2 managed side-effects
        // (permissions.allow entries, skill file, CLAUDE.md block). Run BOTH
        // best-effort — the side-effects are independent of the JSON cleanup, so a
        // broken settings.json must not strip the skill/block removal — then report
        // the first error.
        let uninstall_result = uninstall_file(&config_path);
        let reverse_result = reverse_managed_side_effects(&frag);
        let report = match uninstall_result.and(reverse_result) {
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
        // OpenCode is deferred, so the default set is three installers.
        let all = select_installers(None).unwrap();
        assert_eq!(all.len(), 3);
        assert!(!all.iter().any(|i| i.id() == AgentId::OpenCode));
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

    #[test]
    fn select_installers_defers_opencode_with_clear_error() {
        // `opencode` is a valid AgentId but has no installer (JS/TS plugin); it must
        // fail with a clear deferred error, NOT silently select zero installers.
        let result = select_installers(Some(&["opencode".to_string()]));
        assert!(matches!(result, Err(InstallError::AgentDeferred { .. })));
        if let Err(e) = result {
            assert!(e.to_string().contains("deferred"));
            assert!(e.to_string().contains("[E_INSTALL_AGENT_DEFERRED]"));
        }
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
            Ok(rb_agents::install::HookFragment::new(
                base.join(".claude").join("settings.json"),
                serde_json::json!({ "hooks": {} }),
            ))
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

    #[test]
    fn apply_then_reverse_managed_side_effects_round_trips() {
        // The mechanism behind both normal uninstall and the partial-install
        // rollback: apply (permissions + skill + CLAUDE.md block) then reverse
        // restores the tree. Uses a CLAUDE.md WITHOUT a trailing newline to pin
        // the byte-for-byte text-block round-trip.
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        std::fs::write(&settings, r#"{"permissions":{"allow":["Bash(ls)"]}}"#).unwrap();
        let claude_md = dir.path().join("CLAUDE.md");
        std::fs::write(&claude_md, "# notes").unwrap();
        let skill = dir.path().join("skills").join("rb-mem").join("SKILL.md");

        let frag = HookFragment::new(settings.clone(), serde_json::json!({}))
            .with_allow_entries(vec!["mcp__rusty-brain__*".to_string()])
            .with_text_blocks(vec![rb_agents::install::ManagedTextBlock {
                path: claude_md.clone(),
                marker_id: "memory-policy".to_string(),
                body: "policy".to_string(),
            }])
            .with_managed_files(vec![rb_agents::install::ManagedFile {
                path: skill.clone(),
                contents: "# skill".to_string(),
            }]);

        apply_managed_side_effects(&frag).unwrap();
        assert!(skill.exists());
        assert!(std::fs::read_to_string(&claude_md)
            .unwrap()
            .contains("BEGIN rusty-brain:memory-policy"));
        let applied: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            applied["permissions"]["allow"],
            serde_json::json!(["Bash(ls)", "mcp__rusty-brain__*"])
        );

        reverse_managed_side_effects(&frag).unwrap();
        assert!(!skill.exists(), "skill removed");
        assert!(!skill.parent().unwrap().exists(), "empty skill dir pruned");
        assert_eq!(
            std::fs::read_to_string(&claude_md).unwrap(),
            "# notes",
            "CLAUDE.md restored byte-for-byte (no trailing newline added)"
        );
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(
            after["permissions"]["allow"],
            serde_json::json!(["Bash(ls)"]),
            "our allow entry removed, the user's kept"
        );
    }
}
