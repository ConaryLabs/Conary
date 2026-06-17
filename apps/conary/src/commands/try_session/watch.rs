// apps/conary/src/commands/try_session/watch.rs
//! Watch-mode try-session orchestration.

use anyhow::{Result, bail};

pub(super) struct TryWatchOptions<'a> {
    pub(super) db_path: &'a str,
    pub(super) target: &'a str,
    pub(super) recipe: Option<&'a str>,
    pub(super) json: bool,
}

pub(super) async fn cmd_try_watch(options: TryWatchOptions<'_>) -> Result<()> {
    let _ = (
        options.db_path,
        options.target,
        options.recipe,
        options.json,
    );
    bail!("conary try --watch is not wired yet")
}
