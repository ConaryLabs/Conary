// crates/conary-core/src/repository/catalog/profile/tests.rs

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCopyScratchV1, CatalogFinalizationScratchV1,
    CatalogMetadataScratchV1, CatalogMetadataStreamAdmission, CatalogMetadataStreamScratchV1,
    CatalogPackageRecordV1, CatalogProfileCandidateScratchV1, CatalogProfileMemberScratchV1,
    CatalogProvideRecordV1, CatalogRequirementAtomV1, CatalogRequirementGroupV1,
    CatalogScratchAdmission, CatalogScratchCapacityError, SOURCE_SNAPSHOT_SCHEMA_V1,
    SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1,
    SourceStreamKindV1, SourceStreamV1, write_catalog_candidate,
};
use crate::repository::dependency_model::{
    DebianMultiArch, ProvideArchitectureQualifier, ProvideVersionRelation,
    RepositoryRequirementClause, RepositoryRequirementExpression,
};
use crate::repository::dependency_source::CapabilityProvenance;
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

mod debian_pockets;

struct ProfileAdmission {
    requirement: Mutex<Option<CatalogProfileCandidateScratchV1>>,
    events: Arc<Mutex<Vec<&'static str>>>,
    refuse_growth: bool,
}

struct EventLease {
    events: Arc<Mutex<Vec<&'static str>>>,
    event: &'static str,
}

impl Drop for EventLease {
    fn drop(&mut self) {
        self.events.lock().unwrap().push(self.event);
    }
}

impl CatalogScratchAdmission for ProfileAdmission {
    fn reserve_source_candidate(
        &self,
        _candidate_path: &Path,
        _requirement: crate::repository::catalog::CatalogSourceCandidateScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("profile writer must not request source growth admission")
    }

    fn reserve_profile_candidate(
        &self,
        _candidate_path: &Path,
        requirement: CatalogProfileCandidateScratchV1,
    ) -> Result<Box<dyn Send>> {
        self.events.lock().unwrap().push("growth-reserve");
        *self.requirement.lock().unwrap() = Some(requirement.clone());
        if self.refuse_growth {
            return Err(CatalogScratchCapacityError {
                required_bytes: requirement.required_additional_bytes,
                available_bytes: requirement.required_additional_bytes - 1,
                reserved_bytes: 0,
            }
            .into());
        }
        Ok(Box::new(EventLease {
            events: Arc::clone(&self.events),
            event: "growth-drop",
        }))
    }

    fn reserve_metadata(
        &self,
        _work_directory: &Path,
        _requirement: CatalogMetadataScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("profile writer must not request metadata admission")
    }

    fn stream_metadata(
        &self,
        _work_directory: &Path,
        _requirement: CatalogMetadataStreamScratchV1,
    ) -> Result<Box<dyn CatalogMetadataStreamAdmission>> {
        panic!("profile writer must not request streamed metadata admission")
    }

    fn reserve_finalization(
        &self,
        _candidate_path: &Path,
        _requirement: CatalogFinalizationScratchV1,
    ) -> Result<Box<dyn Send>> {
        self.events.lock().unwrap().push("finalization-reserve");
        Ok(Box::new(EventLease {
            events: Arc::clone(&self.events),
            event: "finalization-drop",
        }))
    }

    fn reserve_copy(
        &self,
        _destination_root: &Path,
        _requirement: CatalogCopyScratchV1,
    ) -> Result<Box<dyn Send>> {
        panic!("profile writer must not request catalog-copy admission")
    }
}

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn source_content(
    repository_identity: &str,
    package_name: &str,
    evidence_digest: char,
) -> CatalogContentV1 {
    CatalogContentV1::new(
        CatalogScopeV1::Source {
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: repository_identity.to_string(),
        },
        vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: SourceMetadataObjectRoleV1::RpmPrimary,
            source_path: "repodata/primary.xml.zst".to_string(),
            sha256: digest(evidence_digest),
            size: 1024,
        }],
        vec![CatalogPackageRecordV1 {
            package_key_sha256: String::new(),
            origin: CatalogPackageOriginV1::Source {
                source_identity: "fedora-project".to_string(),
                repository_identity: repository_identity.to_string(),
            },
            source_profile: "fedora-44".to_string(),
            name: package_name.to_string(),
            version: "1.0-1".to_string(),
            package_release: "1".to_string(),
            architecture: Some("x86_64".to_string()),
            debian_multi_arch: None,
            description: None,
            checksum: digest('c'),
            size: 2048,
            download_url: format!("https://example.test/{package_name}.rpm"),
            metadata: Some("{}".to_string()),
            is_security_update: false,
            severity: None,
            cve_ids: None,
            advisory_id: None,
            advisory_url: None,
            version_scheme: VersionScheme::Rpm,
            provides: Vec::new(),
            requirement_groups: Vec::new(),
        }],
    )
    .unwrap()
}

