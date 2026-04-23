// apps/conary/src/commands/generation/export.rs
//! Generation disk-image export command wrapper.

use anyhow::{Context, Result};
use conary_core::generation::export::{
    GenerationExportFormat, GenerationExportOptions, export_generation_image,
};
use conary_core::image::size::ImageSize;
use std::path::PathBuf;
use std::str::FromStr;

pub async fn cmd_generation_export(
    generation: Option<i64>,
    path: Option<&str>,
    format: &str,
    output: &str,
    size: Option<&str>,
) -> Result<()> {
    let format = parse_generation_export_format(format)?;
    let size_bytes = parse_generation_export_size(size)?;
    let result = export_generation_image(GenerationExportOptions {
        generation,
        generation_path: path.map(PathBuf::from),
        format,
        output: PathBuf::from(output),
        size_bytes,
    })?;

    println!("Generation export complete");
    println!("  Output: {}", result.path.display());
    println!("  Format: {}", result.format);
    println!("  Size:   {} bytes", result.size);
    println!("  Method: {}", generation_export_method(result.format));

    Ok(())
}

fn parse_generation_export_format(format: &str) -> Result<GenerationExportFormat> {
    GenerationExportFormat::from_str(format).map_err(Into::into)
}

fn parse_generation_export_size(size: Option<&str>) -> Result<Option<u64>> {
    size.map(|value| {
        ImageSize::from_str(value)
            .map(|size| size.bytes())
            .with_context(|| format!("Invalid generation export size: {value}"))
    })
    .transpose()
}

fn generation_export_method(format: GenerationExportFormat) -> &'static str {
    match format {
        GenerationExportFormat::Raw => "systemd-repart raw image",
        GenerationExportFormat::Qcow2 => "systemd-repart raw image + qemu-img qcow2 conversion",
        GenerationExportFormat::Iso => "reserved ISO export",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::filesystem::object_path;
    use conary_core::generation::artifact::{
        ArtifactWriteInputs, BootAssetsManifest, CasObjectRef, write_generation_artifact,
    };
    use conary_core::generation::metadata::{GENERATION_FORMAT, GenerationMetadata};
    use conary_core::hash::sha256;
    use std::path::Path;
    use tempfile::TempDir;

    struct ExportFixture {
        _tmp: TempDir,
        generation_dir: PathBuf,
    }

    fn write_cas_object(objects_dir: &Path, bytes: &[u8]) -> CasObjectRef {
        let sha256 = sha256(bytes);
        let object_path = object_path(objects_dir, &sha256).unwrap();
        std::fs::create_dir_all(object_path.parent().unwrap()).unwrap();
        std::fs::write(object_path, bytes).unwrap();
        CasObjectRef {
            sha256,
            size: bytes.len() as u64,
        }
    }

    impl ExportFixture {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let artifact_root = tmp.path().join("artifact");
            let generation_dir = artifact_root.join("generations/7");
            let objects_dir = artifact_root.join("objects");
            let boot_assets_dir = generation_dir.join("boot-assets");
            std::fs::create_dir_all(boot_assets_dir.join("EFI/BOOT")).unwrap();
            std::fs::create_dir_all(&objects_dir).unwrap();
            std::fs::write(generation_dir.join("root.erofs"), b"root-erofs").unwrap();
            std::fs::write(boot_assets_dir.join("vmlinuz"), b"kernel").unwrap();
            std::fs::write(boot_assets_dir.join("initramfs.img"), b"initramfs").unwrap();
            std::fs::write(boot_assets_dir.join("EFI/BOOT/BOOTX64.EFI"), b"efi").unwrap();

            let cas_object = write_cas_object(&objects_dir, b"hello");
            let boot_assets = BootAssetsManifest {
                version: 1,
                generation: 7,
                architecture: "x86_64".to_string(),
                kernel_version: "6.19.8-conary".to_string(),
                kernel: "vmlinuz".to_string(),
                kernel_sha256: sha256(b"kernel"),
                initramfs: "initramfs.img".to_string(),
                initramfs_sha256: sha256(b"initramfs"),
                efi_bootloader: "EFI/BOOT/BOOTX64.EFI".to_string(),
                efi_bootloader_sha256: sha256(b"efi"),
                created_at: "2026-04-22T00:00:00Z".to_string(),
            };
            let artifact_digest = write_generation_artifact(ArtifactWriteInputs {
                generation_dir: &generation_dir,
                generation: 7,
                architecture: "x86_64",
                erofs_path: &generation_dir.join("root.erofs"),
                cas_base_rel: "../../objects",
                cas_objects: vec![cas_object],
                boot_assets,
            })
            .unwrap();
            GenerationMetadata {
                generation: 7,
                format: GENERATION_FORMAT.to_string(),
                erofs_size: Some(10),
                cas_objects_referenced: Some(1),
                fsverity_enabled: false,
                erofs_verity_digest: None,
                artifact_manifest_sha256: Some(artifact_digest),
                created_at: "2026-04-22T00:00:00Z".to_string(),
                package_count: 1,
                kernel_version: Some("6.19.8-conary".to_string()),
                summary: "fixture".to_string(),
            }
            .write_to(&generation_dir)
            .unwrap();

            Self {
                _tmp: tmp,
                generation_dir,
            }
        }
    }

    #[test]
    fn format_parse_errors_list_allowed_values() {
        let err = parse_generation_export_format("vmdk").unwrap_err();
        assert!(err.to_string().contains("raw, qcow2, or iso"));
    }

    #[test]
    fn parses_optional_size_with_shared_parser() {
        assert_eq!(
            parse_generation_export_size(Some("1G")).unwrap(),
            Some(1024 * 1024 * 1024)
        );
        assert_eq!(parse_generation_export_size(None).unwrap(), None);
        assert!(parse_generation_export_size(Some("nope")).is_err());
    }

    #[test]
    fn output_method_describes_export_backend() {
        assert_eq!(
            generation_export_method(GenerationExportFormat::Raw),
            "systemd-repart raw image"
        );
        assert_eq!(
            generation_export_method(GenerationExportFormat::Qcow2),
            "systemd-repart raw image + qemu-img qcow2 conversion"
        );
        assert_eq!(
            generation_export_method(GenerationExportFormat::Iso),
            "reserved ISO export"
        );
    }

    #[tokio::test]
    async fn iso_returns_reserved_error() {
        let err = cmd_generation_export(
            None,
            Some("/does/not/exist"),
            "iso",
            "/tmp/unused.iso",
            None,
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("ISO export is reserved on the generation artifact contract")
        );
    }

    #[tokio::test]
    async fn undersized_image_error_surfaces_without_panic() {
        let fixture = ExportFixture::new();
        let output = fixture._tmp.path().join("undersized.raw");
        let generation_path = fixture.generation_dir.to_string_lossy();
        let output_path = output.to_string_lossy();

        let err = cmd_generation_export(
            None,
            Some(generation_path.as_ref()),
            "raw",
            output_path.as_ref(),
            Some("1"),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("requested image size 1 bytes"));
        assert!(err.to_string().contains("minimum"));
    }
}
