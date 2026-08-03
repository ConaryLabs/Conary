// conary-core/src/repository/static_repo/publish_gate/tests.rs

use super::*;
use crate::ccs::attestation::{
    BUILD_ATTESTATION_SCHEMA_V1, BuildAttestationPayload, FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
    ForeignConversionBoundary, canonical_json_hash, compute_build_output_identity_from_v3,
    sign_build_attestation,
};
use crate::ccs::builder::{BuildResult, write_signed_current_ccs_package};
use crate::ccs::signing::SigningKeyPair;
use crate::repository::static_repo::publish_context::STATIC_PUBLISH_POLICY_DIGEST_V1;
use crate::repository::static_repo::{PackageKeyEntry, PackageKeyStatus, PackageKeysFile};
use crate::security::command_risk::{
    COMMAND_RISK_CLASSIFIER_VERSION, CommandRiskEntry, CommandRiskSeverity, ROOT_DELETION,
};
use tempfile::TempDir;

fn package_key(id: &str, public_key: &str, status: PackageKeyStatus) -> PackageKeyEntry {
    PackageKeyEntry {
        algorithm: "ed25519".to_string(),
        public_key: public_key.to_string(),
        key_id: Some(id.to_string()),
        status,
        comment: None,
    }
}

#[test]
fn accepted_signers_include_only_active_package_keys() {
    let keys = PackageKeysFile {
        schema: 1,
        keys: vec![
            package_key("active", "pub-active", PackageKeyStatus::Active),
            package_key("retired", "pub-retired", PackageKeyStatus::Retired),
        ],
    };
    let set = AcceptedStaticSignerSet::from_verified_package_keys(&keys).unwrap();

    assert!(set.accepts_key_id("active"));
    assert!(!set.accepts_key_id("retired"));
}

#[test]
fn retired_signer_cannot_authorize_new_publish() {
    let keys = PackageKeysFile {
        schema: 1,
        keys: vec![package_key(
            "retired",
            "pub-retired",
            PackageKeyStatus::Retired,
        )],
    };
    let err = AcceptedStaticSignerSet::from_verified_package_keys(&keys).unwrap_err();

    assert!(err.to_string().contains("no active package keys"));
}

#[test]
fn duplicate_active_signers_fail_closed() {
    let keys = PackageKeysFile {
        schema: 1,
        keys: vec![
            package_key("dup", "pub-one", PackageKeyStatus::Active),
            package_key("dup", "pub-two", PackageKeyStatus::Active),
        ],
    };
    let err = AcceptedStaticSignerSet::from_verified_package_keys(&keys).unwrap_err();

    assert!(err.to_string().contains("duplicate active package key id"));
}

#[test]
fn accepted_signer_set_canonical_hash_is_stable() {
    let set = AcceptedStaticSignerSet::from_trusted_artifact_signers(&[
        TrustedArtifactSigner {
            key_id: "b".to_string(),
            public_key: "pub-b".to_string(),
        },
        TrustedArtifactSigner {
            key_id: "a".to_string(),
            public_key: "pub-a".to_string(),
        },
    ])
    .unwrap();

    let first = set.canonical_hash().unwrap();
    let second = set.canonical_hash().unwrap();
    assert_eq!(first, second);
    assert!(first.starts_with("sha256:"));
}

#[test]
fn artifact_gate_accepts_attested_hermetic_package() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let (_temp, package_path) = attested_artifact_for_tests(&signer, &signer, |_| {}, |_| {});
    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(report.is_passed(), "{report:?}");
}

#[test]
fn artifact_gate_accepts_attested_v3_package() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("attested-v3.ccs");
    let authority = crate::ccs::v3::test_support::package_authority_with_one_file("attested-v3");
    let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
    let envelope = crate::ccs::attestation::test_support::sample_v3_envelope_for_tests(
        &authority,
        &signer,
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    );
    crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        Some(&envelope),
        None,
    )
    .unwrap();

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(report.is_passed(), "{report:?}");
}

