// apps/conary-test/src/config/manifest/tests.rs

use super::*;

#[test]
fn qemu_boot_defaults_to_qcow2_image_format() {
    let manifest: TestManifest = toml::from_str(
        r#"
            [suite]
            name = "QEMU format"
            phase = 3

            [[test]]
            id = "TQEMU01"
            name = "default format"
            description = "default qemu image format"
            timeout = 1

            [[test.step]]
            [test.step.qemu_boot]
            image = "minimal-boot-v3"
            commands = ["true"]
            "#,
    )
    .unwrap();

    let qemu = manifest.test[0].step[0].qemu_boot.as_ref().unwrap();
    assert_eq!(qemu.image_format, QemuImageFormat::Qcow2);
}

#[test]
fn qemu_boot_parses_iso_image_format() {
    let manifest: TestManifest = toml::from_str(
        r#"
            [suite]
            name = "QEMU format"
            phase = 3

            [[test]]
            id = "TQEMU02"
            name = "iso format"
            description = "iso qemu image format"
            timeout = 1

            [[test.step]]
            [test.step.qemu_boot]
            image = "local-generation-iso"
            image_format = "iso"
            commands = ["true"]
            "#,
    )
    .unwrap();

    let qemu = manifest.test[0].step[0].qemu_boot.as_ref().unwrap();
    assert_eq!(qemu.image_format, QemuImageFormat::Iso);
}
