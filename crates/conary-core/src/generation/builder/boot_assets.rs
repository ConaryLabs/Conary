// conary-core/src/generation/builder/boot_assets.rs

use std::path::{Path, PathBuf};

use super::cas::artifact_root_for_generations_root;
use super::initramfs::generate_runtime_initramfs;
use super::kernel::{
    collect_boot_kernel_releases, collect_module_kernel_releases, module_kernel_path,
    regular_file_exists, system_root_for_boot_root,
};
use super::runtime_inputs;
use super::sysroot::materialize_runtime_generation_sysroot;
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
fn resolve_runtime_boot_asset_sources(boot_root: &Path) -> crate::Result<RuntimeBootAssetSources> {
    resolve_runtime_boot_asset_sources_with_tools(
        boot_root,
        Path::new("dracut"),
        Path::new("depmod"),
        Path::new("cpio"),
    )
}

pub(super) fn resolve_generation_boot_asset_sources(
    runtime_inputs: &runtime_inputs::RuntimeGenerationInputs,
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

fn resolve_generation_boot_asset_sources_with_tools(
    runtime_inputs: &runtime_inputs::RuntimeGenerationInputs,
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
    sources._sysroot_workspace = Some(sysroot_workspace);
    Ok(sources)
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
    let system_root = system_root_for_boot_root(boot_root);
    let mut candidate_releases = Vec::new();
    collect_boot_kernel_releases(boot_root, &mut candidate_releases)?;
    collect_module_kernel_releases(&system_root, &mut candidate_releases)?;
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
            .ok_or_else(|| {
                crate::error::Error::NotFound(format!(
                    "missing exact versioned boot asset kernel for {release}; expected {} or a module kernel at lib/modules/{release}/vmlinuz",
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

#[cfg(test)]
mod tests {
    use super::super::initramfs::{
        CONARY_DRACUT_MODULE_SETUP, RUNTIME_DRACUT_ADD_MODULES, RUNTIME_DRACUT_OMIT_MODULES,
    };
    use super::super::runtime_inputs;
    use super::*;
    use crate::filesystem::CasStore;
    use crate::generation::root_manifest::{
        GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
        MutableStateManifest,
    };
    use crate::payload::{
        PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, ResolvedPayloadNode,
    };
    use std::collections::BTreeSet;
    use std::path::Path;

    #[cfg(unix)]
    use super::super::test_support::write_executable;

    fn exact_runtime_inputs(
        files: Vec<(String, String, u64)>,
    ) -> runtime_inputs::RuntimeGenerationInputs {
        let mut directory_paths = BTreeSet::new();
        for (path, _, _) in &files {
            let mut parent = Path::new(path).parent();
            while let Some(path) = parent {
                if path == Path::new("/") {
                    break;
                }
                directory_paths.insert(path.to_string_lossy().into_owned());
                parent = path.parent();
            }
        }
        let mut entries = directory_paths
            .into_iter()
            .map(|path| GenerationRootEntry {
                path,
                node: directory_node(),
                content: None,
            })
            .collect::<Vec<_>>();
        entries.extend(
            files
                .into_iter()
                .map(|(path, sha256, size)| GenerationRootEntry {
                    path,
                    node: owned_regular_node(0o644),
                    content: Some(PayloadContentAuthority { sha256, size }),
                }),
        );
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        runtime_inputs::RuntimeGenerationInputs {
            generation: GenerationRootManifest {
                version: GENERATION_ROOT_MANIFEST_VERSION,
                root: directory_node(),
                entries,
            },
            state: MutableStateManifest::empty(),
            adopted_track_count: 0,
        }
    }

    fn directory_node() -> ResolvedPayloadNode {
        let mut node = owned_regular_source(0o755);
        node.kind = PayloadNodeKind::Directory;
        node.mode = libc::S_IFDIR | 0o755;
        ResolvedPayloadNode::from_numeric_source(node).unwrap()
    }

    fn owned_regular_node(permissions: u32) -> ResolvedPayloadNode {
        ResolvedPayloadNode::from_numeric_source(owned_regular_source(permissions)).unwrap()
    }

    fn owned_regular_source(permissions: u32) -> PayloadNode {
        let mut node = PayloadNode::regular(permissions);
        node.user = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        };
        node.group = PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        };
        node
    }

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

        let sources = resolve_runtime_boot_asset_sources(&boot_root).unwrap();

        assert_eq!(sources.kernel_version, release);
        assert_eq!(sources.kernel, module_dir.join("vmlinuz"));
        assert_eq!(
            sources.initramfs,
            boot_root.join(format!("initramfs-{release}.img"))
        );
    }
    #[test]
    fn runtime_boot_asset_resolution_rejects_unversioned_assets_without_release_authority() {
        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.19.8";
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("vmlinuz"), b"kernel").unwrap();
        std::fs::write(boot_root.join("initramfs.img"), b"initramfs").unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let error = resolve_runtime_boot_asset_sources(&boot_root)
            .unwrap_err()
            .to_string();

        assert!(error.contains("no exact versioned kernel identity"));
        assert!(!error.contains(release));
    }

    #[test]
    fn runtime_boot_asset_resolution_never_pairs_unversioned_kernel_with_versioned_initramfs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        let release = "6.19.8";
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("vmlinuz"), b"wrong-kernel").unwrap();
        std::fs::write(
            boot_root.join(format!("initramfs-{release}.img")),
            b"initramfs",
        )
        .unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

        let error = resolve_runtime_boot_asset_sources(&boot_root)
            .unwrap_err()
            .to_string();

        assert!(error.contains("missing exact versioned boot asset kernel"));
        assert!(error.contains(release));
    }

    #[test]
    fn runtime_boot_asset_resolution_rejects_multiple_complete_kernels() {
        let tmp = tempfile::TempDir::new().unwrap();
        let boot_root = tmp.path().join("boot");
        std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
        std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
        for release in ["6.19.8", "6.20.1"] {
            std::fs::write(boot_root.join(format!("vmlinuz-{release}")), b"kernel").unwrap();
            std::fs::write(
                boot_root.join(format!("initramfs-{release}.img")),
                b"initramfs",
            )
            .unwrap();
        }

        let error = resolve_runtime_boot_asset_sources(&boot_root)
            .unwrap_err()
            .to_string();

        assert!(error.contains("multiple complete kernel asset sets"));
        assert!(error.contains("6.19.8"));
        assert!(error.contains("6.20.1"));
    }

    #[test]
    fn generation_boot_asset_resolution_materializes_default_boot_from_cas_inputs() {
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
        let runtime_inputs = exact_runtime_inputs(vec![
            (
                format!("/boot/vmlinuz-{release}"),
                kernel_hash,
                b"cas-kernel".len() as u64,
            ),
            (
                format!("/boot/initramfs-{release}.img"),
                initramfs_hash,
                b"cas-initramfs".len() as u64,
            ),
            (
                "/boot/EFI/BOOT/BOOTX64.EFI".to_string(),
                efi_hash,
                b"cas-efi".len() as u64,
            ),
            (
                format!("/usr/lib/modules/{release}/modules.dep"),
                modules_dep_hash,
                b"modules-dep".len() as u64,
            ),
        ]);
        write_executable(
            &fake_dracut,
            "#!/bin/sh\nprev=\nfor arg in \"$@\"; do out=\"$prev\"; prev=\"$arg\"; done\nprintf generated-initramfs > \"$out\"\n",
        );
        write_executable(&fake_depmod, "#!/bin/sh\nexit 99\n");
        write_executable(&fake_cpio, "#!/bin/sh\nexit 0\n");
        let sources = resolve_generation_boot_asset_sources_with_tools(
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
        let runtime_inputs = exact_runtime_inputs(vec![
            (
                format!("/boot/vmlinuz-{release}"),
                kernel_hash,
                b"cas-kernel".len() as u64,
            ),
            (
                format!("/boot/initramfs-{release}.img"),
                adopted_initramfs_hash,
                b"adopted-host-initramfs".len() as u64,
            ),
            (
                "/boot/EFI/BOOT/BOOTX64.EFI".to_string(),
                efi_hash,
                b"cas-efi".len() as u64,
            ),
            (
                format!("/usr/lib/modules/{release}/modules.dep"),
                modules_dep_hash,
                b"modules-dep".len() as u64,
            ),
        ]);
        write_executable(
            &fake_dracut,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprev=\nfor arg in \"$@\"; do out=\"$prev\"; prev=\"$arg\"; done\nprintf conary-initramfs > \"$out\"\n",
                dracut_args.display()
            ),
        );
        write_executable(&fake_depmod, "#!/bin/sh\nexit 99\n");
        write_executable(&fake_cpio, "#!/bin/sh\nexit 0\n");
        let sources = resolve_generation_boot_asset_sources_with_tools(
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

        let sources = resolve_runtime_boot_asset_sources_with_tools(
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

        resolve_runtime_boot_asset_sources_with_tools(
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

        let error = resolve_runtime_boot_asset_sources_with_tools(
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
