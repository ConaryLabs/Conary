// conary-test/src/paths.rs

use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

const WORKSPACE_REMI_ROOT: &str = "apps/conary/tests/integration/remi";
const WORKSPACE_FIXTURES_ROOT: &str = "apps/conary/tests/fixtures";
pub(crate) const FIXTURE_CCS_KEY_RELATIVE: &str = "ccs-test-authority/fixture-signing-key.private";
pub(crate) const FIXTURE_CCS_PUBLIC_KEY_RELATIVE: &str =
    "ccs-test-authority/fixture-signing-key.public";
pub(crate) const FIXTURE_CCS_POLICY_RELATIVE: &str = "ccs-test-authority/trust-policy.toml";
pub(crate) const FIXTURE_CCS_EXPIRED_POLICY_RELATIVE: &str =
    "ccs-test-authority/expired-trust-policy.toml";
const DEFAULT_STATE_SUBDIR: &str = ".local/state/conary-test";
const ROLLOUT_PROVENANCE_FILE: &str = "forge-rollout.json";

fn find_workspace_root_from(start: &Path) -> Option<PathBuf> {
    let mut candidate = start.to_path_buf();
    let mut first_manifest_root = None;

    loop {
        let manifest = candidate.join("Cargo.toml");
        if manifest.is_file() {
            if fs::read_to_string(&manifest)
                .map(|contents| contents.contains("[workspace]"))
                .unwrap_or(false)
            {
                return Some(candidate);
            }

            if first_manifest_root.is_none() {
                first_manifest_root = Some(candidate.clone());
            }
        }

        if !candidate.pop() {
            return first_manifest_root;
        }
    }
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
    Ok(project_dir()?.join(WORKSPACE_REMI_ROOT))
}

pub fn fixtures_root() -> Result<PathBuf> {
    Ok(project_dir()?.join(WORKSPACE_FIXTURES_ROOT))
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
    project_root.join(WORKSPACE_FIXTURES_ROOT)
}

pub(crate) fn fixture_ccs_key_path_for(fixtures_root: &Path) -> PathBuf {
    fixtures_root.join(FIXTURE_CCS_KEY_RELATIVE)
}

pub(crate) fn fixture_ccs_public_key_path_for(fixtures_root: &Path) -> PathBuf {
    fixtures_root.join(FIXTURE_CCS_PUBLIC_KEY_RELATIVE)
}

pub(crate) fn fixture_ccs_policy_path_for(fixtures_root: &Path) -> PathBuf {
    fixtures_root.join(FIXTURE_CCS_POLICY_RELATIVE)
}

pub(crate) fn fixture_ccs_expired_policy_path_for(fixtures_root: &Path) -> PathBuf {
    fixtures_root.join(FIXTURE_CCS_EXPIRED_POLICY_RELATIVE)
}

pub(crate) fn rollout_provenance_path_for(state_dir: &Path) -> PathBuf {
    state_dir.join(ROLLOUT_PROVENANCE_FILE)
}

fn sibling_conary_binary_for(executable: &Path) -> Option<PathBuf> {
    let parent = executable.parent()?;
    let profile_dir = if parent.file_name().is_some_and(|name| name == "deps") {
        parent.parent()?
    } else {
        parent
    };

    Some(profile_dir.join(format!("conary{}", std::env::consts::EXE_SUFFIX)))
}

pub(crate) fn find_host_conary_binary(project_root: &Path) -> Result<PathBuf> {
    let sibling_target_binary = std::env::current_exe()
        .ok()
        .and_then(|executable| sibling_conary_binary_for(&executable));
    let candidates = [
        std::env::var_os("CONARY_HOST_BIN").map(PathBuf::from),
        std::env::var_os("CONARY_BIN").map(PathBuf::from),
        Some(project_root.join("conary")),
        sibling_target_binary,
        Some(project_root.join("target/debug/conary")),
        Some(project_root.join("target/release/conary")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "failed to locate host conary binary; tried CONARY_HOST_BIN, CONARY_BIN, ./conary, the current executable's Cargo profile directory, target/debug/conary, and target/release/conary"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_root(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("conary-test-paths-{label}-{unique}"))
    }

    #[test]
    fn resolve_fixtures_root_uses_workspace_app_layout() {
        let root = unique_temp_root("workspace-fixtures");
        let resolved = resolve_fixtures_root_for(&root);
        assert_eq!(resolved, root.join(WORKSPACE_FIXTURES_ROOT));
    }

    #[test]
    fn fixture_ccs_authority_paths_share_the_fixture_root() {
        let root = Path::new("/opt/remi-tests/fixtures");

        assert_eq!(
            fixture_ccs_key_path_for(root),
            root.join("ccs-test-authority/fixture-signing-key.private")
        );
        assert_eq!(
            fixture_ccs_public_key_path_for(root),
            root.join("ccs-test-authority/fixture-signing-key.public")
        );
        assert_eq!(
            fixture_ccs_policy_path_for(root),
            root.join("ccs-test-authority/trust-policy.toml")
        );
        assert_eq!(
            fixture_ccs_expired_policy_path_for(root),
            root.join("ccs-test-authority/expired-trust-policy.toml")
        );
    }

    #[test]
    fn rollout_provenance_path_uses_state_directory() {
        let state_root = unique_temp_root("state-root");
        let provenance_path = rollout_provenance_path_for(&state_root);

        assert_eq!(provenance_path, state_root.join(ROLLOUT_PROVENANCE_FILE));
    }

    #[test]
    fn sibling_conary_binary_uses_shared_cargo_profile_directory() {
        let profile_dir = Path::new("/cache/target/Conary/debug");
        let expected = profile_dir.join(format!("conary{}", std::env::consts::EXE_SUFFIX));

        assert_eq!(
            sibling_conary_binary_for(
                &profile_dir.join(format!("conary-test{}", std::env::consts::EXE_SUFFIX))
            ),
            Some(expected.clone())
        );
        assert_eq!(
            sibling_conary_binary_for(&profile_dir.join("deps/conary_test-hash")),
            Some(expected)
        );
    }
}
