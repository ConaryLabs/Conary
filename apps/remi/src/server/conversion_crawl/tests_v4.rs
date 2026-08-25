// apps/remi/src/server/conversion_crawl/tests_v4.rs

use super::*;
use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
use conary_core::corpus::FailureKind;

fn package_with_key(profile: &str, name: &str, marker: &str) -> CatalogPackageRecordV1 {
    let mut package = package(profile, name, "1.0", "1", Some("x86_64"), 42, marker);
    package.package_key_sha256 = conary_core::hash::sha256(format!("key-{marker}").as_bytes());
    package.checksum = format!("sha256:{}", "a".repeat(64));
    package
}

fn proof(package: &CatalogPackageRecordV1) -> ConversionProofV1 {
    let key = ConversionProofKeyV1::current(package, "a".repeat(64), "3".repeat(64))
        .expect("current proof key");
    let proof = ConversionProofV1 {
        schema_version: CONVERSION_PROOF_SCHEMA_V1,
        proof_key_sha256: key.sha256().expect("proof key digest"),
        key,
        validated_profile_revision_sha256: conary_core::hash::sha256(
            package.source_profile.as_bytes(),
        ),
        ccs_sha256: "c".repeat(64),
        ccs_reopen_proof: CcsArtifactReopenProofV1 {
            schema_version: CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1,
            ccs_format_version: conary_core::ccs::v3::FORMAT_VERSION_V3,
            foreign_conversion_boundary_schema_version:
                conary_core::ccs::attestation::FOREIGN_CONVERSION_BOUNDARY_SCHEMA_V1,
            signer_public_key_sha256: "3".repeat(64),
            transport_sha256: "4".repeat(64),
            verified_files: 1,
            verified_objects: 1,
        },
        target_compatibility_proofs: conary_core::ccs::supported_target_contracts()
            .iter()
            .map(|contract| CcsTargetCompatibilityProofV1 {
                schema_version: CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                ccs_sha256: "c".repeat(64),
                compatibility: conary_core::ccs::StaticTargetCompatibilityProofV1 {
                    schema_version: conary_core::ccs::STATIC_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
                    target_profile: contract.target_profile,
                    target_contract_sha256: contract.sha256().expect("target contract digest"),
                    required_capabilities: Vec::new(),
                    required_systemd_operations: Vec::new(),
                    required_linux_process_capabilities: Vec::new(),
                },
            })
            .collect(),
    };
    proof.validate_current().expect("current proof");
    proof
}

fn success_outcome(profile: &str, name: &str, marker: &str) -> ConversionCrawlPackageOutcomeV4 {
    let package = package_with_key(profile, name, marker);
    package_outcome(
        package.clone(),
        Ok((ConversionProofDispositionV1::Validated, proof(&package))),
    )
}

fn valid_report() -> RemiConversionCrawlV4 {
    RemiConversionCrawlV4 {
        schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V4,
        profiles: conary_core::repository::supported_profiles::public_profiles()
            .iter()
            .map(|profile| ConversionCrawlProfileV4 {
                profile: profile.id().to_string(),
                profile_revision_sha256: conary_core::hash::sha256(profile.id().as_bytes()),
                expected_packages: 1,
                outcomes: vec![success_outcome(profile.id(), "demo", profile.id())],
            })
            .collect(),
    }
}

#[test]
fn crawl_contract_requires_every_ordered_public_profile() {
    let report = valid_report();
    report.validate_complete().expect("valid complete crawl");
    assert_eq!(
        report
            .profiles
            .iter()
            .map(|profile| profile.profile.as_str())
            .collect::<Vec<_>>(),
        vec!["fedora-44", "ubuntu-26.04", "arch"]
    );

    let mut missing = report.clone();
    missing.profiles.pop();
    assert!(missing.validate_structure().is_err());

    let mut candidate = report;
    candidate.profiles[2].profile = "solus".to_string();
    assert!(candidate.validate_structure().is_err());
}

