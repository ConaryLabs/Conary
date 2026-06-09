// conary-core/src/generation/builder.rs

//! Generation builder — creates EROFS images from system state.
//!
//! This module provides two levels of API:
//!
//! - [`build_erofs_image`]: Low-level function that takes slices of
//!   [`FileEntryRef`] and [`SymlinkEntryRef`] and produces an EROFS image
//!   at the given path. Uses composefs-rs for image building.
//!
//! - [`build_generation_from_db`]: Higher-level function that queries the
//!   database for installed troves and their files, creates a state
//!   snapshot, and builds a complete generation directory with EROFS
//!   image and metadata JSON.

use std::path::Path;

use tracing::info;

use crate::db::models::{FileEntry, Trove};
use crate::generation::artifact::{
    ArtifactWriteInputs, CasObjectVerification, deduplicate_sort_cas_objects,
    write_generation_artifact,
};
use crate::generation::metadata::{
    GENERATION_FORMAT, GenerationMetadata, clear_generation_pending,
};
mod activation;
mod boot_assets;
mod cas;
mod create;
mod erofs;
mod initramfs;
mod kernel;
mod root_validation;
mod runtime_inputs;
mod sysroot;

#[cfg(test)]
pub(super) mod test_support;

use boot_assets::{resolve_generation_boot_asset_sources, stage_runtime_boot_assets_from_sources};
use cas::{cas_objects_from_file_refs, verify_runtime_generation_cas_object_presence};
use root_validation::validate_runtime_generation_root_is_self_contained;
use sysroot::runtime_generation_architecture;

pub use activation::GenerationActivation;
pub use create::{
    build_generation_from_db, build_generation_from_db_with_activation,
    build_generation_from_db_with_boot_root,
    build_generation_from_db_with_boot_root_and_activation,
};
pub use erofs::{BuildResult, FileEntryRef, SymlinkEntryRef, build_erofs_image, hex_to_digest};
pub use kernel::detect_kernel_version_from_troves;
/// Rebuild the EROFS image for an existing generation without allocating a
/// new state number. Used by recovery to restore a generation that was already
/// recorded in the database.
///
/// Unlike [`build_generation_from_db`], this does NOT create a new system state
/// snapshot. It only rebuilds the EROFS image and metadata for the specified
/// generation number, using the current DB package state.
pub(crate) fn rebuild_generation_image(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    gen_number: i64,
    summary: &str,
) -> crate::Result<BuildResult> {
    rebuild_generation_image_with_boot_root(
        conn,
        generations_root,
        gen_number,
        summary,
        Path::new("/boot"),
    )
}