fn source_manifest(
    repository_identity: &str,
    evidence_digest: char,
    binding: &CatalogBindingV1,
) -> SourceSnapshotV1 {
    let parser_config = RepositoryParserConfig::Rpm {
        architecture: "x86_64".to_string(),
    };
    let trust_policy = RepositoryTrustPolicy::Rpm {
        metadata: RpmMetadataAuthority::Metalink {
            url: "https://example.test/metalink".to_string(),
        },
        package_keys: vec![
            OpenPgpTrustRoot::new(
                "https://example.test/fedora.gpg".to_string(),
                "A".repeat(40),
            )
            .unwrap(),
        ],
    };
    SourceSnapshotV1 {
        schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
        source_profile: "fedora-44".to_string(),
        source_identity: "fedora-project".to_string(),
        repository_identity: repository_identity.to_string(),
        stream: SourceStreamV1 {
            kind: SourceStreamKindV1::Release,
            identity: "44".to_string(),
        },
        stream_binding_sha256: digest('e'),
        parser_projection_version: crate::repository::catalog::SOURCE_CATALOG_PROJECTION_VERSION_V2,
        provenance: SourceProvenanceV1 {
            ecosystem: SourceEcosystemV1::Rpm,
            metadata_url: "https://example.test/repository".to_string(),
            content_url: Some("https://content.example.test/repository".to_string()),
            parser_config_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&parser_config).unwrap(),
            ),
            parser_config,
            trust_policy_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&trust_policy).unwrap(),
            ),
            trust_policy,
        },
        authenticated_root: CatalogArtifactV1 {
            sha256: digest('f'),
            size: 512,
        },
        authenticated_objects: vec![SourceMetadataObjectV1 {
            role: SourceMetadataObjectRoleV1::RpmPrimary,
            source_path: "repodata/primary.xml.zst".to_string(),
            sha256: digest(evidence_digest),
            size: 1024,
        }],
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    }
}

fn debian_requirement(kind: &str, capability: &str) -> CatalogRequirementGroupV1 {
    let clause = RepositoryRequirementClause::name_only(capability.to_string());
    CatalogRequirementGroupV1 {
        kind: kind.to_string(),
        behavior: "hard".to_string(),
        description: None,
        native_text: Some(capability.to_string()),
        expression_json: serde_json::to_string(&RepositoryRequirementExpression::Atom(clause))
            .unwrap(),
        atoms: vec![CatalogRequirementAtomV1 {
            capability: capability.to_string(),
            version_constraint: None,
            kind: "package".to_string(),
            dependency_type: "runtime".to_string(),
            raw: Some(capability.to_string()),
        }],
    }
}

