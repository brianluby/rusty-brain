//! Persistent OMP extension lifecycle and ownership boundaries.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_install::engine::{run_install, run_status, run_uninstall};
use rb_install::{AgentStatus, OmpInstaller, ReportStatus};

// Exercise the real fragment/writer without depending on OMP being installed on
// the test host. Only detection is replaced, not any filesystem behavior.
struct DetectedOmp;

impl AgentInstaller for DetectedOmp {
    fn id(&self) -> AgentId {
        AgentId::Omp
    }

    fn detect(&self) -> Option<String> {
        Some("1.0.0".to_string())
    }

    fn hook_fragment(&self, hooks: &Path, scope: &InstallScope) -> rb_types::Result<HookFragment> {
        OmpInstaller.hook_fragment(hooks, scope)
    }
}

fn installers() -> Vec<Box<dyn AgentInstaller>> {
    vec![Box::new(DetectedOmp)]
}

fn scope(root: &Path) -> InstallScope {
    InstallScope::Project(root.to_path_buf())
}

fn hooks(root: &Path) -> PathBuf {
    root.join("bin/rusty-brain-hooks")
}

fn extension(root: &Path) -> PathBuf {
    root.join(".omp/extensions/rusty-brain.ts")
}

#[test]
fn project_install_is_idempotent_and_uninstall_preserves_neighbors() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    let installers = installers();
    let scope = scope(root);
    let binary = hooks(root);
    fs::create_dir_all(root.join(".omp/extensions")).unwrap();
    let neighbor = root.join(".omp/extensions/user.ts");
    fs::write(&neighbor, "export default function user() {}\n").unwrap();
    let settings = root.join(".omp/settings.json");
    fs::write(&settings, "{\"model\":\"keep\"}\n").unwrap();

    assert_eq!(
        run_install(&installers, &binary, &scope, false).agents[0].status,
        AgentStatus::Configured
    );
    let original = fs::read(extension(root)).unwrap();
    assert_eq!(
        run_status(&installers, &binary, &scope).agents[0].status,
        AgentStatus::Present
    );
    assert_eq!(
        run_install(&installers, &binary, &scope, false).agents[0].status,
        AgentStatus::Upgraded
    );
    assert_eq!(fs::read(extension(root)).unwrap(), original);
    assert_eq!(
        run_uninstall(&installers, &binary, &scope, true).agents[0].status,
        AgentStatus::WouldRemove
    );
    assert_eq!(fs::read(extension(root)).unwrap(), original);

    // Removal and status must not require the previously installed executable.
    let missing = Path::new("rusty-brain-missing-hooks-for-uninstall");
    assert_eq!(
        run_uninstall(&installers, missing, &scope, false).agents[0].status,
        AgentStatus::Removed
    );
    assert!(!extension(root).exists());
    assert_eq!(
        run_status(&installers, missing, &scope).agents[0].status,
        AgentStatus::Absent
    );
    assert_eq!(
        run_uninstall(&installers, missing, &scope, false).agents[0].status,
        AgentStatus::Removed
    );
    assert_eq!(
        fs::read_to_string(neighbor).unwrap(),
        "export default function user() {}\n"
    );
    assert_eq!(
        fs::read_to_string(settings).unwrap(),
        "{\"model\":\"keep\"}\n"
    );
}

#[test]
fn dry_run_leaves_a_fresh_project_untouched() {
    let project = tempfile::tempdir().unwrap();
    let installers = installers();
    let scope = scope(project.path());
    let binary = hooks(project.path());
    assert_eq!(
        run_install(&installers, &binary, &scope, true).agents[0].status,
        AgentStatus::WouldConfigure
    );
    assert_eq!(
        run_uninstall(&installers, &binary, &scope, true).agents[0].status,
        AgentStatus::WouldRemove
    );
    assert!(!project.path().join(".omp").exists());
}

#[test]
fn unowned_extension_survives_install_and_uninstall_including_dry_run() {
    let project = tempfile::tempdir().unwrap();
    let path = extension(project.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Even identical runtime source is unowned unless the installer claimed it.
    let fragment = OmpInstaller
        .hook_fragment(&hooks(project.path()), &scope(project.path()))
        .unwrap();
    let user_source = fragment.owned_asset.unwrap().contents;
    fs::write(&path, &user_source).unwrap();
    let installers = installers();
    for dry_run in [true, false] {
        assert_eq!(
            run_install(
                &installers,
                &hooks(project.path()),
                &scope(project.path()),
                dry_run
            )
            .agents[0]
                .status,
            AgentStatus::Failed
        );
        assert_eq!(
            run_uninstall(
                &installers,
                &hooks(project.path()),
                &scope(project.path()),
                dry_run
            )
            .agents[0]
                .status,
            AgentStatus::Failed
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), user_source);
    }
}

