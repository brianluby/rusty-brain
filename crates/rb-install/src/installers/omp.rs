//! OMP discovers standalone TypeScript extensions in the current project's
//! `.omp/extensions/` directory. It does not consume JSON command-hook settings.

use std::path::{Path, PathBuf};

use rb_agents::cli::AgentId;
use rb_agents::install::{AgentInstaller, HookFragment, InstallScope};
use rb_types::{Error, Result};

use crate::detect::{find_binary_on_path, version_of};

const SOURCE: &str = include_str!("../../assets/omp-extension.ts");
const HOOKS_DECLARATION: &str = "const INSTALLED_HOOKS_BINARY = \"rusty-brain-hooks\";";

/// Installer for OMP's project-local native extension.
pub struct OmpInstaller;

impl AgentInstaller for OmpInstaller {
    fn id(&self) -> AgentId {
        AgentId::Omp
    }

    fn detect(&self) -> Option<String> {
        let bin = find_binary_on_path("omp")?;
        version_of(&bin).or_else(|| Some(String::new()))
    }

    fn hook_fragment(&self, hooks_bin: &Path, scope: &InstallScope) -> Result<HookFragment> {
        let InstallScope::Project(root) = scope else {
            return Err(Error::Io(
                "[E_INSTALL_SCOPE_UNSUPPORTED] OMP supports project installs only; run without --global in the project directory".to_string(),
            ));
        };
        let root = root.canonicalize().map_err(|e| Error::Io(e.to_string()))?;
        let binary = absolute_hooks_path(hooks_bin)?;
        let binary = binary
            .to_str()
            .ok_or_else(|| Error::Io("OMP hooks binary path must be valid UTF-8".to_string()))?;
        let literal =
            serde_json::to_string(binary).map_err(|e| Error::Serialization(e.to_string()))?;
        if SOURCE.matches(HOOKS_DECLARATION).count() != 1 {
            return Err(Error::Io(
                "embedded OMP extension has an invalid hooks declaration".to_string(),
            ));
        }
        let contents = SOURCE.replacen(
            HOOKS_DECLARATION,
            &format!("const INSTALLED_HOOKS_BINARY = {literal};"),
            1,
        );
        Ok(HookFragment::asset(
            root.join(".omp/extensions/rusty-brain.ts"),
            root,
            contents,
        ))
    }
}

fn absolute_hooks_path(path: &Path) -> Result<PathBuf> {
    let resolved = if path.components().count() == 1 {
        path.to_str().and_then(find_binary_on_path)
    } else {
        None
    };
    // Do not canonicalize the binary: an absolute symlink installed by a package
    // manager should keep following future binary upgrades. Status/uninstall must
    // also work after the executable has been removed.
    std::path::absolute(resolved.as_deref().unwrap_or(path)).map_err(|e| Error::Io(e.to_string()))
}
