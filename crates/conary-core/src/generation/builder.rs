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

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use tracing::{debug, info, warn};

use crate::db::models::{FileEntry, InstallSource, StateEngine, SystemState, Trove};
use crate::generation::artifact::{
    ArtifactWriteInputs, BootAssetSources, CasObjectRef, stage_boot_assets,
    write_generation_artifact,
};
use crate::generation::metadata::{
    GENERATION_FORMAT, GenerationMetadata, ROOT_SYMLINKS, clear_generation_pending,
    mark_generation_pending,
};
mod erofs;

pub use erofs::{BuildResult, FileEntryRef, SymlinkEntryRef, build_erofs_image, hex_to_digest};

const CONARY_DRACUT_MODULE_SETUP: &str =
    include_str!("../../../../packaging/dracut/90conary/module-setup.sh");
const CONARY_DRACUT_GENERATOR: &str =
    include_str!("../../../../packaging/dracut/90conary/conary-generator.sh");
const RUNTIME_DRACUT_ADD_MODULES: &str = "conary";
const RUNTIME_DRACUT_OMIT_MODULES: &str = "systemd";

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
    build_generation_from_db_with_boot_root(conn, generations_root, summary, Path::new("/boot"))
}

pub fn build_generation_from_db_with_boot_root(
    conn: &rusqlite::Connection,
    generations_root: &Path,
    summary: &str,
    boot_root: &Path,
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

    // Step 3: Collect file entries from all installed troves (single bulk query).
    // Exclude files belonging to adopted-track troves: those troves are metadata-
    // only and their file records use placeholder hashes that cannot be resolved
    // in the CAS. Filtering here makes the intent explicit and avoids silently
    // relying on hex parse failures to skip them.
    let troves = Trove::list_all(conn)?;
    // Build the adopted-track trove id set so we can exclude their files.
    let adopted_track_ids: std::collections::HashSet<i64> = troves
        .iter()
        .filter(|t| t.install_source == InstallSource::AdoptedTrack)
        .filter_map(|t| t.id)
        .collect();
    let all_files_raw = FileEntry::find_all_ordered(conn)?;
    let all_files: Vec<FileEntry> = all_files_raw
        .into_iter()
        .filter(|f| !adopted_track_ids.contains(&f.trove_id))
        .collect();

    let file_refs: Vec<FileEntryRef> = all_files
        .iter()
        .map(|file| {
            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;
            #[allow(clippy::cast_sign_loss)]
            let size = file.size as u64;

            FileEntryRef {
                path: file.path.clone(),
                sha256_hash: file.sha256_hash.clone(),
                size,
                permissions,
                owner: file.owner.clone(),
                group_name: file.group_name.clone(),
            }
        })
        .collect();

    // Step 4: Build EROFS image with symlinks from DB.
    // This must succeed before we commit state to the database.
    let symlink_refs = collect_symlink_refs(conn, &adopted_track_ids)?;
    validate_runtime_generation_root_is_self_contained(&file_refs, &symlink_refs)?;
    let result = build_erofs_image(&file_refs, &symlink_refs, &gen_dir)?;

    // Step 5: Stage boot assets and write the export artifact contract before
    // committing metadata. Export must not scrape live /boot later.
    let architecture = runtime_generation_architecture()?;
    let boot_asset_sources = resolve_runtime_boot_asset_sources(&troves, boot_root)?;
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
        cas_objects: cas_objects_from_file_refs(&file_refs),
        boot_assets,
    })?;

    // Step 6: Create system state snapshot at the reserved number -- only
    // after successful image build so we never leave orphaned state records
    // on build failure. Using create_snapshot_at() ensures the DB state
    // number matches the directory number we already created.
    let engine = StateEngine::new(conn);
    let _state = engine
        .create_snapshot_at(gen_number, summary, None, None)
        .map_err(|e| {
            crate::error::Error::InternalError(format!(
                "Failed to create system state snapshot: {e}"
            ))
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
        "Generation {} built: {} CAS objects, {} packages, composefs format",
        gen_number,
        result.cas_objects_referenced,
        troves.len()
    );

    Ok((gen_number, result))
}

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
    let adopted_track_ids: HashSet<i64> = troves
        .iter()
        .filter(|t| t.install_source == InstallSource::AdoptedTrack)
        .filter_map(|t| t.id)
        .collect();
    let all_files_raw = FileEntry::find_all_ordered(conn)?;
    let all_files: Vec<FileEntry> = all_files_raw
        .into_iter()
        .filter(|f| !adopted_track_ids.contains(&f.trove_id))
        .collect();

    let file_refs: Vec<FileEntryRef> = all_files
        .iter()
        .map(|file| {
            #[allow(clippy::cast_sign_loss)]
            let permissions = file.permissions as u32;
            #[allow(clippy::cast_sign_loss)]
            let size = file.size as u64;
            FileEntryRef {
                path: file.path.clone(),
                sha256_hash: file.sha256_hash.clone(),
                size,
                permissions,
                owner: file.owner.clone(),
                group_name: file.group_name.clone(),
            }
        })
        .collect();

    let symlink_refs = collect_symlink_refs(conn, &adopted_track_ids)?;
    validate_runtime_generation_root_is_self_contained(&file_refs, &symlink_refs)?;
    let result = build_erofs_image(&file_refs, &symlink_refs, &gen_dir)?;
    let architecture = runtime_generation_architecture()?;
    let boot_asset_sources = resolve_runtime_boot_asset_sources(&troves, boot_root)?;
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
        cas_objects: cas_objects_from_file_refs(&file_refs),
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

    info!(
        "Generation {} rebuilt in place: {} CAS objects, {} packages",
        gen_number,
        result.cas_objects_referenced,
        troves.len()
    );

    Ok(result)
}

