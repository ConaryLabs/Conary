// conary-core/src/ccs/convert/converter/tests.rs

use super::*;
use crate::ccs::v3::PackageKindV3;
use crate::packages::config_authority::{ConfigPayloadAssociation, SourceConfigDeclaration};
use crate::packages::native_abi::*;
use crate::packages::traits::{
    DiagnosticScriptletPhase, ExtractedFile, PackageFile, PackageFormat,
};
use crate::payload::{PayloadContentAuthority, PayloadNode};

const TEST_PAYLOAD: &[u8] = b"#!/bin/sh\necho test";

struct TestConverter {
    converter: NativePackageConverter,
    policy: crate::ccs::verify::TrustPolicy,
}

impl TestConverter {
    fn with_source_profile(mut self, profile: impl Into<String>) -> Self {
        self.converter = self.converter.with_source_profile(profile);
        self
    }

    fn with_source_release(mut self, release: impl Into<String>) -> Self {
        self.converter = self.converter.with_source_release(release);
        self
    }

    fn with_conversion_tool(mut self, tool: impl Into<String>) -> Self {
        self.converter = self.converter.with_conversion_tool(tool);
        self
    }

    fn author_payload(
        &self,
        metadata: &ForeignConversionInput,
        files: &[crate::packages::payload::PackagePayloadFile],
        format: &str,
        checksum: &crate::hash::Hash,
    ) -> Result<PendingConversionResult, ConversionError> {
        self.converter
            .convert_payload(metadata, files, format, checksum)
    }

    fn finalize(
        &self,
        pending: PendingConversionResult,
    ) -> anyhow::Result<FinalizedTestConversion> {
        pending.verify(&self.policy).map(FinalizedTestConversion)
    }

    fn convert_payload(
        &self,
        metadata: &ForeignConversionInput,
        files: &[crate::packages::payload::PackagePayloadFile],
        format: &str,
        checksum: &crate::hash::Hash,
    ) -> anyhow::Result<FinalizedTestConversion> {
        self.finalize(self.author_payload(metadata, files, format, checksum)?)
    }
}

#[derive(Debug)]
struct FinalizedTestConversion(VerifiedConversionResult);

impl std::ops::Deref for FinalizedTestConversion {
    type Target = ConversionResult;

    fn deref(&self) -> &Self::Target {
        self.0.conversion()
    }
}

trait InMemoryConversionForTest {
    fn convert_in_memory_for_test(
        &self,
        metadata: &ForeignConversionInput,
        files: &[ExtractedFile],
        format: &str,
        checksum: &str,
    ) -> anyhow::Result<FinalizedTestConversion>;
}

