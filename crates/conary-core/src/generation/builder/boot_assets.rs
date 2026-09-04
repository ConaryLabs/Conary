// conary-core/src/generation/builder/boot_assets.rs

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::cas::artifact_root_for_generations_root;
use super::initramfs::generate_runtime_initramfs;
use super::kernel::{
    collect_boot_kernel_releases, collect_module_kernel_releases, kernel_module_dir,
    module_kernel_path, regular_file_exists, solus_kernel_path, system_root_for_boot_root,
};
use super::runtime_inputs;
use super::sysroot::materialize_runtime_generation_sysroot;
use crate::filesystem::CasStore;
use crate::generation::artifact::{
    BootAssetSources, BootAssetsManifest, VerifiedGenerationBootAssets, stage_boot_assets,
    stage_reused_boot_assets,
};
use crate::generation::root_manifest::{GenerationRootEntry, capture_existing_payload_node};
use crate::payload::{PayloadContentAuthority, PayloadNodeKind};

#[derive(Debug)]
pub(super) struct RuntimeBootAssetSources {
    pub(super) kernel_version: String,
    pub(super) kernel: PathBuf,
    pub(super) initramfs: PathBuf,
    pub(super) efi_bootloader: PathBuf,
    pub(super) _sysroot_workspace: Option<tempfile::TempDir>,
    pub(super) staging: RuntimeBootAssetStaging,
}

