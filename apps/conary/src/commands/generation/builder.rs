// src/commands/generation/builder.rs
//! Generation builder — CLI wrapper around conary_core::generation::builder.
//!
//! This module handles CLI-specific concerns (convergence intent checks,
//! composefs preflight, fs-verity enablement, progress output) and delegates
//! the actual EROFS image building to the core library.

use super::composefs::preflight_composefs;
use super::metadata::{GenerationMetadata, generation_path, generations_dir};
use anyhow::{Context, Result, anyhow};
use conary_core::db::models::{InstallSource, Trove};
use conary_core::db::paths::objects_dir;
use conary_core::filesystem::fsverity::{FsVerityError, enable_fsverity};
use conary_core::generation::builder as core_builder;
use conary_core::model;
use conary_core::model::ConvergenceIntent;
use std::path::Path;
use tracing::{debug, info, warn};

pub(crate) fn requested_generation_verity(digest: Option<&str>, fsverity_enabled: bool) -> bool {
    fsverity_enabled && digest.is_some()
}

pub(crate) fn enable_generation_rootfs_verity(gen_dir: &Path, image_path: &Path) -> Result<()> {
    enable_generation_rootfs_verity_with(gen_dir, image_path, enable_fsverity)
}

fn enable_generation_rootfs_verity_with<F>(
    gen_dir: &Path,
    image_path: &Path,
    enable: F,
) -> Result<()>
where
    F: FnOnce(&Path) -> std::result::Result<bool, FsVerityError>,
{
    let enable_outcome = enable(image_path).map_err(|error| match error {
        FsVerityError::Open { path, source } => anyhow!(
            "Failed to open generation image {} for fs-verity enablement: {source}",
            path.display()
        ),
        FsVerityError::NotSupported(path) => anyhow!(
            "Generation image {} is on a filesystem without fs-verity support; refusing to build a composefs generation that would remount without truthful verity protection",
            path.display()
        ),
        FsVerityError::IoctlFailed { path, source } => anyhow!(
            "Failed to enable fs-verity on generation image {}: {source}",
            path.display()
        ),
    })?;

    let mut metadata = GenerationMetadata::read_from(gen_dir).with_context(|| {
        format!(
            "Failed to read generation metadata from {}",
            gen_dir.display()
        )
    })?;
    metadata.fsverity_enabled = true;
    metadata.write_to(gen_dir).with_context(|| {
        format!(
            "Failed to update generation metadata after enabling fs-verity on {}",
            image_path.display()
        )
    })?;

    if enable_outcome {
        info!(
            "Enabled fs-verity on generation image {}",
            image_path.display()
        );
    } else {
        debug!(
            "Generation image {} already had fs-verity enabled",
            image_path.display()
        );
    }

    Ok(())
}

