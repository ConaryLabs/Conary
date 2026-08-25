// crates/conary-core/src/repository/catalog/parity/debian/tests.rs

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V2, ProfileSourceMemberV2,
    SOURCE_SNAPSHOT_SCHEMA_V1, SourceProvenanceV1, SourceStreamKindV1, SourceStreamV1,
};
use crate::repository::supported_profiles::ProfileSourceRole;
use crate::repository::{OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy};

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn packages_fixture(shared_checksum: &str, include_primary: bool) -> Vec<u8> {
    let primary = if include_primary {
        format!(
            "Package: alpha\n\
             Version: 1:2.0-3\n\
             Architecture: amd64\n\
             Multi-Arch: same\n\
             Filename: pool/main/a/alpha_2.0-3_amd64.deb\n\
             Size: 123\n\
             SHA256: {}\n\
             Provides: alpha-abi:any (= 1:2.0-3), mailer\n\
             Pre-Depends: setup\n\
             Depends: libc6:any (>= 2.40), helper:native | alternative:arm64 (<< 3)\n\
             Recommends: companion\n\
             Suggests: documentation\n\
             Enhances: desktop\n\
             Conflicts: old-alpha\n\
             Breaks: broken-alpha (<= 1)\n\
             Replaces: legacy-alpha\n\
             Description: alpha\n\n",
            digest('a')
        )
    } else {
        format!(
            "Package: beta\n\
             Version: 1.0-1\n\
             Architecture: amd64\n\
             Multi-Arch: foreign\n\
             Filename: pool/main/b/beta_1.0-1_amd64.deb\n\
             Size: 77\n\
             SHA256: {}\n\
             Description: beta\n\n",
            digest('b')
        )
    };
    format!(
        "{primary}Package: shared\n\
         Version: 1.0-1\n\
         Architecture: all\n\
         Filename: pool/main/s/shared_1.0-1_all.deb\n\
         Size: 42\n\
         SHA256: {shared_checksum}\n\
         Description: shared\n"
    )
    .into_bytes()
}

fn write_packages(
    directory: &Path,
    repository: &str,
    shared_checksum: &str,
    include_primary: bool,
) -> PathBuf {
    let path = directory.join(format!("{repository}-Packages.zst"));
    let compressed = zstd::stream::encode_all(
        packages_fixture(shared_checksum, include_primary).as_slice(),
        1,
    )
    .unwrap();
    fs::write(&path, compressed).unwrap();
    path
}

fn source_snapshot(repository: &str, packages: &Path) -> SourceSnapshotV1 {
    let packages_bytes = fs::read(packages).unwrap();
    let parser_config = RepositoryParserConfig::Deb {
        distribution: "resolute".to_string(),
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
        repository_identity: repository.to_string(),
        stream: SourceStreamV1 {
            kind: SourceStreamKindV1::Release,
            identity: "26.04".to_string(),
        },
        stream_binding_sha256: digest('1'),
        parser_projection_version: 1,
        provenance: SourceProvenanceV1 {
            ecosystem: SourceEcosystemV1::Deb,
            metadata_url: "https://metadata.example.test/ubuntu".to_string(),
            content_url: Some("https://content.example.test/ubuntu".to_string()),
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
            sha256: digest('2'),
            size: 1024,
        },
        authenticated_objects: vec![SourceMetadataObjectV1 {
            role: SourceMetadataObjectRoleV1::DebianPackages,
            source_path: format!("dists/resolute/main/binary-amd64/{repository}-Packages.zst"),
            sha256: crate::hash::sha256(&packages_bytes),
            size: packages_bytes.len() as u64,
        }],
        catalog: CatalogArtifactV1 {
            sha256: digest('3'),
            size: 4096,
        },
        logical_digest_sha256: digest('4'),
        counts: CatalogCountsV1 {
            source_evidence: 2,
            ..CatalogCountsV1::default()
        },
    }
}

fn profile(snapshots: &[SourceSnapshotV1]) -> ProfileRevisionV2 {
    ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V2,
        profile: "ubuntu-26.04".to_string(),
        projection_version: 1,
        members: snapshots
            .iter()
            .enumerate()
            .map(|(ordinal, snapshot)| ProfileSourceMemberV2 {
                ordinal: u32::try_from(ordinal).unwrap(),
                role: ProfileSourceRole::Base,
                source_identity: snapshot.source_identity.clone(),
                repository_identity: snapshot.repository_identity.clone(),
                stream: snapshot.stream.clone(),
                precedence: 100 - i32::try_from(ordinal).unwrap() * 10,
                required: true,
                source_snapshot_sha256: snapshot.manifest_sha256().unwrap(),
            })
            .collect(),
        catalog: CatalogArtifactV1 {
            sha256: digest('5'),
            size: 8192,
        },
        logical_digest_sha256: digest('6'),
        counts: CatalogCountsV1 {
            packages: 3,
            source_evidence: snapshots.len() as u64,
            ..CatalogCountsV1::default()
        },
    }
}