#[derive(Debug)]
pub(super) enum RuntimeBootAssetStaging {
    Copy,
    Reuse(Box<VerifiedGenerationBootAssets>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InitramfsPolicy {
    ReuseExisting,
    GenerateConary,
}

pub(super) fn stage_runtime_boot_assets_from_sources(
    gen_dir: &Path,
    generation: i64,
    architecture: &str,
    sources: &RuntimeBootAssetSources,
) -> crate::Result<BootAssetsManifest> {
    let kernel_version = sources.kernel_version.as_str();
    if kernel_version.contains('/') || kernel_version.contains('\\') {
        return Err(crate::error::Error::InvalidPath(format!(
            "kernel version must not contain path separators: {kernel_version}"
        )));
    }

    match &sources.staging {
        RuntimeBootAssetStaging::Copy => stage_boot_assets(BootAssetSources {
            generation_dir: gen_dir,
            generation,
            architecture,
            kernel_version,
            kernel: &sources.kernel,
            initramfs: &sources.initramfs,
            efi_bootloader: &sources.efi_bootloader,
        }),
        RuntimeBootAssetStaging::Reuse(source) => {
            stage_reused_boot_assets(gen_dir, generation, architecture, source)
        }
    }
}

#[cfg(test)]
fn resolve_runtime_boot_asset_sources(boot_root: &Path) -> crate::Result<RuntimeBootAssetSources> {
    resolve_runtime_boot_asset_sources_with_tools(
        boot_root,
        Path::new("dracut"),
        Path::new("depmod"),
        Path::new("cpio"),
    )
}

pub(super) fn resolve_generation_boot_asset_sources(
    runtime_inputs: &mut runtime_inputs::RuntimeGenerationInputs,
    generations_root: &Path,
    boot_root: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    resolve_generation_boot_asset_sources_with_tools(
        runtime_inputs,
        generations_root,
        boot_root,
        Path::new("dracut"),
        Path::new("depmod"),
        Path::new("cpio"),
    )
}

pub(super) fn resolve_generation_boot_asset_sources_for_publication(
    conn: &rusqlite::Connection,
    runtime_inputs: &mut runtime_inputs::RuntimeGenerationInputs,
    generations_root: &Path,
    boot_root: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    if boot_root == Path::new("/boot")
        && let Some(sources) =
            super::boot_reuse::resolve_reusable_boot_assets(conn, generations_root)?
    {
        return Ok(sources);
    }
    resolve_generation_boot_asset_sources(runtime_inputs, generations_root, boot_root)
}

fn resolve_generation_boot_asset_sources_with_tools(
    runtime_inputs: &mut runtime_inputs::RuntimeGenerationInputs,
    generations_root: &Path,
    boot_root: &Path,
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    if boot_root != Path::new("/boot") {
        return resolve_runtime_boot_asset_sources_with_tools(boot_root, dracut, depmod, cpio);
    }

    let artifact_root = artifact_root_for_generations_root(generations_root)?;
    let objects_dir = artifact_root.join("objects");
    let sysroot_workspace =
        materialize_runtime_generation_sysroot(runtime_inputs, &objects_dir, &artifact_root)?;
    let generation_boot_root = sysroot_workspace.path().join("boot");
    let mut sources = resolve_runtime_boot_asset_sources_with_tools_and_policy(
        &generation_boot_root,
        dracut,
        depmod,
        cpio,
        InitramfsPolicy::GenerateConary,
    )?;
    retain_generated_kernel_module_metadata(
        runtime_inputs,
        &objects_dir,
        sysroot_workspace.path(),
        &sources.kernel_version,
    )?;
    sources._sysroot_workspace = Some(sysroot_workspace);
    Ok(sources)
}

/// Retain exact depmod output in the immutable generation authority.
///
/// The boot-asset sysroot starts as an exact materialization of
/// `runtime_inputs`. Any additional node under the selected kernel's module
/// directory was therefore produced by the typed depmod preparation that ran
/// before dracut. Capture every such node and its bytes instead of keeping the
/// mutation only in the temporary initramfs workspace.
fn retain_generated_kernel_module_metadata(
    runtime_inputs: &mut runtime_inputs::RuntimeGenerationInputs,
    objects_dir: &Path,
    sysroot: &Path,
    release: &str,
) -> crate::Result<()> {
    let (module_dir, _) = kernel_module_dir(sysroot, release).ok_or_else(|| {
        crate::Error::NotFound(format!(
            "missing kernel module directory for {release}; expected lib/modules/{release} or usr/lib/modules/{release}"
        ))
    })?;
    // A usr-merged root exposes the same tree through `/lib -> usr/lib`.
    // Resolve that manifest-owned alias before deriving virtual paths so the
    // generated entries retain the canonical `/usr/lib/modules` authority.
    let module_dir = std::fs::canonicalize(&module_dir)?;
    let existing_paths = runtime_inputs
        .generation
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    let mut workspace_paths = Vec::new();
    collect_workspace_nodes(&module_dir, &mut workspace_paths)?;
    workspace_paths.sort();

    let cas = CasStore::new(objects_dir)?;
    let mut generated_entries = Vec::new();
    for path in workspace_paths {
        let relative = path.strip_prefix(sysroot).map_err(|error| {
            crate::Error::InvalidPath(format!(
                "generated kernel-module path {} is outside sysroot {}: {error}",
                path.display(),
                sysroot.display()
            ))
        })?;
        let virtual_path = Path::new("/").join(relative);
        let virtual_path = virtual_path.to_str().ok_or_else(|| {
            crate::Error::InvalidPath(format!(
                "generated kernel-module path is not UTF-8: {}",
                virtual_path.display()
            ))
        })?;
        if existing_paths.contains(virtual_path) {
            continue;
        }

        let node = capture_existing_payload_node(&path)?;
        let content = if matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
            let bytes = std::fs::read(&path)?;
            let size = u64::try_from(bytes.len()).map_err(|_| {
                crate::Error::IoError(format!(
                    "generated kernel-module metadata is too large to capture: {}",
                    path.display()
                ))
            })?;
            Some(PayloadContentAuthority {
                sha256: cas.store_private_copy(&bytes)?,
                size,
            })
        } else {
            None
        };
        generated_entries.push(GenerationRootEntry {
            path: virtual_path.to_string(),
            node,
            content,
        });
    }

    runtime_inputs.generation.entries.extend(generated_entries);
    runtime_inputs
        .generation
        .entries
        .sort_by(|left, right| left.path.cmp(&right.path));
    runtime_inputs.generation.validate()
}

fn collect_workspace_nodes(root: &Path, paths: &mut Vec<PathBuf>) -> crate::Result<()> {
    let mut entries = std::fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        paths.push(path.clone());
        if metadata.is_dir() {
            collect_workspace_nodes(&path, paths)?;
        }
    }
    Ok(())
}

fn resolve_runtime_boot_asset_sources_with_tools(
    boot_root: &Path,
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
) -> crate::Result<RuntimeBootAssetSources> {
    resolve_runtime_boot_asset_sources_with_tools_and_policy(
        boot_root,
        dracut,
        depmod,
        cpio,
        InitramfsPolicy::ReuseExisting,
    )
}

