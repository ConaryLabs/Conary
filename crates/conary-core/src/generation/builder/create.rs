// conary-core/src/generation/builder/create.rs

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use super::GenerationActivation;
use super::boot_assets::{
    resolve_generation_boot_asset_sources, stage_runtime_boot_assets_from_sources,
};
use super::cas::{cas_objects_from_file_refs, verify_runtime_generation_cas_object_presence};
use super::erofs::{BuildResult, build_erofs_image};
use super::root_validation::validate_runtime_generation_root_is_self_contained;
use super::runtime_inputs;
use super::sysroot::runtime_generation_architecture;
use crate::db::models::{FileEntry, StateEngine, SystemState, Trove};
use crate::generation::artifact::{
    ArtifactWriteInputs, CasObjectVerification, deduplicate_sort_cas_objects,
    write_generation_artifact,
};
use crate::generation::metadata::{
    GENERATION_FORMAT, GenerationMetadata, clear_generation_pending, mark_generation_pending,
};

/// Build a complete generation from the current database state.
///
/// This is the high-level entry point that:
/// 1. Queries all installed troves and their file entries
/// 2. Builds the EROFS image via [`build_erofs_image`]
/// 3. Creates a system state snapshot (only after successful image build)
/// 4. Writes generation metadata JSON
///
/// The state snapshot is deliberately created *after* the EROFS image build
/// succeeds. Creating it before would leave an orphaned DB state record if
/// the image build fails.
///
/// Returns `(generation_number, BuildResult)`.
pub fn build_generation_from_db(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    summary: &str,
) -> crate::Result<(i64, BuildResult)> {
    build_generation_from_db_with_activation(
        conn,
        generations_root,
        summary,
        GenerationActivation::Active,
    )
}

pub fn build_generation_from_db_with_activation(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    summary: &str,
    activation: GenerationActivation,
) -> crate::Result<(i64, BuildResult)> {
    build_generation_from_db_with_boot_root_and_activation(
        conn,
        generations_root,
        summary,
        Path::new("/boot"),
        activation,
    )
}

pub fn build_generation_from_db_with_boot_root(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    summary: &str,
    boot_root: &Path,
) -> crate::Result<(i64, BuildResult)> {
    build_generation_from_db_with_boot_root_and_activation(
        conn,
        generations_root,
        summary,
        boot_root,
        GenerationActivation::Active,
    )
}

pub fn build_generation_from_db_with_boot_root_and_activation(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    summary: &str,
    boot_root: &Path,
    activation: GenerationActivation,
) -> crate::Result<(i64, BuildResult)> {
    struct PendingGenerationGuard {
        gen_dir: PathBuf,
        armed: bool,
    }

    impl PendingGenerationGuard {
        fn new(gen_dir: PathBuf) -> Self {
            Self {
                gen_dir,
                armed: true,
            }
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for PendingGenerationGuard {
        fn drop(&mut self) {
            if !self.armed {
                return;
            }

            if let Err(error) = std::fs::remove_dir_all(&self.gen_dir) {
                warn!(
                    "Failed to clean up incomplete generation {}: {}",
                    self.gen_dir.display(),
                    error
                );
            }
        }
    }

    // Step 1: Ensure generations base directory exists
    std::fs::create_dir_all(generations_root).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generations directory {}: {e}",
            generations_root.display()
        ))
    })?;

    // Step 2: Reserve the generation number and create the directory.
    //
    // TOCTOU guard: hold an exclusive advisory lock on the generations
    // directory for the duration of number-allocation + directory-creation.
    // Without this, two concurrent `build_generation_from_db` calls could
    // read the same `next_state_number`, both try to create the same
    // directory, and one would silently overwrite the other's work.
    //
    // The lock is released automatically when `_gen_lock` is dropped at the
    // end of this function (or on any early-return error path).
    let lock_path = generations_root.join(".generation-build.lock");
    let lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| {
            crate::error::Error::IoError(format!(
                "Failed to open generation lock file {}: {e}",
                lock_path.display()
            ))
        })?;
    use fs2::FileExt as _;
    lock_file.lock_exclusive().map_err(|e| {
        crate::error::Error::IoError(format!("Failed to acquire generation build lock: {e}"))
    })?;
    // RAII guard: lock is released when this drops.
    let _gen_lock = lock_file;

    let gen_number = SystemState::next_state_number(conn).map_err(|e| {
        crate::error::Error::InternalError(format!("Failed to determine next state number: {e}"))
    })?;
    let gen_dir = generations_root.join(gen_number.to_string());
    if gen_dir.exists() {
        return Err(crate::error::Error::AlreadyExists(format!(
            "Generation directory already exists: {}",
            gen_dir.display()
        )));
    }
    std::fs::create_dir_all(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to create generation directory {}: {e}",
            gen_dir.display()
        ))
    })?;
    mark_generation_pending(&gen_dir).map_err(|e| {
        crate::error::Error::IoError(format!(
            "Failed to mark generation {} as pending: {e}",
            gen_dir.display()
        ))
    })?;
    let mut pending_guard = PendingGenerationGuard::new(gen_dir.clone());

    // Step 3: Collect and validate exportable runtime inputs before building.
    let troves = Trove::list_all(conn)?;
    let all_files = FileEntry::find_all_ordered(conn)?;
    let runtime_inputs = runtime_inputs::collect_runtime_generation_inputs(&troves, all_files)?;

    // Step 4: Build EROFS image with symlinks from DB.
    // This must succeed before we commit state to the database.
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

    // Step 5: Stage boot assets and write the export artifact contract before
    // committing metadata. Export must not scrape live /boot later.
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

    // Step 6: Create system state snapshot at the reserved number -- only
    // after successful image build so we never leave orphaned state records
    // on build failure. Using create_snapshot_at() ensures the DB state
    // number matches the directory number we already created.
    let engine = StateEngine::new(conn);
    let _state = if activation.activates_state() {
        engine.create_snapshot_at(gen_number, summary, None, None)
    } else {
        engine.create_inactive_snapshot_at(gen_number, summary, None, None)
    }
    .map_err(|e| {
        crate::error::Error::InternalError(format!("Failed to create system state snapshot: {e}"))
    })?;

    // Step 7: Write generation metadata
    #[allow(clippy::cast_possible_wrap)]
    let metadata = GenerationMetadata {
        generation: gen_number,
        format: GENERATION_FORMAT.to_string(),
        erofs_size: Some(result.image_size as i64),
        cas_objects_referenced: Some(result.cas_objects_referenced as i64),
        fsverity_enabled: false, // Caller can enable separately
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
    pending_guard.disarm();

    info!(
        "Generation {} built: {} CAS objects, {} packages ({} metadata-only), composefs format",
        gen_number,
        result.cas_objects_referenced,
        troves.len(),
        runtime_inputs.adopted_track_count
    );

    Ok((gen_number, result))
}

