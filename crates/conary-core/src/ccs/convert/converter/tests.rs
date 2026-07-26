// conary-core/src/ccs/convert/converter/tests.rs

use super::*;
use crate::ccs::v2::PackageKindV2;
use crate::packages::native_abi::*;
use crate::packages::traits::{
    ConfigFileInfo, DiagnosticScriptletPhase, ExtractedFile, PackageFile, PackageFormat,
};
use crate::payload::{PayloadContentAuthority, PayloadNode};

const TEST_PAYLOAD: &[u8] = b"#!/bin/sh\necho test";

trait InMemoryConversionForTest {
    fn convert_in_memory_for_test(
        &self,
        metadata: &PackageMetadata,
        files: &[ExtractedFile],
        format: &str,
        checksum: &str,
    ) -> Result<ConversionResult, ConversionError>;
}

impl InMemoryConversionForTest for NativePackageConverter {
    fn convert_in_memory_for_test(
        &self,
        metadata: &PackageMetadata,
        files: &[ExtractedFile],
        format: &str,
        checksum: &str,
    ) -> Result<ConversionResult, ConversionError> {
        let payload = crate::packages::PackagePayload::from_extracted_in_memory(files.to_vec())
            .map_err(|error| ConversionError::IoError(error.to_string()))?;
        self.convert_payload(metadata, payload.files(), format, checksum)
    }
}

fn content_authority(content: &[u8]) -> PayloadContentAuthority {
    PayloadContentAuthority {
        sha256: crate::hash::sha256(content),
        size: content.len() as u64,
    }
}

fn extracted_file(path: &str, content: &[u8], mode: u32) -> ExtractedFile {
    ExtractedFile {
        path: path.to_string(),
        node: PayloadNode::regular(mode),
        content: content.to_vec(),
        content_authority: Some(content_authority(content)),
    }
}

#[test]
fn scriptlet_bundle_types_are_publicly_exported() {
    let summary = crate::ccs::convert::ScriptletBundleSummary::default();
    let nested_summary = crate::ccs::convert::scriptlet_bundle::ScriptletBundleSummary::default();
    assert_eq!(summary, nested_summary);

    assert!(
        std::any::type_name::<crate::ccs::convert::ScriptletBundleInput<'static>>()
            .contains("ScriptletBundleInput")
    );
    assert!(
            std::any::type_name::<
                crate::ccs::convert::scriptlet_bundle::ScriptletBundleInput<'static>,
            >()
            .contains("ScriptletBundleInput")
        );
    assert!(
        std::any::type_name::<crate::ccs::convert::ScriptletBundleBuild>()
            .contains("ScriptletBundleBuild")
    );
    assert!(
        std::any::type_name::<crate::ccs::convert::scriptlet_bundle::ScriptletBundleBuild>()
            .contains("ScriptletBundleBuild")
    );

    let _root_builder: for<'a> fn(
        crate::ccs::convert::ScriptletBundleInput<'a>,
    )
        -> anyhow::Result<crate::ccs::convert::ScriptletBundleBuild> =
        crate::ccs::convert::build_native_lifecycle_bundle;
    let _module_builder: for<'a> fn(
        crate::ccs::convert::scriptlet_bundle::ScriptletBundleInput<'a>,
    ) -> anyhow::Result<
        crate::ccs::convert::scriptlet_bundle::ScriptletBundleBuild,
    > = crate::ccs::convert::scriptlet_bundle::build_native_lifecycle_bundle;
}

fn make_test_metadata() -> PackageMetadata {
    PackageMetadata {
        package_path: PathBuf::from("/tmp/test-1.0.0.rpm"),
        name: "test-package".to_string(),
        version: "1.0.0".to_string(),
        version_scheme: crate::repository::versioning::VersionScheme::Rpm,
        architecture: Some("x86_64".to_string()),
        debian_multi_arch: None,
        description: Some("Test package".to_string()),
        files: vec![PackageFile {
            path: "/usr/bin/test".to_string(),
            node: PayloadNode::regular(0o755),
            content: Some(content_authority(TEST_PAYLOAD)),
        }],
        requirements: vec![],
        provides: vec![],
        relations: vec![],
        diagnostic_scriptlet_evidence: Vec::new(),
        native_scriptlet_abi: vec![rpm_native_entry(
            "rpm:%pre",
            "%pre",
            "getent passwd testuser || useradd -r testuser",
            RpmScriptletSlot::Pre,
            NativeLifecyclePath::PreInstall,
            NativeTransactionPosition::BeforePayload,
            NativeScriptletSupport::Parsed,
        )],
        config_files: vec![],
    }
}