/// Build a new generation as an EROFS image from the current system state.
///
/// This is the CLI entry point that wraps `conary_core::generation::builder`.
/// It adds CLI-specific checks (convergence intent, composefs preflight,
/// fs-verity) around the core builder.
pub fn build_generation(conn: &rusqlite::Connection, db_path: &str, summary: &str) -> Result<i64> {
    // Step 0: Check convergence intent -- generation building requires at least CAS-backed
    let convergence = if model::model_exists(None) {
        model::load_model(None)
            .ok()
            .map(|m| m.system.convergence.clone())
    } else {
        None
    };
    if let Some(ref intent) = convergence {
        info!(
            "Convergence intent: {} (target: {})",
            intent.display_name(),
            intent.target_install_source()
        );
        if *intent == ConvergenceIntent::TrackOnly {
            warn!(
                "Convergence intent is 'track-only' -- packages at AdoptedTrack \
                 lack CAS content and will be skipped in the generation image. \
                 Set convergence to 'cas-backed' or 'full-ownership' for complete generations."
            );
        }
    }

    // Check for non-CAS-backed packages that will be excluded from the generation.
    let all_troves = Trove::list_all(conn).unwrap_or_default();
    let track_only_count = all_troves
        .iter()
        .filter(|t| t.install_source == InstallSource::AdoptedTrack)
        .count();
    if track_only_count > 0 {
        warn!(
            "{track_only_count} package(s) are at AdoptedTrack (no CAS content) \
             and may have incomplete file coverage in the generation image. \
             Use 'conary adopt-system --full' or 'conary system adopt --takeover' \
             to promote them along the ownership ladder."
        );
    }

    // Step 1: Composefs preflight check
    let obj_dir = objects_dir(db_path);
    let caps = preflight_composefs(&obj_dir).context("Composefs preflight failed")?;

    // Step 2: Delegate to core builder
    let generations_root = generations_dir();
    let (gen_number, result) =
        core_builder::build_generation_from_db(conn, &generations_root, summary)
            .map_err(|e| anyhow!("Generation build failed: {e}"))?;

    info!(
        "EROFS image built: {} bytes, {} CAS objects",
        result.image_size, result.cas_objects_referenced
    );

    let gen_dir = generation_path(gen_number);
    enable_generation_rootfs_verity(&gen_dir, &result.image_path).with_context(|| {
        format!(
            "Failed to finalize fs-verity on generation image {}",
            result.image_path.display()
        )
    })?;

    // Step 3: Enable fs-verity on CAS objects (if supported)
    if caps.fsverity {
        debug!("fs-verity supported, enabling on CAS objects");
        let (enabled, already, errors) =
            conary_core::filesystem::fsverity::enable_fsverity_on_cas(&obj_dir);
        info!("fs-verity: {enabled} newly enabled, {already} already enabled, {errors} errors");

        // A non-zero error count means some CAS objects could not be
        // protected. Warn rather than hard-fail: the generation is still
        // usable, but integrity verification will be incomplete.
        if errors > 0 {
            warn!(
                "fs-verity: {errors} CAS object(s) could not have verity enabled; \
                 the generation will work but may lack full integrity protection"
            );
        }
    } else {
        debug!("fs-verity not supported on CAS filesystem, skipping");
    }

    Ok(gen_number)
}

// hex_to_digest tests live in conary_core::generation::builder::tests

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_metadata() -> GenerationMetadata {
        GenerationMetadata {
            generation: 7,
            format: conary_core::generation::metadata::GENERATION_FORMAT.to_string(),
            erofs_size: Some(4096),
            cas_objects_referenced: Some(3),
            fsverity_enabled: false,
            erofs_verity_digest: Some("ab".repeat(32)),
            artifact_manifest_sha256: None,
            created_at: "2026-04-21T00:00:00Z".to_string(),
            package_count: 2,
            kernel_version: Some("6.16.1".to_string()),
            summary: "test generation".to_string(),
        }
    }

    #[test]
    fn requested_generation_verity_requires_both_digest_and_fsverity_ready_image() {
        assert!(!requested_generation_verity(None, true));
        assert!(!requested_generation_verity(Some("ab"), false));
        assert!(requested_generation_verity(Some("ab"), true));
    }

    #[test]
    fn enable_generation_rootfs_verity_marks_metadata_enabled_after_success() {
        let tmp = TempDir::new().unwrap();
        let gen_dir = tmp.path();
        let image_path = gen_dir.join("root.erofs");
        std::fs::write(&image_path, b"erofs-image").unwrap();
        test_metadata().write_to(gen_dir).unwrap();

        enable_generation_rootfs_verity_with(gen_dir, &image_path, |_| Ok(true)).unwrap();

        let metadata = GenerationMetadata::read_from(gen_dir).unwrap();
        assert!(
            metadata.fsverity_enabled,
            "generation metadata must only advertise verity after root.erofs is actually finalized"
        );
    }

    #[test]
    fn enable_generation_rootfs_verity_keeps_metadata_false_on_failure() {
        let tmp = TempDir::new().unwrap();
        let gen_dir = tmp.path();
        let image_path = gen_dir.join("root.erofs");
        std::fs::write(&image_path, b"erofs-image").unwrap();
        test_metadata().write_to(gen_dir).unwrap();

        let err = enable_generation_rootfs_verity_with(gen_dir, &image_path, |_| {
            Err(FsVerityError::NotSupported(image_path.clone()))
        })
        .unwrap_err();

        assert!(err.to_string().contains("fs-verity support"));
        let metadata = GenerationMetadata::read_from(gen_dir).unwrap();
        assert!(
            !metadata.fsverity_enabled,
            "metadata must stay false when root.erofs could not be protected"
        );
    }
}
