// crates/conary-core/src/generation/builder/boot_assets/tests.rs

use super::super::initramfs::{
    CONARY_DRACUT_MODULE_SETUP, RUNTIME_DRACUT_ADD_MODULES, RUNTIME_DRACUT_OMIT_MODULES,
};
use super::super::runtime_inputs;
use super::*;
use crate::filesystem::CasStore;
use crate::generation::builder::BootRoot;
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
use crate::test_support::{HostToolFixture, link_host_tool};

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
    let mut runtime_inputs = exact_runtime_inputs(vec![
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
    link_host_tool(&fake_dracut, HostToolFixture::Dracut);
    link_host_tool(&fake_depmod, HostToolFixture::ExitFailure);
    link_host_tool(&fake_cpio, HostToolFixture::ExitSuccess);
    let sources = resolve_generation_boot_asset_sources_with_tools(
        &mut runtime_inputs,
        &generations_root,
        &BootRoot::Host,
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
        b"fixture-initramfs"
    );
    assert_eq!(std::fs::read(sources.efi_bootloader).unwrap(), b"cas-efi");
}

#[cfg(unix)]
#[test]
fn generation_boot_asset_resolution_retains_exact_depmod_output_in_manifest_and_cas() {
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
    let efi_hash = cas.store(b"cas-efi").unwrap();
    let vfat_hash = cas.store(b"vfat-module").unwrap();
    let mut runtime_inputs = exact_runtime_inputs(vec![
        (
            format!("/boot/vmlinuz-{release}"),
            kernel_hash,
            b"cas-kernel".len() as u64,
        ),
        (
            "/boot/EFI/BOOT/BOOTX64.EFI".to_string(),
            efi_hash,
            b"cas-efi".len() as u64,
        ),
        (
            format!("/usr/lib/modules/{release}/kernel/fs/fat/vfat.ko"),
            vfat_hash,
            b"vfat-module".len() as u64,
        ),
    ]);
    let mut lib_symlink = owned_regular_source(0o777);
    lib_symlink.kind = PayloadNodeKind::Symlink {
        target: "usr/lib".to_string(),
    };
    lib_symlink.mode = libc::S_IFLNK | 0o777;
    runtime_inputs.generation.entries.push(GenerationRootEntry {
        path: "/lib".to_string(),
        node: ResolvedPayloadNode::from_numeric_source(lib_symlink).unwrap(),
        content: None,
    });
    runtime_inputs
        .generation
        .entries
        .sort_by(|left, right| left.path.cmp(&right.path));
    runtime_inputs.generation.validate().unwrap();
    link_host_tool(&fake_depmod, HostToolFixture::Depmod);
    link_host_tool(&fake_dracut, HostToolFixture::Dracut);
    link_host_tool(&fake_cpio, HostToolFixture::ExitSuccess);

    let _sources = resolve_generation_boot_asset_sources_with_tools(
        &mut runtime_inputs,
        &generations_root,
        &BootRoot::Host,
        &fake_dracut,
        &fake_depmod,
        &fake_cpio,
    )
    .unwrap();

    for (path, expected) in [
        (
            format!("/usr/lib/modules/{release}/modules.dep"),
            b"kernel/fs/fat/vfat.ko:\n".as_slice(),
        ),
        (
            format!("/usr/lib/modules/{release}/modules.alias"),
            b"alias fs-vfat vfat\n".as_slice(),
        ),
    ] {
        let entry = runtime_inputs
            .generation
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("missing retained depmod output {path}"));
        let content = entry.content.as_ref().expect("depmod output is regular");
        assert_eq!(cas.retrieve(&content.sha256).unwrap(), expected);
    }
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
    std::fs::create_dir_all(&generations_root).unwrap();
    let cas = CasStore::new(&objects_dir).unwrap();

    let release = "6.20.0-conary";
    let kernel_hash = cas.store(b"cas-kernel").unwrap();
    let adopted_initramfs_hash = cas.store(b"adopted-host-initramfs").unwrap();
    let efi_hash = cas.store(b"cas-efi").unwrap();
    let modules_dep_hash = cas.store(b"modules-dep").unwrap();
    let mut runtime_inputs = exact_runtime_inputs(vec![
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
    link_host_tool(&fake_dracut, HostToolFixture::Dracut);
    link_host_tool(&fake_depmod, HostToolFixture::ExitFailure);
    link_host_tool(&fake_cpio, HostToolFixture::ExitSuccess);
    let sources = resolve_generation_boot_asset_sources_with_tools(
        &mut runtime_inputs,
        &generations_root,
        &BootRoot::Host,
        &fake_dracut,
        &fake_depmod,
        &fake_cpio,
    )
    .unwrap();

    assert_eq!(sources.kernel_version, release);
    assert_eq!(
        std::fs::read(&sources.initramfs).unwrap(),
        b"fixture-initramfs"
    );
    let args = std::fs::read_to_string(format!("{}.args", sources.initramfs.display())).unwrap();
    assert!(args.lines().any(|line| line == "--add"));
    assert!(args.lines().any(|line| line == RUNTIME_DRACUT_ADD_MODULES));
    assert!(args.lines().any(|line| line == "--omit"));
    assert!(args.lines().any(|line| line == RUNTIME_DRACUT_OMIT_MODULES));
}

#[cfg(unix)]
#[test]
fn generation_boot_assets_stage_systemd_boot_binary_when_the_sysroot_has_no_esp() {
    let tmp = tempfile::TempDir::new().unwrap();
    let generations_root = tmp.path().join("generations");
    let objects_dir = tmp.path().join("objects");
    let gen_dir = generations_root.join("7");
    let fake_dracut = tmp.path().join("dracut");
    let fake_depmod = tmp.path().join("depmod");
    let fake_cpio = tmp.path().join("cpio");
    std::fs::create_dir_all(&gen_dir).unwrap();
    let cas = CasStore::new(&objects_dir).unwrap();

    let release = "6.20.0-conary";
    let kernel_hash = cas.store(b"cas-kernel").unwrap();
    let modules_dep_hash = cas.store(b"modules-dep").unwrap();
    let systemd_boot_hash = cas.store(b"systemd-boot-efi").unwrap();
    let mut runtime_inputs = exact_runtime_inputs(vec![
        (
            format!("/boot/vmlinuz-{release}"),
            kernel_hash,
            b"cas-kernel".len() as u64,
        ),
        (
            format!("/usr/lib/modules/{release}/modules.dep"),
            modules_dep_hash,
            b"modules-dep".len() as u64,
        ),
        (
            "/usr/lib/systemd/boot/efi/systemd-bootx64.efi".to_string(),
            systemd_boot_hash,
            b"systemd-boot-efi".len() as u64,
        ),
    ]);
    link_host_tool(&fake_dracut, HostToolFixture::Dracut);
    link_host_tool(&fake_depmod, HostToolFixture::ExitFailure);
    link_host_tool(&fake_cpio, HostToolFixture::ExitSuccess);

    let sources = resolve_generation_boot_asset_sources_with_tools(
        &mut runtime_inputs,
        &generations_root,
        &BootRoot::Host,
        &fake_dracut,
        &fake_depmod,
        &fake_cpio,
    )
    .unwrap();

    assert!(
        sources
            .efi_bootloader
            .ends_with("usr/lib/systemd/boot/efi/systemd-bootx64.efi"),
        "the generation must stage systemd-boot's own binary when no ESP layout exists; got {}",
        sources.efi_bootloader.display()
    );

    let manifest = stage_runtime_boot_assets_from_sources(&gen_dir, 7, "x86_64", &sources).unwrap();

    assert_eq!(manifest.efi_bootloader, "EFI/BOOT/BOOTX64.EFI");
    let staged = gen_dir.join("boot-assets/EFI/BOOT/BOOTX64.EFI");
    assert_eq!(std::fs::read(&staged).unwrap(), b"systemd-boot-efi");
}

#[test]
fn runtime_boot_asset_resolution_names_both_efi_sources_when_neither_exists() {
    let tmp = tempfile::TempDir::new().unwrap();
    let boot_root = tmp.path().join("boot");
    let release = "6.17.1-300.fc44.x86_64";
    let module_dir = tmp.path().join("lib/modules").join(release);
    std::fs::create_dir_all(&module_dir).unwrap();
    std::fs::create_dir_all(&boot_root).unwrap();
    std::fs::write(module_dir.join("vmlinuz"), b"kernel").unwrap();
    std::fs::write(
        boot_root.join(format!("initramfs-{release}.img")),
        b"initramfs",
    )
    .unwrap();

    let error = resolve_runtime_boot_asset_sources(&boot_root)
        .unwrap_err()
        .to_string();

    assert!(error.contains("missing required boot asset efi_bootloader"));
    assert!(
        error.contains(
            boot_root
                .join("EFI/BOOT/BOOTX64.EFI")
                .to_str()
                .expect("temp path is utf-8")
        )
    );
    assert!(
        error.contains(
            tmp.path()
                .join("usr/lib/systemd/boot/efi/systemd-bootx64.efi")
                .to_str()
                .expect("temp path is utf-8")
        )
    );
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
    std::fs::create_dir_all(&module_dir).unwrap();
    std::fs::create_dir_all(boot_root.join("EFI/BOOT")).unwrap();
    std::fs::write(module_dir.join("vmlinuz"), b"kernel").unwrap();
    std::fs::write(module_dir.join("modules.dep"), b"deps").unwrap();
    std::fs::write(boot_root.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();
    link_host_tool(&fake_dracut, HostToolFixture::Dracut);
    link_host_tool(&fake_depmod, HostToolFixture::ExitFailure);
    link_host_tool(&fake_cpio, HostToolFixture::ExitSuccess);

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
        b"fixture-initramfs"
    );
    let args =
        std::fs::read_to_string(boot_root.join(format!("initramfs-{release}.img.args"))).unwrap();
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
    link_host_tool(&fake_depmod, HostToolFixture::Depmod);
    link_host_tool(&fake_dracut, HostToolFixture::Dracut);
    link_host_tool(&fake_cpio, HostToolFixture::ExitSuccess);

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
    link_host_tool(&fake_dracut, HostToolFixture::ExitFailure);
    link_host_tool(&fake_depmod, HostToolFixture::ExitFailure);

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