impl InMemoryConversionForTest for TestConverter {
    fn convert_in_memory_for_test(
        &self,
        metadata: &ForeignConversionInput,
        files: &[ExtractedFile],
        format: &str,
        checksum: &str,
    ) -> anyhow::Result<FinalizedTestConversion> {
        let payload = crate::packages::PackagePayload::from_extracted_in_memory(files.to_vec())
            .map_err(|error| ConversionError::IoError(error.to_string()))?;
        let checksum =
            crate::hash::Hash::parse_prefixed(checksum).expect("valid conversion test checksum");
        self.convert_payload(metadata, payload.files(), format, &checksum)
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

fn make_test_metadata() -> ForeignConversionInput {
    let mut metadata = ForeignConversionInput::new(
        PathBuf::from("/tmp/test-1.0.0.rpm"),
        "test-package".to_string(),
        "1.0.0".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
    );
    set_test_identity(
        &mut metadata,
        "test-package",
        "1.0.0",
        crate::repository::versioning::VersionScheme::Rpm,
        Some("x86_64"),
        None,
    );
    metadata.description = Some("Test package".to_string());
    metadata.files = vec![PackageFile {
        path: "/usr/bin/test".to_string(),
        node: PayloadNode::regular(0o755),
        content: Some(content_authority(TEST_PAYLOAD)),
    }];
    metadata.native_scriptlet_abi = vec![rpm_native_entry(
        "rpm:%pre",
        "%pre",
        "getent passwd testuser || useradd -r testuser",
        RpmScriptletSlot::Pre,
        NativeLifecyclePath::PreInstall,
        NativeTransactionPosition::BeforePayload,
        NativeScriptletSupport::Parsed,
    )];
    metadata
}

fn set_test_identity(
    metadata: &mut ForeignConversionInput,
    name: &str,
    version: &str,
    version_scheme: crate::repository::versioning::VersionScheme,
    architecture: Option<&str>,
    debian_multi_arch: Option<crate::repository::dependency_model::DebianMultiArch>,
) {
    metadata.source_authority = crate::packages::source_authority::SourcePackageAuthority::Ccs(
        crate::packages::source_authority::CcsPackageAuthority {
            name: name.to_string(),
            version: version.to_string(),
            version_scheme,
            architecture: architecture.map(str::to_string),
            debian_multi_arch,
            capabilities: Vec::new(),
            config: Vec::new(),
        },
    );
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
                flags: {
                    // Derive the stamp exactly as the parser does: the slot's
                    // class authority with no CRITICAL header flag. Hand-set
                    // combinations the parser cannot emit are rejected by the
                    // bundle's class cross-check.
                    let criticality = crate::scriptlet::rpm_package_slot_authority(slot)
                        .effective_criticality(false);
                    RpmScriptletFlagsMetadata {
                        names: Vec::new(),
                        raw_bits: 0,
                        unknown_bits: 0,
                        expand: false,
                        query_format: false,
                        critical: criticality.is_critical(),
                        criticality,
                    }
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
    metadata: &mut ForeignConversionInput,
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

fn passive_test_converter(output_dir: &std::path::Path) -> TestConverter {
    let signing_key = std::sync::Arc::new(
        crate::ccs::signing::SigningKeyPair::generate().with_key_id("converter-test"),
    );
    test_converter_with_signing_key(output_dir, signing_key)
}

fn test_converter_with_signing_key(
    output_dir: &std::path::Path,
    signing_key: std::sync::Arc<crate::ccs::signing::SigningKeyPair>,
) -> TestConverter {
    let policy = crate::ccs::verify::TrustPolicy::strict(vec![signing_key.public_key_base64()]);
    TestConverter {
        converter: NativePackageConverter::new(ConversionOptions {
            output_dir: output_dir.to_path_buf(),
        })
        .with_signing_key(signing_key),
        policy,
    }
}

fn repack_ccs_at_gzip_level_zero(source: &std::path::Path, destination: &std::path::Path) {
    use std::io::Read;

    let decoder = flate2::read::GzDecoder::new(std::fs::File::open(source).unwrap());
    let mut source_archive = tar::Archive::new(decoder);
    let entries = source_archive
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let header = entry.header().clone();
            let path = entry.path().unwrap().into_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (header, path, bytes)
        })
        .collect::<Vec<_>>();

    let encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(destination).unwrap(),
        flate2::Compression::none(),
    );
    let mut destination_archive = tar::Builder::new(encoder);
    for (mut header, path, bytes) in entries {
        destination_archive
            .append_data(&mut header, path, bytes.as_slice())
            .unwrap();
    }
    destination_archive.into_inner().unwrap().finish().unwrap();
}

#[test]
fn conversion_requires_typed_sha256_source_identity() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();
    let files = make_test_files();
    let payload = crate::packages::PackagePayload::from_extracted_in_memory(files).unwrap();
    let checksum =
        crate::hash::Hash::new(crate::hash::HashAlgorithm::Xxh128, "a".repeat(32)).unwrap();

    let error = passive_test_converter(temp_dir.path())
        .convert_payload(&metadata, payload.files(), "rpm", &checksum)
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("source checksum must use SHA-256"),
        "{error}"
    );
}

