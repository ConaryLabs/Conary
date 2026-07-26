// conary-core/src/bootstrap/system_config/tests.rs

use super::*;

fn seed_minimal_boot_inputs(root: &Path) {
    let boot_dir = root.join("boot");
    let efi_dir = root.join("usr/lib/systemd/boot/efi");
    std::fs::create_dir_all(&boot_dir).unwrap();
    std::fs::create_dir_all(&efi_dir).unwrap();
    std::fs::write(boot_dir.join("vmlinuz-6.19.8-conary"), b"kernel").unwrap();
    std::fs::write(efi_dir.join("systemd-bootx64.efi"), b"efi").unwrap();

    for rel in INITRAMFS_BINARIES.iter().chain(INITRAMFS_LIBRARIES.iter()) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("fake initramfs input: {rel}\n")).unwrap();
    }

    let sh = root.join("usr/bin/sh");
    let _ = std::fs::remove_file(&sh);
    #[cfg(unix)]
    std::os::unix::fs::symlink("bash", sh).unwrap();
    #[cfg(not(unix))]
    std::fs::write(sh, b"fake sh").unwrap();
}

#[test]
fn test_configure_nonexistent_root() {
    let result = configure_system(Path::new("/nonexistent/root/path"));
    assert!(result.is_err());
    match result.unwrap_err() {
        SystemConfigError::RootNotFound(msg) => {
            assert!(msg.contains("/nonexistent/root/path"));
        }
        other => panic!("Expected RootNotFound, got: {other}"),
    }
}

#[test]
fn test_configure_system_creates_all_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    std::fs::create_dir_all(&root).unwrap();
    seed_minimal_boot_inputs(&root);

    configure_system(&root).unwrap();

    assert!(root.join("etc/passwd").exists());
    assert!(root.join("etc/group").exists());
    assert!(root.join("etc/shadow").exists());
    assert!(root.join("etc/hostname").exists());
    assert!(root.join("etc/os-release").exists());
    assert!(root.join("etc/machine-id").exists());
    assert!(root.join("etc/fstab").exists());
    assert!(root.join("etc/nsswitch.conf").exists());
    assert!(std::fs::symlink_metadata(root.join("etc/resolv.conf")).is_ok());
    assert!(root.join("etc/locale.conf").exists());
    assert!(root.join("etc/inputrc").exists());
    assert!(root.join("etc/systemd/network/80-dhcp.network").exists());
    assert!(root.join("root/.bashrc").exists());

    let passwd = std::fs::read_to_string(root.join("etc/passwd")).unwrap();
    assert!(passwd.contains("root:x:0:0"));
    assert!(passwd.contains("nobody:x:65534"));
    assert!(passwd.contains("messagebus:x:18:18:"));
    assert!(passwd.contains("systemd-network:x:76:76:"));
    assert!(passwd.contains("systemd-resolve:x:77:77:"));
    assert!(passwd.contains("systemd-timesync:x:78:78:"));
    assert!(passwd.contains("uuidd:x:80:80:"));

    let group = std::fs::read_to_string(root.join("etc/group")).unwrap();
    assert!(group.contains("root:x:0"));
    assert!(group.contains("wheel:x:97"));
    assert!(group.contains("messagebus:x:18:"));
    assert!(group.contains("systemd-network:x:76:"));
    assert!(group.contains("systemd-resolve:x:77:"));
    assert!(group.contains("systemd-timesync:x:78:"));
    assert!(group.contains("uuidd:x:80:"));

    let hostname = std::fs::read_to_string(root.join("etc/hostname")).unwrap();
    assert_eq!(hostname.trim(), "conaryos");

    let os_release = std::fs::read_to_string(root.join("etc/os-release")).unwrap();
    assert!(os_release.contains("conaryOS"));
    assert!(os_release.contains("conaryos.com"));

    let machine_id = std::fs::read_to_string(root.join("etc/machine-id")).unwrap();
    assert!(machine_id.is_empty());

    let fstab = std::fs::read_to_string(root.join("etc/fstab")).unwrap();
    assert!(fstab.contains("PARTLABEL=CONARY_ROOT"));
    assert!(fstab.contains("PARTLABEL=CONARY_ESP"));
    assert!(fstab.contains("tmpfs"));

    let nsswitch = std::fs::read_to_string(root.join("etc/nsswitch.conf")).unwrap();
    assert!(nsswitch.contains("hosts:"));
    assert!(nsswitch.contains("files dns"));

    let locale = std::fs::read_to_string(root.join("etc/locale.conf")).unwrap();
    assert!(locale.contains("LANG=en_US.UTF-8"));

    let inputrc = std::fs::read_to_string(root.join("etc/inputrc")).unwrap();
    assert!(inputrc.contains("set input-meta on"));
    assert!(inputrc.contains("show-all-if-ambiguous"));

    let dhcp = std::fs::read_to_string(root.join("etc/systemd/network/80-dhcp.network")).unwrap();
    assert!(dhcp.contains("[Match]"));
    assert!(dhcp.contains("DHCP=yes"));

    let bashrc = std::fs::read_to_string(root.join("root/.bashrc")).unwrap();
    assert!(bashrc.contains("PS1="));
    assert!(!root.join("etc/ssh/sshd_config").exists());
}