fn debian_source_content(
    repository_identity: &str,
    distribution: &str,
    evidence_digest: char,
    mutate: impl FnOnce(&mut CatalogPackageRecordV1),
) -> CatalogContentV1 {
    let mut package = CatalogPackageRecordV1 {
        package_key_sha256: String::new(),
        origin: CatalogPackageOriginV1::Source {
            source_identity: "ubuntu".to_string(),
            repository_identity: repository_identity.to_string(),
        },
        source_profile: "ubuntu-26.04".to_string(),
        name: "linux-headers-virtual-7.0".to_string(),
        version: "7.0.0-30.30".to_string(),
        package_release: String::new(),
        architecture: Some("amd64".to_string()),
        debian_multi_arch: Some(DebianMultiArch::No),
        description: Some("Virtual Linux kernel headers".to_string()),
        checksum: "7c1a655f3d6cfb1d0f03d6ad484c32a9a43cdfa8dc175e83314f10c08bc02e2d"
            .to_string(),
        size: 1646,
        download_url: "https://archive.example.test/pool/main/l/linux-meta/linux-headers-virtual-7.0_7.0.0-30.30_amd64.deb".to_string(),
        metadata: Some(
            serde_json::json!({
                "format": "deb",
                "distribution": distribution,
                "component": "main",
                "section": "kernel",
                "installed_size": "8"
            })
            .to_string(),
        ),
        is_security_update: false,
        severity: None,
        cve_ids: None,
        advisory_id: None,
        advisory_url: None,
        version_scheme: VersionScheme::Debian,
        provides: vec![CatalogProvideRecordV1 {
            capability: "linux-headers-virtual-7.0".to_string(),
            version: Some("7.0.0-30.30".to_string()),
            version_relation: Some(ProvideVersionRelation::Equal),
            kind: "package".to_string(),
            raw: Some("linux-headers-virtual-7.0 (= 7.0.0-30.30)".to_string()),
            version_scheme: VersionScheme::Debian,
            architecture_qualifier: ProvideArchitectureQualifier::Implicit,
            provenance: CapabilityProvenance::ExactIdentity,
        }],
        requirement_groups: vec![
            debian_requirement("depends", "linux-headers-7.0.0-30-generic"),
            debian_requirement("conflict", "linux-headers-virtual-legacy"),
            debian_requirement("replace", "linux-headers-virtual-old"),
        ],
    };
    mutate(&mut package);
    CatalogContentV1::new(
        CatalogScopeV1::Source {
            source_profile: "ubuntu-26.04".to_string(),
            source_identity: "ubuntu".to_string(),
            repository_identity: repository_identity.to_string(),
        },
        vec![CatalogSourceEvidenceV1::AuthenticatedObject {
            role: SourceMetadataObjectRoleV1::DebianPackages,
            source_path: format!("dists/{distribution}/main/binary-amd64/Packages.zst"),
            sha256: digest(evidence_digest),
            size: 4096,
        }],
        vec![package],
    )
    .unwrap()
}

fn debian_source_manifest(
    repository_identity: &str,
    distribution: &str,
    evidence_digest: char,
    binding: &CatalogBindingV1,
) -> SourceSnapshotV1 {
    let parser_config = RepositoryParserConfig::Deb {
        distribution: distribution.to_string(),
        component: "main".to_string(),
        architecture: "amd64".to_string(),
    };
    let trust_policy = RepositoryTrustPolicy::Debian {
        release_keys: vec![
            OpenPgpTrustRoot::new(
                "https://example.test/ubuntu.gpg".to_string(),
                "A".repeat(40),
            )
            .unwrap(),
        ],
    };
    SourceSnapshotV1 {
        schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
        source_profile: "ubuntu-26.04".to_string(),
        source_identity: "ubuntu".to_string(),
        repository_identity: repository_identity.to_string(),
        stream: SourceStreamV1 {
            kind: SourceStreamKindV1::Release,
            identity: "26.04".to_string(),
        },
        stream_binding_sha256: digest('9'),
        parser_projection_version: crate::repository::catalog::SOURCE_CATALOG_PROJECTION_VERSION_V2,
        provenance: SourceProvenanceV1 {
            ecosystem: SourceEcosystemV1::Deb,
            metadata_url: "https://metadata.example.test/ubuntu".to_string(),
            content_url: Some("https://archive.example.test/ubuntu".to_string()),
            parser_config_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&parser_config).unwrap(),
            ),
            parser_config,
            trust_policy_sha256: crate::hash::sha256(
                &crate::json::canonical_json(&trust_policy).unwrap(),
            ),
            trust_policy,
        },
        authenticated_root: CatalogArtifactV1 {
            sha256: digest('8'),
            size: 1024,
        },
        authenticated_objects: vec![SourceMetadataObjectV1 {
            role: SourceMetadataObjectRoleV1::DebianPackages,
            source_path: format!("dists/{distribution}/main/binary-amd64/Packages.zst"),
            sha256: digest(evidence_digest),
            size: 4096,
        }],
        catalog: binding.artifact.clone(),
        logical_digest_sha256: binding.logical_digest_sha256.clone(),
        counts: binding.counts,
    }
}