/// Collect symlink entries from all installed troves.
///
/// Queries file entries that have a non-NULL symlink_target and returns them
/// as `SymlinkEntryRef` values suitable for EROFS image building.
///
/// Returns an empty vec if the `file_entries` table does not have a
/// `symlink_target` column (older schema or test databases).
fn collect_symlink_refs(
    conn: &rusqlite::Connection,
    excluded_trove_ids: &HashSet<i64>,
) -> crate::Result<Vec<SymlinkEntryRef>> {
    let mut stmt = match conn.prepare(
        "SELECT path, symlink_target, trove_id FROM files \
         WHERE symlink_target IS NOT NULL AND symlink_target != ''",
    ) {
        Ok(s) => s,
        Err(e) => {
            // Column may not exist in pre-v60 schemas.
            debug!("Skipping symlink collection: {e}");
            return Ok(Vec::new());
        }
    };

    let refs = stmt
        .query_map([], |row| {
            Ok((
                SymlinkEntryRef {
                    path: row.get(0)?,
                    target: row.get(1)?,
                },
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| crate::error::Error::InternalError(format!("Failed to query symlinks: {e}")))?
        .filter_map(|r| match r {
            Ok((symlink, trove_id)) if !excluded_trove_ids.contains(&trove_id) => Some(symlink),
            Ok(_) => None,
            Err(error) => {
                debug!("Skipping unreadable symlink entry: {error}");
                None
            }
        })
        .collect();

    Ok(refs)
}

fn validate_runtime_generation_root_is_self_contained(
    file_refs: &[FileEntryRef],
    symlink_refs: &[SymlinkEntryRef],
) -> crate::Result<()> {
    if generation_root_has_init_entrypoint(file_refs, symlink_refs) {
        return Ok(());
    }

    Err(crate::error::Error::NotFound(
        "exportable runtime generation is not self-contained: missing executable /sbin/init in the CAS-backed generation root; refusing to scrape the live host root to make the image bootable".to_string(),
    ))
}

fn generation_root_has_init_entrypoint(
    file_refs: &[FileEntryRef],
    symlink_refs: &[SymlinkEntryRef],
) -> bool {
    let symlink_paths: HashSet<String> = symlink_refs
        .iter()
        .filter_map(|symlink| normalize_virtual_path(&symlink.path, "/"))
        .collect();
    let files: HashMap<String, u32> = file_refs
        .iter()
        .filter_map(|file| {
            let path = normalize_virtual_path(&file.path, "/")?;
            if symlink_paths.contains(&path) || hex_to_digest(&file.sha256_hash).is_err() {
                return None;
            }
            Some((path, file.permissions))
        })
        .collect();
    let symlinks = generation_symlink_map(symlink_refs);

    resolve_virtual_path("/sbin/init", &symlinks)
        .and_then(|resolved| files.get(&resolved).copied())
        .is_some_and(|permissions| permissions & 0o111 != 0)
}

fn generation_symlink_map(symlink_refs: &[SymlinkEntryRef]) -> HashMap<String, String> {
    let mut symlinks = HashMap::new();
    for symlink in symlink_refs {
        if let Some(path) = normalize_virtual_path(&symlink.path, "/") {
            symlinks.insert(path, symlink.target.clone());
        }
    }
    for (link, target) in ROOT_SYMLINKS {
        symlinks.insert(format!("/{link}"), (*target).to_string());
    }
    symlinks
}

fn resolve_virtual_path(path: &str, symlinks: &HashMap<String, String>) -> Option<String> {
    let mut current = normalize_virtual_path(path, "/")?;
    for _ in 0..40 {
        let Some(next) = rewrite_first_symlink_component(&current, symlinks) else {
            return Some(current);
        };
        current = next;
    }
    None
}

fn rewrite_first_symlink_component(
    path: &str,
    symlinks: &HashMap<String, String>,
) -> Option<String> {
    let components: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();

    for index in 0..components.len() {
        let prefix = format!("/{}", components[..=index].join("/"));
        let Some(target) = symlinks.get(&prefix) else {
            continue;
        };
        let base = parent_virtual_path(&prefix);
        let mut rewritten = normalize_virtual_path(target, &base)?;
        for component in &components[index + 1..] {
            if rewritten != "/" {
                rewritten.push('/');
            }
            rewritten.push_str(component);
        }
        return normalize_virtual_path(&rewritten, "/");
    }

    None
}

fn normalize_virtual_path(path: &str, base: &str) -> Option<String> {
    let combined = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{}", base.trim_end_matches('/'), path)
    };
    let mut components = Vec::new();
    for component in combined.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            component => components.push(component),
        }
    }
    Some(format!("/{}", components.join("/")))
}

