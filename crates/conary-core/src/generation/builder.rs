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

use tracing::{info, warn};

use crate::db::models::{FileEntry, StateEngine, SystemState, Trove};
use crate::generation::artifact::{
    ArtifactWriteInputs, BootAssetSources, CasObjectRef, CasObjectVerification,
    deduplicate_sort_cas_objects, stage_boot_assets,
    verify_cas_object_files_exist_with_expected_sizes, write_generation_artifact,
};
use crate::generation::metadata::{
    GENERATION_FORMAT, GenerationMetadata, ROOT_SYMLINKS, clear_generation_pending,
    mark_generation_pending,
};
mod erofs;
mod runtime_inputs;

pub use erofs::{BuildResult, FileEntryRef, SymlinkEntryRef, build_erofs_image, hex_to_digest};

const CONARY_DRACUT_MODULE_SETUP: &str =
    include_str!("../../../../packaging/dracut/90conary/module-setup.sh");
const CONARY_DRACUT_INIT: &str =
    include_str!("../../../../packaging/dracut/90conary/conary-init.sh");
const CONARY_DRACUT_GENERATOR: &str =
    include_str!("../../../../packaging/dracut/90conary/conary-generator.sh");
const RUNTIME_DRACUT_ADD_MODULES: &str = "conary";
const RUNTIME_DRACUT_OMIT_MODULES: &str = "systemd";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationActivation {
    /// Publish the generated DB snapshot as the active state immediately.
    ///
    /// Use only for paths that also publish/mount the generation in the same
    /// operation, such as composefs-native package mutation.
    Active,
    /// Leave the generated DB snapshot inactive until an explicit generation
    /// switch selects it for the next boot.
    Inactive,
}

impl GenerationActivation {
    fn activates_state(self) -> bool {
        matches!(self, Self::Active)
    }
}

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

fn verify_runtime_generation_cas_object_presence(
    generations_root: &Path,
    cas_objects: &[CasObjectRef],
) -> crate::Result<()> {
    let artifact_root = artifact_root_for_generations_root(generations_root)?;
    verify_cas_object_files_exist_with_expected_sizes(&artifact_root.join("objects"), cas_objects)
}

fn artifact_root_for_generations_root(generations_root: &Path) -> crate::Result<PathBuf> {
    generations_root
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            crate::error::Error::InvalidPath(format!(
                "generation root {} has no parent artifact root",
                generations_root.display()
            ))
        })
}

fn materialize_runtime_generation_sysroot(
    runtime_inputs: &runtime_inputs::RuntimeGenerationInputs,
    objects_dir: &Path,
    artifact_root: &Path,
) -> crate::Result<tempfile::TempDir> {
    let sysroot = tempfile::Builder::new()
        .prefix(".generation-sysroot-")
        .tempdir_in(artifact_root)
        .map_err(|e| {
            crate::error::Error::IoError(format!(
                "failed to create temporary generation sysroot under {}: {e}",
                artifact_root.display()
            ))
        })?;

    for file in &runtime_inputs.file_refs {
        materialize_runtime_regular_file(sysroot.path(), objects_dir, file)?;
    }
    for symlink in &runtime_inputs.symlink_refs {
        materialize_runtime_symlink(sysroot.path(), symlink)?;
    }
    materialize_root_symlinks(sysroot.path())?;
    materialize_runtime_sysroot_base_dirs(sysroot.path())?;

    Ok(sysroot)
}

fn materialize_runtime_regular_file(
    sysroot: &Path,
    objects_dir: &Path,
    file: &FileEntryRef,
) -> crate::Result<()> {
    let rel_path = relative_runtime_path(&file.path)?;
    let dest = sysroot.join(rel_path);
    let source = crate::filesystem::object_path(objects_dir, &file.sha256_hash)?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match std::fs::hard_link(&source, &dest) {
        Ok(()) => Ok(()),
        Err(_) => std::fs::copy(&source, &dest)
            .map(|_| ())
            .map_err(crate::error::Error::Io),
    }
}

fn materialize_runtime_symlink(sysroot: &Path, symlink: &SymlinkEntryRef) -> crate::Result<()> {
    let rel_path = relative_runtime_path(&symlink.path)?;
    let dest = sysroot.join(rel_path);

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if dest.exists() || dest.is_symlink() {
        std::fs::remove_file(&dest)?;
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&symlink.target, &dest)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        Err(crate::error::Error::NotImplemented(
            "runtime generation sysroot materialization requires Unix symlinks".to_string(),
        ))
    }
}