#[test]
fn artifact_gate_does_not_require_command_risk_diagnostics() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("attested-v3-no-command-diagnostics.ccs");
    let authority =
        crate::ccs::v3::test_support::package_authority_with_one_file("no-command-diagnostics");
    let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
    let mut envelope = crate::ccs::attestation::test_support::sample_v3_envelope_for_tests(
        &authority,
        &signer,
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    );
    envelope.payload.build_command_risk_report_hash.clear();
    envelope.payload.command_risk_classifier_version.clear();
    let envelope = sign_build_attestation(envelope.payload, &signer).unwrap();
    crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        Some(&envelope),
        None,
    )
    .unwrap();

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(
        report.is_passed(),
        "diagnostic absence must not decide publication: {:?}",
        report.failures
    );
}

#[test]
fn artifact_gate_candidate_returns_verified_v3_package_for_native_intake() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("candidate-v3.ccs");
    let authority = crate::ccs::v3::test_support::package_authority_with_one_file("candidate-v3");
    let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
    let envelope = crate::ccs::attestation::test_support::sample_v3_envelope_for_tests(
        &authority,
        &signer,
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    );
    crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        Some(&envelope),
        None,
    )
    .unwrap();

    let candidate = verify_static_artifact_publish_candidate(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(candidate.lint.is_passed(), "{:?}", candidate.lint);
    let verified_authority = candidate.package.v3_authority().unwrap();
    assert_eq!(verified_authority.identity.name, "candidate-v3");
    assert_eq!(verified_authority.identity.release, "1");
    assert_eq!(
        verified_authority.identity.architecture.as_deref(),
        Some("x86_64")
    );
}

#[test]
fn artifact_gate_rejects_local_dev_v3_package() {
    let signer = SigningKeyPair::generate().with_key_id("local-dev");
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("local-dev-v3.ccs");
    let mut authority = crate::ccs::v3::test_support::package_authority_with_one_file("local-dev");
    authority.provenance.hardening_level = Some("host".to_string());
    let payloads = crate::ccs::v3::test_support::one_file_payloads_for_tests();
    crate::ccs::builder::write_v3_ccs_package_from_bounded_memory_for_tests(
        &authority,
        &payloads,
        &package_path,
        &signer,
        None,
        None,
        None,
    )
    .unwrap();

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &AcceptedStaticSignerSet::from_initial_key("local-dev", signer.public_key_base64()),
        "m2-static-publish-policy-v1",
    )
    .unwrap();

    assert!(!report.is_passed());
    assert!(
        report
            .failures
            .iter()
            .any(|failure| { matches!(failure.code, PublishGateFailureCode::MissingAttestation) })
    );
}

#[test]
fn m4a_preserves_active_publish_gate_failure_codes() {
    let active = active_failure_codes_for_tests();
    for expected in [
        PublishGateFailureCode::MissingAttestation,
        PublishGateFailureCode::BuildAttestationSignatureMismatch,
        PublishGateFailureCode::PackageSignatureMismatch,
        PublishGateFailureCode::TomlIntegrityMismatch,
        PublishGateFailureCode::OutputIdentityMismatch,
        PublishGateFailureCode::UnacceptedSignerKey,
        PublishGateFailureCode::NonHermeticHardeningLevel,
        PublishGateFailureCode::StaleOrUnknownPolicy,
        PublishGateFailureCode::ForeignConversionMissingBoundary,
        PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
        PublishGateFailureCode::RecordedDraftArtifact,
    ] {
        assert!(
            active.contains(&expected),
            "missing active publish gate code {expected:?}"
        );
    }
}

#[test]
fn m4a_preserves_reserved_publish_gate_mappings() {
    let reserved = [
        PublishGateFailureCode::RetiredSignerKey,
        PublishGateFailureCode::AbsentOrUnknownProvenanceClass,
    ];
    assert_eq!(reserved.len(), 2);
}

