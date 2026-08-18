// apps/conary/src/commands/try_session/watch/test_control.rs

//! Private deterministic controls for watch-mode integration tests.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

pub(super) fn pause_refresh_cook(is_refresh: bool) -> Result<()> {
    if !is_refresh || std::env::var_os("CONARY_TEST_TRY_WATCH_PAUSE_DURING_COOK").is_none() {
        return Ok(());
    }
    if let Some(path) = std::env::var_os("CONARY_TEST_TRY_WATCH_COOK_STARTED_FILE") {
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