fn make_test_files() -> Vec<ExtractedFile> {
    vec![extracted_file("/usr/bin/test", TEST_PAYLOAD, 0o755)]
}

fn rpm_native_entry(
    id: &str,
    slot_name: &str,
    body: &str,
    slot: RpmScriptletSlot,
    lifecycle: NativeLifecyclePath,
    position: NativeTransactionPosition,
    support: NativeScriptletSupport,
) -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: id.to_string(),
        format: NativeScriptletFormat::Rpm,
        kind: NativeScriptletKind::Executable,
        native_slot: slot_name.to_string(),
        primary_lifecycle: lifecycle,
        lifecycle_paths: vec![lifecycle],
        interpreter: Some("/bin/sh".to_string()),
        interpreter_args: vec![],
        body: NativeScriptletBody::from_bytes(body.as_bytes().to_vec()),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(position),
        support,
        metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
            slot,
            runtime: RpmScriptletRuntimeMetadata {
                program: RpmScriptletProgram::External,
                flags: RpmScriptletFlagsMetadata {
                    names: Vec::new(),
                    raw_bits: 0,
                    unknown_bits: 0,
                    expand: false,
                    query_format: false,
                    critical: false,
                    criticality: RpmScriptletCriticality::WarningOnly,
                },
                install_prefixes: Vec::new(),
                macro_context: Default::default(),
                header_context: Default::default(),
                package_rpm_version: None,
            },
            trigger: None,
            sysusers: None,
        }),
    }
}

fn set_test_scriptlet(
    metadata: &mut PackageMetadata,
    phase: DiagnosticScriptletPhase,
    content: impl Into<String>,
) {
    let (id, slot_name, slot, lifecycle, position) = match phase {
        DiagnosticScriptletPhase::PreInstall => (
            "rpm:%pre",
            "%pre",
            RpmScriptletSlot::Pre,
            NativeLifecyclePath::PreInstall,
            NativeTransactionPosition::BeforePayload,
        ),
        DiagnosticScriptletPhase::PostInstall => (
            "rpm:%post",
            "%post",
            RpmScriptletSlot::Post,
            NativeLifecyclePath::PostInstall,
            NativeTransactionPosition::AfterPayload,
        ),
        other => panic!("unsupported test lifecycle phase: {other}"),
    };
    metadata.diagnostic_scriptlet_evidence.clear();
    metadata.native_scriptlet_abi = vec![rpm_native_entry(
        id,
        slot_name,
        &content.into(),
        slot,
        lifecycle,
        position,
        NativeScriptletSupport::Parsed,
    )];
}

fn arch_install_function_entry(function_name: &str, install_source: &str) -> NativeScriptletEntry {
    NativeScriptletEntry {
        id: format!("arch:{function_name}"),
        format: NativeScriptletFormat::Arch,
        kind: NativeScriptletKind::Executable,
        native_slot: function_name.to_string(),
        primary_lifecycle: NativeLifecyclePath::PostInstall,
        lifecycle_paths: vec![NativeLifecyclePath::PostInstall],
        interpreter: None,
        interpreter_args: vec![],
        body: NativeScriptletBody::from_bytes(install_source.as_bytes().to_vec()),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(NativeTransactionPosition::AfterPayload),
        support: NativeScriptletSupport::Parsed,
        metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(
            ArchInstallScriptletMetadata {
                install_source_sha256: crate::hash::sha256_prefixed(install_source.as_bytes()),
                function_name: function_name.to_string(),
                selection_contract:
                    crate::packages::native_abi::ArchInstallSelectionContract::LibalpmGrepV1,
            },
        )),
    }
}

fn passive_test_converter(output_dir: &std::path::Path) -> NativePackageConverter {
    NativePackageConverter::new(ConversionOptions {
        output_dir: output_dir.to_path_buf(),
    })
    .with_signing_key(std::sync::Arc::new(
        crate::ccs::signing::SigningKeyPair::generate().with_key_id("converter-test"),
    ))
}

