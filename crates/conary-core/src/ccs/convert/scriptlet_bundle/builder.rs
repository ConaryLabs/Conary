// conary-core/src/ccs/convert/scriptlet_bundle/builder.rs

use super::digest::evidence_digest;
use super::entries::build_entries;
use super::summary::summary_from_bundle;
use super::types::{ScriptletBundleBuild, ScriptletBundleInput};
use crate::ccs::native_lifecycle::{
    NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1, NativeLifecycleBundle,
    ScriptletFidelity, SourceFormat, VersionScheme,
};
use crate::packages::common::PackageMetadata;
use crate::packages::traits::{ExtractedFile, PackageFormat};
use std::path::PathBuf;

pub fn build_native_lifecycle_bundle(
    input: ScriptletBundleInput<'_>,
) -> anyhow::Result<ScriptletBundleBuild> {
    let source_format = source_format(input.source_format)?;
    let entries = build_entries(&input)?;
    let scriptlet_fidelity = if entries.is_empty() {
        ScriptletFidelity::NativeFree
    } else {
        ScriptletFidelity::NativeLifecycle
    };
    let source_checksum = input
        .source_checksum
        .map(validate_source_checksum)
        .transpose()?
        .map(str::to_string);
    let mut bundle = NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format,
        source_family: source_family(source_format).to_string(),
        source_distro: input.source_distro.map(str::to_string),
        source_release: input.source_release.map(str::to_string),
        source_arch: input
            .source_arch
            .or(input.source_metadata.architecture.as_deref())
            .map(str::to_string),
        source_package: input.source_metadata.name.clone(),
        source_version: input.source_metadata.version.clone(),
        source_checksum,
        version_scheme: version_scheme(source_format),
        conversion_tool: input.conversion_tool.to_string(),
        conversion_tool_version: input.conversion_tool_version.to_string(),
        conversion_policy: "typed-source-lifecycle-v1".to_string(),
        evidence_digest: None,
        scriptlet_fidelity,
        entries,
    };

    let digest = evidence_digest(&bundle)?;
    bundle.evidence_digest = Some(digest.clone());
    for entry in &mut bundle.entries {
        entry.evidence_digest = Some(digest.clone());
    }
    bundle.validate()?;

    Ok(ScriptletBundleBuild {
        summary: summary_from_bundle(&bundle),
        bundle,
    })
}

/// Build the exact lifecycle authority used by direct native-package installs.
///
/// Native parsers must expose byte-preserving ABI entries for every script or
/// hook they report. There is deliberately no flattened-scriptlet fallback.
pub fn build_direct_native_lifecycle_bundle(
    package: &dyn PackageFormat,
    files: &[ExtractedFile],
    source_format: &str,
) -> anyhow::Result<Option<NativeLifecycleBundle>> {
    let native_entries = package.native_scriptlet_abi();
    if native_entries.is_empty() {
        return Ok(None);
    }

    let metadata = PackageMetadata {
        package_path: PathBuf::new(),
        name: package.name().to_string(),
        version: package.version().to_string(),
        version_scheme: package.version_scheme(),
        architecture: package.architecture().map(str::to_string),
        description: package.description().map(str::to_string),
        files: package.files().to_vec(),
        requirements: package.requirements().to_vec(),
        provides: package.provides().to_vec(),
        relations: package.relations().to_vec(),
        diagnostic_scriptlet_evidence: Vec::new(),
        native_scriptlet_abi: native_entries.to_vec(),
        config_files: package.config_files().to_vec(),
    };
    let build = build_native_lifecycle_bundle(ScriptletBundleInput {
        source_metadata: &metadata,
        final_metadata: &metadata,
        source_files: files,
        final_files: files,
        source_format,
        source_distro: None,
        source_release: None,
        source_arch: package.architecture(),
        source_checksum: None,
        conversion_tool: "conary-native-install",
        conversion_tool_version: env!("CARGO_PKG_VERSION"),
    })?;
    Ok(Some(build.bundle))
}

fn source_format(value: &str) -> anyhow::Result<SourceFormat> {
    match value {
        "rpm" => Ok(SourceFormat::Rpm),
        "deb" => Ok(SourceFormat::Deb),
        "arch" => Ok(SourceFormat::Arch),
        other => anyhow::bail!("unsupported scriptlet source format '{other}'"),
    }
}

fn source_family(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::Rpm => "rpm",
        SourceFormat::Deb => "deb",
        SourceFormat::Arch => "arch",
    }
}

fn version_scheme(format: SourceFormat) -> VersionScheme {
    match format {
        SourceFormat::Rpm => VersionScheme::Rpm,
        SourceFormat::Deb => VersionScheme::Deb,
        SourceFormat::Arch => VersionScheme::Arch,
    }
}

fn validate_source_checksum(value: &str) -> anyhow::Result<&str> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow::anyhow!("source checksum is not prefixed SHA-256"))?;
    anyhow::ensure!(
        hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "source checksum is not prefixed SHA-256"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_checksum_is_explicit_or_rejected() {
        assert!(validate_source_checksum("sha256:not-a-digest").is_err());
        assert_eq!(
            validate_source_checksum(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            )
            .unwrap(),
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }
}