#[test]
fn profile_composition_uses_explicit_member_order_and_binds_exact_content() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("first.sqlite");
    let second_path = directory.path().join("second.sqlite");
    let first_binding = write_catalog_candidate(
        &first_path,
        &source_content("fedora-everything-x86_64", "bash", 'a'),
    )
    .unwrap();
    let second_binding = write_catalog_candidate(
        &second_path,
        &source_content("fedora-updates-x86_64", "glibc", 'b'),
    )
    .unwrap();
    let first_manifest = source_manifest("fedora-everything-x86_64", 'a', &first_binding);
    let second_manifest = source_manifest("fedora-updates-x86_64", 'b', &second_binding);
    let first_reader = CatalogReader::open_verified(&first_path, &first_binding).unwrap();
    let second_reader = CatalogReader::open_verified(&second_path, &second_binding).unwrap();

    let compose = |reverse: bool| {
        let first = ProfileCatalogMemberInputV2 {
            ordinal: 0,
            role: ProfileSourceRole::Base,
            precedence: 10,
            required: true,
            manifest: &first_manifest,
            reader: &first_reader,
        };
        let second = ProfileCatalogMemberInputV2 {
            ordinal: 1,
            role: ProfileSourceRole::Updates,
            precedence: 20,
            required: true,
            manifest: &second_manifest,
            reader: &second_reader,
        };
        ProfileCatalogCandidateV2::compose(
            "fedora-44",
            1,
            if reverse {
                vec![second, first]
            } else {
                vec![first, second]
            },
        )
        .unwrap()
    };

    let forward = compose(false);
    let reversed = compose(true);
    assert_eq!(forward.content(), reversed.content());
    assert_eq!(forward.members(), reversed.members());
    assert_eq!(
        forward.members()[0].repository_identity,
        "fedora-everything-x86_64"
    );
    assert_eq!(
        forward.members()[1].repository_identity,
        "fedora-updates-x86_64"
    );

    let identity_only_members = derive_profile_catalog_members(
        "fedora-44",
        1,
        vec![
            ProfileCatalogMemberInputV2 {
                ordinal: 1,
                role: ProfileSourceRole::Updates,
                precedence: 20,
                required: true,
                manifest: &second_manifest,
                reader: &second_reader,
            },
            ProfileCatalogMemberInputV2 {
                ordinal: 0,
                role: ProfileSourceRole::Base,
                precedence: 10,
                required: true,
                manifest: &first_manifest,
                reader: &first_reader,
            },
        ],
    )
    .unwrap();
    assert_eq!(identity_only_members, forward.members());

    let profile_path = directory.path().join("profile.sqlite");
    let profile_binding = write_catalog_candidate(&profile_path, forward.content()).unwrap();
    let revision = forward.bind(&profile_binding).unwrap();
    assert_eq!(revision.members.len(), 2);
    assert_eq!(
        revision.logical_digest_sha256,
        profile_binding.logical_digest_sha256
    );
    CatalogReader::open_verified(&profile_path, &profile_binding).unwrap();
}

#[test]
fn profile_streaming_composition_deduplicates_identical_package_origins() {
    let directory = tempfile::tempdir().unwrap();
    let base_path = directory.path().join("base.sqlite");
    let updates_path = directory.path().join("updates.sqlite");
    let base = source_content("fedora-base", "shared", 'a');
    let mut updates = source_content("fedora-updates", "shared", 'b');
    updates.packages[0].download_url = "https://updates.example.test/shared.rpm".to_string();
    let base_binding = write_catalog_candidate(&base_path, &base).unwrap();
    let updates_binding = write_catalog_candidate(&updates_path, &updates).unwrap();
    let base_manifest = source_manifest("fedora-base", 'a', &base_binding);
    let updates_manifest = source_manifest("fedora-updates", 'b', &updates_binding);
    let base_reader = CatalogReader::open_verified(&base_path, &base_binding).unwrap();
    let updates_reader = CatalogReader::open_verified(&updates_path, &updates_binding).unwrap();

    let profile_path = directory.path().join("profile.sqlite");
    let profile = write_profile_catalog_candidate(
        &profile_path,
        "fedora-44",
        2,
        vec![
            ProfileCatalogMemberInputV2 {
                ordinal: 0,
                role: ProfileSourceRole::Updates,
                precedence: 100,
                required: true,
                manifest: &updates_manifest,
                reader: &updates_reader,
            },
            ProfileCatalogMemberInputV2 {
                ordinal: 1,
                role: ProfileSourceRole::Base,
                precedence: 90,
                required: true,
                manifest: &base_manifest,
                reader: &base_reader,
            },
        ],
    )
    .unwrap();

    assert_eq!(profile.counts.packages, 1);
    let binding = CatalogBindingV1 {
        scope: CatalogScopeV1::Profile {
            profile: profile.profile.clone(),
        },
        artifact: profile.catalog.clone(),
        logical_digest_sha256: profile.logical_digest_sha256.clone(),
        counts: profile.counts,
    };
    let reader = CatalogReader::open_verified(&profile_path, &binding).unwrap();
    let packages = reader.packages().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(
        packages[0].origin,
        CatalogPackageOriginV1::Profile {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-updates".to_string(),
            source_snapshot_sha256: updates_manifest.manifest_sha256().unwrap(),
        }
    );
    assert_eq!(
        packages[0].download_url,
        "https://updates.example.test/shared.rpm"
    );
}

