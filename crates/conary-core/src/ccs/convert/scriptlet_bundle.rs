// conary-core/src/ccs/convert/scriptlet_bundle.rs
//! Passive legacy scriptlet bundle construction for legacy package conversion.

mod classification;
mod digest;
mod entries;
mod format_metadata;
mod native_contracts;
mod summary;
#[cfg(test)]
mod test_support;
mod types;

pub use types::{
    ScriptletBundleBuild, ScriptletBundleInput, ScriptletBundleSummary,
    ScriptletDecisionCountsSummary,
};

use crate::ccs::legacy_scriptlets::{
    ForeignReplayPolicy, LEGACY_SCRIPTLET_SCHEMA_V1, LegacyScriptletBundle, SourceFormat,
    VersionScheme,
};
use std::collections::BTreeMap;

use digest::evidence_digest;
use entries::build_entries;
use summary::{aggregate_status, decision_counts, summary_from_bundle};

pub fn build_legacy_scriptlet_bundle(
    input: ScriptletBundleInput<'_>,
) -> anyhow::Result<ScriptletBundleBuild> {
    let format = source_format(input.source_format)?;
    let source_distro = input.source_distro.unwrap_or("unknown").to_string();
    let source_release = input.source_release.unwrap_or("unknown").to_string();
    let source_arch = input
        .source_arch
        .or(input.source_metadata.architecture.as_deref())
        .unwrap_or("unknown")
        .to_string();
    let source_checksum = input
        .source_checksum
        .filter(|checksum| valid_prefixed_sha256(checksum))
        .map(str::to_string);

    let entries = build_entries(&input)?;
    let decision_counts = decision_counts(&entries);
    let (scriptlet_fidelity, target_compatibility, publication_policy, publication_status) =
        aggregate_status(&entries, &decision_counts);

    let mut bundle = LegacyScriptletBundle {
        schema: LEGACY_SCRIPTLET_SCHEMA_V1.to_string(),
        schema_revision: 1,
        source_format: format.clone(),
        source_family: source_family(&format).to_string(),
        source_distro: Some(source_distro),
        source_release: Some(source_release),
        source_arch: Some(source_arch),
        source_package: input.source_metadata.name.clone(),
        source_version: input.source_metadata.version.clone(),
        source_checksum,
        version_scheme: version_scheme(&format),
        conversion_tool: input.conversion_tool.to_string(),
        conversion_tool_version: input.conversion_tool_version.to_string(),
        conversion_policy: "passive-scriptlet-bundle-goal4".to_string(),
        adapter_registry_digest: None,
        target_policy_digest: None,
        evidence_digest: None,
        target_compatibility,
        allowed_targets: Vec::new(),
        foreign_replay_policy: ForeignReplayPolicy::Deny,
        publication_policy,
        publication_status,
        scriptlet_fidelity,
        decision_counts,
        unsupported_class_counts: input.classification.unsupported_class_counts.clone(),
        entries,
        extra: BTreeMap::new(),
    };

    let digest = evidence_digest(&bundle, &input)?;
    bundle.evidence_digest = Some(digest.clone());
    for entry in &mut bundle.entries {
        entry.evidence_digest = Some(digest.clone());
    }
    bundle.validate()?;

    Ok(ScriptletBundleBuild {
        summary: summary_from_bundle(&bundle, Some(digest)),
        bundle,
    })
}

fn source_format(value: &str) -> anyhow::Result<SourceFormat> {
    match value {
        "rpm" => Ok(SourceFormat::Rpm),
        "deb" => Ok(SourceFormat::Deb),
        "arch" => Ok(SourceFormat::Arch),
        other => anyhow::bail!("unsupported scriptlet source format '{other}'"),
    }
}

fn source_family(format: &SourceFormat) -> &'static str {
    match format {
        SourceFormat::Rpm => "rpm",
        SourceFormat::Deb => "deb",
        SourceFormat::Arch => "arch",
        SourceFormat::Unknown(_) => "unknown",
    }
}

fn version_scheme(format: &SourceFormat) -> VersionScheme {
    match format {
        SourceFormat::Rpm => VersionScheme::Rpm,
        SourceFormat::Deb => VersionScheme::Deb,
        SourceFormat::Arch => VersionScheme::Arch,
        SourceFormat::Unknown(_) => VersionScheme::Semver,
    }
}

fn valid_prefixed_sha256(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::test_support::{bundle_for_metadata, package_metadata};
    use super::*;
    use crate::ccs::convert::effects::ScriptletClassificationReport;
    use crate::packages::traits::{Scriptlet, ScriptletPhase};

    #[test]
    fn native_free_input_builds_zero_entry_bundle() {
        let metadata = package_metadata("native-free", "1.0");
        let files = Vec::new();
        let classification = ScriptletClassificationReport::default();

        let build = build_legacy_scriptlet_bundle(ScriptletBundleInput {
            source_metadata: &metadata,
            final_metadata: &metadata,
            source_files: &files,
            final_files: &files,
            source_format: "rpm",
            source_distro: Some("fedora-44"),
            source_release: Some("44"),
            source_arch: Some("x86_64"),
            source_checksum: Some(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            classification: &classification,
            conversion_tool: "remi",
            conversion_tool_version: "0.1.0",
        })
        .unwrap();

        assert!(build.bundle.entries.is_empty());
        assert_eq!(build.bundle.scriptlet_fidelity.as_str(), "native-free");
        assert_eq!(
            build.bundle.target_compatibility.as_str(),
            "conary-portable"
        );
        assert_eq!(
            build.bundle.publication_policy.as_str(),
            "public-if-no-blocked"
        );
        assert_eq!(build.bundle.publication_status.as_str(), "public");
        assert_eq!(build.bundle.decision_counts.total(), 0);
        assert_eq!(build.summary.scriptlet_fidelity, "native-free");
        assert_eq!(build.summary.target_compatibility, "conary-portable");
        assert_eq!(build.summary.publication_status, "public");
        assert!(
            build
                .summary
                .evidence_digest
                .as_deref()
                .unwrap()
                .starts_with("sha256:")
        );
        build.bundle.validate().unwrap();
    }

    #[test]
    fn tampered_body_after_build_fails_strict_bundle_validation() {
        let mut metadata = package_metadata("tamper", "1.0");
        metadata.scriptlets.push(Scriptlet {
            phase: ScriptletPhase::PreInstall,
            interpreter: "/bin/sh".to_string(),
            content: "echo ok\n".to_string(),
            flags: None,
        });
        let files = Vec::new();
        let classification = ScriptletClassificationReport::default();
        let mut build = bundle_for_metadata(&metadata, &files, &classification).unwrap();

        build.bundle.entries[0].body.push_str("tampered\n");

        assert!(build.bundle.validate().is_err());
    }
}