fn verified_converted_package(result: &ConversionResult) -> crate::ccs::CcsPackage {
    let path = result
        .package_path
        .as_ref()
        .expect("converted package path");
    let verified = crate::ccs::verify::verify_package(
        path,
        &crate::ccs::verify::TrustPolicy::strict(vec![result.signing_public_key.clone()]),
    )
    .expect("converted CCS authority should verify");
    crate::ccs::CcsPackage::from_verified_archive(path.to_str().unwrap(), &verified)
        .expect("verified converted CCS package should open")
}

#[test]
fn conversion_result_embeds_native_lifecycle_bundle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    set_test_scriptlet(
        &mut metadata,
        DiagnosticScriptletPhase::PostInstall,
        "/sbin/ldconfig\n",
    );

    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .unwrap();
    let bundle = result
        .build_result
        .manifest
        .native_lifecycle
        .as_ref()
        .unwrap();
    assert_eq!(bundle.source_package, metadata.name);
    assert_eq!(
        result
            .native_lifecycle
            .as_ref()
            .unwrap()
            .evidence_digest
            .as_deref(),
        bundle.evidence_digest.as_deref()
    );
    assert_eq!(
        result.scriptlet_metadata.evidence_digest.as_deref(),
        bundle.evidence_digest.as_deref()
    );
    bundle.validate().unwrap();
}

#[test]
fn arch_install_conversion_requires_exact_source_profile() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.package_path = PathBuf::from("/tmp/test-1.0.0.pkg.tar.zst");
    metadata.version_scheme = crate::repository::versioning::VersionScheme::Arch;
    metadata.diagnostic_scriptlet_evidence.clear();
    metadata.native_scriptlet_abi = vec![arch_install_function_entry(
        "post_install",
        "post_install() { :; }\n",
    )];

    let error = passive_test_converter(temp_dir.path())
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "arch",
            "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires an exact ALPM source profile ID"),
        "{error}"
    );
}

#[test]
fn conversion_rejects_format_version_authority_mismatch() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();

    let error = passive_test_converter(temp_dir.path())
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "deb",
            "sha256:abababababababababababababababababababababababababababababababab",
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("format 'deb' requires 'debian' version authority, got 'rpm'"),
        "{error}"
    );
}

#[test]
fn conversion_result_attaches_foreign_boundary_provenance() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        )
        .unwrap();

    let provenance = result
        .build_result
        .manifest
        .provenance
        .as_ref()
        .expect("conversion provenance");
    assert_eq!(
        provenance.origin_class.as_deref(),
        Some("foreign-converted")
    );
    assert_eq!(provenance.hardening_level.as_deref(), Some("hermetic"));
    let boundary = provenance
        .foreign_conversion_boundary
        .as_ref()
        .expect("foreign conversion boundary");
    assert_eq!(boundary.source_format, "rpm");
    assert_eq!(
        boundary.source_checksum,
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
    );
    let build_risk_report = boundary
        .build_risk_report
        .as_ref()
        .expect("build risk report");
    assert_eq!(
        build_risk_report.highest_severity,
        crate::security::command_risk::CommandRiskSeverity::None
    );
    assert_eq!(
        boundary.build_risk_report_hash.as_deref(),
        Some(canonical_json_hash(build_risk_report).unwrap().as_str())
    );
    let scriptlet_risk_report = boundary
        .scriptlet_risk_report
        .as_ref()
        .expect("scriptlet risk report");
    assert_eq!(
        boundary.scriptlet_risk_report_hash.as_deref(),
        Some(canonical_json_hash(scriptlet_risk_report).unwrap().as_str())
    );

    let package = verified_converted_package(&result);
    assert!(
        package
            .manifest()
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.foreign_conversion_boundary.as_ref())
            .is_some()
    );
}

