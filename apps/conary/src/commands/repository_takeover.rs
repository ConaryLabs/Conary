// apps/conary/src/commands/repository_takeover.rs

//! CLI boundary for native repository takeover preview, apply, and rollback.

use crate::ui;
use anyhow::{Context, Result, bail};
use conary_core::repository::declarations::takeover::{
    NativeRepositoryTakeoverManifest, NativeRepositoryTakeoverPreviewEnvelope, TakeoverApplyStatus,
    apply_native_repository_takeover, preview_native_repository_takeover,
    rollback_native_repository_takeover,
};
use std::path::Path;

pub fn cmd_repository_takeover(
    db_path: &str,
    root: &str,
    manifest_path: Option<&Path>,
    expected_preview_sha256: Option<&str>,
    dry_run: bool,
    rollback: bool,
) -> Result<()> {
    let _runtime_lock = (!dry_run)
        .then(|| super::generation::selected_root::LockedRuntimeRoot::acquire(db_path))
        .transpose()
        .context("acquire repository takeover mutation lock")?;
    let conn = if dry_run {
        conary_core::db::open_read_only(db_path)
            .context("open current Conary database read-only for takeover preview")?
    } else {
        conary_core::db::open(db_path).context("open current Conary database")?
    };
    if rollback {
        rollback_native_repository_takeover(&conn, root)
            .context("roll back native repository takeover")?;
        ui::status("Rolled back", "native repository takeover");
        return Ok(());
    }

    let manifest_path = manifest_path
        .ok_or_else(|| anyhow::anyhow!("--manifest is required unless --rollback is used"))?;
    let bytes = std::fs::read(manifest_path)
        .with_context(|| format!("read takeover manifest {}", manifest_path.display()))?;
    let manifest: NativeRepositoryTakeoverManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("decode takeover manifest {}", manifest_path.display()))?;
    let preview = preview_native_repository_takeover(&conn, root, &manifest)
        .context("preview native repository takeover")?;
    let envelope = NativeRepositoryTakeoverPreviewEnvelope::new(preview)?;
    let digest = envelope.preview_sha256.clone();
    println!("{}", serde_json::to_string_pretty(&envelope)?);
    ui::field("Preview SHA-256", &digest);
    if dry_run {
        if !envelope.preview.blockers.is_empty() {
            ui::warn(&format!(
                "takeover preview has {} blocker(s)",
                envelope.preview.blockers.len()
            ));
        }
        return Ok(());
    }
    let expected = expected_preview_sha256.ok_or_else(|| {
        anyhow::anyhow!(
            "--preview-sha256 is required for apply; run --dry-run and inspect the exact preview first"
        )
    })?;
    if expected != digest {
        bail!("provided preview SHA-256 {expected} does not match current preview {digest}");
    }
    let outcome = apply_native_repository_takeover(&conn, root, &manifest, expected)
        .context("apply native repository takeover")?;
    match outcome.status {
        TakeoverApplyStatus::Applied => ui::status(
            "Applied",
            &format!(
                "native repository takeover for {} repository row(s)",
                outcome.repository_ids.len()
            ),
        ),
        TakeoverApplyStatus::AlreadyApplied => ui::status(
            "Verified",
            &format!(
                "existing native repository takeover for {} repository row(s)",
                outcome.repository_ids.len()
            ),
        ),
    }
    Ok(())
}