#[cfg(all(test, feature = "composefs-rs"))]
mod tests {
    use super::super::test_support::{
        assert_invalid_runtime_input_error, assert_missing_cas_object_error,
        runtime_generation_db_with_invalid_regular_file,
        runtime_generation_db_with_missing_regular_file_cas_object,
    };
    use super::*;
    use crate::db::models::{FileEntry, Trove, TroveType};
    use crate::db::schema::migrate;
    use crate::filesystem::CasStore;
    use crate::generation::metadata::GenerationMetadata;

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn build_generation_from_db_writes_export_artifact_contract() {
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
        migrate(&conn).unwrap();
        let mut trove = Trove::new(
            "kernel".to_string(),
            "6.19.8-conary".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = FileEntry::new(
            "/usr/bin/hello".to_string(),
            hello_hash,
            b"hello".len() as i64,
            0o755,
            trove_id,
        );
        file.insert(&conn).unwrap();
        let mut init = FileEntry::new(
            "/usr/sbin/init".to_string(),
            init_hash,
            b"init".len() as i64,
            0o755,
            trove_id,
        );
        init.insert(&conn).unwrap();

        let (generation, _result) = build_generation_from_db_with_boot_root(
            &conn,
            &generations_root,
            "runtime artifact test",
            &boot_root,
        )
        .unwrap();
        let gen_dir = generations_root.join(generation.to_string());

        assert!(gen_dir.join(".conary-artifact.json").is_file());
        assert!(gen_dir.join("cas-manifest.json").is_file());
        assert!(gen_dir.join("boot-assets/manifest.json").is_file());
        assert!(gen_dir.join("boot-assets/vmlinuz").is_file());
        assert!(gen_dir.join("boot-assets/initramfs.img").is_file());
        assert!(gen_dir.join("boot-assets/EFI/BOOT/BOOTX64.EFI").is_file());
        let metadata = GenerationMetadata::read_from(&gen_dir).unwrap();
        assert!(metadata.artifact_manifest_sha256.is_some());
        assert_eq!(metadata.kernel_version.as_deref(), Some("6.19.8-conary"));
        crate::generation::artifact::load_generation_artifact(&gen_dir).unwrap();
    }

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn build_generation_from_db_rejects_invalid_runtime_input() {
        let (_tmp, conn, generations_root, boot_root) =
            runtime_generation_db_with_invalid_regular_file();

        let error = build_generation_from_db_with_boot_root(
            &conn,
            &generations_root,
            "invalid runtime input",
            &boot_root,
        )
        .unwrap_err()
        .to_string();

        assert_invalid_runtime_input_error(&error);
        assert!(!generations_root.join("0/.conary-artifact.json").exists());
    }

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn build_generation_from_db_rejects_missing_regular_file_cas_object() {
        let (_tmp, conn, generations_root, boot_root, missing_hash) =
            runtime_generation_db_with_missing_regular_file_cas_object();

        let error = build_generation_from_db_with_boot_root(
            &conn,
            &generations_root,
            "missing runtime CAS object",
            &boot_root,
        )
        .unwrap_err()
        .to_string();

        assert_missing_cas_object_error(&error, &missing_hash);
        assert!(!generations_root.join("0/.conary-artifact.json").exists());
        assert!(!generations_root.join("0/cas-manifest.json").exists());
    }

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn build_generation_from_db_rejects_root_without_init() {
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
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut trove = Trove::new(
            "kernel".to_string(),
            "6.19.8-conary".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        let mut file = FileEntry::new(
            "/usr/bin/hello".to_string(),
            hello_hash,
            b"hello".len() as i64,
            0o755,
            trove_id,
        );
        file.insert(&conn).unwrap();

        let error = build_generation_from_db_with_boot_root(
            &conn,
            &generations_root,
            "runtime artifact test",
            &boot_root,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("not self-contained"));
        assert!(error.contains("/sbin/init"));
        assert!(!generations_root.join("0").exists());
    }
}