#[test]
fn conversion_preserves_exact_source_architecture_tokens_in_signed_identity() {
    use crate::repository::dependency_model::DebianMultiArch;
    use crate::repository::versioning::VersionScheme;

    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path());
    for (name, scheme, architecture, multi_arch, source_format) in [
        (
            "debian-all-token",
            VersionScheme::Debian,
            "all",
            Some(DebianMultiArch::No),
            "deb",
        ),
        ("arch-any-token", VersionScheme::Arch, "any", None, "arch"),
        (
            "rpm-noarch-token",
            VersionScheme::Rpm,
            "noarch",
            None,
            "rpm",
        ),
    ] {
        let mut metadata = make_test_metadata();
        metadata.name = name.to_string();
        metadata.version = "1".to_string();
        metadata.version_scheme = scheme;
        metadata.architecture = Some(architecture.to_string());
        metadata.debian_multi_arch = multi_arch;
        metadata.native_scriptlet_abi.clear();
        metadata.diagnostic_scriptlet_evidence.clear();

        let result = converter
            .convert_in_memory_for_test(
                &metadata,
                &make_test_files(),
                source_format,
                &format!("sha256:{}", crate::hash::sha256(name.as_bytes())),
            )
            .unwrap();
        let package = verified_converted_package(&result);
        let identity = &package.v2_authority().unwrap().identity;
        assert_eq!(identity.version_scheme, scheme);
        assert_eq!(identity.architecture.as_deref(), Some(architecture));
        assert_eq!(identity.debian_multi_arch, multi_arch);
    }
}

#[test]
fn converted_rpm_preserves_exact_config_and_ghost_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    let config_content = b"setting=true\n";
    metadata.files.push(PackageFile {
        path: "/etc/test-package.conf".to_string(),
        node: PayloadNode::regular(0o644),
        content: Some(content_authority(config_content)),
    });
    metadata.config_files = vec![
        ConfigFileInfo {
            path: "/etc/test-package.conf".to_string(),
            noreplace: false,
            ghost: false,
            remove_on_upgrade: false,
        },
        ConfigFileInfo {
            path: "/var/lib/test-package/runtime.conf".to_string(),
            noreplace: true,
            ghost: true,
            remove_on_upgrade: false,
        },
    ];
    let mut files = make_test_files();
    files.push(extracted_file(
        "/etc/test-package.conf",
        config_content,
        0o644,
    ));

    let result = passive_test_converter(temp_dir.path())
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:1212121212121212121212121212121212121212121212121212121212121212",
        )
        .unwrap();
    let package = verified_converted_package(&result);

    assert_eq!(package.config_files(), metadata.config_files);
    assert_eq!(package.manifest().config.files, metadata.config_files);
    let authority = package.v2_authority().unwrap();
    let PackageKindV2::Package(data) = &authority.kind else {
        panic!("converted package did not carry package authority");
    };
    assert_eq!(data.config.len(), 2);
    assert!(
        data.files
            .iter()
            .find(|file| file.path == "/etc/test-package.conf")
            .unwrap()
            .config
            .is_some()
    );
    assert!(
        data.files
            .iter()
            .all(|file| file.path != "/var/lib/test-package/runtime.conf")
    );
}

#[test]
fn converted_deb_preserves_remove_on_upgrade_without_inventing_payload() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.version_scheme = crate::repository::versioning::VersionScheme::Debian;
    metadata.architecture = Some("amd64".to_string());
    metadata.debian_multi_arch = Some(crate::repository::dependency_model::DebianMultiArch::No);
    metadata.native_scriptlet_abi.clear();
    metadata.config_files = vec![ConfigFileInfo {
        path: "/etc/test-package.retired".to_string(),
        noreplace: true,
        ghost: false,
        remove_on_upgrade: true,
    }];

    let result = passive_test_converter(temp_dir.path())
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "deb",
            "sha256:3434343434343434343434343434343434343434343434343434343434343434",
        )
        .unwrap();
    let package = verified_converted_package(&result);

    assert_eq!(package.config_files(), metadata.config_files);
    assert_eq!(package.manifest().config.files, metadata.config_files);
    assert!(
        package
            .files()
            .iter()
            .all(|file| file.path != "/etc/test-package.retired")
    );
}