fn resolve_runtime_boot_asset_sources_with_tools_and_policy(
    boot_root: &Path,
    dracut: &Path,
    depmod: &Path,
    cpio: &Path,
    initramfs_policy: InitramfsPolicy,
) -> crate::Result<RuntimeBootAssetSources> {
    let system_root = system_root_for_boot_root(boot_root)?;
    let mut candidate_releases = Vec::new();
    collect_boot_kernel_releases(boot_root, &mut candidate_releases)?;
    collect_module_kernel_releases(&system_root, boot_root, &mut candidate_releases)?;
    if candidate_releases.is_empty() {
        return Err(crate::error::Error::NotFound(format!(
            "generation boot root {} has no exact versioned kernel identity in /boot or /lib/modules",
            boot_root.display()
        )));
    }

    let mut complete = Vec::new();
    let mut failures = Vec::new();
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
            Ok(sources) => complete.push(sources),
            Err(error) => failures.push((release, error)),
        }
    }
    match complete.len() {
        1 => Ok(complete.pop().expect("one complete boot asset set")),
        0 => Err(crate::error::Error::NotFound(format!(
            "generation boot root {} has no complete exact kernel/initramfs/EFI asset set: {}",
            boot_root.display(),
            failures
                .into_iter()
                .map(|(release, error)| format!("{release}: {error}"))
                .collect::<Vec<_>>()
                .join("; ")
        ))),
        _ => Err(crate::error::Error::InvalidPath(format!(
            "generation boot root {} has multiple complete kernel asset sets {:?}; select one exact kernel before building the generation",
            boot_root.display(),
            complete
                .iter()
                .map(|sources| sources.kernel_version.as_str())
                .collect::<Vec<_>>()
        ))),
    }
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
    let kernel = if regular_file_exists(&versioned_kernel) {
        versioned_kernel
    } else {
        module_kernel_path(system_root, release)
            .or_else(|| solus_kernel_path(boot_root, release))
            .ok_or_else(|| {
                crate::error::Error::NotFound(format!(
                    "missing exact versioned boot asset kernel for {release}; expected {}, a module kernel at lib/modules/{release}/vmlinuz, or the exact Solus module-release pair",
                    boot_root.join(format!("vmlinuz-{release}")).display(),
                ))
            })?
    };

    let versioned_initramfs = boot_root.join(format!("initramfs-{release}.img"));
    let force_conary_initramfs = initramfs_policy == InitramfsPolicy::GenerateConary;
    let initramfs = versioned_initramfs;
    if force_conary_initramfs || !regular_file_exists(&initramfs) {
        generate_runtime_initramfs(dracut, depmod, cpio, system_root, release, &initramfs)?;
    }
    if !regular_file_exists(&initramfs) {
        return Err(crate::error::Error::NotFound(format!(
            "missing required boot asset initramfs for {release} at {}; generate it with dracut or install a package hook that stages runtime boot assets before building a generation",
            initramfs.display()
        )));
    }

    let efi_bootloader = resolve_efi_bootloader_source(boot_root, system_root)?;

    Ok(RuntimeBootAssetSources {
        kernel_version: release.to_string(),
        kernel,
        initramfs,
        efi_bootloader,
        _sysroot_workspace: None,
        staging: RuntimeBootAssetStaging::Copy,
    })
}

/// ESP location the generation stages its EFI bootloader from.
const ESP_BOOTLOADER_REL: &str = "EFI/BOOT/BOOTX64.EFI";

/// systemd-boot's own installed EFI binary, the file `bootctl install` copies
/// to the ESP. Conary captures that mutation instead of executing it, so the
/// generation builder owns the copy.
const SYSTEMD_BOOT_EFI_REL: &str = "usr/lib/systemd/boot/efi/systemd-bootx64.efi";

/// Resolve the exact file staged as the generation's `BOOTX64.EFI`.
///
/// A sysroot that already carries an ESP layout wins. Otherwise the
/// systemd-boot package's shipped binary is staged directly, which is what
/// `bootctl install` would have done had Conary executed it.
fn resolve_efi_bootloader_source(boot_root: &Path, system_root: &Path) -> crate::Result<PathBuf> {
    let staged_esp = boot_root.join(ESP_BOOTLOADER_REL);
    if regular_file_exists(&staged_esp) {
        return Ok(staged_esp);
    }

    let systemd_boot_efi = system_root.join(SYSTEMD_BOOT_EFI_REL);
    if regular_file_exists(&systemd_boot_efi) {
        return Ok(systemd_boot_efi);
    }

    Err(crate::error::Error::NotFound(format!(
        "missing required boot asset efi_bootloader; expected a staged ESP binary at {} or systemd-boot's installed binary at {}",
        staged_esp.display(),
        systemd_boot_efi.display()
    )))
}

#[cfg(test)]
mod tests;