#[test]
fn artifact_gate_reports_release_policy_failures() {
    type ArtifactGateCase = (
        &'static str,
        Box<dyn FnOnce() -> (TempDir, std::path::PathBuf, String)>,
    );

    let cases: Vec<ArtifactGateCase> = vec![
        (
            "artifact is missing a build attestation",
            Box::new(|| {
                let signer = SigningKeyPair::generate().with_key_id("publish");
                let (temp, package_path) = artifact_without_attestation_for_tests(&signer);
                let text =
                    failure_text_for_artifact(&package_path, &accepted_signers_for_key(&signer));
                (temp, package_path, text)
            }),
        ),
        (
            "build attestation signer is not accepted",
            Box::new(|| {
                let package_signer = SigningKeyPair::generate().with_key_id("publish");
                let attestation_signer = SigningKeyPair::generate().with_key_id("other");
                let (temp, package_path) = attested_artifact_for_tests(
                    &attestation_signer,
                    &package_signer,
                    |_| {},
                    |_| {},
                );
                let text = failure_text_for_artifact(
                    &package_path,
                    &accepted_signers_for_key(&package_signer),
                );
                (temp, package_path, text)
            }),
        ),
        (
            "build attestation policy digest is not accepted",
            Box::new(|| {
                let signer = SigningKeyPair::generate().with_key_id("publish");
                let (temp, package_path) = attested_artifact_for_tests(
                    &signer,
                    &signer,
                    |_| {},
                    |payload| {
                        payload.publish_policy_digest = "m1-preview-policy".to_string();
                    },
                );
                let text =
                    failure_text_for_artifact(&package_path, &accepted_signers_for_key(&signer));
                (temp, package_path, text)
            }),
        ),
        (
            "recorded-draft artifacts are not publishable",
            Box::new(|| {
                let signer = SigningKeyPair::generate().with_key_id("publish");
                let (temp, package_path) = attested_artifact_for_tests(
                    &signer,
                    &signer,
                    |_| {},
                    |payload| {
                        payload.origin_class = "recorded-draft".to_string();
                    },
                );
                let text =
                    failure_text_for_artifact(&package_path, &accepted_signers_for_key(&signer));
                (temp, package_path, text)
            }),
        ),
    ];

    for (expected, build_case) in cases {
        let (_temp, _package_path, text) = build_case();
        assert!(
            text.contains(expected),
            "expected {expected:?} in gate failure text:\n{text}"
        );
    }
}

#[test]
fn foreign_converted_publish_requires_manifest_boundary() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let (_temp, package_path) = foreign_attested_artifact_for_tests(&signer, false, |_| {}, |_| {});

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(report.failures.iter().any(|failure| {
        failure.code == PublishGateFailureCode::ForeignConversionMissingBoundary
    }));
}

#[test]
fn foreign_converted_publish_rejects_boundary_hash_mismatch() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let (_temp, package_path) = foreign_attested_artifact_for_tests(
        &signer,
        true,
        |boundary| {
            boundary.source_checksum = "sha256:mutated-after-signing".to_string();
        },
        |_| {},
    );

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(report.failures.iter().any(|failure| {
        failure.code == PublishGateFailureCode::ForeignConversionBoundaryHashMismatch
    }));
}

#[test]
fn foreign_converted_publish_rejects_boundary_output_identity_mismatch() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let (_temp, package_path) = foreign_attested_artifact_for_tests(
        &signer,
        true,
        |boundary| {
            boundary.output_identity.package_name = "other".to_string();
        },
        |payload| {
            payload.conversion_boundary_hash = None;
        },
    );

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(
        report
            .failures
            .iter()
            .any(|failure| { failure.message.contains("boundary output identity") })
    );
}

