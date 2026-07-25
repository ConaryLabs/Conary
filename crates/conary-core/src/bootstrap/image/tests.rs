// conary-core/src/bootstrap/image/tests.rs

use super::*;
use crate::generation::metadata::GENERATION_FORMAT;

#[test]
fn test_image_format_from_str() {
    assert_eq!(ImageFormat::from_str("raw").unwrap(), ImageFormat::Raw);
    assert_eq!(ImageFormat::from_str("qcow2").unwrap(), ImageFormat::Qcow2);
    assert_eq!(ImageFormat::from_str("iso").unwrap(), ImageFormat::Iso);
    assert_eq!(ImageFormat::from_str("erofs").unwrap(), ImageFormat::Erofs);
    assert_eq!(
        ImageFormat::from_str("composefs").unwrap(),
        ImageFormat::Erofs
    );
    assert_eq!(ImageFormat::from_str("RAW").unwrap(), ImageFormat::Raw);
    assert_eq!(ImageFormat::from_str("EROFS").unwrap(), ImageFormat::Erofs);
    assert!(ImageFormat::from_str("invalid").is_err());
}

#[test]
fn test_image_format_extension() {
    assert_eq!(ImageFormat::Raw.extension(), "img");
    assert_eq!(ImageFormat::Qcow2.extension(), "qcow2");
    assert_eq!(ImageFormat::Iso.extension(), "iso");
    assert_eq!(ImageFormat::Erofs.extension(), "erofs");
}

#[test]
fn test_image_size_from_str() {
    assert_eq!(ImageSize::from_str("4G").unwrap().gigabytes(), 4);
    assert_eq!(ImageSize::from_str("512M").unwrap().megabytes(), 512);
    assert_eq!(ImageSize::from_str("1024K").unwrap().bytes(), 1024 * 1024);
    assert_eq!(ImageSize::from_str("1T").unwrap().gigabytes(), 1024);
    assert_eq!(ImageSize::from_str("1048576").unwrap().bytes(), 1048576);
    assert!(ImageSize::from_str("").is_err());
    assert!(ImageSize::from_str("abc").is_err());
}

#[test]
fn test_image_size_display() {
    assert_eq!(ImageSize::from_str("4G").unwrap().to_string(), "4G");
    assert_eq!(ImageSize::from_str("512M").unwrap().to_string(), "512M");
}

#[test]
fn test_image_tools_check() {
    let tools = ImageTools::check();
    assert!(tools.is_ok());
    let tools = tools.unwrap();
    assert!(tools.dd.exists());
}

#[test]
fn test_initramfs_init_script_content() {
    let script = "#!/bin/sh\n\
        mount -t proc proc /proc\n\
        mount -t sysfs sys /sys\n\
        mount -t devtmpfs dev /dev\n\
        mount /dev/vda2 /mnt/root\n\
        exec switch_root /mnt/root /lib/systemd/systemd\n";
    assert!(script.starts_with("#!/bin/sh"));
    assert!(script.contains("switch_root"));
    assert!(script.contains("/lib/systemd/systemd"));
    assert!(script.contains("devtmpfs"));
}

#[test]
fn test_image_tools_repart_detection() {
    let tools = ImageTools::check().unwrap();
    let _ = tools.systemd_repart;
    let _ = tools.ukify;
}

#[test]
fn test_erofs_format_no_tools_required() {
    let tools = ImageTools::check().unwrap();
    assert!(tools.check_for_format(ImageFormat::Erofs).is_ok());
}

