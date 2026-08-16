// apps/conary/src/commands/cook/foreign_package.rs

use anyhow::{Context, Result};
use conary_core::ccs::convert::ForeignConversionInput;
use conary_core::ccs::convert::{ConversionOptions, NativePackageConverter};
use conary_core::packages::registry::{PackageFormatType, detect_format, parse_package};
use conary_core::repository::supported_profiles::profile_by_public_id;
use std::io::Write;
use std::path::Path;

pub(super) fn foreign_package_format(path: &Path) -> Option<&'static str> {
    detect_format(path).ok().map(|format| format.name())
}

pub(super) fn cook_foreign_package(
    package_path: &Path,
    source_profile: Option<&str>,
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
    let checksum = conary_core::hash::hash_reader(
        conary_core::hash::HashAlgorithm::Sha256,
        &mut package_file,
    )?;
    let payload = package.package_payload().with_context(|| {
        format!(
            "Failed to open payload for foreign package: {}",
            package_path.display()
        )
    })?;
    let metadata =
        ForeignConversionInput::from_package(package_path.to_path_buf(), package.as_ref())?;
    let mut converter = NativePackageConverter::new(ConversionOptions {
        output_dir: output_dir.to_path_buf(),
    })
    .with_signing_key(std::sync::Arc::new(
        crate::commands::ccs::load_or_create_local_dev_key()?,
    ));
    if let Some(profile_id) = exact_source_profile(format, source_profile)? {
        converter = converter.with_source_profile(profile_id);
    }
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

fn exact_source_profile(
    format: PackageFormatType,
    source_profile: Option<&str>,
) -> Result<Option<&'static str>> {
    let Some(source_profile) = source_profile else {
        return Ok(None);
    };
    let profile = profile_by_public_id(source_profile).ok_or_else(|| {
        anyhow::anyhow!(
            "unsupported source profile '{source_profile}'; use an exact public profile ID"
        )
    })?;
    if profile.package_format().as_str() != format.name() {
        anyhow::bail!(
            "source profile '{}' uses package format '{}', but foreign package format is '{}'",
            profile.id(),
            profile.package_format().as_str(),
            format.name()
        );
    }
    Ok(Some(profile.id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreign_cook_source_profile_is_exact_and_format_matched() {
        assert_eq!(
            exact_source_profile(PackageFormatType::Arch, Some("arch")).unwrap(),
            Some("arch")
        );
        assert_eq!(
            exact_source_profile(PackageFormatType::Deb, None).unwrap(),
            None
        );

        let unknown = exact_source_profile(PackageFormatType::Arch, Some("rolling"))
            .unwrap_err()
            .to_string();
        assert!(unknown.contains("use an exact public profile ID"));

        let mismatch = exact_source_profile(PackageFormatType::Arch, Some("fedora-44"))
            .unwrap_err()
            .to_string();
        assert!(mismatch.contains("foreign package format is 'arch'"));
    }
}
