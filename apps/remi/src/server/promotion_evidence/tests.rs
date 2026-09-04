// apps/remi/src/server/promotion_evidence/tests.rs

use std::collections::BTreeMap;

use conary_core::canonical::{
    CANONICAL_MAP_SCHEMA_VERSION, CanonicalMapEntry, CanonicalMapSnapshot,
};
use conary_core::repository::catalog::{
    CatalogPackageOriginV1, NATIVE_PARITY_PACKAGE_FILE_NAME, NATIVE_RESOLUTION_ROOT_FILE_NAME,
    NativeParityEcosystemV1, NativeParityImplementationV1, NativeParityOracleWriter,
    NativeParityPackageV1, NativeResolutionArchitectureAdmissionV1,
    NativeResolutionInstalledStateV1, NativeResolutionOracleWriter, NativeResolutionOutcomeV1,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionRootV1,
    write_native_parity_oracle_manifest, write_native_resolution_oracle_manifest,
};

use super::*;
use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
use crate::server::conversion_crawl::{
    CCS_ARTIFACT_REOPEN_PROOF_SCHEMA_V1, CCS_TARGET_COMPATIBILITY_PROOF_SCHEMA_V1,
    CONVERSION_PROOF_SCHEMA_V1, CcsArtifactReopenProofV1, CcsTargetCompatibilityProofV1,
    ConversionCrawlOutcomeStateV4, ConversionCrawlPackageOutcomeV4, ConversionCrawlProfileV4,
    ConversionProofDispositionV1, ConversionProofKeyV1, ConversionProofTargetContractV1,
    ConversionProofV1, REMI_CONVERSION_CRAWL_SCHEMA_V4, RemiConversionCrawlV4,
    write_and_reopen_conversion_crawl,
};

struct ProfileProofFixture {
    input: RemiPromotionProfileEvidenceInput,
    _package_oracle: tempfile::TempDir,
    _native_resolution: tempfile::TempDir,
    _candidate_resolution: tempfile::TempDir,
}

fn ecosystem(profile: &str) -> NativeParityEcosystemV1 {
    match profile {
        "fedora-44" => NativeParityEcosystemV1::Rpm,
        "ubuntu-26.04" => NativeParityEcosystemV1::Debian,
        "arch" => NativeParityEcosystemV1::Alpm,
        _ => panic!("unexpected public profile {profile}"),
    }
}

pub(crate) fn architecture(profile: &str) -> &'static str {
    if profile == "ubuntu-26.04" {
        "amd64"
    } else {
        "x86_64"
    }
}

fn oracle_package(package: &CatalogPackageRecordV1) -> NativeParityPackageV1 {
    let CatalogPackageOriginV1::Profile {
        member_ordinal,
        source_identity,
        repository_identity,
        source_snapshot_sha256,
    } = &package.origin
    else {
        panic!("fixture package lacks profile origin")
    };
    NativeParityPackageV1 {
        package_key_sha256: package.package_key_sha256.clone(),
        member_ordinal: *member_ordinal,
        source_identity: source_identity.clone(),
        repository_identity: repository_identity.clone(),
        source_snapshot_sha256: source_snapshot_sha256.clone(),
        source_profile: package.source_profile.clone(),
        name: package.name.clone(),
        version: package.version.clone(),
        package_release: package.package_release.clone(),
        architecture: package.architecture.clone(),
        debian_multi_arch: package.debian_multi_arch,
        checksum: package.checksum.clone(),
        size: package.size,
        download_url: package.download_url.clone(),
        version_scheme: package.version_scheme,
        provides: package.provides.clone(),
        requirement_groups: package.requirement_groups.clone(),
    }
}

pub(crate) fn write_package_oracle(
    revision: &ProfileRevisionV2,
    packages: &[CatalogPackageRecordV1],
) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("package oracle directory");
    let family = ecosystem(&revision.profile);
    let mut writer = NativeParityOracleWriter::create(
        directory.path().join(NATIVE_PARITY_PACKAGE_FILE_NAME),
        revision,
        NativeParityImplementationV1 {
            ecosystem: family,
            name: format!("fixture-{family:?}").to_ascii_lowercase(),
            version: "1.0".to_string(),
            projection_schema: 1,
        },
    )
    .expect("create package oracle");
    let mut rows = packages.iter().map(oracle_package).collect::<Vec<_>>();
    rows.sort_by(|left, right| left.package_key_sha256.cmp(&right.package_key_sha256));
    for row in &rows {
        writer.package(row).expect("write package oracle row");
    }
    let manifest = writer.finish().expect("finish package oracle");
    write_native_parity_oracle_manifest(directory.path(), &manifest)
        .expect("write package oracle manifest");
    directory
}