#[test]
fn profile_growth_refusal_precedes_candidate_creation_by_one_byte() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.sqlite");
    let source_binding = write_catalog_candidate(
        &source_path,
        &source_content("fedora-everything-x86_64", "bash", 'a'),
    )
    .unwrap();
    let source_manifest = source_manifest("fedora-everything-x86_64", 'a', &source_binding);
    let source_reader = CatalogReader::open_verified(&source_path, &source_binding).unwrap();
    let profile_path = directory.path().join("profile.sqlite");
    let events = Arc::new(Mutex::new(Vec::new()));
    let admission = Arc::new(ProfileAdmission {
        requirement: Mutex::new(None),
        events: Arc::clone(&events),
        refuse_growth: true,
    });

    let error = write_profile_catalog_candidate_with_scratch_admission(
        &profile_path,
        "fedora-44",
        2,
        vec![ProfileCatalogMemberInputV2 {
            ordinal: 0,
            role: ProfileSourceRole::Base,
            precedence: 10,
            required: true,
            manifest: &source_manifest,
            reader: &source_reader,
        }],
        admission.clone(),
    )
    .unwrap_err();

    let requirement = admission.requirement.lock().unwrap().clone().unwrap();
    let Error::CatalogScratchCapacity(capacity) = error else {
        panic!("expected typed profile growth refusal");
    };
    assert_eq!(
        capacity.required_bytes,
        requirement.required_additional_bytes
    );
    assert_eq!(capacity.available_bytes, capacity.required_bytes - 1);
    assert_eq!(requirement.members.len(), 1);
    assert_eq!(
        requirement.members[0].catalog_bytes,
        source_binding.artifact.size
    );
    assert_eq!(requirement.input_package_count, 1);
    assert_eq!(
        requirement.candidate_database_bytes,
        source_binding.artifact.size * 2 + 4096
    );
    assert_eq!(
        requirement.required_additional_bytes,
        source_binding.artifact.size * 3 + 4096
    );
    assert_eq!(*events.lock().unwrap(), vec!["growth-reserve"]);
    assert!(!profile_path.exists());
}

#[test]
fn profile_growth_facts_require_exact_reopened_source_before_admission() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.sqlite");
    let source_binding = write_catalog_candidate(
        &source_path,
        &source_content("fedora-everything-x86_64", "bash", 'a'),
    )
    .unwrap();
    let mut source_manifest = source_manifest("fedora-everything-x86_64", 'a', &source_binding);
    source_manifest.catalog.size += 4096;
    let source_reader = CatalogReader::open_verified(&source_path, &source_binding).unwrap();
    let profile_path = directory.path().join("profile.sqlite");
    let events = Arc::new(Mutex::new(Vec::new()));
    let admission = Arc::new(ProfileAdmission {
        requirement: Mutex::new(None),
        events: Arc::clone(&events),
        refuse_growth: false,
    });

    let error = write_profile_catalog_candidate_with_scratch_admission(
        &profile_path,
        "fedora-44",
        2,
        vec![ProfileCatalogMemberInputV2 {
            ordinal: 0,
            role: ProfileSourceRole::Base,
            precedence: 10,
            required: true,
            manifest: &source_manifest,
            reader: &source_reader,
        }],
        admission.clone(),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ConflictError(_)));
    assert!(admission.requirement.lock().unwrap().is_none());
    assert!(events.lock().unwrap().is_empty());
    assert!(!profile_path.exists());
}