fn fixture(directory: &Path, conflicting_duplicate: bool) -> (Vec<PathBuf>, Vec<SourceSnapshotV1>) {
    let shared = digest('c');
    let conflict = digest('d');
    let packages = vec![
        write_packages(directory, "ubuntu-main", &shared, true),
        write_packages(
            directory,
            "ubuntu-updates",
            if conflicting_duplicate {
                &conflict
            } else {
                &shared
            },
            false,
        ),
    ];
    let snapshots = packages
        .iter()
        .zip(["ubuntu-main", "ubuntu-updates"])
        .map(|(packages, repository)| source_snapshot(repository, packages))
        .collect();
    (packages, snapshots)
}

fn inputs<'a>(
    snapshots: &'a [SourceSnapshotV1],
    packages: &'a [PathBuf],
) -> Vec<DebianParityMemberInput<'a>> {
    snapshots
        .iter()
        .zip(packages)
        .map(|(source_snapshot, packages)| DebianParityMemberInput {
            source_snapshot,
            packages,
        })
        .collect()
}

#[test]
fn apt_pkg_reopens_every_stanza_and_projects_native_relations() {
    let directory = tempfile::tempdir().unwrap();
    let packages_path = directory.path().join("Packages");
    fs::write(
        &packages_path,
        "Package: fixture\n\
         Version: 1:2.0-3\n\
         Architecture: amd64\n\
         Multi-Arch: same\n\
         Filename: pool/main/f/fixture_2.0-3_amd64.deb\n\
         Size: 123\n\
         SHA256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
         Provides: fixture-abi:any (= 1:2.0-3), mailer\n\
         Depends: libc6:any (>= 2.40), helper:native | alternative:arm64 (<< 3)\n\
         Recommends: companion\n\
         Conflicts: old-fixture\n\
         Breaks: broken-fixture (<= 1)\n\
         Replaces: legacy-fixture\n\
         Description: fixture\n\n\
         Package: helper\n\
         Version: 1.0-1\n\
         Architecture: all\n\
         Filename: pool/main/h/helper_1.0-1_all.deb\n\
         Size: 42\n\
         SHA256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n\
         Description: helper\n",
    )
    .unwrap();

    assert_eq!(AptPackages::version().unwrap(), "3.2.0");
    let packages = AptPackages::open(&packages_path)
        .unwrap()
        .packages()
        .unwrap();
    assert_eq!(packages.len(), 2);
    let fixture = &packages[0];
    assert_eq!(fixture.name, "fixture");
    assert_eq!(fixture.multi_arch.as_deref(), Some("same"));
    assert_eq!(fixture.provides.len(), 2);
    assert_eq!(
        fixture.provides[0].architecture_qualifier,
        AptArchitectureQualifier::Any
    );
    assert_eq!(fixture.relation_groups.len(), 6);
    let alternative = fixture
        .relation_groups
        .iter()
        .find(|group| group.atoms.len() == 2)
        .unwrap();
    assert_eq!(alternative.kind, AptRelationKind::Depends);
    assert_eq!(
        alternative.atoms[0].architecture_qualifier,
        AptArchitectureQualifier::Native
    );
    assert_eq!(alternative.atoms[1].architecture.as_deref(), Some("arm64"));
    assert_eq!(
        alternative.native_text,
        "helper:native | alternative:arm64 (<< 3)"
    );
}

#[test]
fn apt_pkg_rejects_repeated_authority_fields() {
    let directory = tempfile::tempdir().unwrap();
    let packages_path = directory.path().join("Packages");
    fs::write(
        &packages_path,
        "Package: fixture\n\
         Package: contradiction\n\
         Version: 1\n\
         Architecture: all\n\
         Filename: pool/f.deb\n\
         Size: 1\n\
         SHA256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    )
    .unwrap();
    let error = AptPackages::open(&packages_path).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("repeats authority field package")
    );
}