#[test]
fn modified_managed_extension_reports_drift_and_is_never_replaced_or_deleted() {
    let project = tempfile::tempdir().unwrap();
    let installers = installers();
    let scope = scope(project.path());
    let binary = hooks(project.path());
    assert_eq!(
        run_install(&installers, &binary, &scope, false).agents[0].status,
        AgentStatus::Configured
    );
    let path = extension(project.path());
    let mut changed = fs::read_to_string(&path).unwrap();
    changed.push_str("\n// user's local customization\n");
    fs::write(&path, &changed).unwrap();
    let status = run_status(&installers, &binary, &scope);
    assert_eq!(status.agents[0].status, AgentStatus::Drifted);
    assert_eq!(status.status, ReportStatus::Failed);
    for dry_run in [true, false] {
        assert_eq!(
            run_install(&installers, &binary, &scope, dry_run).agents[0].status,
            AgentStatus::Failed
        );
        assert_eq!(
            run_uninstall(&installers, &binary, &scope, dry_run).agents[0].status,
            AgentStatus::Failed
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
    }
}

#[test]
fn owned_previous_source_upgrades_without_a_historical_bundle() {
    struct PreviousOmp;
    impl AgentInstaller for PreviousOmp {
        fn id(&self) -> AgentId {
            AgentId::Omp
        }
        fn detect(&self) -> Option<String> {
            Some("1.0.0".to_string())
        }
        fn hook_fragment(
            &self,
            hooks: &Path,
            scope: &InstallScope,
        ) -> rb_types::Result<HookFragment> {
            let mut fragment = OmpInstaller.hook_fragment(hooks, scope)?;
            fragment.owned_asset.as_mut().unwrap().contents =
                "export default function previous() {}\n".to_string();
            Ok(fragment)
        }
    }
    let project = tempfile::tempdir().unwrap();
    let scope = scope(project.path());
    let binary = hooks(project.path());
    let previous: Vec<Box<dyn AgentInstaller>> = vec![Box::new(PreviousOmp)];
    assert_eq!(
        run_install(&previous, &binary, &scope, false).agents[0].status,
        AgentStatus::Configured
    );
    let before = fs::read(extension(project.path())).unwrap();
    let current = installers();
    assert_eq!(
        run_status(&current, &binary, &scope).agents[0].status,
        AgentStatus::Present
    );
    assert_eq!(
        run_install(&current, &binary, &scope, false).agents[0].status,
        AgentStatus::Upgraded
    );
    assert_ne!(fs::read(extension(project.path())).unwrap(), before);
    assert_eq!(
        run_status(&current, &binary, &scope).agents[0].status,
        AgentStatus::Present
    );
    assert_eq!(
        run_uninstall(&current, &binary, &scope, false).agents[0].status,
        AgentStatus::Removed
    );
}

#[test]
fn global_scope_is_explicitly_unsupported_for_every_operation() {
    let installers = installers();
    let missing = Path::new("missing-hooks");
    for dry_run in [true, false] {
        assert_eq!(
            run_install(&installers, missing, &InstallScope::Global, dry_run).agents[0].status,
            AgentStatus::Failed
        );
        assert_eq!(
            run_uninstall(&installers, missing, &InstallScope::Global, dry_run).agents[0].status,
            AgentStatus::Failed
        );
    }
    assert_eq!(
        run_status(&installers, missing, &InstallScope::Global).agents[0].status,
        AgentStatus::Failed
    );
}

#[cfg(unix)]
#[test]
fn symlinked_asset_components_never_modify_external_files() {
    use std::os::unix::fs::symlink;
    for component in [".omp", ".omp/extensions", ".omp/extensions/rusty-brain.ts"] {
        let project = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::create_dir_all(external.path().join("extensions")).unwrap();
        let external_file = external.path().join("extensions/rusty-brain.ts");
        fs::write(&external_file, "external user source\n").unwrap();
        let destination = project.path().join(component);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let target = match component {
            ".omp" => external.path().to_path_buf(),
            ".omp/extensions" => external.path().join("extensions"),
            _ => external_file.clone(),
        };
        symlink(target, &destination).unwrap();
        let installers = installers();
        let scope = scope(project.path());
        let binary = hooks(project.path());
        for dry_run in [true, false] {
            assert_eq!(
                run_install(&installers, &binary, &scope, dry_run).agents[0].status,
                AgentStatus::Failed
            );
            assert_eq!(
                run_uninstall(&installers, &binary, &scope, dry_run).agents[0].status,
                AgentStatus::Failed
            );
        }
        assert_eq!(
            run_status(&installers, &binary, &scope).agents[0].status,
            AgentStatus::Failed
        );
        assert!(fs::symlink_metadata(destination)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(external_file).unwrap(),
            "external user source\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn dangling_asset_symlink_is_not_treated_as_absent() {
    use std::os::unix::fs::symlink;
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let path = extension(project.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let target = outside.path().join("absent.ts");
    symlink(&target, &path).unwrap();
    let installers = installers();
    let scope = scope(project.path());
    let binary = hooks(project.path());
    assert_eq!(
        run_install(&installers, &binary, &scope, false).agents[0].status,
        AgentStatus::Failed
    );
    assert_eq!(
        run_uninstall(&installers, &binary, &scope, false).agents[0].status,
        AgentStatus::Failed
    );
    assert_eq!(fs::read_link(path).unwrap(), target);
    assert!(!target.exists());
}
