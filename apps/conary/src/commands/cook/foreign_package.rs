// apps/conary/src/commands/cook/foreign_package.rs

use anyhow::{Context, Result};
use conary_core::ccs::convert::{ConversionOptions, LegacyConverter};
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::registry::{detect_format, parse_package};
use std::io::Write;
use std::path::Path;

pub(super) fn foreign_package_format(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".rpm") {
        Some("rpm")
    } else if name.ends_with(".deb") {
        Some("deb")
    } else if name.ends_with(".pkg.tar.zst") {
        Some("arch")
    } else {
        None
    }
}

pub(super) fn cook_foreign_package(
    package_path: &Path,
    output_dir: &Path,
    output: &mut impl Write,
) -> Result<()> {
    let format = detect_format(package_path).with_context(|| {
        format!(
            "Failed to parse foreign package: {}",
            package_path.display()
        )
    })?;
    let package = parse_package(package_path).with_context(|| {
        format!(
            "Failed to parse foreign package: {}",
            package_path.display()
        )
    })?;
    let package_bytes = std::fs::read(package_path)
        .with_context(|| format!("Failed to read foreign package: {}", package_path.display()))?;
    let checksum = conary_core::hash::sha256_prefixed(&package_bytes);
    let extracted = package.extract_file_contents().with_context(|| {
        format!(
            "Failed to extract files for foreign package: {}",
            package_path.display()
        )
    })?;
    let metadata = PackageMetadata {
        package_path: package_path.to_path_buf(),
        name: package.name().to_string(),
        version: package.version().to_string(),
        architecture: package.architecture().map(str::to_string),
        description: package.description().map(str::to_string),
        files: package.files().to_vec(),
        dependencies: package.dependencies().to_vec(),
        provides: package.provides().to_vec(),
        scriptlets: package.scriptlets().to_vec(),
        native_scriptlet_abi: package.native_scriptlet_abi().to_vec(),
        config_files: Vec::new(),
    };
    let converter = LegacyConverter::new(ConversionOptions {
        enable_chunking: true,
        output_dir: output_dir.to_path_buf(),
    });
    let result = converter
        .convert(&metadata, &extracted, format.name(), &checksum)
        .with_context(|| {
            format!(
                "Failed to convert foreign package {}",
                package_path.display()
            )
        })?;
    let converted = result
        .package_path
        .as_ref()
        .context("foreign conversion succeeded without a CCS output path")?;

    writeln!(output, "Converted foreign package: {}", converted.display())?;
    Ok(())
}