#[cfg(feature = "composefs-rs")]
#[test]
fn test_erofs_generation_from_sysroot() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().unwrap();
    let sysroot = tmp.path().join("sysroot");
    let output = tmp.path().join("output");

    fs::create_dir_all(sysroot.join("usr/bin")).unwrap();
    fs::create_dir_all(sysroot.join("usr/lib")).unwrap();
    fs::create_dir_all(sysroot.join("etc")).unwrap();
    fs::create_dir_all(sysroot.join("boot/EFI/BOOT")).unwrap();

    fs::write(sysroot.join("usr/bin/hello"), b"#!/bin/sh\necho hello\n").unwrap();
    fs::set_permissions(
        sysroot.join("usr/bin/hello"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    fs::write(sysroot.join("usr/lib/libtest.so"), b"fake shared lib").unwrap();
    fs::write(sysroot.join("etc/hostname"), b"conaryos\n").unwrap();
    fs::write(sysroot.join("boot/vmlinuz"), b"kernel").unwrap();
    fs::write(sysroot.join("boot/initramfs.img"), b"initramfs").unwrap();
    fs::write(sysroot.join("boot/EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

    let config = BootstrapConfig::new();
    let mut builder = ImageBuilder::new(
        tmp.path(),
        &config,
        &sysroot,
        &output,
        ImageFormat::Erofs,
        ImageSize(0),
    )
    .unwrap();

    let result = builder.build().unwrap();

    assert_eq!(result.format, ImageFormat::Erofs);
    assert_eq!(result.method, "composefs-rs");
    assert!(result.size > 0);
    assert!(output.join("objects").is_dir());
    assert!(output.join("generations/1/root.erofs").is_file());
    assert!(output.join("generations/1/generation-root.json").is_file());
    assert!(output.join("generations/1/mutable-state.json").is_file());
    assert!(output.join("generations/1/.conary-gen.json").is_file());
    assert!(output.join("generations/1/.conary-artifact.json").is_file());
    assert!(output.join("generations/1/cas-manifest.json").is_file());
    assert!(
        output
            .join("generations/1/boot-assets/manifest.json")
            .is_file()
    );
    assert!(output.join("generations/1/boot-assets/vmlinuz").is_file());
    assert!(
        output
            .join("generations/1/boot-assets/initramfs.img")
            .is_file()
    );
    assert!(
        output
            .join("generations/1/boot-assets/EFI/BOOT/BOOTX64.EFI")
            .is_file()
    );
    assert!(output.join("db.sqlite3").is_file());
    assert!(output.join("current").exists());

    let artifact =
        crate::generation::artifact::load_generation_artifact(&output.join("generations/1"))
            .unwrap();
    assert!(
        artifact
            .mutable_state
            .entries
            .iter()
            .any(|entry| entry.path == "/etc/hostname")
    );

    let erofs_bytes = fs::read(output.join("generations/1/root.erofs")).unwrap();
    assert!(erofs_bytes.len() > 1028, "EROFS image too small");
    let magic = u32::from_le_bytes([
        erofs_bytes[1024],
        erofs_bytes[1025],
        erofs_bytes[1026],
        erofs_bytes[1027],
    ]);
    assert_eq!(magic, 0xE0F5_E1E2, "EROFS magic mismatch");

    let conn = rusqlite::Connection::open(output.join("db.sqlite3")).unwrap();
    let trove_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM troves", [], |row| row.get(0))
        .unwrap();
    assert_eq!(trove_count, 1);

    let file_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        file_count, 13,
        "all directories and files are authoritative"
    );

    let metadata =
        crate::generation::metadata::GenerationMetadata::read_from(&output.join("generations/1"))
            .unwrap();
    assert_eq!(metadata.generation, 1);
    assert_eq!(metadata.format, GENERATION_FORMAT);
    assert_eq!(metadata.package_count, 1);
    assert!(metadata.artifact_manifest_sha256.is_some());
    assert!(metadata.erofs_verity_digest.is_some());
    assert!(metadata.cas_objects_referenced.unwrap() > 0);
}

#[test]
fn test_dir_size_helper() {
    let tmp = tempfile::TempDir::new().unwrap();
    fs::write(tmp.path().join("a"), b"hello").unwrap();
    fs::create_dir(tmp.path().join("sub")).unwrap();
    fs::write(tmp.path().join("sub/b"), b"world!").unwrap();

    let size = dir_size(tmp.path());
    assert_eq!(size, 11, "5 bytes + 6 bytes = 11 bytes");
}

#[test]
fn test_detect_kernel_in_sysroot() {
    use crate::generation::metadata::detect_kernel_version;

    let tmp = tempfile::TempDir::new().unwrap();
    assert!(detect_kernel_version(tmp.path()).unwrap().is_none());

    fs::create_dir_all(tmp.path().join("usr/lib/modules/6.12.1-conary")).unwrap();
    let version = detect_kernel_version(tmp.path()).unwrap();
    assert_eq!(version.as_deref(), Some("6.12.1-conary"));
}

#[test]
fn test_enable_ext4_verity_feature_adds_verity_to_ext4_features() {
    let input = "\
[defaults]
base_features = sparse_super

[fs_types]
ext4 = {
    features = has_journal,extent,64bit
}
";

    let updated = enable_ext4_verity_feature(input).expect("ext4 stanza should be updated");
    assert!(updated.contains("features = has_journal,extent,64bit,verity"));
}

#[test]
fn test_enable_ext4_verity_feature_is_idempotent() {
    let input = "\
[fs_types]
ext4 = {
    features = has_journal,extent,verity
}
";

    let updated = enable_ext4_verity_feature(input).expect("ext4 stanza with verity should parse");
    assert_eq!(updated.matches("verity").count(), 1);
}

#[test]
fn test_enable_ext4_verity_feature_rejects_missing_ext4_features_line() {
    let input = "\
[fs_types]
ext4 = {
    inode_size = 256
}
";

    let err = enable_ext4_verity_feature(input).unwrap_err();
    assert!(err.contains("ext4 features line"));
}

#[test]
fn test_build_raw_rejects_missing_efi_boot_artifacts() {
    let tmp = tempfile::TempDir::new().unwrap();
    let sysroot = tmp.path().join("sysroot");
    fs::create_dir_all(sysroot.join("boot")).unwrap();
    fs::write(sysroot.join("boot/vmlinuz"), b"kernel").unwrap();

    let config = BootstrapConfig::new();
    let mut builder = ImageBuilder::new(
        tmp.path(),
        &config,
        &sysroot,
        tmp.path().join("out.raw"),
        ImageFormat::Raw,
        ImageSize::from_str("1G").unwrap(),
    )
    .unwrap();

    let err = builder.build_raw().unwrap_err();
    assert!(err.to_string().contains("EFI binary not found"));
}

#[test]
fn test_tier1_root_repart_definition_excludes_bootstrap_workdirs_but_keeps_ssh_profile() {
    let def = ImageBuilder::tier1_root_repart_definition(crate::bootstrap::TargetArch::X86_64);

    assert!(def.exclude_files.contains(&"/boot".to_string()));
    assert!(
        def.exclude_files
            .contains(&"/var/tmp/conary-bootstrap".to_string())
    );
    assert!(
        def.exclude_files
            .contains(&"/var/tmp/conary-bootstrap/*".to_string())
    );
    assert!(def.exclude_files.contains(&"/tools".to_string()));
    assert!(def.exclude_files.contains(&"/root/.cargo".to_string()));
    assert!(
        !def.exclude_files.contains(&"/root".to_string()),
        "the QEMU guest profile stores root authorized_keys under /root/.ssh"
    );
    assert!(def.make_directories.contains(&"/tmp".to_string()));
    assert!(def.make_directories.contains(&"/var/tmp".to_string()));
}

#[test]
fn test_raw_qcow2_formats_require_systemd_repart() {
    let tools = ImageTools {
        dd: PathBuf::from("/bin/dd"),
        mkfs_fat: None,
        qemu_img: Some(PathBuf::from("/usr/bin/qemu-img")),
        xorriso: None,
        mksquashfs: None,
        systemd_repart: None,
        ukify: None,
    };

    let err = tools.check_for_format(ImageFormat::Raw).unwrap_err();
    assert!(err.to_string().contains("systemd-repart"));
}
