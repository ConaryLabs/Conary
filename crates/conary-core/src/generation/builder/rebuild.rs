// conary-core/src/generation/builder/rebuild.rs

use std::path::Path;

use tracing::info;

use super::BuildResult;
use super::boot_assets::{
    resolve_generation_boot_asset_sources, stage_runtime_boot_assets_from_sources,
};
use super::cas::{cas_objects_from_manifests, verify_runtime_generation_cas_object_presence};
use super::root_validation::validate_runtime_generation_root_is_self_contained;
use super::runtime_inputs;
use super::sysroot::runtime_generation_architecture;
use crate::db::models::{FileEntry, Trove};
use crate::generation::artifact::{
    ArtifactWriteInputs, CasObjectVerification, deduplicate_sort_cas_objects,
    write_generation_artifact,
};
use crate::generation::metadata::{
    GENERATION_FORMAT, GenerationMetadata, clear_generation_pending,
};
use crate::generation::root_manifest::build_erofs_image_from_root_manifest;

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
    let runtime_inputs = runtime_inputs::collect_runtime_generation_inputs(
        conn,
        &troves,
        all_files,
        Path::new("/"),
    )?;
    let security_capability_xattr_count = runtime_inputs.security_capability_xattr_count();

    validate_runtime_generation_root_is_self_contained(&runtime_inputs.generation)?;
    let cas_objects = deduplicate_sort_cas_objects(cas_objects_from_manifests(
        &runtime_inputs.generation,
        &runtime_inputs.state,
    ))?;
    verify_runtime_generation_cas_object_presence(generations_root, &cas_objects)?;
    let result = build_erofs_image_from_root_manifest(&runtime_inputs.generation, &gen_dir)?;
    runtime_inputs.state.write_to(&gen_dir)?;
    let architecture = runtime_generation_architecture()?;
    let boot_asset_sources =
        resolve_generation_boot_asset_sources(&runtime_inputs, generations_root, boot_root)?;
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
        security_capability_xattr_count: (security_capability_xattr_count > 0)
            .then_some(security_capability_xattr_count as i64),
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

#[cfg(all(test, feature = "composefs-rs"))]
mod tests {
    use super::super::test_support::{
        assert_cas_size_mismatch_error, assert_missing_cas_object_error,
        insert_regular_file_with_parents,
        runtime_generation_db_with_missing_regular_file_cas_object,
        runtime_generation_db_with_wrong_sized_regular_file_cas_object,
    };
    use super::*;
    use crate::ccs::manifest::FileCapability;
    use crate::db::models::{InstalledFileCapability, Trove, TroveType};
    use crate::db::schema::ensure_current;
    use crate::filesystem::CasStore;
    use crate::generation::builder::file_capabilities::SECURITY_CAPABILITY_XATTR;
    use crate::generation::metadata::{is_generation_pending, mark_generation_pending};

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn rebuild_generation_image_rejects_wrong_sized_regular_file_cas_object() {
        let (_tmp, conn, generations_root, boot_root, bad_hash) =
            runtime_generation_db_with_wrong_sized_regular_file_cas_object();

        let error = rebuild_generation_image_with_boot_root(
            &conn,
            &generations_root,
            7,
            "wrong-sized runtime CAS object",
            &boot_root,
        )
        .unwrap_err()
        .to_string();

        assert_cas_size_mismatch_error(&error, &bad_hash);
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
        ensure_current(&conn).unwrap();
        let mut trove = Trove::new(
            "kernel-core".to_string(),
            "6.19.8-conary".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        insert_regular_file_with_parents(
            &conn,
            "/usr/bin/hello",
            hello_hash,
            b"hello".len(),
            0o100755,
            trove_id,
        );
        insert_regular_file_with_parents(
            &conn,
            "/sbin/init",
            init_hash,
            b"init".len(),
            0o100755,
            trove_id,
        );

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

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn rebuild_generation_image_records_capability_xattr_count() {
        let tmp = tempfile::TempDir::new().unwrap();
        let generations_root = tmp.path().join("generations");
        let objects_dir = tmp.path().join("objects");
        let boot_root = tmp.path().join("boot");
        std::fs::create_dir_all(&generations_root).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
        std::fs::write(boot_root.join("initramfs-6.19.8-conary.img"), b"initramfs").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let cas = CasStore::new(&objects_dir).unwrap();
        let hello_hash = cas.store(b"hello").unwrap();
        let init_hash = cas.store(b"init").unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ensure_current(&conn).unwrap();
        let mut trove = Trove::new(
            "kernel".to_string(),
            "6.19.8-conary".to_string(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        insert_regular_file_with_parents(
            &conn,
            "/usr/bin/hello",
            hello_hash,
            b"hello".len(),
            0o755,
            trove_id,
        );
        insert_regular_file_with_parents(
            &conn,
            "/sbin/init",
            init_hash,
            b"init".len(),
            0o755,
            trove_id,
        );

        InstalledFileCapability::replace_for_trove(
            &conn,
            trove_id,
            &[FileCapability {
                path: "/usr/bin/hello".to_string(),
                capabilities: vec!["cap_net_bind_service".to_string()],
                permitted: true,
                effective: true,
                inheritable: false,
            }],
        )
        .unwrap();

        rebuild_generation_image_with_boot_root(
            &conn,
            &generations_root,
            7,
            "recovery rebuild",
            &boot_root,
        )
        .unwrap();

        let gen_dir = generations_root.join("7");
        let metadata = GenerationMetadata::read_from(&gen_dir).unwrap();
        let artifact = crate::generation::artifact::load_generation_artifact(&gen_dir).unwrap();
        let image_bytes = std::fs::read(&artifact.erofs_path).unwrap();
        let fs = composefs::erofs::reader::erofs_to_filesystem::<
            composefs::fsverity::Sha256HashValue,
        >(&image_bytes)
        .unwrap();
        let leaf = fs
            .as_dir()
            .get_directory_ref(std::ffi::OsStr::new("usr"))
            .unwrap()
            .get_directory_ref(std::ffi::OsStr::new("bin"))
            .unwrap()
            .leaf(
                fs.as_dir()
                    .get_directory_ref(std::ffi::OsStr::new("usr"))
                    .unwrap()
                    .get_directory_ref(std::ffi::OsStr::new("bin"))
                    .unwrap()
                    .leaf_id(std::ffi::OsStr::new("hello"))
                    .unwrap(),
            );

        assert_eq!(metadata.security_capability_xattr_count, Some(1));
        assert!(
            leaf.stat
                .xattrs
                .contains_key(std::ffi::OsStr::new(SECURITY_CAPABILITY_XATTR))
        );
    }
}