#[test]
fn conversion_boundary_records_foreign_scriptlet_command_risk() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.package_path = PathBuf::from("/tmp/test-1.0.0.pkg.tar.zst");
    metadata.version_scheme = crate::repository::versioning::VersionScheme::Arch;
    metadata.diagnostic_scriptlet_evidence.clear();
    metadata.native_scriptlet_abi = vec![arch_install_function_entry(
        "post_install",
        "post_install() { npm install synthetic-atomic-lockfile; }\n",
    )];
    let converter = passive_test_converter(temp_dir.path()).with_source_profile("arch");

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "arch",
            "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        )
        .unwrap();
    assert_eq!(
        result.native_lifecycle.as_ref().unwrap().entries[0].interpreter,
        "/usr/bin/bash"
    );

    let boundary = result
        .build_result
        .manifest
        .provenance
        .as_ref()
        .and_then(|provenance| provenance.foreign_conversion_boundary.as_ref())
        .expect("foreign conversion boundary");
    let report = boundary
        .scriptlet_risk_report
        .as_ref()
        .expect("scriptlet risk report");
    assert_eq!(
        report.highest_severity,
        crate::security::command_risk::CommandRiskSeverity::Notice
    );
    assert!(report.entries.iter().any(|entry| {
        entry.reason_code == crate::security::command_risk::PACKAGE_MANAGER_FETCH
    }));
    assert_eq!(
        boundary.scriptlet_risk_report_hash.as_deref(),
        Some(canonical_json_hash(report).unwrap().as_str())
    );
}

#[test]
fn converted_ccs_archive_round_trip_preserves_native_lifecycle_bundle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    set_test_scriptlet(
        &mut metadata,
        DiagnosticScriptletPhase::PostInstall,
        "/sbin/ldconfig\n",
    );
    let converter = passive_test_converter(temp_dir.path());

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        )
        .unwrap();
    let package_path = result.package_path.as_ref().unwrap();

    let file = std::fs::File::open(package_path).unwrap();
    let archive = crate::ccs::archive_reader::inspect_untrusted_ccs_archive(file).unwrap();
    let bundle = archive.manifest.native_lifecycle.as_ref().unwrap();

    assert_eq!(bundle.source_package, metadata.name);
    bundle.validate().unwrap();
}

#[test]
fn remi_converter_context_flows_into_bundle_metadata() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();
    let converter = passive_test_converter(temp_dir.path())
        .with_source_profile("fedora-44")
        .with_source_release("44")
        .with_conversion_tool("remi");

    let result = converter
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        )
        .unwrap();

    let bundle = result
        .build_result
        .manifest
        .native_lifecycle
        .as_ref()
        .unwrap();
    assert_eq!(bundle.source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(bundle.source_release.as_deref(), Some("44"));
    assert_eq!(bundle.conversion_tool, "remi");
    assert_eq!(
        bundle.source_checksum.as_deref(),
        Some("sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
    );
}

fn convert_scriptlet_body(converter: &NativePackageConverter, content: &str) -> ConversionResult {
    let mut metadata = make_test_metadata();
    set_test_scriptlet(
        &mut metadata,
        DiagnosticScriptletPhase::PostInstall,
        content,
    );
    converter
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("conversion succeeds")
}

#[test]
fn conversion_preserves_profile_allowed_sysctl_lifecycle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path());

    let result = convert_scriptlet_body(&converter, "sysctl -w kernel.example=1\n");

    let sysctl_hooks = &result.build_result.manifest.hooks.sysctl;
    assert!(sysctl_hooks.is_empty());
    let bundle = result.native_lifecycle.as_ref().expect("scriptlet bundle");
    assert_eq!(bundle.entries.len(), 1);
}

#[test]
fn sysctl_text_remains_in_the_exact_native_lifecycle_body() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path());

    let result = convert_scriptlet_body(&converter, "sysctl -w net.ipv4.ip_forward=1\n");

    let sysctl_hooks = &result.build_result.manifest.hooks.sysctl;
    assert!(sysctl_hooks.is_empty());
    let bundle = result.native_lifecycle.as_ref().expect("scriptlet bundle");
    let bundle_summary =
        ScriptletBundleSummary::from_bundle(bundle, bundle.evidence_digest.clone());
    assert_eq!(bundle_summary.scriptlet_fidelity, "native-lifecycle");
    assert!(bundle.entries[0].body.contains("net.ipv4.ip_forward"));
}

#[path = "tests/manifest.rs"]
mod manifest;
