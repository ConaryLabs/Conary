// conary-test/src/paths.rs

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const WORKSPACE_REMI_ROOT: &str = "apps/conary/tests/integration/remi";
const LEGACY_REMI_ROOT: &str = "tests/integration/remi";
const WORKSPACE_FIXTURES_ROOT: &str = "apps/conary/tests/fixtures";
const LEGACY_FIXTURES_ROOT: &str = "tests/fixtures";
const DEFAULT_STATE_SUBDIR: &str = ".local/state/conary-test";
const ROLLOUT_PROVENANCE_FILE: &str = "forge-rollout.json";

fn find_workspace_root_from(start: &Path) -> Option<PathBuf> {
    let mut candidate = start.to_path_buf();
    loop {
        if candidate.join("Cargo.toml").is_file() {
            return Some(candidate);
        }
        if !candidate.pop() {
            return None;
        }
    }
}

fn resolve_layout_root(project_root: &Path, preferred: &str, legacy: &str) -> PathBuf {
    let preferred_path = project_root.join(preferred);
    if preferred_path.exists() {
        return preferred_path;
    }

    let legacy_path = project_root.join(legacy);
    if legacy_path.exists() {
        return legacy_path;
    }

    preferred_path
}

pub fn project_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("CONARY_PROJECT_DIR") {
        return Ok(PathBuf::from(dir));
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(root) = find_workspace_root_from(&exe)
    {
        return Ok(root);
    }

    let cwd = std::env::current_dir().context("cannot determine project directory")?;
    if let Some(root) = find_workspace_root_from(&cwd) {
        Ok(root)
    } else {
        Ok(cwd)
    }
}

pub fn remi_integration_root() -> Result<PathBuf> {
    Ok(resolve_layout_root(
        &project_dir()?,
        WORKSPACE_REMI_ROOT,
        LEGACY_REMI_ROOT,
    ))
}

pub fn fixtures_root() -> Result<PathBuf> {
    Ok(resolve_layout_root(
        &project_dir()?,
        WORKSPACE_FIXTURES_ROOT,
        LEGACY_FIXTURES_ROOT,
    ))
}

pub fn default_config_path() -> Result<PathBuf> {
    Ok(remi_integration_root()?.join("config.toml"))
}

pub fn default_manifest_dir() -> Result<PathBuf> {
    Ok(remi_integration_root()?.join("manifests"))
}

pub fn default_container_dir() -> Result<PathBuf> {
    Ok(remi_integration_root()?.join("containers"))
}

pub fn default_results_dir() -> Result<PathBuf> {
    Ok(remi_integration_root()?.join("results"))
}

pub fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("CONARY_TEST_STATE_DIR") {
        return Ok(PathBuf::from(dir));
    }

    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(dir).join("conary-test"));
    }

    let home = std::env::var_os("HOME").context("cannot determine conary-test state directory")?;
    Ok(PathBuf::from(home).join(DEFAULT_STATE_SUBDIR))
}

pub fn rollout_provenance_path() -> Result<PathBuf> {
    Ok(rollout_provenance_path_for(&state_dir()?))
}

pub(crate) fn host_conary_binary() -> Result<PathBuf> {
    find_host_conary_binary(&project_dir()?)
}

pub(crate) fn resolve_fixtures_root_for(project_root: &Path) -> PathBuf {
    resolve_layout_root(project_root, WORKSPACE_FIXTURES_ROOT, LEGACY_FIXTURES_ROOT)
}

pub(crate) fn rollout_provenance_path_for(state_dir: &Path) -> PathBuf {
    state_dir.join(ROLLOUT_PROVENANCE_FILE)
}

pub(crate) fn find_host_conary_binary(project_root: &Path) -> Result<PathBuf> {
    let candidates = [
        std::env::var_os("CONARY_HOST_BIN").map(PathBuf::from),
        std::env::var_os("CONARY_BIN").map(PathBuf::from),
        Some(project_root.join("conary")),
        Some(project_root.join("target/debug/conary")),
        Some(project_root.join("target/release/conary")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "failed to locate host conary binary; tried CONARY_HOST_BIN, CONARY_BIN, ./conary, target/debug/conary, and target/release/conary"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("conary-test-paths-{label}-{unique}"))
    }

    #[test]
    fn resolve_remi_root_prefers_workspace_app_layout() {
        let root = unique_temp_root("workspace-remi");
        fs::create_dir_all(root.join(WORKSPACE_REMI_ROOT)).expect("create preferred root");
        fs::create_dir_all(root.join(LEGACY_REMI_ROOT)).expect("create legacy root");

        let resolved = resolve_layout_root(&root, WORKSPACE_REMI_ROOT, LEGACY_REMI_ROOT);
        assert_eq!(resolved, root.join(WORKSPACE_REMI_ROOT));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn resolve_fixtures_root_falls_back_to_legacy_layout() {
        let root = unique_temp_root("legacy-fixtures");
        fs::create_dir_all(root.join(LEGACY_FIXTURES_ROOT)).expect("create legacy root");

        let resolved = resolve_fixtures_root_for(&root);
        assert_eq!(resolved, root.join(LEGACY_FIXTURES_ROOT));

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rollout_provenance_path_uses_state_directory() {
        let state_root = unique_temp_root("state-root");
        let provenance_path = rollout_provenance_path_for(&state_root);

        assert_eq!(provenance_path, state_root.join(ROLLOUT_PROVENANCE_FILE));
    }
}
