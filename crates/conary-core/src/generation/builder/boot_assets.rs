// conary-core/src/generation/builder/boot_assets.rs

use std::path::{Path, PathBuf};

use super::cas::artifact_root_for_generations_root;
use super::initramfs::generate_runtime_initramfs;
use super::kernel::{
    collect_boot_kernel_releases, collect_module_kernel_releases,
    detect_kernel_version_from_troves, module_kernel_path, push_unique_release,
    regular_file_exists, system_root_for_boot_root,
};
use super::runtime_inputs;
use super::sysroot::materialize_runtime_generation_sysroot;
use crate::db::models::Trove;
use crate::generation::artifact::{BootAssetSources, BootAssetsManifest, stage_boot_assets};

#[derive(Debug)]
pub(super) struct RuntimeBootAssetSources {
    pub(super) kernel_version: String,
    pub(super) kernel: PathBuf,
    pub(super) initramfs: PathBuf,
    pub(super) efi_bootloader: PathBuf,
    pub(super) _sysroot_workspace: Option<tempfile::TempDir>,
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

pub(super) fn resolve_generation_boot_asset_sources(
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

#[cfg(test)]
mod tests {
    use super::super::initramfs::{
        CONARY_DRACUT_MODULE_SETUP, RUNTIME_DRACUT_ADD_MODULES, RUNTIME_DRACUT_OMIT_MODULES,
    };
    use super::super::{FileEntryRef, runtime_inputs};
    use super::*;
    use crate::db::models::{Trove, TroveType};
    use crate::filesystem::CasStore;
    use std::path::Path;

    #[cfg(unix)]
    use super::super::test_support::write_executable;

    #[test]
    fn runtime_boot_asset_resolution_uses_arch_qualified_module_release() {
        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.17.1-300.fc44.x86_64";
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
            "6.17.1-300.fc44".to_string(),
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
    fn runtime_boot_asset_resolution_accepts_unversioned_boot_fixture_assets() {
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
        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.17.1-300.fc44.x86_64";
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
            "6.17.1-300.fc44".to_string(),
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
        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.17.1-300.fc44.x86_64";
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
            "6.17.1-300.fc44".to_string(),
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
        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.17.1-300.fc44.x86_64";
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
            "6.17.1-300.fc44".to_string(),
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
