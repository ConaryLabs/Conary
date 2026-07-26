// apps/conary/src/commands/cook/foreign_package.rs

use anyhow::{Context, Result};
use conary_core::ccs::convert::{ConversionOptions, NativePackageConverter};
use conary_core::packages::common::PackageMetadata;
use conary_core::packages::registry::{detect_format, parse_package};
use std::io::Write;
use std::path::Path;

pub(super) fn foreign_package_format(path: &Path) -> Option<&'static str> {
    detect_format(path).ok().map(|format| format.name())
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
    let mut package_file = std::fs::File::open(package_path)
        .with_context(|| format!("Failed to open foreign package: {}", package_path.display()))?;
    let checksum = format!(
        "sha256:{}",
        conary_core::hash::hash_reader(
            conary_core::hash::HashAlgorithm::Sha256,
            &mut package_file,
        )?
        .value
    );
    let payload = package.package_payload().with_context(|| {
        format!(
            "Failed to open payload for foreign package: {}",
            package_path.display()
        )
    })?;
    let metadata = PackageMetadata {
        package_path: package_path.to_path_buf(),
        name: package.name().to_string(),
        version: package.version().to_string(),
        version_scheme: package.version_scheme(),
        architecture: package.architecture().map(str::to_string),
        debian_multi_arch: package.debian_multi_arch(),
        description: package.description().map(str::to_string),
        files: package.files().to_vec(),
        requirements: package.requirements().to_vec(),
        provides: package.provides().to_vec(),
        relations: package.relations().to_vec(),
        diagnostic_scriptlet_evidence: Vec::new(),
        native_scriptlet_abi: package.native_scriptlet_abi().to_vec(),
        config_files: package.config_files().to_vec(),
    };
    let converter = NativePackageConverter::new(ConversionOptions {
        output_dir: output_dir.to_path_buf(),
    })
    .with_signing_key(std::sync::Arc::new(
        crate::commands::ccs::load_or_create_local_dev_key()?,
    ));
    let result = converter
        .convert_payload(&metadata, payload.files(), format.name(), &checksum)
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