#[test]
fn test_configure_system_root_shadow_does_not_force_password_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    std::fs::create_dir_all(&root).unwrap();
    seed_minimal_boot_inputs(&root);

    configure_system(&root).unwrap();

    let shadow = std::fs::read_to_string(root.join("etc/shadow")).unwrap();
    assert!(
        shadow.lines().any(|line| line == "root::1:0:99999:7:::"),
        "empty root password needs a nonzero last-change day so login works without first-login rotation"
    );
}

#[test]
fn test_configure_system_creates_bootloader_artifacts_from_versioned_kernel() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    std::fs::create_dir_all(&root).unwrap();
    seed_minimal_boot_inputs(&root);

    configure_system(&root).unwrap();

    assert_eq!(std::fs::read(root.join("boot/vmlinuz")).unwrap(), b"kernel");
    assert_eq!(
        std::fs::read(root.join("boot/EFI/BOOT/BOOTX64.EFI")).unwrap(),
        b"efi"
    );

    let loader = std::fs::read_to_string(root.join("boot/loader/loader.conf")).unwrap();
    assert!(loader.contains("default conaryos"));

    let entry = std::fs::read_to_string(root.join("boot/loader/entries/conaryos.conf")).unwrap();
    assert!(entry.contains("title   conaryOS"));
    assert!(entry.contains("linux   /vmlinuz"));
    assert!(entry.contains("initrd  /initramfs.img"));
    assert!(entry.contains("root=PARTLABEL=CONARY_ROOT"));
    assert!(entry.contains("console=ttyS0"));
    assert!(root.join("boot/initramfs.img").is_file());
}

#[test]
fn boot_kernel_selection_rejects_filesystem_order_as_authority() {
    let dir = tempfile::tempdir().unwrap();
    let boot = dir.path().join("boot");
    std::fs::create_dir_all(&boot).unwrap();
    std::fs::write(boot.join("vmlinuz-6.19.8"), b"old").unwrap();
    std::fs::write(boot.join("vmlinuz-6.20.1"), b"new").unwrap();

    let error = detect_boot_kernel(dir.path()).unwrap_err().to_string();

    assert!(error.contains("multiple versioned kernels"));
    assert!(error.contains("6.19.8"));
    assert!(error.contains("6.20.1"));
}

#[test]
fn bootstrap_initramfs_supports_readonly_iso_carrier() {
    assert!(INITRAMFS_INIT.contains("rootfstype=*) ROOT_FSTYPE=\"${opt#rootfstype=}\""));
    assert!(INITRAMFS_INIT.contains("rootflags=*) ROOT_FLAGS=\"${opt#rootflags=}\""));
    assert!(INITRAMFS_INIT.contains("conary.carrier=*) CONARY_CARRIER=\"${opt#conary.carrier=}\""));
    assert!(
        INITRAMFS_INIT
            .contains("mount -t \"$ROOT_FSTYPE\" -o \"$ROOT_FLAGS\" \"$ROOT_SPEC\" /sysroot")
    );
    assert!(INITRAMFS_INIT.contains("mount -t tmpfs tmpfs /sysroot/run"));
    assert!(INITRAMFS_INIT.contains("mount -t tmpfs tmpfs /sysroot/var"));
    assert!(INITRAMFS_INIT.contains("mkdir -p /sysroot/var/cache /sysroot/var/lib/sshd"));
    assert!(INITRAMFS_INIT.contains("ETC_BASE=\"/sysroot/run/conary/etc-state\""));
}

#[cfg(unix)]
#[test]
fn test_configure_system_bridges_lib64_to_usr_lib_for_exported_generations() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    std::fs::create_dir_all(&root).unwrap();
    seed_minimal_boot_inputs(&root);

    configure_system(&root).unwrap();

    assert_eq!(
        std::fs::read_link(root.join("usr/lib64")).unwrap(),
        PathBuf::from("lib")
    );
    assert!(
        root.join("usr/lib64/ld-linux-x86-64.so.2").exists(),
        "exported generations expose /lib64 through usr/lib64, so the ELF interpreter must resolve there"
    );
}

#[test]
fn test_configure_system_preserves_package_created_service_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    let etc = root.join("etc");
    std::fs::create_dir_all(&etc).unwrap();
    seed_minimal_boot_inputs(&root);

    std::fs::write(
        etc.join("passwd"),
        "sshd:x:50:50:sshd PrivSep:/var/lib/sshd:/bin/false\n",
    )
    .unwrap();
    std::fs::write(etc.join("group"), "sshd:x:50:\n").unwrap();

    configure_system(&root).unwrap();

    let passwd = std::fs::read_to_string(etc.join("passwd")).unwrap();
    assert!(passwd.contains("root:x:0:0:root:/root:/bin/bash"));
    assert!(passwd.contains("sshd:x:50:50:sshd PrivSep:/var/lib/sshd:/bin/false"));

    let group = std::fs::read_to_string(etc.join("group")).unwrap();
    assert!(group.contains("wheel:x:97:"));
    assert!(group.contains("sshd:x:50:"));
}

