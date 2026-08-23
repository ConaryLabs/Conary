// crates/conary-core/src/repository/catalog/parity/alpm/tests.rs

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::{Compression, GzBuilder};

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V2, ProfileSourceMemberV2,
    SOURCE_SNAPSHOT_SCHEMA_V1, SourceMetadataObjectV1, SourceProvenanceV1, SourceStreamKindV1,
    SourceStreamV1,
};
use crate::repository::supported_profiles::ProfileSourceRole;
use crate::repository::{
    ArchKeyringFormat, ArchKeyringTrust, ArchSigLevel, RepositoryParserConfig,
    RepositoryTrustPolicy,
};

#[derive(Clone)]
struct PackageFixture<'a> {
    name: &'a str,
    version: &'a str,
    architecture: &'a str,
    checksum: &'a str,
    size: u64,
    provides: &'a [&'a str],
    depends: &'a [&'a str],
    optional: &'a [&'a str],
    conflicts: &'a [&'a str],
    replaces: &'a [&'a str],
    include_checksum: bool,
}

impl<'a> PackageFixture<'a> {
    fn new(name: &'a str, checksum: &'a str) -> Self {
        Self {
            name,
            version: "1.0-1",
            architecture: "x86_64",
            checksum,
            size: 42,
            provides: &[],
            depends: &[],
            optional: &[],
            conflicts: &[],
            replaces: &[],
            include_checksum: true,
        }
    }
}

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn write_database(path: &Path, packages: &[PackageFixture<'_>]) {
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    let mut archive = tar::Builder::new(encoder);
    archive.mode(tar::HeaderMode::Deterministic);
    for package in packages {
        let directory = format!("{}-{}/desc", package.name, package.version);
        let filename = format!(
            "{}-{}-{}.pkg.tar.zst",
            package.name, package.version, package.architecture
        );
        let mut desc = format!(
            "%FILENAME%\n{filename}\n\n%NAME%\n{}\n\n%BASE%\n{}\n\n%VERSION%\n{}\n\n%DESC%\n{} fixture\n\n%CSIZE%\n{}\n\n%ISIZE%\n84\n\n%ARCH%\n{}\n\n%BUILDDATE%\n1\n\n%PACKAGER%\nConary Tests\n\n",
            package.name,
            package.name,
            package.version,
            package.name,
            package.size,
            package.architecture,
        );
        if package.include_checksum {
            desc.push_str(&format!("%SHA256SUM%\n{}\n\n", package.checksum));
        }
        push_field(&mut desc, "PROVIDES", package.provides);
        push_field(&mut desc, "DEPENDS", package.depends);
        push_field(&mut desc, "OPTDEPENDS", package.optional);
        push_field(&mut desc, "CONFLICTS", package.conflicts);
        push_field(&mut desc, "REPLACES", package.replaces);
        append_archive_file(&mut archive, &directory, desc.as_bytes());
    }
    let encoder = archive.into_inner().unwrap();
    let bytes = encoder.finish().unwrap();
    fs::write(path, bytes).unwrap();
}

fn push_field(output: &mut String, name: &str, values: &[&str]) {
    if values.is_empty() {
        return;
    }
    output.push('%');
    output.push_str(name);
    output.push_str("%\n");
    for value in values {
        output.push_str(value);
        output.push('\n');
    }
    output.push('\n');
}

fn append_archive_file<W: Write>(archive: &mut tar::Builder<W>, path: &str, bytes: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(u64::try_from(bytes.len()).unwrap());
    header.set_cksum();
    archive.append_data(&mut header, path, bytes).unwrap();
}

fn source_snapshot(repository: &str, database: &Path) -> SourceSnapshotV1 {
    let bytes = fs::read(database).unwrap();
    let database_digest = crate::hash::sha256(&bytes);
    let parser_config = RepositoryParserConfig::Arch {
        database: repository.to_string(),
    };
    let trust_policy = RepositoryTrustPolicy::Arch {
        keyring: ArchKeyringTrust {
            url: "https://keys.example.test/archlinux.gpg".to_string(),
            format: ArchKeyringFormat::OpenPgp,
            master_fingerprints: vec!["A".repeat(40)],
            packager_key_threshold: 1,
        },
        sig_level: ArchSigLevel::distribution_default(),
    };
    SourceSnapshotV1 {
        schema_version: SOURCE_SNAPSHOT_SCHEMA_V1,
        source_profile: "arch".to_string(),
        source_identity: "archlinux".to_string(),
        repository_identity: repository.to_string(),
        stream: SourceStreamV1 {
            kind: SourceStreamKindV1::Rolling,
            identity: "archlinux".to_string(),
        },
        stream_binding_sha256: digest('1'),
        parser_projection_version: 1,
        provenance: SourceProvenanceV1 {
            ecosystem: SourceEcosystemV1::Alpm,
            metadata_url: format!("https://mirror.example.test/{repository}"),
            content_url: None,
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
            sha256: database_digest.clone(),
            size: bytes.len() as u64,
        },
        authenticated_objects: vec![SourceMetadataObjectV1 {
            role: SourceMetadataObjectRoleV1::ArchDatabase,
            source_path: format!("{repository}.db"),
            sha256: database_digest,
            size: bytes.len() as u64,
        }],
        catalog: CatalogArtifactV1 {
            sha256: digest('2'),
            size: 4096,
        },
        logical_digest_sha256: digest('3'),
        counts: CatalogCountsV1 {
            source_evidence: 1,
            ..CatalogCountsV1::default()
        },
    }
}

fn profile(snapshots: &[SourceSnapshotV1]) -> ProfileRevisionV2 {
    let repositories = ["arch-core-x86_64", "arch-extra-x86_64"];
    ProfileRevisionV2 {
        schema_version: PROFILE_REVISION_SCHEMA_V2,
        profile: "arch".to_string(),
        projection_version: 1,
        members: snapshots
            .iter()
            .zip(repositories)
            .enumerate()
            .map(
                |(ordinal, (snapshot, repository_identity))| ProfileSourceMemberV2 {
                    ordinal: u32::try_from(ordinal).unwrap(),
                    role: ProfileSourceRole::Base,
                    source_identity: "archlinux".to_string(),
                    repository_identity: repository_identity.to_string(),
                    stream: snapshot.stream.clone(),
                    precedence: 80 - i32::try_from(ordinal).unwrap() * 10,
                    required: true,
                    source_snapshot_sha256: snapshot.manifest_sha256().unwrap(),
                },
            )
            .collect(),
        catalog: CatalogArtifactV1 {
            sha256: digest('4'),
            size: 8192,
        },
        logical_digest_sha256: digest('5'),
        counts: CatalogCountsV1 {
            packages: 3,
            source_evidence: 2,
            ..CatalogCountsV1::default()
        },
    }
}

fn fixture_databases(
    directory: &Path,
    conflicting_duplicate: bool,
    missing_checksum: bool,
) -> (Vec<PathBuf>, Vec<SourceSnapshotV1>) {
    let core = directory.join("core.db");
    let extra = directory.join("extra.db");
    let alpha_digest = digest('a');
    let shared_digest = digest('b');
    let beta_digest = digest('c');
    let conflict_digest = digest('d');
    let mut alpha = PackageFixture::new("alpha", &alpha_digest);
    alpha.provides = &["virtual-alpha=1.0", "lib:libalpha.so.1"];
    alpha.depends = &["runtime>=1", "lib:libc.so.6"];
    alpha.optional = &["optional>=2: useful extra"];
    alpha.conflicts = &["old-alpha<1"];
    alpha.replaces = &["older-alpha"];
    alpha.include_checksum = !missing_checksum;
    let shared_core = PackageFixture::new("shared", &shared_digest);
    write_database(&core, &[alpha, shared_core.clone()]);

    let beta = PackageFixture::new("beta", &beta_digest);
    let mut shared_extra = shared_core;
    if conflicting_duplicate {
        shared_extra.checksum = &conflict_digest;
    }
    write_database(&extra, &[beta, shared_extra]);
    let snapshots = vec![
        source_snapshot("arch-core-x86_64", &core),
        source_snapshot("arch-extra-x86_64", &extra),
    ];
    (vec![core, extra], snapshots)
}

fn inputs<'a>(
    snapshots: &'a [SourceSnapshotV1],
    databases: &'a [PathBuf],
) -> Vec<AlpmParityMemberInput<'a>> {
    snapshots
        .iter()
        .zip(databases)
        .map(|(source_snapshot, database)| AlpmParityMemberInput {
            source_snapshot,
            database,
        })
        .collect()
}