#[test]
fn producer_projects_complete_typed_debian_facts_and_reopens_bundle() {
    let directory = tempfile::tempdir().unwrap();
    let (packages, snapshots) = fixture(directory.path(), false);
    let profile = profile(&snapshots);
    let output = directory.path().join("oracle");

    let manifest =
        produce_debian_parity_oracle(&profile, &inputs(&snapshots, &packages), &output).unwrap();

    assert_eq!(manifest.implementation.name, "apt-pkg");
    assert_eq!(manifest.implementation.version, "3.2.0");
    assert_eq!(manifest.artifact.counts.packages, 3);
    let reader = verify_native_parity_oracle_bundle(&output, &profile).unwrap();
    let mut projected = Vec::new();
    reader
        .for_each_package(|package| {
            projected.push(package);
            Ok(())
        })
        .unwrap();
    let alpha = projected
        .iter()
        .find(|package| package.name == "alpha")
        .unwrap();
    assert_eq!(alpha.version, "1:2.0-3");
    assert_eq!(alpha.package_release, "");
    assert_eq!(alpha.architecture.as_deref(), Some("amd64"));
    assert_eq!(alpha.debian_multi_arch, Some(DebianMultiArch::Same));
    assert_eq!(alpha.checksum, format!("sha256:{}", digest('a')));
    assert_eq!(alpha.size, 123);
    assert_eq!(
        alpha.download_url,
        "https://content.example.test/ubuntu/pool/main/a/alpha_2.0-3_amd64.deb"
    );
    assert!(alpha.provides.iter().any(|provide| {
        provide.capability == "alpha-abi"
            && provide.version.as_deref() == Some("1:2.0-3")
            && provide.architecture_qualifier == ProvideArchitectureQualifier::Any
    }));
    for kind in [
        "pre_depends",
        "depends",
        "recommends",
        "suggests",
        "enhances",
        "conflict",
        "breaks",
        "replace",
    ] {
        assert!(
            alpha
                .requirement_groups
                .iter()
                .any(|group| group.kind == kind),
            "missing Debian relation {kind}"
        );
    }
    let alternative = alpha
        .requirement_groups
        .iter()
        .find(|group| group.atoms.len() == 2)
        .unwrap();
    assert!(alternative.expression_json.contains("\"operator\":\"or\""));
    assert!(alternative.atoms.iter().any(|atom| {
        atom.capability == "helper" && atom.raw.as_deref() == Some("helper:native")
    }));
    let shared = projected
        .iter()
        .find(|package| package.name == "shared")
        .unwrap();
    assert_eq!(shared.member_ordinal, 0);
    assert_eq!(shared.repository_identity, "ubuntu-main");
}

#[test]
fn producer_rejects_changed_authenticated_packages_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let (packages, snapshots) = fixture(directory.path(), false);
    let profile = profile(&snapshots);
    fs::OpenOptions::new()
        .append(true)
        .open(&packages[0])
        .unwrap()
        .write_all(b"changed")
        .unwrap();

    let error = produce_debian_parity_oracle(
        &profile,
        &inputs(&snapshots, &packages),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ChecksumMismatch { .. }));
}

#[test]
fn producer_rejects_conflicting_exact_identity() {
    let directory = tempfile::tempdir().unwrap();
    let (packages, snapshots) = fixture(directory.path(), true);
    let profile = profile(&snapshots);

    let error = produce_debian_parity_oracle(
        &profile,
        &inputs(&snapshots, &packages),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ConflictError(_)));
    assert!(error.to_string().contains("contradictory package identity"));
}

#[test]
fn producer_requires_exact_debian_packages_role() {
    let directory = tempfile::tempdir().unwrap();
    let (packages, mut snapshots) = fixture(directory.path(), false);
    snapshots[0].authenticated_objects[0].role = SourceMetadataObjectRoleV1::RpmPrimary;
    let profile = profile(&snapshots);

    let error = produce_debian_parity_oracle(
        &profile,
        &inputs(&snapshots, &packages),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("exactly one DebianPackages"));
}

#[test]
fn apt_pkg_rejects_malformed_relation_without_dropping_the_stanza() {
    let directory = tempfile::tempdir().unwrap();
    let packages_path = directory.path().join("Packages");
    fs::write(
        &packages_path,
        "Package: fixture\n\
         Version: 1\n\
         Architecture: all\n\
         Filename: pool/f.deb\n\
         Size: 1\n\
         SHA256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
         Depends: broken (>= )\n",
    )
    .unwrap();
    let error = AptPackages::open(&packages_path).unwrap_err();
    assert!(error.to_string().contains("rejected Debian relation field"));
}