#[test]
fn test_configure_system_fails_without_kernel_boot_input() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    let efi_dir = root.join("usr/lib/systemd/boot/efi");
    std::fs::create_dir_all(&efi_dir).unwrap();
    std::fs::write(efi_dir.join("systemd-bootx64.efi"), b"efi").unwrap();

    let err = configure_system(&root).unwrap_err();
    assert!(matches!(err, SystemConfigError::ConfigFailed(_)));
    assert!(err.to_string().contains("kernel"));
}

#[test]
fn test_configure_system_shadow_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    std::fs::create_dir_all(&root).unwrap();
    seed_minimal_boot_inputs(&root);

    configure_system(&root).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(root.join("etc/shadow")).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "shadow should have mode 0600, got {mode:o}");
    }
}

#[cfg(unix)]
#[test]
fn test_configure_system_tolerates_read_only_machine_id() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    let etc = root.join("etc");
    std::fs::create_dir_all(&etc).unwrap();
    seed_minimal_boot_inputs(&root);

    let machine_id = etc.join("machine-id");
    std::fs::write(&machine_id, "").unwrap();
    std::fs::set_permissions(&machine_id, std::fs::Permissions::from_mode(0o444)).unwrap();

    configure_system(&root).unwrap();

    let metadata = std::fs::metadata(&machine_id).unwrap();
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(mode, 0o644, "machine-id should be made writable for reruns");
    assert!(std::fs::read_to_string(&machine_id).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn test_configure_system_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    std::fs::create_dir_all(&root).unwrap();
    seed_minimal_boot_inputs(&root);

    configure_system(&root).unwrap();

    assert!(std::fs::symlink_metadata(root.join("etc/systemd/system/default.target")).is_ok());
    assert!(
        std::fs::symlink_metadata(
            root.join("etc/systemd/system/multi-user.target.wants/systemd-networkd.service")
        )
        .is_ok()
    );
    assert!(
        std::fs::symlink_metadata(
            root.join("etc/systemd/system/multi-user.target.wants/systemd-resolved.service")
        )
        .is_ok()
    );
    assert!(
        std::fs::symlink_metadata(
            root.join("etc/systemd/system/getty.target.wants/serial-getty@ttyS0.service")
        )
        .is_ok()
    );
    assert!(
        std::fs::symlink_metadata(root.join("etc/systemd/system/sys-firmware-efi-efivars.mount"))
            .is_ok()
    );

    let target = std::fs::read_link(root.join("etc/systemd/system/default.target")).unwrap();
    assert_eq!(
        target.to_str().unwrap(),
        "/usr/lib/systemd/system/multi-user.target"
    );
    let resolv = std::fs::read_link(root.join("etc/resolv.conf")).unwrap();
    assert_eq!(
        resolv.to_str().unwrap(),
        "/run/systemd/resolve/stub-resolv.conf"
    );
    let efivars_mask =
        std::fs::read_link(root.join("etc/systemd/system/sys-firmware-efi-efivars.mount")).unwrap();
    assert_eq!(efivars_mask.to_str().unwrap(), "/dev/null");
}

#[cfg(unix)]
#[test]
fn test_configure_system_tolerates_preexisting_systemd_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sysroot");
    let systemd_system = root.join("etc/systemd/system");
    std::fs::create_dir_all(systemd_system.join("multi-user.target.wants")).unwrap();
    std::fs::create_dir_all(systemd_system.join("getty.target.wants")).unwrap();
    seed_minimal_boot_inputs(&root);

    std::os::unix::fs::symlink(
        "/usr/lib/systemd/system/multi-user.target",
        systemd_system.join("default.target"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        "/usr/lib/systemd/system/systemd-networkd.service",
        systemd_system.join("multi-user.target.wants/systemd-networkd.service"),
    )
    .unwrap();

    configure_system(&root).unwrap();

    let default_target = std::fs::read_link(systemd_system.join("default.target")).unwrap();
    assert_eq!(
        default_target.to_str().unwrap(),
        "/usr/lib/systemd/system/multi-user.target"
    );
    let networkd_target =
        std::fs::read_link(systemd_system.join("multi-user.target.wants/systemd-networkd.service"))
            .unwrap();
    assert_eq!(
        networkd_target.to_str().unwrap(),
        "/usr/lib/systemd/system/systemd-networkd.service"
    );
    let resolved_target =
        std::fs::read_link(systemd_system.join("multi-user.target.wants/systemd-resolved.service"))
            .unwrap();
    assert_eq!(
        resolved_target.to_str().unwrap(),
        "/usr/lib/systemd/system/systemd-resolved.service"
    );
    let serial_getty =
        std::fs::read_link(systemd_system.join("getty.target.wants/serial-getty@ttyS0.service"))
            .unwrap();
    assert_eq!(
        serial_getty.to_str().unwrap(),
        "/usr/lib/systemd/system/serial-getty@.service"
    );
    let efivars_mask =
        std::fs::read_link(systemd_system.join("sys-firmware-efi-efivars.mount")).unwrap();
    assert_eq!(efivars_mask.to_str().unwrap(), "/dev/null");
}