#[test]
fn profile_growth_lease_covers_replay_and_releases_before_finalization() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("source.sqlite");
    let source_binding = write_catalog_candidate(
        &source_path,
        &source_content("fedora-everything-x86_64", "bash", 'a'),
    )
    .unwrap();
    let source_manifest = source_manifest("fedora-everything-x86_64", 'a', &source_binding);
    let source_reader = CatalogReader::open_verified(&source_path, &source_binding).unwrap();
    let profile_path = directory.path().join("profile.sqlite");
    let events = Arc::new(Mutex::new(Vec::new()));
    let admission = Arc::new(ProfileAdmission {
        requirement: Mutex::new(None),
        events: Arc::clone(&events),
        refuse_growth: false,
    });

    let revision = write_profile_catalog_candidate_with_scratch_admission(
        &profile_path,
        "fedora-44",
        2,
        vec![ProfileCatalogMemberInputV2 {
            ordinal: 0,
            role: ProfileSourceRole::Base,
            precedence: 10,
            required: true,
            manifest: &source_manifest,
            reader: &source_reader,
        }],
        admission.clone(),
    )
    .unwrap();

    assert_eq!(revision.counts.packages, 1);
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            "growth-reserve",
            "growth-drop",
            "finalization-reserve",
            "finalization-drop"
        ]
    );
    assert!(admission.requirement.lock().unwrap().is_some());
    assert!(profile_path.exists());
}

#[test]
fn profile_writer_rejects_database_growth_above_admitted_ceiling() {
    let directory = tempfile::tempdir().unwrap();
    let profile_path = directory.path().join("profile.sqlite");
    let events = Arc::new(Mutex::new(Vec::new()));
    let admission = Arc::new(ProfileAdmission {
        requirement: Mutex::new(None),
        events: Arc::clone(&events),
        refuse_growth: false,
    });
    let requirement =
        CatalogProfileCandidateScratchV1::from_members(vec![CatalogProfileMemberScratchV1 {
            ordinal: 0,
            catalog_bytes: 4096,
            package_count: 0,
        }])
        .unwrap();
    let writer = CatalogCandidateWriter::create_with_profile_scratch_admission(
        &profile_path,
        CatalogScopeV1::Profile {
            profile: "fedora-44".to_string(),
        },
        admission,
        requirement,
    )
    .unwrap();

    let error = writer
        .finish(vec![CatalogSourceEvidenceV1::SourceSnapshot {
            member_ordinal: 0,
            source_identity: "fedora-project".to_string(),
            repository_identity: "fedora-everything-x86_64".to_string(),
            source_snapshot_sha256: digest('a'),
        }])
        .unwrap_err();

    assert!(error.to_string().contains("above its admitted"));
    assert_eq!(
        *events.lock().unwrap(),
        vec!["growth-reserve", "growth-drop"]
    );
    assert!(!profile_path.exists());
}

#[test]
fn profile_streaming_composition_rejects_contradictory_duplicate_identity() {
    let directory = tempfile::tempdir().unwrap();
    let base_path = directory.path().join("base.sqlite");
    let updates_path = directory.path().join("updates.sqlite");
    let base = source_content("fedora-base", "shared", 'a');
    let mut updates = source_content("fedora-updates", "shared", 'b');
    updates.packages[0].checksum = digest('d');
    let base_binding = write_catalog_candidate(&base_path, &base).unwrap();
    let updates_binding = write_catalog_candidate(&updates_path, &updates).unwrap();
    let base_manifest = source_manifest("fedora-base", 'a', &base_binding);
    let updates_manifest = source_manifest("fedora-updates", 'b', &updates_binding);
    let base_reader = CatalogReader::open_verified(&base_path, &base_binding).unwrap();
    let updates_reader = CatalogReader::open_verified(&updates_path, &updates_binding).unwrap();

    let error = write_profile_catalog_candidate(
        directory.path().join("profile.sqlite"),
        "fedora-44",
        2,
        vec![
            ProfileCatalogMemberInputV2 {
                ordinal: 0,
                role: ProfileSourceRole::Updates,
                precedence: 100,
                required: true,
                manifest: &updates_manifest,
                reader: &updates_reader,
            },
            ProfileCatalogMemberInputV2 {
                ordinal: 1,
                role: ProfileSourceRole::Base,
                precedence: 90,
                required: true,
                manifest: &base_manifest,
                reader: &base_reader,
            },
        ],
    )
    .unwrap_err();
    assert!(matches!(error, Error::ConflictError(_)));
    assert!(error.to_string().contains("disagrees between repositories"));
}

