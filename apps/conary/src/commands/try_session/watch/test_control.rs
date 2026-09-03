// apps/conary/src/commands/try_session/watch/test_control.rs

//! Private deterministic controls for watch-mode integration tests.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

pub(super) fn pause_refresh_cook(is_refresh: bool) -> Result<()> {
    if !is_refresh || !crate::test_hooks::get().try_watch_pause_during_cook() {
        return Ok(());
    }
    if let Some(path) = crate::test_hooks::get().try_watch_cook_started_file() {
        let path = PathBuf::from(path);
        fs::write(&path, b"started\n").with_context(|| {
            format!(
                "failed to write try watch cook-started test marker {}",
                path.display()
            )
        })?;
    }
    std::thread::sleep(Duration::from_millis(1200));
    Ok(())
}