fn materialize_root_symlinks(sysroot: &Path) -> crate::Result<()> {
    for (link, target) in ROOT_SYMLINKS {
        let dest = sysroot.join(link);
        if dest.exists() || dest.is_symlink() {
            continue;
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, dest)?;
        }

        #[cfg(not(unix))]
        {
            return Err(crate::error::Error::NotImplemented(
                "runtime generation sysroot materialization requires Unix symlinks".to_string(),
            ));
        }
    }
    Ok(())
}

fn materialize_runtime_sysroot_base_dirs(sysroot: &Path) -> crate::Result<()> {
    for dir in ["dev", "proc", "run", "sys", "tmp", "var", "var/tmp"] {
        std::fs::create_dir_all(sysroot.join(dir))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        for dir in ["tmp", "var/tmp"] {
            std::fs::set_permissions(sysroot.join(dir), std::fs::Permissions::from_mode(0o1777))?;
        }
    }

    Ok(())
}

fn relative_runtime_path(path: &str) -> crate::Result<&Path> {
    let rel = path.strip_prefix('/').ok_or_else(|| {
        crate::error::Error::InvalidPath(format!(
            "runtime generation path must be absolute: {path}"
        ))
    })?;
    if rel.is_empty() || rel.split('/').any(|component| component == "..") {
        return Err(crate::error::Error::InvalidPath(format!(
            "runtime generation path escapes root: {path}"
        )));
    }
    Ok(Path::new(rel))
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

#[derive(Debug)]
struct RuntimeBootAssetSources {
    kernel_version: String,
    kernel: PathBuf,
    initramfs: PathBuf,
    efi_bootloader: PathBuf,
    _sysroot_workspace: Option<tempfile::TempDir>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitramfsPolicy {
    ReuseExisting,
    GenerateConary,
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

#[cfg(test)]
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

fn resolve_generation_boot_asset_sources(
    troves: &[Trove],
    runtime_inputs: &runtime_inputs::RuntimeGenerationInputs,
    generations_root: &Path,
    boot_root: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    resolve_generation_boot_asset_sources_with_tools(
        troves,
        runtime_inputs,
        generations_root,
        boot_root,
        Path::new("dracut"),
        Path::new("depmod"),
        Path::new("cpio"),
    )
}

fn resolve_generation_boot_asset_sources_with_tools(
    troves: &[Trove],
    runtime_inputs: &runtime_inputs::RuntimeGenerationInputs,
    generations_root: &Path,
    boot_root: &Path,
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    if boot_root != Path::new("/boot") {
        return resolve_runtime_boot_asset_sources_with_tools(
            troves, boot_root, dracut, depmod, cpio,
        );
    }

    let artifact_root = artifact_root_for_generations_root(generations_root)?;
    let objects_dir = artifact_root.join("objects");
    let sysroot_workspace =
        materialize_runtime_generation_sysroot(runtime_inputs, &objects_dir, &artifact_root)?;
    let generation_boot_root = sysroot_workspace.path().join("boot");
    let mut sources = resolve_runtime_boot_asset_sources_with_tools_and_policy(
        troves,
        &generation_boot_root,
        dracut,
        depmod,
        cpio,
        InitramfsPolicy::GenerateConary,
    )?;
    sources._sysroot_workspace = Some(sysroot_workspace);
    Ok(sources)
}

fn resolve_runtime_boot_asset_sources_with_tools(
    troves: &[Trove],
    boot_root: &Path,
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    resolve_runtime_boot_asset_sources_with_tools_and_policy(
        troves,
        boot_root,
        dracut,
        depmod,
        cpio,
        InitramfsPolicy::ReuseExisting,
    )
}

fn resolve_runtime_boot_asset_sources_with_tools_and_policy(
    troves: &[Trove],
    boot_root: &Path,
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
    initramfs_policy: InitramfsPolicy,
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
            initramfs_policy,
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
    initramfs_policy: InitramfsPolicy,
) -> crate::Result<RuntimeBootAssetSources> {
    let versioned_kernel = boot_root.join(format!("vmlinuz-{release}"));
    let unversioned_kernel = boot_root.join("vmlinuz");
    let kernel = if regular_file_exists(&versioned_kernel) {
        versioned_kernel
    } else {
        module_kernel_path(system_root, release)
            .or_else(|| regular_file_exists(&unversioned_kernel).then_some(unversioned_kernel))
            .ok_or_else(|| {
                crate::error::Error::NotFound(format!(
                    "missing required boot asset kernel for {release}; expected {}, {}, or a module kernel at lib/modules/{release}/vmlinuz",
                    boot_root.join(format!("vmlinuz-{release}")).display(),
                    boot_root.join("vmlinuz").display()
                ))
            })?
    };

    let versioned_initramfs = boot_root.join(format!("initramfs-{release}.img"));
    let unversioned_initramfs = boot_root.join("initramfs.img");
    let force_conary_initramfs = initramfs_policy == InitramfsPolicy::GenerateConary;
    let initramfs = if force_conary_initramfs {
        versioned_initramfs
    } else {
        select_existing_or_versioned_initramfs(versioned_initramfs, unversioned_initramfs)
    };
    if force_conary_initramfs || !regular_file_exists(&initramfs) {
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
        _sysroot_workspace: None,
    })
}

fn select_existing_or_versioned_initramfs(
    versioned_initramfs: PathBuf,
    unversioned_initramfs: PathBuf,
) -> PathBuf {
    if regular_file_exists(&versioned_initramfs) {
        versioned_initramfs
    } else if regular_file_exists(&unversioned_initramfs) {
        unversioned_initramfs
    } else {
        versioned_initramfs
    }
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
    let (runtime_module_dir, _module_dir_arg) =
        kernel_module_dir(system_root, release).ok_or_else(|| {
            crate::error::Error::NotFound(format!(
                "missing kernel module directory for {release}; expected lib/modules/{release} or usr/lib/modules/{release}"
            ))
        })?;

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
    write_dracut_module_file(&module_dir.join("conary-init.sh"), CONARY_DRACUT_INIT)?;
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
        .arg("--sysroot")
        .arg(system_root)
        .arg("--kmoddir")
        .arg(&runtime_module_dir)
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
        Ok(())
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
        if matches!(
            trove.name.as_str(),
            "kernel-core" | "kernel-modules-core" | "kernel-modules"
        ) || trove.name.starts_with("linux-image")
        {
            return Some(trove.version.clone());
        }
    }

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
    fn runtime_generation_db_with_invalid_regular_file()
    -> (tempfile::TempDir, rusqlite::Connection, PathBuf, PathBuf) {
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
        let mut bad = FileEntry::new(
            "/usr/bin/bad".to_string(),
            "not-a-sha256".to_string(),
            0,
            0o100755,
            trove_id,
        );
        bad.insert(&conn).unwrap();
        let mut init = FileEntry::new(
            "/usr/sbin/init".to_string(),
            init_hash,
            b"init".len() as i64,
            0o100755,
            trove_id,
        );
        init.insert(&conn).unwrap();

        (tmp, conn, generations_root, boot_root)
    }

    #[cfg(feature = "composefs-rs")]
    fn runtime_generation_db_with_missing_regular_file_cas_object() -> (
        tempfile::TempDir,
        rusqlite::Connection,
        PathBuf,
        PathBuf,
        String,
    ) {
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
        let init_hash = cas.store(b"init").unwrap();
        let missing_hash = CasStore::compute_sha256(b"missing");
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut trove = Trove::new(
            "kernel-core".to_string(),
            "6.19.8-conary".to_string(),
            TroveType::Package,
        );
        trove.architecture = Some("x86_64".to_string());
        let trove_id = trove.insert(&conn).unwrap();
        let mut missing = FileEntry::new(
            "/usr/bin/missing".to_string(),
            missing_hash.clone(),
            b"missing".len() as i64,
            0o100755,
            trove_id,
        );
        missing.insert(&conn).unwrap();
        let mut init = FileEntry::new(
            "/usr/sbin/init".to_string(),
            init_hash,
            b"init".len() as i64,
            0o100755,
            trove_id,
        );
        init.insert(&conn).unwrap();

        (tmp, conn, generations_root, boot_root, missing_hash)
    }

    fn assert_invalid_runtime_input_error(error: &str) {
        for snippet in [
            "exportable runtime generation is not self-contained",
            "package kernel-core",
            "/usr/bin/bad",
            "invalid SHA-256 digest for regular file",
            "conary system adopt --system --full",
            "conary system takeover --up-to generation",
        ] {
            assert!(
                error.contains(snippet),
                "expected error to contain {snippet:?}; got {error}"
            );
        }
    }

    fn assert_missing_cas_object_error(error: &str, hash: &str) {
        for snippet in ["missing CAS object", hash] {
            assert!(
                error.contains(snippet),
                "expected error to contain {snippet:?}; got {error}"
            );
        }
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

    #[test]
    fn detect_kernel_version_prefers_payload_kernel_over_meta_package() {
        use crate::db::models::TroveType;

        let troves = vec![
            Trove::new(
                "kernel".to_string(),
                "6.17.1-300.fc43".to_string(),
                TroveType::Package,
            ),
            Trove::new(
                "kernel-core".to_string(),
                "6.19.10-300.fc44".to_string(),
                TroveType::Package,
            ),
        ];

        assert_eq!(
            detect_kernel_version_from_troves(&troves).as_deref(),
            Some("6.19.10-300.fc44")
        );
    }

    #[test]
    fn runtime_boot_asset_resolution_accepts_unversioned_boot_fixture_assets() {
        use crate::db::models::TroveType;

        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.19.8";
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("vmlinuz"), b"kernel").unwrap();
        std::fs::write(boot_root.join("initramfs.img"), b"initramfs").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let troves = vec![Trove::new(
            "kernel-core".to_string(),
            release.to_string(),
            TroveType::Package,
        )];

        let sources = resolve_runtime_boot_asset_sources(&troves, &boot_root).unwrap();

        assert_eq!(sources.kernel_version, release);
        assert_eq!(sources.kernel, boot_root.join("vmlinuz"));
        assert_eq!(sources.initramfs, boot_root.join("initramfs.img"));
    }

    #[test]
    fn generation_boot_asset_resolution_materializes_default_boot_from_cas_inputs() {
        use crate::db::models::TroveType;
        use crate::filesystem::CasStore;

        let tmp = tempfile::TempDir::new().unwrap();
        let generations_root = tmp.path().join("generations");
        let objects_dir = tmp.path().join("objects");
        let fake_dracut = tmp.path().join("dracut");
        let fake_depmod = tmp.path().join("depmod");
        let fake_cpio = tmp.path().join("cpio");
        std::fs::create_dir_all(&generations_root).unwrap();
        let cas = CasStore::new(&objects_dir).unwrap();

        let release = "6.20.0-conary";
        let kernel_hash = cas.store(b"cas-kernel").unwrap();
        let initramfs_hash = cas.store(b"cas-initramfs").unwrap();
        let efi_hash = cas.store(b"cas-efi").unwrap();
        let modules_dep_hash = cas.store(b"modules-dep").unwrap();
        let runtime_inputs = runtime_inputs::RuntimeGenerationInputs {
            file_refs: vec![
                FileEntryRef {
                    path: format!("/boot/vmlinuz-{release}"),
                    sha256_hash: kernel_hash,
                    size: b"cas-kernel".len() as u64,
                    permissions: 0o100644,
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: format!("/boot/initramfs-{release}.img"),
                    sha256_hash: initramfs_hash,
                    size: b"cas-initramfs".len() as u64,
                    permissions: 0o100644,
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: "/boot/EFI/BOOT/BOOTX64.EFI".to_string(),
                    sha256_hash: efi_hash,
                    size: b"cas-efi".len() as u64,
                    permissions: 0o100644,
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: format!("/usr/lib/modules/{release}/modules.dep"),
                    sha256_hash: modules_dep_hash,
                    size: b"modules-dep".len() as u64,
                    permissions: 0o100644,
                    owner: None,
                    group_name: None,
                },
            ],
            symlink_refs: Vec::new(),
            adopted_track_count: 0,
        };
        write_executable(
            &fake_dracut,
            "#!/bin/sh\nprev=\nfor arg in \"$@\"; do out=\"$prev\"; prev=\"$arg\"; done\nprintf generated-initramfs > \"$out\"\n",
        );
        write_executable(&fake_depmod, "#!/bin/sh\nexit 99\n");
        write_executable(&fake_cpio, "#!/bin/sh\nexit 0\n");
        let troves = vec![Trove::new(
            "kernel-core".to_string(),
            release.to_string(),
            TroveType::Package,
        )];

        let sources = resolve_generation_boot_asset_sources_with_tools(
            &troves,
            &runtime_inputs,
            &generations_root,
            Path::new("/boot"),
            &fake_dracut,
            &fake_depmod,
            &fake_cpio,
        )
        .unwrap();

        assert!(sources.kernel.starts_with(tmp.path()));
        let sysroot = sources
            ._sysroot_workspace
            .as_ref()
            .expect("default runtime boot assets should retain their sysroot workspace");
        assert!(sysroot.path().join("tmp").is_dir());
        assert!(sysroot.path().join("var/tmp").is_dir());
        assert_eq!(std::fs::read(sources.kernel).unwrap(), b"cas-kernel");
        assert_eq!(
            std::fs::read(sources.initramfs).unwrap(),
            b"generated-initramfs"
        );
        assert_eq!(std::fs::read(sources.efi_bootloader).unwrap(), b"cas-efi");
    }

    #[cfg(unix)]
    #[test]
    fn generation_boot_asset_resolution_regenerates_conary_initramfs_from_materialized_sysroot() {
        use crate::db::models::TroveType;
        use crate::filesystem::CasStore;

        let tmp = tempfile::TempDir::new().unwrap();
        let generations_root = tmp.path().join("generations");
        let objects_dir = tmp.path().join("objects");
        let fake_dracut = tmp.path().join("dracut");
        let fake_depmod = tmp.path().join("depmod");
        let fake_cpio = tmp.path().join("cpio");
        let dracut_args = tmp.path().join("dracut.args");
        std::fs::create_dir_all(&generations_root).unwrap();
        let cas = CasStore::new(&objects_dir).unwrap();

        let release = "6.20.0-conary";
        let kernel_hash = cas.store(b"cas-kernel").unwrap();
        let adopted_initramfs_hash = cas.store(b"adopted-host-initramfs").unwrap();
        let efi_hash = cas.store(b"cas-efi").unwrap();
        let modules_dep_hash = cas.store(b"modules-dep").unwrap();
        let runtime_inputs = runtime_inputs::RuntimeGenerationInputs {
            file_refs: vec![
                FileEntryRef {
                    path: format!("/boot/vmlinuz-{release}"),
                    sha256_hash: kernel_hash,
                    size: b"cas-kernel".len() as u64,
                    permissions: 0o100644,
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: format!("/boot/initramfs-{release}.img"),
                    sha256_hash: adopted_initramfs_hash,
                    size: b"adopted-host-initramfs".len() as u64,
                    permissions: 0o100644,
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: "/boot/EFI/BOOT/BOOTX64.EFI".to_string(),
                    sha256_hash: efi_hash,
                    size: b"cas-efi".len() as u64,
                    permissions: 0o100644,
                    owner: None,
                    group_name: None,
                },
                FileEntryRef {
                    path: format!("/usr/lib/modules/{release}/modules.dep"),
                    sha256_hash: modules_dep_hash,
                    size: b"modules-dep".len() as u64,
                    permissions: 0o100644,
                    owner: None,
                    group_name: None,
                },
            ],
            symlink_refs: Vec::new(),
            adopted_track_count: 0,
        };
        write_executable(
            &fake_dracut,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprev=\nfor arg in \"$@\"; do out=\"$prev\"; prev=\"$arg\"; done\nprintf conary-initramfs > \"$out\"\n",
                dracut_args.display()
            ),
        );
        write_executable(&fake_depmod, "#!/bin/sh\nexit 99\n");
        write_executable(&fake_cpio, "#!/bin/sh\nexit 0\n");
        let troves = vec![Trove::new(
            "kernel-core".to_string(),
            release.to_string(),
            TroveType::Package,
        )];

        let sources = resolve_generation_boot_asset_sources_with_tools(
            &troves,
            &runtime_inputs,
            &generations_root,
            Path::new("/boot"),
            &fake_dracut,
            &fake_depmod,
            &fake_cpio,
        )
        .unwrap();

        assert_eq!(sources.kernel_version, release);
        assert_eq!(
            std::fs::read(&sources.initramfs).unwrap(),
            b"conary-initramfs"
        );
        let args = std::fs::read_to_string(dracut_args).unwrap();
        assert!(args.lines().any(|line| line == "--add"));
        assert!(args.lines().any(|line| line == RUNTIME_DRACUT_ADD_MODULES));
        assert!(args.lines().any(|line| line == "--omit"));
        assert!(args.lines().any(|line| line == RUNTIME_DRACUT_OMIT_MODULES));
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