#[test]
fn bounded_source_and_profile_catalog_peak_rss() {
    const CHILD_ENV: &str = "CONARY_SLICE3_CATALOG_RSS_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "repository::catalog::profile::tests::bounded_source_and_profile_catalog_peak_rss",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&output.stdout));
        std::io::stderr().write_all(&output.stderr).unwrap();
        assert!(output.status.success(), "catalog RSS child failed");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("SLICE3_VM_HWM_KIB="),
            "catalog RSS child did not report VmHWM"
        );
        return;
    }

    const PACKAGES_PER_SOURCE: usize = 5_000;
    const RSS_LIMIT_KIB: u64 = 384 * 1024;
    let target_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("target"));
    std::fs::create_dir_all(&target_root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("slice3-catalog-rss-")
        .tempdir_in(target_root)
        .unwrap();

    let build_source = |repository_identity: &str, marker: char, path: &std::path::Path| {
        let scope = CatalogScopeV1::Source {
            source_profile: "fedora-44".to_string(),
            source_identity: "fedora-project".to_string(),
            repository_identity: repository_identity.to_string(),
        };
        let mut writer = CatalogCandidateWriter::create(path, scope).unwrap();
        for index in 0..PACKAGES_PER_SOURCE {
            let name = format!("generated-{marker}-{index:05}");
            let mut record = source_content(repository_identity, &name, marker)
                .packages
                .pop()
                .unwrap();
            record.checksum = crate::hash::sha256(name.as_bytes());
            record.metadata = Some(format!("{{\"presentation\":\"{}\"}}", "x".repeat(8 * 1024)));
            for provide in 0..16 {
                record.provides.push(CatalogProvideRecordV1 {
                    capability: format!("{name}-capability-{provide}"),
                    version: None,
                    version_relation: None,
                    kind: "package".to_string(),
                    raw: None,
                    version_scheme: VersionScheme::Rpm,
                    architecture_qualifier:
                        crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
                    provenance:
                        crate::repository::dependency_source::CapabilityProvenance::SourceDeclared {
                            format: crate::repository::dependency_model::SourcePackageFormat::Rpm,
                            record_index: provide,
                        },
                });
            }
            writer.package(record).unwrap();
        }
        writer
            .finish(vec![CatalogSourceEvidenceV1::AuthenticatedObject {
                role: SourceMetadataObjectRoleV1::RpmPrimary,
                source_path: "repodata/primary.xml.zst".to_string(),
                sha256: digest(marker),
                size: 1024,
            }])
            .unwrap()
    };

    let first_path = directory.path().join("first.sqlite");
    let second_path = directory.path().join("second.sqlite");
    let first_binding = build_source("fedora-everything-x86_64", 'a', &first_path);
    let second_binding = build_source("fedora-updates-x86_64", 'b', &second_path);
    let first_manifest = source_manifest("fedora-everything-x86_64", 'a', &first_binding);
    let second_manifest = source_manifest("fedora-updates-x86_64", 'b', &second_binding);
    let first_reader = CatalogReader::open_verified(&first_path, &first_binding).unwrap();
    let second_reader = CatalogReader::open_verified(&second_path, &second_binding).unwrap();
    let profile = write_profile_catalog_candidate(
        directory.path().join("profile.sqlite"),
        "fedora-44",
        1,
        vec![
            ProfileCatalogMemberInputV2 {
                ordinal: 0,
                role: ProfileSourceRole::Base,
                precedence: 10,
                required: true,
                manifest: &first_manifest,
                reader: &first_reader,
            },
            ProfileCatalogMemberInputV2 {
                ordinal: 1,
                role: ProfileSourceRole::Updates,
                precedence: 20,
                required: true,
                manifest: &second_manifest,
                reader: &second_reader,
            },
        ],
    )
    .unwrap();
    assert_eq!(profile.counts.packages, 2 * PACKAGES_PER_SOURCE as u64);

    let high_water_kib = vm_hwm_kib().unwrap();
    println!("SLICE3_VM_HWM_KIB={high_water_kib}");
    assert!(
        high_water_kib < RSS_LIMIT_KIB,
        "VmHWM {high_water_kib} KiB exceeded fixed {RSS_LIMIT_KIB} KiB bound"
    );
}

fn vm_hwm_kib() -> Option<u64> {
    let mut status = String::new();
    std::fs::File::open("/proc/self/status")
        .ok()?
        .read_to_string(&mut status)
        .ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}
