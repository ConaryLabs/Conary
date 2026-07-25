// apps/conary/src/commands/remove/ccs_hook.rs

//! Exact installed CCS pre-remove hook loading, validation, and execution.

use anyhow::{Context, Result};
use conary_core::db::models::{InstalledCcsRemoveHook, Trove};
use conary_core::repository::versioning::VersionScheme;
use conary_core::scriptlet::{PackageFormat, SandboxMode, ScriptletExecutor};
use std::path::Path;
use tracing::info;

pub(crate) fn preflight_ccs_remove_hook(
    conn: &rusqlite::Connection,
    trove: &Trove,
    root: &str,
    sandbox_mode: SandboxMode,
) -> Result<Option<InstalledCcsRemoveHook>> {
    let hook = load_ccs_remove_hook(conn, trove)?;
    let Some(hook) = hook else {
        return Ok(None);
    };
    preflight_loaded_ccs_remove_hook(trove, &hook, Path::new(root), sandbox_mode)?;
    Ok(Some(hook))
}

pub(crate) fn load_ccs_remove_hook(
    conn: &rusqlite::Connection,
    trove: &Trove,
) -> Result<Option<InstalledCcsRemoveHook>> {
    let trove_id = trove.id.context("CCS remove-hook owner has no trove id")?;
    let hook = InstalledCcsRemoveHook::find_by_trove(conn, trove_id)?;
    let Some(hook) = hook else {
        return Ok(None);
    };
    let version_scheme = trove.version_scheme;
    if version_scheme != VersionScheme::Conary {
        anyhow::bail!(
            "installed CCS remove hook for '{}' conflicts with package version scheme '{}'",
            trove.name,
            version_scheme.as_str()
        );
    }
    Ok(Some(hook))
}

pub(crate) fn preflight_loaded_ccs_remove_hook(
    trove: &Trove,
    hook: &InstalledCcsRemoveHook,
    root: &Path,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    executor(trove, root, sandbox_mode).preflight_ccs_remove_hook(hook)?;
    Ok(())
}

pub(crate) fn execute_preflighted_ccs_remove_hook(
    trove: &Trove,
    hook: &InstalledCcsRemoveHook,
    root: impl AsRef<Path>,
    sandbox_mode: SandboxMode,
) -> Result<()> {
    info!(package = trove.name, "Running CCS pre-remove hook");
    executor(trove, root.as_ref(), sandbox_mode).execute_ccs_remove_hook(hook)?;
    Ok(())
}

fn executor(trove: &Trove, root: &Path, sandbox_mode: SandboxMode) -> ScriptletExecutor {
    ScriptletExecutor::new(root, &trove.name, &trove.version, PackageFormat::Conary)
        .with_sandbox_mode(sandbox_mode)
}