pub(crate) fn write_resolution(
    revision: &ProfileRevisionV2,
    package_oracle_dir: &Path,
    packages: &[CatalogPackageRecordV1],
    implementation_name: &str,
) -> tempfile::TempDir {
    let package_oracle = verify_native_parity_oracle_bundle(package_oracle_dir, revision)
        .expect("reopen package oracle");
    let directory = tempfile::tempdir().expect("resolution directory");
    let mut writer = NativeResolutionOracleWriter::create(
        directory.path().join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
        revision,
        package_oracle.manifest(),
        NativeParityImplementationV1 {
            ecosystem: ecosystem(&revision.profile),
            name: implementation_name.to_string(),
            version: "1.0".to_string(),
            projection_schema: 1,
        },
        NativeResolutionPolicyV1 {
            architecture: architecture(&revision.profile).to_string(),
            architecture_admission: NativeResolutionArchitectureAdmissionV1::NativeOnly,
            installed_state: NativeResolutionInstalledStateV1::Empty,
            roots: NativeResolutionRootPolicyV1::EveryExactPackage,
            positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
            provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
        },
    )
    .expect("create resolution oracle");
    let mut keys = packages
        .iter()
        .map(|package| package.package_key_sha256.clone())
        .collect::<Vec<_>>();
    keys.sort();
    for key in keys {
        writer
            .root(&NativeResolutionRootV1 {
                root_package_key_sha256: key.clone(),
                outcome: NativeResolutionOutcomeV1::Resolved {
                    closure_package_keys_sha256: vec![key],
                },
            })
            .expect("write resolution root");
    }
    let manifest = writer.finish().expect("finish resolution oracle");
    write_native_resolution_oracle_manifest(directory.path(), &manifest)
        .expect("write resolution manifest");
    directory
}

fn proof(package: &CatalogPackageRecordV1, revision_sha256: &str) -> ConversionProofV1 {
    let source_sha256 = package
        .checksum
        .strip_prefix("sha256:")
        .expect("fixture source digest")
        .to_string();
    let key = ConversionProofKeyV1 {
        schema_version: crate::server::CONVERSION_PROOF_KEY_SCHEMA_V1,
        source_profile: package.source_profile.clone(),
        package_key_sha256: package.package_key_sha256.clone(),
        package_name: package.name.clone(),
        package_version: package.version.clone(),
        package_release: package.package_release.clone(),
        package_architecture: package.architecture.clone(),
        source_artifact_sha256: source_sha256,
        converter_schema_version: conary_core::db::models::CONVERSION_VERSION,
        converter_version: env!("CARGO_PKG_VERSION").to_string(),
        ccs_format_version: conary_core::ccs::v3::FORMAT_VERSION_V3,
        targets_signer_public_key_sha256: "3".repeat(64),
        target_contracts: conary_core::ccs::supported_target_contracts()
            .iter()
            .map(|contract| ConversionProofTargetContractV1 {
                target_profile: contract.target_profile,
                target_contract_sha256: contract.sha256().expect("target digest"),
            })
            .collect(),
    };
    key.validate_current().expect("promotion proof key");
    let proof = ConversionProofV1 {
        schema_version: CONVERSION_PROOF_SCHEMA_V1,
        proof_key_sha256: key.sha256().expect("proof key digest"),
        key,
        validated_profile_revision_sha256: revision_sha256.to_string(),
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
                    target_contract_sha256: contract.sha256().expect("target digest"),
                    required_capabilities: Vec::new(),
                    required_systemd_operations: Vec::new(),
                    required_linux_process_capabilities: Vec::new(),
                },
            })
            .collect(),
    };
    proof.validate_current().expect("valid promotion proof");
    proof
}

pub(crate) fn outcome(
    package: &CatalogPackageRecordV1,
    revision_sha256: &str,
) -> ConversionCrawlPackageOutcomeV4 {
    ConversionCrawlPackageOutcomeV4 {
        package_key_sha256: package.package_key_sha256.clone(),
        name: package.name.clone(),
        version: package.version.clone(),
        package_release: package.package_release.clone(),
        architecture: package.architecture.clone(),
        repository_checksum: package.checksum.clone(),
        state: ConversionCrawlOutcomeStateV4::Succeeded,
        proof_disposition: Some(ConversionProofDispositionV1::Validated),
        conversion_proof: Some(proof(package, revision_sha256)),
        failure: None,
    }
}