#[test]
fn pending_conversion_rejects_a_policy_without_the_configured_authoring_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path());
    let metadata = make_test_metadata();
    let payload =
        crate::packages::PackagePayload::from_extracted_in_memory(make_test_files()).unwrap();
    let checksum = crate::hash::Hash::parse_prefixed(
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .unwrap();
    let pending = converter
        .author_payload(&metadata, payload.files(), "rpm", &checksum)
        .unwrap();
    let other_key = crate::ccs::signing::SigningKeyPair::generate();
    let wrong_policy = crate::ccs::verify::TrustPolicy::strict(vec![other_key.public_key_base64()]);

    let error = pending.verify(&wrong_policy).unwrap_err();

    assert!(format!("{error:#}").contains("not trusted"), "{error:#}");
}

#[test]
fn pending_conversion_rejects_archive_tampering_before_finalization() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path());
    let metadata = make_test_metadata();
    let payload =
        crate::packages::PackagePayload::from_extracted_in_memory(make_test_files()).unwrap();
    let checksum = crate::hash::Hash::parse_prefixed(
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
    .unwrap();
    let pending = converter
        .author_payload(&metadata, payload.files(), "rpm", &checksum)
        .unwrap();
    std::fs::write(
        pending.unverified_package_path(),
        b"tampered pending archive",
    )
    .unwrap();

    let error = converter.finalize(pending).unwrap_err();

    assert!(
        format!("{error:#}").contains("verify streaming CCS v3 archive"),
        "{error:#}"
    );
}

#[test]
fn pending_conversion_rejects_another_valid_archive_from_the_same_trusted_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path());
    let metadata = make_test_metadata();
    let payload =
        crate::packages::PackagePayload::from_extracted_in_memory(make_test_files()).unwrap();
    let checksum = crate::hash::Hash::parse_prefixed(
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    )
    .unwrap();
    let pending = converter
        .author_payload(&metadata, payload.files(), "rpm", &checksum)
        .unwrap();

    let mut other_metadata = metadata.clone();
    set_test_identity(
        &mut other_metadata,
        "other-package",
        "1.0.0",
        crate::repository::versioning::VersionScheme::Rpm,
        Some("x86_64"),
        None,
    );
    let other_checksum = crate::hash::Hash::parse_prefixed(
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    )
    .unwrap();
    let other_pending = converter
        .author_payload(&other_metadata, payload.files(), "rpm", &other_checksum)
        .unwrap();
    let other_path = other_pending.unverified_package_path().to_path_buf();

    let error = pending
        .verify_staged_copy(&other_path, &converter.policy)
        .unwrap_err();

    assert!(
        format!("{error:#}").contains("differs from the pending native conversion"),
        "{error:#}"
    );
}

#[test]
fn pending_conversion_rejects_same_authority_resigned_by_another_trusted_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let authoring_dir = temp_dir.path().join("authoring");
    let resigned_dir = temp_dir.path().join("resigned");
    let authoring_key = std::sync::Arc::new(
        crate::ccs::signing::SigningKeyPair::generate().with_key_id("authoring"),
    );
    let resigning_key = std::sync::Arc::new(
        crate::ccs::signing::SigningKeyPair::generate().with_key_id("resigning"),
    );
    let authoring = test_converter_with_signing_key(&authoring_dir, authoring_key.clone());
    let resigning = test_converter_with_signing_key(&resigned_dir, resigning_key.clone());
    let metadata = make_test_metadata();
    let payload =
        crate::packages::PackagePayload::from_extracted_in_memory(make_test_files()).unwrap();
    let checksum = crate::hash::Hash::parse_prefixed(
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    )
    .unwrap();
    let pending = authoring
        .author_payload(&metadata, payload.files(), "rpm", &checksum)
        .unwrap();
    let resigned = resigning
        .author_payload(&metadata, payload.files(), "rpm", &checksum)
        .unwrap();
    let resigned_path = resigned.unverified_package_path().to_path_buf();
    let policy = crate::ccs::verify::TrustPolicy::strict(vec![
        authoring_key.public_key_base64(),
        resigning_key.public_key_base64(),
    ]);

    let error = pending
        .verify_staged_copy(&resigned_path, &policy)
        .unwrap_err();

    assert!(
        format!("{error:#}").contains("signer differs from the pending native conversion"),
        "{error:#}"
    );
}