#[test]
fn crawl_contract_rejects_incomplete_drifted_reordered_and_invented_reuse() {
    let mut missing = valid_report();
    missing.profiles[0].expected_packages = 2;
    assert!(missing.validate_structure().is_err());

    let mut superseded = valid_report();
    superseded.schema_version = 3;
    assert!(superseded.validate_structure().is_err());

    let mut missing_proof = valid_report();
    missing_proof.profiles[0].outcomes[0].conversion_proof = None;
    assert!(missing_proof.validate_structure().is_err());

    let mut missing_target = valid_report();
    missing_target.profiles[0].outcomes[0]
        .conversion_proof
        .as_mut()
        .expect("proof")
        .target_compatibility_proofs
        .pop();
    assert!(missing_target.validate_structure().is_err());

    let mut drifted_key = valid_report();
    drifted_key.profiles[0].outcomes[0]
        .conversion_proof
        .as_mut()
        .expect("proof")
        .key
        .converter_schema_version += 1;
    assert!(drifted_key.validate_structure().is_err());

    let mut invented_reuse = valid_report();
    invented_reuse.profiles[0].outcomes[0].proof_disposition =
        Some(ConversionProofDispositionV1::Reused);
    assert!(invented_reuse.validate_structure().is_err());

    let mut repeated = valid_report();
    let duplicate = repeated.profiles[0].outcomes[0].clone();
    repeated.profiles[0].outcomes.push(duplicate);
    repeated.profiles[0].expected_packages = 2;
    assert!(repeated.validate_structure().is_err());

    let mut reordered = valid_report();
    reordered.profiles[0].outcomes = vec![
        success_outcome("fedora-44", "z-package", "z"),
        success_outcome("fedora-44", "a-package", "a"),
    ];
    reordered.profiles[0].expected_packages = 2;
    assert!(reordered.validate_structure().is_err());

    let mut failed = valid_report();
    let outcome = &mut failed.profiles[0].outcomes[0];
    outcome.state = ConversionCrawlOutcomeStateV4::Failed;
    outcome.proof_disposition = None;
    outcome.conversion_proof = None;
    outcome.failure = Some(ConversionCrawlFailureV4 {
        kind: FailureKind::Publication,
        incident_id: None,
    });
    failed
        .validate_structure()
        .expect("failed evidence remains structurally inspectable");
    assert!(failed.validate_complete().is_err());
}

#[test]
fn crawl_write_is_canonical_strict_and_independently_reopened() {
    let directory = tempfile::tempdir().expect("crawl evidence directory");
    let path = directory.path().join("crawl.json");
    let report = valid_report();
    let reopened = write_and_reopen_conversion_crawl(&path, &report)
        .expect("write and reopen canonical crawl evidence");
    assert_eq!(reopened, report);

    let mut value = serde_json::to_value(&report).expect("crawl JSON");
    value
        .as_object_mut()
        .expect("crawl object")
        .insert("unexpected".to_string(), serde_json::json!(true));
    assert!(serde_json::from_value::<RemiConversionCrawlV4>(value).is_err());
}

#[test]
fn package_outcome_preserves_exact_proof_disposition_and_typed_failure() {
    let package = package_with_key("fedora-44", "demo", "demo");
    let validated = package_outcome(
        package.clone(),
        Ok((ConversionProofDispositionV1::Validated, proof(&package))),
    );
    assert_eq!(validated.state, ConversionCrawlOutcomeStateV4::Succeeded);
    assert_eq!(
        validated.proof_disposition,
        Some(ConversionProofDispositionV1::Validated)
    );

    let reused = package_outcome(
        package.clone(),
        Ok((ConversionProofDispositionV1::Reused, {
            let mut proof = proof(&package);
            proof.validated_profile_revision_sha256 = "9".repeat(64);
            proof
        })),
    );
    assert_eq!(
        reused.proof_disposition,
        Some(ConversionProofDispositionV1::Reused)
    );

    let failed = package_outcome(
        package,
        Err(anyhow::anyhow!("synthetic conversion failure")),
    );
    assert_eq!(failed.state, ConversionCrawlOutcomeStateV4::Failed);
    assert!(failed.conversion_proof.is_none());
    assert!(failed.failure.is_some());
}

#[tokio::test]
async fn crawl_attempts_every_exact_variant_once_and_preserves_canonical_order() {
    let first = package_with_key("fedora-44", "alpha", "alpha");
    let second = package_with_key("fedora-44", "beta", "beta");
    let attempts = std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::<
        String,
        usize,
    >::new()));
    let outcomes = crawl_packages(vec![first, second], 2, {
        let attempts = std::sync::Arc::clone(&attempts);
        move |package| {
            let attempts = std::sync::Arc::clone(&attempts);
            async move {
                *attempts
                    .lock()
                    .expect("attempt counter")
                    .entry(package.package_key_sha256.clone())
                    .or_default() += 1;
                if package.name == "alpha" {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Ok((ConversionProofDispositionV1::Validated, proof(&package)))
            }
        }
    })
    .await;

    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| outcome.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
    assert_eq!(
        attempts
            .lock()
            .expect("attempt counts")
            .values()
            .copied()
            .collect::<Vec<_>>(),
        vec![1, 1]
    );
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.state == ConversionCrawlOutcomeStateV4::Succeeded)
    );
}

#[test]
fn crawl_planning_pins_every_public_profile_and_excludes_candidate_tiers() {
    let fixture = ActiveCatalogFixture::new();
    for (index, profile) in conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .enumerate()
    {
        fixture.activate(
            profile.id(),
            i64::try_from(index + 1).expect("fixture epoch"),
            vec![package(
                profile.id(),
                "demo",
                "1.0",
                "1",
                Some("x86_64"),
                42,
                profile.id(),
            )],
        );
    }
    let plans = build_crawl_plans(fixture.authority()).expect("build public crawl plans");
    assert_eq!(
        plans
            .iter()
            .map(|plan| plan.selection.source_profile.as_str())
            .collect::<Vec<_>>(),
        vec!["fedora-44", "ubuntu-26.04", "arch"]
    );
    assert!(plans.iter().all(|plan| plan.packages.len() == 1));
    assert!(
        plans
            .iter()
            .all(|plan| plan.selection.source_profile != "solus")
    );
}