#[test]
fn producer_accounts_for_every_native_row_and_reopens_bundle() {
    let directory = tempfile::tempdir().unwrap();
    let (databases, snapshots) = fixture_databases(directory.path(), false, false);
    let profile = profile(&snapshots);
    let output = directory.path().join("oracle");

    let manifest =
        produce_alpm_parity_oracle(&profile, &inputs(&snapshots, &databases), &output).unwrap();

    assert_eq!(manifest.implementation.name, "libalpm");
    assert_eq!(manifest.implementation.version, alpm::version());
    assert_eq!(manifest.implementation.projection_schema, 1);
    assert_eq!(manifest.artifact.counts.packages, 3);
    let reader = verify_native_parity_oracle_bundle(&output, &profile).unwrap();
    let mut packages = Vec::new();
    reader
        .for_each_package(|package| {
            packages.push(package);
            Ok(())
        })
        .unwrap();
    let alpha = packages
        .iter()
        .find(|package| package.name == "alpha")
        .unwrap();
    assert_eq!(alpha.checksum, format!("sha256:{}", digest('a')));
    assert_eq!(alpha.size, 42);
    assert_eq!(alpha.architecture.as_deref(), Some("x86_64"));
    assert!(
        alpha
            .provides
            .iter()
            .any(|provide| provide.kind == "virtual")
    );
    assert!(
        alpha
            .provides
            .iter()
            .any(|provide| provide.kind == "soname")
    );
    for kind in ["depends", "optional", "conflict", "replace"] {
        assert!(
            alpha
                .requirement_groups
                .iter()
                .any(|group| group.kind == kind),
            "missing {kind}"
        );
    }
    let optional = alpha
        .requirement_groups
        .iter()
        .find(|group| group.kind == "optional")
        .unwrap();
    assert_eq!(optional.description.as_deref(), Some("useful extra"));
    assert_eq!(
        optional.native_text.as_deref(),
        Some("optional>=2: useful extra")
    );
    assert_eq!(optional.atoms[0].raw, None);
    let soname = alpha
        .requirement_groups
        .iter()
        .find(|group| group.atoms[0].kind == "soname")
        .unwrap();
    assert_eq!(soname.atoms[0].raw.as_deref(), Some("lib:libc.so.6"));
    let shared = packages
        .iter()
        .find(|package| package.name == "shared")
        .unwrap();
    assert_eq!(shared.member_ordinal, 0);
    assert_eq!(shared.repository_identity, "arch-core-x86_64");
}