#[test]
fn pending_conversion_rejects_a_same_authority_same_signer_canonical_repack() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(&temp_dir.path().join("authored"));
    let metadata = make_test_metadata();
    let payload =
        crate::packages::PackagePayload::from_extracted_in_memory(make_test_files()).unwrap();
    let checksum = crate::hash::Hash::parse_prefixed(
        "sha256:abababababababababababababababababababababababababababababababab",
    )
    .unwrap();
    let pending = converter
        .author_payload(&metadata, payload.files(), "rpm", &checksum)
        .unwrap();
    let authored_path = pending.unverified_package_path().to_path_buf();
    let repacked_path = temp_dir.path().join("repacked.ccs");
    repack_ccs_at_gzip_level_zero(&authored_path, &repacked_path);

    let authored = crate::ccs::verify::verify_package(&authored_path, &converter.policy).unwrap();
    let repacked = crate::ccs::verify::verify_package(&repacked_path, &converter.policy).unwrap();
    assert_eq!(repacked.authority(), authored.authority());
    assert_eq!(repacked.signature(), authored.signature());
    assert_ne!(repacked.archive_identity(), authored.archive_identity());
    drop((authored, repacked));

    let error = pending
        .verify_staged_copy(&repacked_path, &converter.policy)
        .unwrap_err();

    assert!(
        format!("{error:#}").contains("archive identity differs from the authored"),
        "{error:#}"
    );
}

#[test]
fn staged_copy_finalizer_verifies_once_into_the_supplied_permanent_cas() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(&temp_dir.path().join("authored"));
    let metadata = make_test_metadata();
    let payload =
        crate::packages::PackagePayload::from_extracted_in_memory(make_test_files()).unwrap();
    let checksum = crate::hash::Hash::parse_prefixed(
        "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    )
    .unwrap();
    let pending = converter
        .author_payload(&metadata, payload.files(), "rpm", &checksum)
        .unwrap();
    let staged_path = temp_dir.path().join("publication").join("staged.ccs");
    std::fs::create_dir_all(staged_path.parent().unwrap()).unwrap();
    std::fs::copy(pending.unverified_package_path(), &staged_path).unwrap();
    let staged_bytes = std::fs::read(&staged_path).unwrap();
    let cas = crate::filesystem::CasStore::new(temp_dir.path().join("objects")).unwrap();

    let verified = pending
        .verify_staged_copy_into_cas(&staged_path, &converter.policy, &cas)
        .unwrap();

    assert_eq!(verified.conversion().package_path, staged_path);
    assert_eq!(
        verified.archive_identity().sha256(),
        crate::hash::sha256(&staged_bytes)
    );
    assert_eq!(
        verified.archive_identity().bytes(),
        staged_bytes.len() as u64
    );
    let metrics = verified.verification().verified_object_metrics().unwrap();
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.hits, 0);
    assert!(metrics.persistent_bytes_written > 0);
}

#[test]
fn conversion_signs_native_file_capability_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let metadata = make_test_metadata();
    let mut files = make_test_files();
    files[0].node.xattrs.insert(
        crate::ccs::manifest::LINUX_SECURITY_CAPABILITY_XATTR.to_string(),
        hex::decode("0100000200040000000000000000000000000000").unwrap(),
    );

    let result = passive_test_converter(temp_dir.path())
        .convert_in_memory_for_test(
            &metadata,
            &files,
            "rpm",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
    let package = verified_converted_package(&result);

    assert_eq!(
        package.manifest().file_capabilities,
        vec![crate::ccs::manifest::FileCapability {
            path: "/usr/bin/test".to_string(),
            capabilities: vec!["cap_net_bind_service".to_string()],
            permitted: true,
            effective: true,
            inheritable: false,
        }]
    );
}

