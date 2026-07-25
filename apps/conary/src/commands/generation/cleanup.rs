// apps/conary/src/commands/generation/cleanup.rs
//! Generation-owned cleanup of persistent `/etc` overlay directories.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

pub(super) fn etc_state_paths(conary_root: &Path, generation: i64) -> [PathBuf; 2] {
    [
        conary_root.join(format!("etc-state/{generation}")),
        conary_root.join(format!("etc-state/{generation}-work")),
    ]
}

pub(super) fn remove_generation_etc_state(conary_root: &Path, generation: i64) -> Result<()> {
    for path in etc_state_paths(conary_root, generation) {
        if !path.exists() {
            continue;
        }

        std::fs::remove_dir_all(&path)
            .map_err(|error| anyhow!("failed to remove {}: {error}", path.display()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{etc_state_paths, remove_generation_etc_state};

    #[test]
    fn remove_generation_etc_state_deletes_both_overlay_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let conary_root = tmp.path();
        let [upper, work] = etc_state_paths(conary_root, 7);
        std::fs::create_dir_all(&upper).unwrap();
        std::fs::create_dir_all(&work).unwrap();

        remove_generation_etc_state(conary_root, 7).unwrap();

        assert!(!upper.exists());
        assert!(!work.exists());
    }

    #[test]
    fn remove_generation_etc_state_is_noop_when_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        remove_generation_etc_state(tmp.path(), 11).unwrap();
    }
}