#[test]
fn complete_promotion_evidence_reopens_and_binds_every_public_candidate() {
    let catalogs = ActiveCatalogFixture::new();
    let mut proof_fixtures = Vec::new();
    let mut crawl_profiles = Vec::new();
    for (index, profile) in conary_core::repository::supported_profiles::public_profiles()
        .iter()
        .enumerate()
    {
        let mut row = package(
            profile.id(),
            "demo",
            "1.0",
            "1",
            Some(architecture(profile.id())),
            42,
            profile.id(),
        );
        row.checksum = format!(
            "sha256:{}",
            conary_core::hash::sha256(profile.id().as_bytes())
        );
        catalogs.activate(profile.id(), i64::try_from(index + 1).unwrap(), vec![row]);
        let pinned = catalogs
            .authority()
            .open_active_profile(profile.id())
            .expect("open active fixture profile");
        let revision = pinned.manifest().clone();
        let mut packages = pinned.reader().packages().expect("candidate packages");
        packages.sort_by(package_identity_order);
        let revision_sha256 = revision.manifest_sha256().expect("revision digest");
        let package_oracle = write_package_oracle(&revision, &packages);
        let native_resolution = write_resolution(
            &revision,
            package_oracle.path(),
            &packages,
            "native-fixture",
        );
        let candidate_resolution =
            write_resolution(&revision, package_oracle.path(), &packages, "conary-sat");
        crawl_profiles.push(ConversionCrawlProfileV4 {
            profile: profile.id().to_string(),
            profile_revision_sha256: revision_sha256,
            expected_packages: u64::try_from(packages.len()).unwrap(),
            outcomes: packages
                .iter()
                .map(|package| outcome(package, &revision.manifest_sha256().unwrap()))
                .collect(),
        });
        proof_fixtures.push(ProfileProofFixture {
            input: RemiPromotionProfileEvidenceInput {
                revision,
                physical_attestation: pinned.physical_attestation().clone(),
                package_oracle_dir: package_oracle.path().to_path_buf(),
                native_resolution_dir: native_resolution.path().to_path_buf(),
                candidate_resolution_dir: candidate_resolution.path().to_path_buf(),
            },
            _package_oracle: package_oracle,
            _native_resolution: native_resolution,
            _candidate_resolution: candidate_resolution,
        });
    }
    let evidence_dir = tempfile::tempdir().expect("promotion evidence directory");
    let crawl_path = evidence_dir.path().join("crawl.json");
    let crawl = RemiConversionCrawlV4 {
        schema_version: REMI_CONVERSION_CRAWL_SCHEMA_V4,
        profiles: crawl_profiles,
    };
    write_and_reopen_conversion_crawl(&crawl_path, &crawl).expect("write crawl");
    let canonical = CanonicalMapSnapshot {
        schema_version: CANONICAL_MAP_SCHEMA_VERSION,
        revision: 1,
        generated_at: Some("2026-08-25T00:00:00Z".to_string()),
        entries: vec![CanonicalMapEntry {
            canonical: "demo".to_string(),
            kind: "package".to_string(),
            category: None,
            implementations: BTreeMap::from([
                ("fedora-44".to_string(), "demo".to_string()),
                ("ubuntu-26.04".to_string(), "demo".to_string()),
                ("arch".to_string(), "demo".to_string()),
            ]),
        }],
    };
    let output_path = evidence_dir.path().join("promotion.json");
    let config = RemiPromotionEvidenceConfig {
        catalog_dir: catalogs.catalog_dir().to_path_buf(),
        conversion_crawl_path: crawl_path.clone(),
        output_path: output_path.clone(),
        profiles: proof_fixtures
            .iter()
            .map(|fixture| fixture.input.clone())
            .collect(),
    };

    let produced =
        produce_remi_promotion_evidence(&config, &canonical).expect("produce promotion evidence");
    let reopened = reopen_remi_promotion_evidence(&output_path).expect("reopen promotion evidence");
    assert_eq!(reopened, produced);
    assert_eq!(produced.schema_version, REMI_PROMOTION_EVIDENCE_SCHEMA_V2);
    assert_eq!(produced.profiles.len(), 3);
    assert_eq!(
        produced.conversion_crawl_sha256,
        conary_core::hash::sha256(&fs::read(crawl_path).unwrap())
    );
    let mut retired = produced.clone();
    retired.schema_version = 1;
    let error = retired
        .validate()
        .expect_err("retired promotion evidence must require rebuild");
    assert!(
        error
            .to_string()
            .contains("unsupported Remi promotion evidence schema 1")
    );

    let stale_crawl_path = evidence_dir.path().join("stale-crawl.json");
    let mut stale_crawl = crawl.clone();
    let stale_revision_sha256 = "9".repeat(64);
    stale_crawl.profiles[0].profile_revision_sha256 = stale_revision_sha256.clone();
    for outcome in &mut stale_crawl.profiles[0].outcomes {
        outcome
            .conversion_proof
            .as_mut()
            .expect("successful fixture proof")
            .validated_profile_revision_sha256 = stale_revision_sha256.clone();
    }
    write_and_reopen_conversion_crawl(&stale_crawl_path, &stale_crawl)
        .expect("write internally valid stale crawl");
    let stale_config = RemiPromotionEvidenceConfig {
        conversion_crawl_path: stale_crawl_path,
        output_path: evidence_dir.path().join("stale-promotion.json"),
        ..config.clone()
    };
    let error = produce_remi_promotion_evidence(&stale_config, &canonical)
        .expect_err("reject crawl for a different profile revision");
    assert!(
        error
            .to_string()
            .contains("conversion crawl differs from promotion candidate profile")
    );

    let foreign_crawl_path = evidence_dir.path().join("foreign-crawl.json");
    let mut foreign_crawl = crawl.clone();
    let foreign_source_sha256 = "8".repeat(64);
    let foreign_outcome = &mut foreign_crawl.profiles[0].outcomes[0];
    foreign_outcome.repository_checksum = format!("sha256:{foreign_source_sha256}");
    let foreign_proof = foreign_outcome
        .conversion_proof
        .as_mut()
        .expect("successful fixture proof");
    foreign_proof.key.source_artifact_sha256 = foreign_source_sha256;
    foreign_proof.proof_key_sha256 = foreign_proof.key.sha256().expect("foreign proof digest");
    write_and_reopen_conversion_crawl(&foreign_crawl_path, &foreign_crawl)
        .expect("write internally valid foreign crawl");
    let foreign_config = RemiPromotionEvidenceConfig {
        conversion_crawl_path: foreign_crawl_path,
        output_path: evidence_dir.path().join("foreign-promotion.json"),
        ..config.clone()
    };
    let error = produce_remi_promotion_evidence(&foreign_config, &canonical)
        .expect_err("reject crawl for a different package artifact");
    assert!(
        error
            .to_string()
            .contains("conversion crawl package evidence differs from candidate package")
    );

    fs::write(
        &output_path,
        [fs::read(&output_path).unwrap(), b"\n".to_vec()].concat(),
    )
    .expect("tamper promotion output");
    assert!(reopen_remi_promotion_evidence(&output_path).is_err());
}