fn verified_converted_package(result: &FinalizedTestConversion) -> crate::ccs::CcsPackage {
    let path = &result.package_path;
    crate::ccs::CcsPackage::from_verified_archive(path.to_str().unwrap(), result.0.verification())
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
    assert_eq!(bundle.source_package, metadata.name());
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
fn conversion_preserves_strong_source_order_in_signed_ccs_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.requirements = vec![
        crate::repository::requirement::parse_native_requirement(
            crate::repository::dependency_model::RepositoryRequirementKind::PreDepends,
            crate::repository::versioning::VersionScheme::Rpm,
            "setup",
        )
        .unwrap(),
    ];

    let result = passive_test_converter(temp_dir.path())
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:1313131313131313131313131313131313131313131313131313131313131313",
        )
        .unwrap();
    let package = verified_converted_package(&result);

    assert_eq!(
        result.build_result.manifest.requirements,
        metadata.requirements
    );
    assert_eq!(package.requirements(), metadata.requirements);
    assert_eq!(
        package.v3_authority().unwrap().requirements,
        metadata.requirements
    );
}

#[test]
fn arch_install_conversion_requires_exact_source_profile() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.package_path = PathBuf::from("/tmp/test-1.0.0.pkg.tar.zst");
    set_test_identity(
        &mut metadata,
        "test-package",
        "1.0.0",
        crate::repository::versioning::VersionScheme::Arch,
        Some("x86_64"),
        None,
    );
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
    for (name, version, scheme, architecture, multi_arch, source_format) in [
        (
            "debian-all-token",
            "1",
            VersionScheme::Debian,
            "all",
            Some(DebianMultiArch::No),
            "deb",
        ),
        (
            "arch-any-token",
            "1",
            VersionScheme::Arch,
            "any",
            None,
            "arch",
        ),
        (
            "rpm-noarch-token",
            "1",
            VersionScheme::Rpm,
            "noarch",
            None,
            "rpm",
        ),
        (
            "eopkg-x86-64-token",
            "1-1",
            VersionScheme::Eopkg,
            "x86_64",
            None,
            "eopkg",
        ),
    ] {
        let mut metadata = make_test_metadata();
        set_test_identity(
            &mut metadata,
            name,
            version,
            scheme,
            Some(architecture),
            multi_arch,
        );
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
        let identity = &package.v3_authority().unwrap().identity;
        assert_eq!(identity.version_scheme, scheme);
        assert_eq!(identity.architecture.as_deref(), Some(architecture));
        assert_eq!(identity.debian_multi_arch, multi_arch);
    }
}