#[test]
fn foreign_converted_publish_treats_classifier_severity_as_diagnostic_only() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let (_temp, package_path) = foreign_attested_artifact_with_signed_boundary_for_tests(
        &signer,
        |boundary| {
            let report = crate::security::command_risk::CommandRiskReport {
                highest_severity: CommandRiskSeverity::Critical,
                classifier_version: COMMAND_RISK_CLASSIFIER_VERSION.to_string(),
                entries: vec![CommandRiskEntry {
                    source: "foreign-scriptlet:post-install".to_string(),
                    command: "rm".to_string(),
                    reason_code: ROOT_DELETION.to_string(),
                    severity: CommandRiskSeverity::Critical,
                    evidence: "rm -rf /".to_string(),
                }],
            };
            boundary.scriptlet_risk_report_hash = Some(canonical_json_hash(&report).unwrap());
            boundary.scriptlet_risk_report = Some(report);
        },
        |_| {},
    );

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(
        report.is_passed(),
        "integrity-valid diagnostic severity must not decide publication: {:?}",
        report.failures
    );
}

#[test]
fn foreign_converted_publish_does_not_require_command_risk_reports() {
    let signer = SigningKeyPair::generate().with_key_id("publish");
    let (_temp, package_path) = foreign_attested_artifact_with_signed_boundary_for_tests(
        &signer,
        |boundary| {
            boundary.build_risk_report_hash = None;
            boundary.build_risk_report = None;
            boundary.scriptlet_risk_report_hash = None;
            boundary.scriptlet_risk_report = None;
        },
        |payload| {
            payload.build_command_risk_report_hash.clear();
            payload.command_risk_classifier_version.clear();
        },
    );

    let report = verify_static_artifact_publish_eligibility(
        &package_path,
        &accepted_signers_for_key(&signer),
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();

    assert!(
        report.is_passed(),
        "diagnostic report absence must not decide publication: {:?}",
        report.failures
    );
}

fn accepted_signers_for_key(key: &SigningKeyPair) -> AcceptedStaticSignerSet {
    AcceptedStaticSignerSet::from_initial_key(
        key.key_id().unwrap_or("publish"),
        key.public_key_base64(),
    )
}

fn active_failure_codes_for_tests() -> Vec<PublishGateFailureCode> {
    vec![
        PublishGateFailureCode::MissingAttestation,
        PublishGateFailureCode::BuildAttestationSignatureMismatch,
        PublishGateFailureCode::PackageSignatureMismatch,
        PublishGateFailureCode::TomlIntegrityMismatch,
        PublishGateFailureCode::OutputIdentityMismatch,
        PublishGateFailureCode::UnacceptedSignerKey,
        PublishGateFailureCode::NonHermeticHardeningLevel,
        PublishGateFailureCode::StaleOrUnknownPolicy,
        PublishGateFailureCode::ForeignConversionMissingBoundary,
        PublishGateFailureCode::ForeignConversionBoundaryHashMismatch,
        PublishGateFailureCode::RecordedDraftArtifact,
    ]
}

fn failure_text_for_artifact(
    package_path: &std::path::Path,
    accepted_signers: &AcceptedStaticSignerSet,
) -> String {
    let report = verify_static_artifact_publish_eligibility(
        package_path,
        accepted_signers,
        STATIC_PUBLISH_POLICY_DIGEST_V1,
    )
    .unwrap();
    assert!(!report.is_passed(), "{report:?}");
    format_publish_gate_failures(&report)
}

fn output_identity_for_build_result(result: &BuildResult) -> BuildOutputIdentity {
    let projected = crate::ccs::v3::project_build_result_to_v3(crate::ccs::v3::V3AuthoringInput {
        build: result,
        local_dev: false,
        debug_toml: None,
    })
    .unwrap();
    compute_build_output_identity_from_v3(&projected.authority).unwrap()
}

fn current_build_result_for_tests(name: &str, version: &str) -> BuildResult {
    let mut result =
        crate::ccs::builder::test_support::minimal_file_build_result(name, version, b"fixture");
    let provenance = result
        .manifest
        .provenance
        .get_or_insert_with(Default::default);
    provenance.origin_class = Some("native-built".to_string());
    provenance.hardening_level = Some("hermetic".to_string());
    provenance.hermetic_evidence =
        Some(crate::ccs::attestation::test_support::sample_hermetic_evidence_for_tests());
    result
}

fn artifact_without_attestation_for_tests(
    signer: &SigningKeyPair,
) -> (TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("missing-attestation.ccs");
    let result = current_build_result_for_tests("widget", "1.0.0");
    write_signed_current_ccs_package(&result, &package_path, signer, false).unwrap();
    (temp, package_path)
}

fn attested_artifact_for_tests(
    attestation_key: &SigningKeyPair,
    package_key: &SigningKeyPair,
    mutate_result: impl FnOnce(&mut BuildResult),
    mutate_payload: impl FnOnce(&mut BuildAttestationPayload),
) -> (TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("artifact.ccs");
    let mut result = current_build_result_for_tests("widget", "1.0.0");
    mutate_result(&mut result);
    let output_identity = output_identity_for_build_result(&result);
    let evidence = result
        .manifest
        .provenance
        .as_ref()
        .unwrap()
        .hermetic_evidence
        .as_ref()
        .unwrap()
        .clone();
    let mut payload = BuildAttestationPayload {
        schema_version: BUILD_ATTESTATION_SCHEMA_V1,
        origin_class: output_identity.origin_class.clone(),
        hardening_level: output_identity.hardening_level.clone(),
        build_input: evidence.build_input.clone(),
        dependency_lock: evidence.dependency_lock.clone(),
        hermetic_evidence_hash: canonical_json_hash(&evidence).unwrap(),
        output_identity,
        build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
        scriptlet_risk_report_hash: None,
        conversion_boundary_hash: None,
        publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
        command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
        sandbox_profile: "kitchen-pristine-network-none".to_string(),
        seccomp_profile: None,
        builder_identity: "conary-test-builder".to_string(),
        conary_version: "test".to_string(),
        issued_at: "2026-06-14T00:00:00Z".to_string(),
    };
    mutate_payload(&mut payload);
    result
        .manifest
        .provenance
        .as_mut()
        .unwrap()
        .build_attestation = Some(sign_build_attestation(payload, attestation_key).unwrap());
    write_signed_current_ccs_package(&result, &package_path, package_key, false).unwrap();
    (temp, package_path)
}

fn foreign_attested_artifact_for_tests(
    signer: &SigningKeyPair,
    include_manifest_boundary: bool,
    mutate_boundary_after_hash: impl FnOnce(&mut ForeignConversionBoundary),
    mutate_payload: impl FnOnce(&mut BuildAttestationPayload),
) -> (TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("foreign.ccs");
    let mut result = current_build_result_for_tests("foreign", "1.0.0");
    result.manifest.provenance.as_mut().unwrap().origin_class =
        Some("foreign-converted".to_string());
    let output_identity = output_identity_for_build_result(&result);
    let evidence = result
        .manifest
        .provenance
        .as_ref()
        .unwrap()
        .hermetic_evidence
        .as_ref()
        .unwrap()
        .clone();
    let build_risk_report = crate::security::command_risk::CommandRiskReport::no_findings();
    let mut boundary = ForeignConversionBoundary {
        schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
        source_format: "rpm".to_string(),
        source_checksum: "sha256:source".to_string(),
        output_identity: output_identity.clone(),
        build_risk_report_hash: Some(canonical_json_hash(&build_risk_report).unwrap()),
        build_risk_report: Some(build_risk_report),
        scriptlet_risk_report_hash: None,
        scriptlet_risk_report: None,
        diagnostics: Vec::new(),
    };
    let signed_boundary_hash = canonical_json_hash(&boundary).unwrap();
    mutate_boundary_after_hash(&mut boundary);
    if include_manifest_boundary {
        result
            .manifest
            .provenance
            .as_mut()
            .unwrap()
            .foreign_conversion_boundary = Some(boundary);
    }
    let mut payload = BuildAttestationPayload {
        schema_version: BUILD_ATTESTATION_SCHEMA_V1,
        origin_class: output_identity.origin_class.clone(),
        hardening_level: output_identity.hardening_level.clone(),
        build_input: evidence.build_input.clone(),
        dependency_lock: evidence.dependency_lock.clone(),
        hermetic_evidence_hash: canonical_json_hash(&evidence).unwrap(),
        output_identity,
        build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
        scriptlet_risk_report_hash: None,
        conversion_boundary_hash: Some(signed_boundary_hash),
        publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
        command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
        sandbox_profile: "foreign-conversion-no-exec".to_string(),
        seccomp_profile: None,
        builder_identity: "conary-foreign-converter".to_string(),
        conary_version: "test".to_string(),
        issued_at: "2026-06-14T00:00:00Z".to_string(),
    };
    mutate_payload(&mut payload);
    result
        .manifest
        .provenance
        .as_mut()
        .unwrap()
        .build_attestation = Some(sign_build_attestation(payload, signer).unwrap());
    write_signed_current_ccs_package(&result, &package_path, signer, false).unwrap();
    (temp, package_path)
}

fn foreign_attested_artifact_with_signed_boundary_for_tests(
    signer: &SigningKeyPair,
    mutate_boundary_before_hash: impl FnOnce(&mut ForeignConversionBoundary),
    mutate_payload: impl FnOnce(&mut BuildAttestationPayload),
) -> (TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let package_path = temp.path().join("foreign.ccs");
    let mut result = current_build_result_for_tests("foreign", "1.0.0");
    result.manifest.provenance.as_mut().unwrap().origin_class =
        Some("foreign-converted".to_string());
    let output_identity = output_identity_for_build_result(&result);
    let evidence = result
        .manifest
        .provenance
        .as_ref()
        .unwrap()
        .hermetic_evidence
        .as_ref()
        .unwrap()
        .clone();
    let build_risk_report = crate::security::command_risk::CommandRiskReport::no_findings();
    let mut boundary = ForeignConversionBoundary {
        schema_version: FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
        source_format: "rpm".to_string(),
        source_checksum: "sha256:source".to_string(),
        output_identity: output_identity.clone(),
        build_risk_report_hash: Some(canonical_json_hash(&build_risk_report).unwrap()),
        build_risk_report: Some(build_risk_report),
        scriptlet_risk_report_hash: None,
        scriptlet_risk_report: None,
        diagnostics: Vec::new(),
    };
    mutate_boundary_before_hash(&mut boundary);
    let signed_boundary_hash = canonical_json_hash(&boundary).unwrap();
    result
        .manifest
        .provenance
        .as_mut()
        .unwrap()
        .foreign_conversion_boundary = Some(boundary);
    let mut payload = BuildAttestationPayload {
        schema_version: BUILD_ATTESTATION_SCHEMA_V1,
        origin_class: output_identity.origin_class.clone(),
        hardening_level: output_identity.hardening_level.clone(),
        build_input: evidence.build_input.clone(),
        dependency_lock: evidence.dependency_lock.clone(),
        hermetic_evidence_hash: canonical_json_hash(&evidence).unwrap(),
        output_identity,
        build_command_risk_report_hash: canonical_json_hash(&evidence.command_risk).unwrap(),
        scriptlet_risk_report_hash: None,
        conversion_boundary_hash: Some(signed_boundary_hash),
        publish_policy_digest: STATIC_PUBLISH_POLICY_DIGEST_V1.to_string(),
        command_risk_classifier_version: evidence.command_risk.classifier_version.clone(),
        sandbox_profile: "foreign-conversion-no-exec".to_string(),
        seccomp_profile: None,
        builder_identity: "conary-foreign-converter".to_string(),
        conary_version: "test".to_string(),
        issued_at: "2026-06-14T00:00:00Z".to_string(),
    };
    mutate_payload(&mut payload);
    result
        .manifest
        .provenance
        .as_mut()
        .unwrap()
        .build_attestation = Some(sign_build_attestation(payload, signer).unwrap());
    write_signed_current_ccs_package(&result, &package_path, signer, false).unwrap();
    (temp, package_path)
}
