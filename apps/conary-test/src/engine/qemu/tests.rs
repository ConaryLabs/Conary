// apps/conary-test/src/engine/qemu/tests.rs

use super::*;

#[test]
fn test_ssh_key_name_tracks_active_fixture() {
    assert_eq!(TEST_SSH_KEY_NAME, "conaryos-test-key-v4");
}

#[test]
fn test_image_filename_appends_qcow2() {
    assert_eq!(
        image_filename("minimal-boot-v1", QemuImageFormat::Qcow2),
        "minimal-boot-v1.qcow2"
    );
    assert_eq!(
        image_filename("minimal-boot-v1.qcow2", QemuImageFormat::Qcow2),
        "minimal-boot-v1.qcow2"
    );
}

#[test]
fn test_image_filename_appends_iso_for_iso_format() {
    assert_eq!(
        image_filename("bootstrap-generation", QemuImageFormat::Iso),
        "bootstrap-generation.iso"
    );
    assert_eq!(
        image_filename("bootstrap-generation.iso", QemuImageFormat::Iso),
        "bootstrap-generation.iso"
    );
}

#[test]
fn test_image_download_url_uses_default_base() {
    assert_eq!(
        image_download_url("minimal-boot-v1", QemuImageFormat::Qcow2),
        "https://remi.conary.io/test-artifacts/minimal-boot-v1.qcow2"
    );
    assert_eq!(
        image_download_url("https://example.com/custom.qcow2", QemuImageFormat::Qcow2),
        "https://example.com/custom.qcow2"
    );
}

#[test]
fn test_image_filename_uses_url_basename() {
    assert_eq!(
        image_filename(
            "https://example.com/test-artifacts/minimal-boot-v1.qcow2",
            QemuImageFormat::Qcow2
        ),
        "minimal-boot-v1.qcow2"
    );
}

#[test]
fn test_image_filename_strips_path_traversal() {
    // Path traversal attempts should be stripped to just the filename.
    assert_eq!(
        image_filename("../../tmp/owned", QemuImageFormat::Qcow2),
        "owned.qcow2"
    );
    assert_eq!(
        image_filename("../../../etc/passwd", QemuImageFormat::Qcow2),
        "passwd.qcow2"
    );
    assert_eq!(
        image_filename("subdir/image.qcow2", QemuImageFormat::Qcow2),
        "image.qcow2"
    );
}

#[test]
fn test_shell_quote_escapes_single_quotes() {
    assert_eq!(shell_quote("uname -r"), "'uname -r'");
    assert_eq!(shell_quote("printf 'hello'"), r#"'printf '\''hello'\'''"#);
}

#[test]
fn test_local_image_path_requires_existing_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let missing = dir.path().join("missing.qcow2");
    let config = QemuBoot {
        image: "local-image".to_string(),
        local_image_path: Some(missing.display().to_string()),
        image_format: QemuImageFormat::Qcow2,
        stage_conary: false,
        scratch_disk_mb: None,
        copy_to_guest: Vec::new(),
        copy_from_guest: Vec::new(),
        memory_mb: 1024,
        timeout_seconds: 120,
        ssh_port: 2222,
        commands: vec!["true".to_string()],
        expect_output: Vec::new(),
    };

    let err = resolve_qemu_image_path(&config).unwrap_err();

    assert!(
        err.to_string()
            .contains("local QEMU image path does not exist")
    );
    assert!(err.to_string().contains("missing.qcow2"));
}

#[test]
fn test_prepare_guest_copy_destination_creates_parent_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("nested/generated/image.qcow2");
    assert!(!dest.parent().unwrap().exists());

    prepare_guest_copy_destination(&dest).unwrap();

    assert!(dest.parent().unwrap().is_dir());
}

#[test]
fn test_prepare_guest_copy_destination_allows_current_dir_destination() {
    prepare_guest_copy_destination(Path::new("image.qcow2")).unwrap();
}

#[test]
fn test_guest_copy_parent_handles_absolute_targets() {
    assert_eq!(
        guest_copy_parent("/var/lib/conary/bootstrap-inputs"),
        Some("/var/lib/conary".to_string())
    );
    assert_eq!(guest_copy_parent("/tmp/"), Some("/".to_string()));
    assert_eq!(guest_copy_parent("relative-file"), None);
}

