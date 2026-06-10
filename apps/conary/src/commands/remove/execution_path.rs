// apps/conary/src/commands/remove/execution_path.rs

use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoveExecutionPath {
    GenerationAware,
    MutableLiveRoot,
}

pub(super) fn remove_execution_path(db_path: &str) -> Result<RemoveExecutionPath> {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(PathBuf::from(db_path));
    let current_link = runtime_root.root().join("current");
    let has_current_link = match std::fs::symlink_metadata(&current_link) {
        Ok(metadata) if metadata.file_type().is_symlink() && !current_link.exists() => {
            let target = std::fs::read_link(&current_link)
                .with_context(|| format!("Failed to read {}", current_link.display()))?;
            anyhow::bail!(
                "current generation symlink {} -> {} is dangling",
                current_link.display(),
                target.display()
            );
        }
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Failed to inspect {}", current_link.display()));
        }
    };
    if !has_current_link && std::env::var_os("CONARY_TEST_SKIP_GENERATION_MOUNT").is_some() {
        return Ok(RemoveExecutionPath::GenerationAware);
    }
    let current = conary_core::generation::mount::current_generation(runtime_root.root())?;
    Ok(match current {
        Some(_) => RemoveExecutionPath::GenerationAware,
        None => RemoveExecutionPath::MutableLiveRoot,
    })
}