#[test]
fn promotion_contract_rejects_incomplete_and_candidate_tier_profiles() {
    let comparison = NativeParityComparisonV1 {
        schema_version: NATIVE_PARITY_COMPARISON_SCHEMA_V1,
        profile: "fedora-44".to_string(),
        profile_revision_sha256: "a".repeat(64),
        oracle_manifest_sha256: "b".repeat(64),
        counts: conary_core::repository::catalog::NativeParityCountsV1 {
            packages: 1,
            provides: 0,
            requirement_groups: 0,
            requirement_atoms: 0,
        },
    };
    let profile = RemiPromotionProfileEvidenceV1 {
        ordinal: 0,
        profile: "fedora-44".to_string(),
        profile_revision_sha256: "a".repeat(64),
        catalog_sha256: "c".repeat(64),
        catalog_size: 1,
        package_parity: comparison,
        resolution_parity: NativeResolutionComparisonV1 {
            schema_version: NATIVE_RESOLUTION_COMPARISON_SCHEMA_V3,
            profile: "fedora-44".to_string(),
            profile_revision_sha256: "a".repeat(64),
            package_oracle_manifest_sha256: "b".repeat(64),
            oracle_manifest_sha256: "d".repeat(64),
            candidate_manifest_sha256: "e".repeat(64),
            counts: conary_core::repository::catalog::NativeResolutionCountsV1 {
                roots: 1,
                resolved_roots: 1,
                unresolved_roots: 0,
                not_installable_roots: 0,
                closure_package_references: 1,
                unresolved_dependencies: 0,
            },
        },
    };
    let mut evidence = RemiPromotionEvidenceV1 {
        schema_version: REMI_PROMOTION_EVIDENCE_SCHEMA_V2,
        conversion_crawl_sha256: "f".repeat(64),
        canonical_map: RemiPromotionCanonicalMapV1 {
            sha256: "1".repeat(64),
            revision: 1,
            entry_count: 1,
        },
        profiles: vec![profile.clone()],
    };
    assert!(evidence.validate().is_err());

    evidence.profiles = vec![profile.clone(), profile.clone(), profile];
    evidence.profiles[1].ordinal = 1;
    evidence.profiles[1].profile = "ubuntu-26.04".to_string();
    evidence.profiles[1].package_parity.profile = "ubuntu-26.04".to_string();
    evidence.profiles[1].resolution_parity.profile = "ubuntu-26.04".to_string();
    evidence.profiles[2].ordinal = 2;
    evidence.profiles[2].profile = "solus".to_string();
    evidence.profiles[2].package_parity.profile = "solus".to_string();
    evidence.profiles[2].resolution_parity.profile = "solus".to_string();
    assert!(evidence.validate().is_err());
}