#[test]
fn test_qemu_image_args_use_snapshot_overlay() {
    let args = qemu_image_args(
        Path::new("/tmp/minimal-boot-v2.qcow2"),
        QemuImageFormat::Qcow2,
    );

    assert_eq!(args[0], "-snapshot");
    assert_eq!(args[1], "-drive");
    assert_eq!(args[2], "file=/tmp/minimal-boot-v2.qcow2,format=qcow2");
}

#[test]
fn test_ovmf_paths_cover_fedora_edk2_layout() {
    assert!(
        OVMF_CODE_PATHS.contains(&"/usr/share/edk2/x64/OVMF_CODE.4m.fd"),
        "Fedora installs non-secure x86_64 OVMF at the 4 MiB edk2 path"
    );
}

#[test]
fn test_qemu_firmware_args_use_readonly_pflash() {
    assert_eq!(
        qemu_firmware_args(Path::new("/usr/share/edk2/x64/OVMF_CODE.4m.fd")),
        [
            "-drive",
            "if=pflash,format=raw,readonly=on,file=/usr/share/edk2/x64/OVMF_CODE.4m.fd"
        ]
    );
}

#[test]
fn test_system_ready_probe_waits_for_boot_transaction() {
    assert!(SYSTEM_READY_COMMAND.contains("is-system-running --wait"));
    assert!(SYSTEM_READY_COMMAND.contains("running|degraded"));
    const { assert!(SYSTEM_READY_TIMEOUT_SECONDS < 600) };
}

#[test]
fn test_qemu_image_args_boot_iso_with_cdrom() {
    let args = qemu_image_args(
        Path::new("/tmp/bootstrap-generation.iso"),
        QemuImageFormat::Iso,
    );

    assert_eq!(args, vec!["-cdrom", "/tmp/bootstrap-generation.iso"]);
}

#[test]
fn test_scratch_disk_args_attach_raw_virtio_disk() {
    let args = qemu_scratch_disk_args(Path::new("/tmp/conary-qemu-scratch.raw"));

    assert_eq!(args[0], "-drive");
    assert_eq!(
        args[1],
        "file=/tmp/conary-qemu-scratch.raw,format=raw,if=none,id=conary-scratch"
    );
    assert_eq!(args[2], "-device");
    assert_eq!(
        args[3],
        "virtio-blk-pci,drive=conary-scratch,serial=conary-scratch"
    );
}

#[test]
fn test_prepare_scratch_disk_creates_sparse_raw_file() {
    let disk = prepare_scratch_disk(64).expect("scratch disk");

    assert!(disk.path.is_file());
    assert_eq!(
        std::fs::metadata(&disk.path).unwrap().len(),
        64 * 1024 * 1024
    );
}

#[test]
fn test_prepare_scratch_disk_uses_persistent_parent_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let parent = dir.path().join("cache/qemu-scratch");
    let disk = prepare_scratch_disk_in(64, &parent).expect("scratch disk");

    assert!(disk.path.starts_with(&parent));
    assert!(disk.path.is_file());
    assert_eq!(
        std::fs::metadata(&disk.path).unwrap().len(),
        64 * 1024 * 1024
    );
}

#[test]
fn test_required_qemu_tools_include_scp_when_staging_conary() {
    let config = QemuBoot {
        image: "minimal-boot-v2".to_string(),
        local_image_path: None,
        image_format: QemuImageFormat::Qcow2,
        stage_conary: true,
        scratch_disk_mb: None,
        copy_to_guest: Vec::new(),
        copy_from_guest: Vec::new(),
        memory_mb: 1024,
        timeout_seconds: 120,
        ssh_port: 2222,
        commands: vec!["conary --version".to_string()],
        expect_output: Vec::new(),
    };

    assert!(required_qemu_tools(&config).contains(&"scp"));
}

#[test]
fn test_required_qemu_tools_include_scp_when_copying_to_guest() {
    let config = QemuBoot {
        image: "minimal-boot-v2".to_string(),
        local_image_path: None,
        image_format: QemuImageFormat::Qcow2,
        stage_conary: false,
        scratch_disk_mb: None,
        copy_to_guest: vec![QemuGuestCopy {
            source: "fixtures/bootstrap".to_string(),
            dest: "/var/lib/conary/bootstrap-inputs".to_string(),
        }],
        copy_from_guest: Vec::new(),
        memory_mb: 1024,
        timeout_seconds: 120,
        ssh_port: 2222,
        commands: vec!["true".to_string()],
        expect_output: Vec::new(),
    };

    assert!(required_qemu_tools(&config).contains(&"scp"));
}