pub(crate) fn rebuild_generation_image_with_boot_root(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    gen_number: i64,
    summary: &str,
    boot_root: &Path,
) -> crate::Result<BuildResult> {
    let gen_dir = generations_root.join(gen_number.to_string());
    std::fs::create_dir_all(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generation directory {}: {e}",
            gen_dir.display()
        ))
    })?;

    let troves = Trove::list_all(conn)?;
    let all_files = FileEntry::find_all_ordered(conn)?;
    let runtime_inputs = runtime_inputs::collect_runtime_generation_inputs(&troves, all_files)?;

    validate_runtime_generation_root_is_self_contained(
        &runtime_inputs.file_refs,
        &runtime_inputs.symlink_refs,
    )?;
    let cas_objects =
        deduplicate_sort_cas_objects(cas_objects_from_file_refs(&runtime_inputs.file_refs))?;
    verify_runtime_generation_cas_object_presence(generations_root, &cas_objects)?;
    let result = build_erofs_image(
        &runtime_inputs.file_refs,
        &runtime_inputs.symlink_refs,
        &gen_dir,
    )?;
    let architecture = runtime_generation_architecture()?;
    let boot_asset_sources = resolve_generation_boot_asset_sources(
        &troves,
        &runtime_inputs,
        generations_root,
        boot_root,
    )?;
    let kernel_version = boot_asset_sources.kernel_version.clone();
    let boot_assets = stage_runtime_boot_assets_from_sources(
        &gen_dir,
        gen_number,
        architecture,
        &boot_asset_sources,
    )?;
    let artifact_manifest_sha256 = write_generation_artifact(ArtifactWriteInputs {
        generation_dir: &gen_dir,
        generation: gen_number,
        architecture,
        erofs_path: &result.image_path,
        cas_base_rel: "../../objects",
        cas_objects,
        cas_verification: CasObjectVerification::AlreadyVerified,
        boot_assets,
    })?;

    #[allow(clippy::cast_possible_wrap)]
    let metadata = GenerationMetadata {
        generation: gen_number,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(result.image_size as i64),
        cas_objects_referenced: Some(result.cas_objects_referenced as i64),
        fsverity_enabled: false,
        erofs_verity_digest: result.erofs_verity_digest.clone(),
        artifact_manifest_sha256: Some(artifact_manifest_sha256),
        created_at: chrono::Utc::now().to_rfc3339(),
        package_count: troves.len() as i64,
        kernel_version: Some(kernel_version),
        summary: summary.to_string(),
    };
    metadata.write_to(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!("Failed to write generation metadata: {e}"))
    })?;
    clear_generation_pending(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to clear pending marker for generation {}: {e}",
            gen_dir.display()
        ))
    })?;

    info!(
        "Generation {} rebuilt in place: {} CAS objects, {} packages ({} metadata-only)",
        gen_number,
        result.cas_objects_referenced,
        troves.len(),
        runtime_inputs.adopted_track_count
    );

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "composefs-rs")]
    use super::test_support::{
        assert_invalid_runtime_input_error, assert_missing_cas_object_error,
        runtime_generation_db_with_invalid_regular_file,
        runtime_generation_db_with_missing_regular_file_cas_object,
    };
    #[cfg(feature = "composefs-rs")]
    #[test]
    fn rebuild_generation_image_rejects_invalid_runtime_input() {
        let (_tmp, conn, generations_root, boot_root) =
            runtime_generation_db_with_invalid_regular_file();

        let error = rebuild_generation_image_with_boot_root(
            &conn,
            &generations_root,
            7,
            "invalid runtime input",
            &boot_root,
        )
        .unwrap_err()
        .to_string();

        assert_invalid_runtime_input_error(&error);
        assert!(!generations_root.join("7/.conary-artifact.json").exists());
    }
    #[cfg(feature = "composefs-rs")]
    #[test]
    fn rebuild_generation_image_rejects_missing_regular_file_cas_object() {
        let (_tmp, conn, generations_root, boot_root, missing_hash) =
            runtime_generation_db_with_missing_regular_file_cas_object();

        let error = rebuild_generation_image_with_boot_root(
            &conn,
            &generations_root,
            7,
            "missing runtime CAS object",
            &boot_root,
        )
        .unwrap_err()
        .to_string();

        assert_missing_cas_object_error(&error, &missing_hash);
        assert!(!generations_root.join("7/.conary-artifact.json").exists());
        assert!(!generations_root.join("7/cas-manifest.json").exists());
    }

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn rebuild_generation_image_clears_stale_pending_marker() {
        use crate::db::models::{FileEntry, Trove, TroveType};
        use crate::db::schema::migrate;
        use crate::filesystem::CasStore;
        use crate::generation::metadata::{is_generation_pending, mark_generation_pending};

        let tmp = tempfile::TempDir::new().unwrap();
        let generations_root = tmp.path().join("generations");
        let objects_dir = tmp.path().join("objects");
        let boot_root = tmp.path().join("boot");
        let gen_dir = generations_root.join("7");
        std::fs::create_dir_all(&gen_dir).unwrap();
        mark_generation_pending(&gen_dir).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
        std::fs::write(boot_root.join("initramfs-6.19.8-conary.img"), b"initramfs").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let cas = CasStore::new(&objects_dir).unwrap();
        let hello_hash = cas.store(b"hello").unwrap();
        let init_hash = cas.store(b"init").unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut trove = Trove::new(
            "kernel-core".to_string(),
            "6.19.8-conary".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = FileEntry::new(
            "/usr/bin/hello".to_string(),
            hello_hash,
            b"hello".len() as i64,
            0o100755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        let mut init = FileEntry::new(
            "/usr/sbin/init".to_string(),
            init_hash,
            b"init".len() as i64,
            0o100755,
            trove_id,
        );
        init.insert(&conn).unwrap();

        rebuild_generation_image_with_boot_root(
            &conn,
            &generations_root,
            7,
            "recovery rebuild",
            &boot_root,
        )
        .unwrap();

        assert!(
            !is_generation_pending(&gen_dir),
            "successful recovery rebuild must clear a stale pending marker"
        );
        crate::generation::artifact::load_generation_artifact(&gen_dir).unwrap();
    }
}