#[test]
fn producer_rejects_changed_database_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let (databases, snapshots) = fixture_databases(directory.path(), false, false);
    let profile = profile(&snapshots);
    fs::OpenOptions::new()
        .append(true)
        .open(&databases[0])
        .unwrap()
        .write_all(b"changed")
        .unwrap();

    let error = produce_alpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &databases),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ChecksumMismatch { .. }));
}

#[test]
fn producer_rejects_source_snapshot_manifest_drift() {
    let directory = tempfile::tempdir().unwrap();
    let (databases, mut snapshots) = fixture_databases(directory.path(), false, false);
    let profile = profile(&snapshots);
    snapshots[0].logical_digest_sha256 = digest('9');

    let error = produce_alpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &databases),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ConflictError(_)));
    assert!(error.to_string().contains("source snapshot disagrees"));
}

#[test]
fn producer_rejects_conflicting_exact_identity() {
    let directory = tempfile::tempdir().unwrap();
    let (databases, snapshots) = fixture_databases(directory.path(), true, false);
    let profile = profile(&snapshots);

    let error = produce_alpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &databases),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ConflictError(_)));
    assert!(error.to_string().contains("contradictory package identity"));
}

#[test]
fn producer_rejects_native_package_missing_payload_authority() {
    let directory = tempfile::tempdir().unwrap();
    let (databases, snapshots) = fixture_databases(directory.path(), false, true);
    let profile = profile(&snapshots);

    let error = produce_alpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &databases),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("missing SHA-256"));
}
