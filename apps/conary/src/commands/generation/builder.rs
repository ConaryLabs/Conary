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
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

pub(crate) fn requested_generation_verity(digest: Option<&str>, fsverity_enabled: bool) -> bool {
    fsverity_enabled && digest.is_some()
}

pub(crate) fn enable_generation_rootfs_verity(gen_dir: &Path, image_path: &Path) -> Result<()> {
    enable_generation_rootfs_verity_with(gen_dir, image_path, enable_fsverity)
}

fn ensure_generation_cas_layout(generations_root: &Path, objects_dir: &Path) -> Result<()> {
    let artifact_root = generations_root.parent().ok_or_else(|| {
        anyhow!(
            "Generation root {} has no parent artifact root",
            generations_root.display()
        )
    })?;
    let contract_objects = artifact_root.join("objects");

    std::fs::create_dir_all(artifact_root).with_context(|| {
        format!(
            "Failed to create generation artifact root {}",
            artifact_root.display()
        )
    })?;
    std::fs::create_dir_all(objects_dir).with_context(|| {
        format!(
            "Failed to create CAS objects directory {}",
            objects_dir.display()
        )
    })?;

    let real_objects = std::fs::canonicalize(objects_dir).with_context(|| {
        format!(
            "Failed to resolve CAS objects directory {}",
            objects_dir.display()
        )
    })?;

    match std::fs::symlink_metadata(&contract_objects) {
        Ok(_) => {
            let resolved = std::fs::canonicalize(&contract_objects).with_context(|| {
                format!(
                    "Generation CAS contract path {} exists but does not resolve",
                    contract_objects.display()
                )
            })?;
            if resolved != real_objects {
                return Err(anyhow!(
                    "Generation CAS contract path {} does not resolve to CAS objects {}; got {}",
                    contract_objects.display(),
                    real_objects.display(),
                    resolved.display()
                ));
            }
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Failed to inspect generation CAS contract path {}",
                    contract_objects.display()
                )
            });
        }
    }

    #[cfg(unix)]
    {
        let artifact_root = std::fs::canonicalize(artifact_root).with_context(|| {
            format!(
                "Failed to resolve generation artifact root {}",
                artifact_root.display()
            )
        })?;
        let link_target = relative_symlink_target(&artifact_root, &real_objects)?;
        std::os::unix::fs::symlink(&link_target, &contract_objects).with_context(|| {
            format!(
                "Failed to link generation CAS contract path {} -> {}",
                contract_objects.display(),
                link_target.display()
            )
        })?;

        let resolved = std::fs::canonicalize(&contract_objects).with_context(|| {
            format!(
                "Failed to resolve generated CAS contract link {}",
                contract_objects.display()
            )
        })?;
        if resolved != real_objects {
            return Err(anyhow!(
                "Generated CAS contract link {} resolves to {}, expected {}",
                contract_objects.display(),
                resolved.display(),
                real_objects.display()
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        Err(anyhow!(
            "Generation CAS contract path {} is missing and non-Unix platforms cannot create the required symlink to {}",
            contract_objects.display(),
            real_objects.display()
        ))
    }
}

#[cfg(unix)]
fn relative_symlink_target(from_dir: &Path, target: &Path) -> Result<PathBuf> {
    let from_components = absolute_normal_components(from_dir).ok_or_else(|| {
        anyhow!(
            "Cannot compute relative CAS symlink from non-normal path {}",
            from_dir.display()
        )
    })?;
    let target_components = absolute_normal_components(target).ok_or_else(|| {
        anyhow!(
            "Cannot compute relative CAS symlink to non-normal path {}",
            target.display()
        )
    })?;

    let common = from_components
        .iter()
        .zip(target_components.iter())
        .take_while(|(left, right)| left == right)
        .count();

    let mut relative = PathBuf::new();
    for _ in common..from_components.len() {
        relative.push("..");
    }
    for component in &target_components[common..] {
        relative.push(component);
    }
    if relative.as_os_str().is_empty() {
        relative.push(".");
    }
    Ok(relative)
}

#[cfg(unix)]
fn absolute_normal_components(path: &Path) -> Option<Vec<std::ffi::OsString>> {
    let mut saw_root = false;
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::RootDir => saw_root = true,
            std::path::Component::Normal(value) => components.push(value.to_os_string()),
            _ => return None,
        }
    }

    saw_root.then_some(components)
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
    let generations_root = generations_dir();
    ensure_generation_cas_layout(&generations_root, &obj_dir)
        .context("Failed to prepare generation CAS layout")?;
    let caps = preflight_composefs(&obj_dir).context("Composefs preflight failed")?;

    // Step 2: Delegate to core builder
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

    #[cfg(unix)]
    #[test]
    fn ensure_generation_cas_layout_links_contract_objects_to_real_cas() {
        let tmp = TempDir::new().unwrap();
        let generations_root = tmp.path().join("conary/generations");
        let objects_dir = tmp.path().join("var/lib/conary/objects");
        std::fs::create_dir_all(&objects_dir).unwrap();

        ensure_generation_cas_layout(&generations_root, &objects_dir).unwrap();

        let contract_objects = tmp.path().join("conary/objects");
        let link_target = std::fs::read_link(&contract_objects).unwrap();
        assert!(
            !link_target.is_absolute(),
            "contract CAS link must be relative so initramfs /sysroot access stays inside the booted root"
        );
        assert_eq!(
            std::fs::canonicalize(&contract_objects).unwrap(),
            std::fs::canonicalize(&objects_dir).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_generation_cas_layout_rejects_conflicting_contract_objects() {
        let tmp = TempDir::new().unwrap();
        let generations_root = tmp.path().join("conary/generations");
        let objects_dir = tmp.path().join("var/lib/conary/objects");
        let wrong_objects = tmp.path().join("wrong/objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        std::fs::create_dir_all(&wrong_objects).unwrap();
        std::fs::create_dir_all(tmp.path().join("conary")).unwrap();
        std::os::unix::fs::symlink("../wrong/objects", tmp.path().join("conary/objects")).unwrap();

        let err = ensure_generation_cas_layout(&generations_root, &objects_dir).unwrap_err();

        assert!(err.to_string().contains("does not resolve to CAS objects"));
    }
}