fn parent_virtual_path(path: &str) -> String {
    let path = path.trim_end_matches('/');
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".to_string(),
        Some((parent, _)) if parent.is_empty() => "/".to_string(),
        Some((parent, _)) => parent.to_string(),
    }
}

fn cas_objects_from_file_refs(file_refs: &[FileEntryRef]) -> Vec<CasObjectRef> {
    file_refs
        .iter()
        .map(|file| CasObjectRef {
            sha256: file.sha256_hash.clone(),
            size: file.size,
        })
        .collect()
}

fn runtime_generation_architecture() -> crate::Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("x86_64"),
        "aarch64" => Err(crate::error::Error::NotImplemented(
            "aarch64 generation export boot assets are reserved but not implemented".to_string(),
        )),
        "riscv64" => Err(crate::error::Error::NotImplemented(
            "riscv64 generation export boot assets are reserved but not implemented".to_string(),
        )),
        other => Err(crate::error::Error::NotImplemented(format!(
            "unsupported runtime architecture for generation export: {other}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeBootAssetSources {
    kernel_version: String,
    kernel: PathBuf,
    initramfs: PathBuf,
    efi_bootloader: PathBuf,
}

fn stage_runtime_boot_assets_from_sources(
    gen_dir: &Path,
    generation: i64,
    architecture: &str,
    sources: &RuntimeBootAssetSources,
) -> crate::Result<crate::generation::artifact::BootAssetsManifest> {
    let kernel_version = sources.kernel_version.as_str();
    if kernel_version.contains('/') || kernel_version.contains('\\') {
        return Err(crate::error::Error::InvalidPath(format!(
            "kernel version must not contain path separators: {kernel_version}"
        )));
    }

    stage_boot_assets(BootAssetSources {
        generation_dir: gen_dir,
        generation,
        architecture,
        kernel_version,
        kernel: &sources.kernel,
        initramfs: &sources.initramfs,
        efi_bootloader: &sources.efi_bootloader,
    })
}

fn resolve_runtime_boot_asset_sources(
    troves: &[Trove],
    boot_root: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    resolve_runtime_boot_asset_sources_with_tools(
        troves,
        boot_root,
        Path::new("dracut"),
        Path::new("depmod"),
        Path::new("cpio"),
    )
}

fn resolve_runtime_boot_asset_sources_with_tools(
    troves: &[Trove],
    boot_root: &Path,
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    let requested_version = detect_kernel_version_from_troves(troves).ok_or_else(|| {
        crate::error::Error::NotFound(
            "could not determine kernel version for generation boot assets".to_string(),
        )
    })?;
    if requested_version.contains('/') || requested_version.contains('\\') {
        return Err(crate::error::Error::InvalidPath(format!(
            "kernel version must not contain path separators: {requested_version}"
        )));
    }

    let system_root = system_root_for_boot_root(boot_root);
    let mut candidate_releases = Vec::new();
    push_unique_release(&mut candidate_releases, requested_version.clone());
    collect_boot_kernel_releases(boot_root, &requested_version, &mut candidate_releases);
    collect_module_kernel_releases(&system_root, &requested_version, &mut candidate_releases);

    let mut last_error = None;
    for release in candidate_releases {
        match runtime_boot_asset_sources_for_release(
            boot_root,
            &system_root,
            &release,
            dracut,
            depmod,
            cpio,
        ) {
            Ok(sources) => return Ok(sources),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        crate::error::Error::NotFound(format!(
            "could not find runtime boot assets for kernel {requested_version}"
        ))
    }))
}

fn runtime_boot_asset_sources_for_release(
    boot_root: &Path,
    system_root: &Path,
    release: &str,
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    let kernel = boot_root.join(format!("vmlinuz-{release}"));
    let kernel = if regular_file_exists(&kernel) {
        kernel
    } else {
        module_kernel_path(system_root, release).ok_or_else(|| {
            crate::error::Error::NotFound(format!(
                "missing required boot asset kernel for {release}; expected {} or a module kernel at lib/modules/{release}/vmlinuz",
                boot_root.join(format!("vmlinuz-{release}")).display()
            ))
        })?
    };

    let initramfs = boot_root.join(format!("initramfs-{release}.img"));
    if !regular_file_exists(&initramfs) {
        generate_runtime_initramfs(dracut, depmod, cpio, system_root, release, &initramfs)?;
    }
    if !regular_file_exists(&initramfs) {
        return Err(crate::error::Error::NotFound(format!(
            "missing required boot asset initramfs for {release} at {}; generate it with dracut or install a package hook that stages runtime boot assets before building a generation",
            initramfs.display()
        )));
    }

    let efi_bootloader = boot_root.join("EFI/BOOT/BOOTX64.EFI");
    if !regular_file_exists(&efi_bootloader) {
        return Err(crate::error::Error::NotFound(format!(
            "missing required boot asset efi_bootloader at {}",
            efi_bootloader.display()
        )));
    }

    Ok(RuntimeBootAssetSources {
        kernel_version: release.to_string(),
        kernel,
        initramfs,
        efi_bootloader,
    })
}

fn generate_runtime_initramfs(
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
    system_root: &Path,
    release: &str,
    initramfs: &Path,
) -> crate::Result<()> {
    let Some(parent) = initramfs.parent() else {
        return Err(crate::error::Error::InvalidPath(format!(
            "initramfs destination has no parent: {}",
            initramfs.display()
        )));
    };
    std::fs::create_dir_all(parent)?;
    ensure_initramfs_tool_available(cpio, "cpio")?;
    ensure_kernel_module_metadata(depmod, system_root, release)?;

    let modules_workspace = tempfile::Builder::new()
        .prefix("conary-dracut-")
        .tempdir()
        .map_err(|e| {
            crate::error::Error::IoError(format!("failed to create dracut workspace: {e}"))
        })?;
    prepare_dracut_workspace(modules_workspace.path())?;
    let module_dir = modules_workspace.path().join("modules.d/90conary");
    std::fs::create_dir_all(&module_dir)?;
    write_dracut_module_file(
        &module_dir.join("module-setup.sh"),
        CONARY_DRACUT_MODULE_SETUP,
    )?;
    write_dracut_module_file(
        &module_dir.join("conary-generator.sh"),
        CONARY_DRACUT_GENERATOR,
    )?;

    let output = std::process::Command::new(dracut)
        .env("dracutbasedir", modules_workspace.path())
        .arg("--force")
        .arg("--no-hostonly")
        // Force dracut's shell init path. The default systemd module alone
        // creates a partial initramfs without the initrd systemd contract.
        .arg("--omit")
        .arg(RUNTIME_DRACUT_OMIT_MODULES)
        .arg("--add")
        .arg(RUNTIME_DRACUT_ADD_MODULES)
        .arg(initramfs)
        .arg(release)
        .output()
        .map_err(|e| {
            crate::error::Error::NotFound(format!(
                "failed to run dracut to generate {} for {release}: {e}",
                initramfs.display()
            ))
        })?;

    if !output.status.success() {
        return Err(crate::error::Error::IoError(format!(
            "dracut failed to generate {} for {release} with status {}:\nstdout:\n{}\nstderr:\n{}",
            initramfs.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

fn ensure_initramfs_tool_available(tool: &Path, name: &str) -> crate::Result<()> {
    match std::process::Command::new(tool).arg("--version").output() {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(crate::error::Error::NotFound(format!(
                "missing required initramfs tool {name} at {}; source images that build runtime generations must include the initramfs toolchain because dracut emits initramfs archives through {name}",
                tool.display()
            )))
        }
        Err(e) => Err(crate::error::Error::IoError(format!(
            "failed to check required initramfs tool {name} at {}: {e}",
            tool.display()
        ))),
    }
}

fn prepare_dracut_workspace(workspace: &Path) -> crate::Result<()> {
    let modules_dir = workspace.join("modules.d");
    std::fs::create_dir_all(&modules_dir)?;

    let system_dracut = Path::new("/usr/lib/dracut");
    if !system_dracut.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(system_dracut)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "modules.d" {
            continue;
        }
        link_or_copy_dracut_entry(&entry.path(), &workspace.join(name))?;
    }

    let system_modules = system_dracut.join("modules.d");
    if system_modules.is_dir() {
        for entry in std::fs::read_dir(system_modules)? {
            let entry = entry?;
            link_or_copy_dracut_entry(&entry.path(), &modules_dir.join(entry.file_name()))?;
        }
    }

    Ok(())
}

fn link_or_copy_dracut_entry(source: &Path, dest: &Path) -> crate::Result<()> {
    if dest.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, dest)?;
        return Ok(());
    }

    #[cfg(not(unix))]
    {
        if source.is_file() {
            std::fs::copy(source, dest)?;
        }
        Ok(())
    }
}

fn ensure_kernel_module_metadata(
    depmod: &Path,
    system_root: &Path,
    release: &str,
) -> crate::Result<()> {
    let (module_dir, module_dir_arg) = kernel_module_dir(system_root, release).ok_or_else(|| {
        crate::error::Error::NotFound(format!(
            "missing kernel module directory for {release}; expected lib/modules/{release} or usr/lib/modules/{release}"
        ))
    })?;
    let modules_dep = module_dir.join("modules.dep");
    if regular_file_exists(&modules_dep) {
        return Ok(());
    }

    let output = std::process::Command::new(depmod)
        .arg("-b")
        .arg(system_root)
        .arg("-m")
        .arg(module_dir_arg)
        .arg(release)
        .output()
        .map_err(|e| {
            crate::error::Error::NotFound(format!(
                "failed to run depmod for kernel {release} under {}: {e}",
                system_root.display()
            ))
        })?;

    if !output.status.success() {
        return Err(crate::error::Error::IoError(format!(
            "depmod failed for kernel {release} under {} with status {}:\nstdout:\n{}\nstderr:\n{}",
            system_root.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    if !regular_file_exists(&modules_dep) {
        return Err(crate::error::Error::NotFound(format!(
            "depmod completed but did not create {}",
            modules_dep.display()
        )));
    }

    Ok(())
}

fn write_dracut_module_file(path: &Path, contents: &str) -> crate::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn collect_boot_kernel_releases(
    boot_root: &Path,
    requested_version: &str,
    releases: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(boot_root) else {
        return;
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
            continue;
        };
        let Some(release) = name.strip_prefix("vmlinuz-") else {
            continue;
        };
        if kernel_release_matches(requested_version, release) {
            found.push(release.to_string());
        }
    }
    found.sort();
    for release in found {
        push_unique_release(releases, release);
    }
}

fn collect_module_kernel_releases(
    system_root: &Path,
    requested_version: &str,
    releases: &mut Vec<String>,
) {
    let mut found = Vec::new();
    for modules_root in [
        system_root.join("lib/modules"),
        system_root.join("usr/lib/modules"),
    ] {
        let Ok(entries) = std::fs::read_dir(modules_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(release) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if kernel_release_matches(requested_version, release)
                && regular_file_exists(&path.join("vmlinuz"))
            {
                found.push(release.to_string());
            }
        }
    }
    found.sort();
    for release in found {
        push_unique_release(releases, release);
    }
}

fn push_unique_release(releases: &mut Vec<String>, release: String) {
    if !releases.iter().any(|existing| existing == &release) {
        releases.push(release);
    }
}

fn kernel_release_matches(requested_version: &str, release: &str) -> bool {
    release == requested_version
        || release
            .strip_prefix(requested_version)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn module_kernel_path(system_root: &Path, release: &str) -> Option<PathBuf> {
    kernel_module_dir(system_root, release)
        .map(|(module_dir, _module_dir_arg)| module_dir.join("vmlinuz"))
        .filter(|path| regular_file_exists(path))
}

fn kernel_module_dir(system_root: &Path, release: &str) -> Option<(PathBuf, &'static str)> {
    [
        (
            system_root.join("lib/modules").join(release),
            "/lib/modules",
        ),
        (
            system_root.join("usr/lib/modules").join(release),
            "/usr/lib/modules",
        ),
    ]
    .into_iter()
    .find(|(path, _module_dir_arg)| path.is_dir())
}

fn regular_file_exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn system_root_for_boot_root(boot_root: &Path) -> PathBuf {
    if boot_root.file_name().is_some_and(|name| name == "boot") {
        return boot_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
    }

    PathBuf::from("/")
}

/// Get kernel version from an already-loaded trove list.
///
/// Looks for kernel-related packages in the trove list, falling back to
/// the running kernel version from `/proc/version`.
pub fn detect_kernel_version_from_troves(troves: &[Trove]) -> Option<String> {
    for trove in troves {
        if trove.name.starts_with("kernel") || trove.name.starts_with("linux-image") {
            return Some(trove.version.clone());
        }
    }
    // Fall back to running kernel version from /proc/sys/kernel/osrelease
    crate::generation::metadata::running_kernel_version()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_executable(path: &Path, contents: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, contents).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(feature = "composefs-rs")]
    #[test]
    fn build_generation_from_db_writes_export_artifact_contract() {
        use crate::db::models::{FileEntry, Trove, TroveType};
        use crate::db::schema::migrate;
        use crate::filesystem::CasStore;

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
    fn build_generation_from_db_rejects_root_without_init() {
        use crate::db::models::{FileEntry, Trove, TroveType};
        use crate::db::schema::migrate;
        use crate::filesystem::CasStore;

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

    #[test]
    fn runtime_root_init_detection_resolves_usr_merge_and_package_symlinks() {
        let file_refs = vec![FileEntryRef {
            path: "/usr/lib/systemd/systemd".to_string(),
            sha256_hash: "a".repeat(64),
            size: 6,
            permissions: 0o755,
            owner: None,
            group_name: None,
        }];
        let symlink_refs = vec![SymlinkEntryRef {
            path: "/usr/sbin/init".to_string(),
            target: "../lib/systemd/systemd".to_string(),
        }];

        assert!(generation_root_has_init_entrypoint(
            &file_refs,
            &symlink_refs
        ));
    }

    #[test]
    fn detect_kernel_version_does_not_panic() {
        let result = detect_kernel_version_from_troves(&[]);
        assert!(result.is_some() || result.is_none());
    }

    #[test]
    fn runtime_boot_asset_resolution_uses_arch_qualified_module_release() {
        use crate::db::models::TroveType;

        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.17.1-300.fc43.x86_64";
        let module_dir = tmp.path().join("lib/modules").join(release);
        std::fs::create_dir_all(&module_dir).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(module_dir.join("vmlinuz"), b"kernel").unwrap();
        std::fs::write(
            boot_root.join(format!("initramfs-{release}.img")),
            b"initramfs",
        )
        .unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let troves = vec![Trove::new(
            "kernel-core".to_string(),
            "6.17.1-300.fc43".to_string(),
            TroveType::Package,
        )];

        let sources = resolve_runtime_boot_asset_sources(&troves, &boot_root).unwrap();

        assert_eq!(sources.kernel_version, release);
        assert_eq!(sources.kernel, module_dir.join("vmlinuz"));
        assert_eq!(
            sources.initramfs,
            boot_root.join(format!("initramfs-{release}.img"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_boot_asset_resolution_generates_missing_initramfs_with_shell_dracut() {
        use crate::db::models::TroveType;

        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.17.1-300.fc43.x86_64";
        let module_dir = tmp.path().join("lib/modules").join(release);
        let fake_dracut = tmp.path().join("dracut");
        let fake_depmod = tmp.path().join("depmod");
        let fake_cpio = tmp.path().join("cpio");
        let dracut_args = tmp.path().join("dracut.args");
        std::fs::create_dir_all(&module_dir).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(module_dir.join("vmlinuz"), b"kernel").unwrap();
        std::fs::write(module_dir.join("modules.dep"), b"deps").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        write_executable(
            &fake_dracut,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprev=\nfor arg in \"$@\"; do out=\"$prev\"; prev=\"$arg\"; done\nprintf initramfs > \"$out\"\n",
                dracut_args.display()
            ),
        );
        write_executable(&fake_depmod, "#!/bin/sh\nexit 99\n");
        write_executable(&fake_cpio, "#!/bin/sh\nexit 0\n");

        let troves = vec![Trove::new(
            "kernel-core".to_string(),
            "6.17.1-300.fc43".to_string(),
            TroveType::Package,
        )];

        let sources = resolve_runtime_boot_asset_sources_with_tools(
            &troves,
            &boot_root,
            &fake_dracut,
            &fake_depmod,
            &fake_cpio,
        )
        .unwrap();

        assert_eq!(sources.kernel_version, release);
        assert_eq!(
            std::fs::read(boot_root.join(format!("initramfs-{release}.img"))).unwrap(),
            b"initramfs"
        );
        let args = std::fs::read_to_string(dracut_args).unwrap();
        assert!(
            args.lines().any(|line| line == "--omit") && args.lines().any(|line| line == "systemd"),
            "generation initramfs must omit dracut's partial systemd path so shell /init runs; got args:\n{args}"
        );
        assert!(
            !CONARY_DRACUT_MODULE_SETUP.contains("dracut-systemd"),
            "the Conary dracut module must not force systemd-initrd dependencies"
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_boot_asset_resolution_runs_depmod_before_dracut_when_modules_dep_is_missing() {
        use crate::db::models::TroveType;

        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.17.1-300.fc43.x86_64";
        let module_dir = tmp.path().join("lib/modules").join(release);
        let fake_dracut = tmp.path().join("dracut");
        let fake_depmod = tmp.path().join("depmod");
        let fake_cpio = tmp.path().join("cpio");
        std::fs::create_dir_all(&module_dir).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(module_dir.join("vmlinuz"), b"kernel").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        write_executable(
            &fake_depmod,
            "#!/bin/sh\nbasedir=/\nmoduledir=/lib/modules\nrelease=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    -b|--basedir) basedir=\"$2\"; shift 2 ;;\n    -m|--moduledir) moduledir=\"$2\"; shift 2 ;;\n    *) release=\"$1\"; shift ;;\n  esac\ndone\nprintf deps > \"${basedir}${moduledir}/${release}/modules.dep\"\n",
        );
        write_executable(
            &fake_dracut,
            "#!/bin/sh\nprev=\nfor arg in \"$@\"; do out=\"$prev\"; prev=\"$arg\"; done\nprintf initramfs > \"$out\"\n",
        );
        write_executable(&fake_cpio, "#!/bin/sh\nexit 0\n");

        let troves = vec![Trove::new(
            "kernel-core".to_string(),
            "6.17.1-300.fc43".to_string(),
            TroveType::Package,
        )];

        resolve_runtime_boot_asset_sources_with_tools(
            &troves,
            &boot_root,
            &fake_dracut,
            &fake_depmod,
            &fake_cpio,
        )
        .unwrap();

        assert!(module_dir.join("modules.dep").is_file());
        assert!(boot_root.join(format!("initramfs-{release}.img")).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn runtime_boot_asset_resolution_reports_missing_cpio_before_dracut() {
        use crate::db::models::TroveType;

        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.17.1-300.fc43.x86_64";
        let module_dir = tmp.path().join("lib/modules").join(release);
        let fake_dracut = tmp.path().join("dracut");
        let fake_depmod = tmp.path().join("depmod");
        let missing_cpio = tmp.path().join("missing-cpio");
        std::fs::create_dir_all(&module_dir).unwrap();
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(module_dir.join("vmlinuz"), b"kernel").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        write_executable(&fake_dracut, "#!/bin/sh\nexit 99\n");
        write_executable(&fake_depmod, "#!/bin/sh\nexit 99\n");

        let troves = vec![Trove::new(
            "kernel-core".to_string(),
            "6.17.1-300.fc43".to_string(),
            TroveType::Package,
        )];

        let error = resolve_runtime_boot_asset_sources_with_tools(
            &troves,
            &boot_root,
            &fake_dracut,
            &fake_depmod,
            &missing_cpio,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("missing required initramfs tool cpio"));
        assert!(!boot_root.join(format!("initramfs-{release}.img")).exists());
    }
}
