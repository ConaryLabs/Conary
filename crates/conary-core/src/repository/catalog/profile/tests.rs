// crates/conary-core/src/repository/catalog/profile/tests.rs

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogPackageRecordV1, CatalogProvideRecordV1, SOURCE_SNAPSHOT_SCHEMA_V1,
    SourceEcosystemV1, SourceMetadataObjectRoleV1, SourceMetadataObjectV1, SourceProvenanceV1,
    SourceStreamKindV1, SourceStreamV1, write_catalog_candidate,
};
use crate::repository::versioning::VersionScheme;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};
use std::io::{Read, Write};

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
        parser_projection_version: 1,
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
        let first = ProfileCatalogMemberInputV1 {
            ordinal: 0,
            priority: 10,
            required: true,
            manifest: &first_manifest,
            reader: &first_reader,
        };
        let second = ProfileCatalogMemberInputV1 {
            ordinal: 1,
            priority: 20,
            required: true,
            manifest: &second_manifest,
            reader: &second_reader,
        };
        ProfileCatalogCandidateV1::compose(
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
            ProfileCatalogMemberInputV1 {
                ordinal: 0,
                priority: 10,
                required: true,
                manifest: &first_manifest,
                reader: &first_reader,
            },
            ProfileCatalogMemberInputV1 {
                ordinal: 1,
                priority: 20,
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