#[test]
fn conversion_rejects_missing_source_architecture_authority() {
    let temp_dir = tempfile::tempdir().unwrap();
    let converter = passive_test_converter(temp_dir.path());
    let mut metadata = make_test_metadata();
    set_test_identity(
        &mut metadata,
        "test-package",
        "1.0.0",
        crate::repository::versioning::VersionScheme::Rpm,
        None,
        None,
    );

    let error = converter
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "rpm",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<ConversionError>(),
        Some(ConversionError::BuildError(message))
            if message.contains("v3 package identity architecture is required")
    ));
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
    let config = vec![
        SourceConfigDeclaration::Rpm(crate::packages::rpm::authority::RpmConfigDeclaration {
            header_index: 0,
            path: "/etc/test-package.conf".to_string(),
            noreplace: false,
            ghost: false,
            missing_ok: false,
            payload: ConfigPayloadAssociation::Matched,
        }),
        SourceConfigDeclaration::Rpm(crate::packages::rpm::authority::RpmConfigDeclaration {
            header_index: 1,
            path: "/var/lib/test-package/runtime.conf".to_string(),
            noreplace: true,
            ghost: true,
            missing_ok: false,
            payload: ConfigPayloadAssociation::Absent,
        }),
    ];
    let crate::packages::source_authority::SourcePackageAuthority::Ccs(authority) =
        &mut metadata.source_authority
    else {
        unreachable!()
    };
    authority.config = config.clone();
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

    assert_eq!(package.config_declarations().unwrap(), config);
    assert_eq!(package.manifest().config.files, config);
    let authority = package.v3_authority().unwrap();
    let PackageKindV3::Package(data) = &authority.kind else {
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
    set_test_identity(
        &mut metadata,
        "test-package",
        "1.0.0",
        crate::repository::versioning::VersionScheme::Debian,
        Some("amd64"),
        Some(crate::repository::dependency_model::DebianMultiArch::No),
    );
    metadata.native_scriptlet_abi.clear();
    let config = vec![SourceConfigDeclaration::Debian(
        crate::packages::deb::authority::DebianConfigDeclaration {
            control_index: 0,
            path: "/etc/test-package.retired".to_string(),
            remove_on_upgrade: true,
            payload: ConfigPayloadAssociation::Absent,
        },
    )];
    let crate::packages::source_authority::SourcePackageAuthority::Ccs(authority) =
        &mut metadata.source_authority
    else {
        unreachable!()
    };
    authority.config = config.clone();

    let result = passive_test_converter(temp_dir.path())
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "deb",
            "sha256:3434343434343434343434343434343434343434343434343434343434343434",
        )
        .unwrap();
    let package = verified_converted_package(&result);

    assert_eq!(package.config_declarations().unwrap(), config);
    assert_eq!(package.manifest().config.files, config);
    assert!(
        package
            .files()
            .iter()
            .all(|file| file.path != "/etc/test-package.retired")
    );
}

#[test]
fn converted_alpm_preserves_unmatched_backup_without_inventing_payload() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    set_test_identity(
        &mut metadata,
        "bash-completion",
        "2.18.0-1",
        crate::repository::versioning::VersionScheme::Arch,
        Some("any"),
        None,
    );
    metadata.native_scriptlet_abi.clear();
    let path = "/etc/bash_completion.d/000_bash_completion_compat.bash";
    let config = vec![SourceConfigDeclaration::Alpm(
        crate::packages::arch::authority::AlpmConfigDeclaration {
            pkginfo_index: 0,
            source_path: path.trim_start_matches('/').to_string(),
            path: path.to_string(),
            installed_hash: None,
            payload: ConfigPayloadAssociation::Absent,
        },
    )];
    let crate::packages::source_authority::SourcePackageAuthority::Ccs(authority) =
        &mut metadata.source_authority
    else {
        unreachable!()
    };
    authority.config = config.clone();

    let result = passive_test_converter(temp_dir.path())
        .convert_in_memory_for_test(
            &metadata,
            &make_test_files(),
            "arch",
            "sha256:5656565656565656565656565656565656565656565656565656565656565656",
        )
        .unwrap();
    let package = verified_converted_package(&result);

    assert_eq!(package.config_declarations().unwrap(), config);
    let PackageKindV3::Package(data) = &package.v3_authority().unwrap().kind else {
        panic!("converted package did not carry package authority");
    };
    assert_eq!(data.config, config);
    assert!(data.files.iter().all(|file| file.path != path));
}

#[test]
fn conversion_boundary_records_foreign_scriptlet_command_risk() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut metadata = make_test_metadata();
    metadata.package_path = PathBuf::from("/tmp/test-1.0.0.pkg.tar.zst");
    set_test_identity(
        &mut metadata,
        "test-package",
        "1.0.0",
        crate::repository::versioning::VersionScheme::Arch,
        Some("x86_64"),
        None,
    );
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
    let package_path = &result.package_path;

    let file = std::fs::File::open(package_path).unwrap();
    let archive = crate::ccs::archive_reader::inspect_untrusted_ccs_archive(file).unwrap();
    let bundle = archive.manifest.native_lifecycle.as_ref().unwrap();

    assert_eq!(bundle.source_package, metadata.name());
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

fn convert_scriptlet_body(converter: &TestConverter, content: &str) -> FinalizedTestConversion {
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
#[path = "tests/metrics.rs"]
mod metrics;
