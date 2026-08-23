// crates/conary-core/src/repository/catalog/parity/rpm/tests.rs

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::{Compression, GzBuilder};

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, PROFILE_REVISION_SCHEMA_V2, ProfileSourceMemberV2,
    SOURCE_SNAPSHOT_SCHEMA_V1, SourceProvenanceV1, SourceStreamKindV1, SourceStreamV1,
};
use crate::repository::supported_profiles::ProfileSourceRole;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

#[derive(Clone)]
struct PackageFixture<'a> {
    name: &'a str,
    version: &'a str,
    release: &'a str,
    checksum: &'a str,
    size: u64,
    format: &'a str,
    files: &'a [&'a str],
}

impl<'a> PackageFixture<'a> {
    fn simple(name: &'a str, checksum: &'a str) -> Self {
        Self {
            name,
            version: "1.0",
            release: "1.fc44",
            checksum,
            size: 42,
            format: "",
            files: &["/usr/share/fixture"],
        }
    }
}

fn digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn package_xml(package: &PackageFixture<'_>) -> String {
    format!(
        r#"<package type="rpm">
  <name>{name}</name>
  <arch>x86_64</arch>
  <version epoch="0" ver="{version}" rel="{release}"/>
  <checksum type="sha256" pkgid="YES">{checksum}</checksum>
  <summary>{name} fixture</summary>
  <description>{name} fixture</description>
  <packager>Conary Tests</packager>
  <url>https://example.test/{name}</url>
  <time file="1" build="1"/>
  <size package="{size}" installed="84" archive="84"/>
  <location href="Packages/{name}.rpm"/>
  <format>
    <rpm:license>MIT</rpm:license>
    <rpm:vendor>Conary</rpm:vendor>
    <rpm:group>System</rpm:group>
    <rpm:buildhost>builder.example.test</rpm:buildhost>
    <rpm:sourcerpm>{name}-{version}-{release}.src.rpm</rpm:sourcerpm>
    <rpm:header-range start="1" end="2"/>
    {format}
  </format>
</package>"#,
        name = package.name,
        version = package.version,
        release = package.release,
        checksum = package.checksum,
        size = package.size,
        format = package.format,
    )
}

fn primary_xml(packages: &[PackageFixture<'_>]) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata xmlns="http://linux.duke.edu/metadata/common"
          xmlns:rpm="http://linux.duke.edu/metadata/rpm"
          packages="{}">
{}
</metadata>
"#,
        packages.len(),
        packages.iter().map(package_xml).collect::<String>()
    )
    .into_bytes()
}

fn filelists_xml(packages: &[PackageFixture<'_>]) -> Vec<u8> {
    let packages = packages
        .iter()
        .map(|package| {
            let files = package
                .files
                .iter()
                .map(|path| format!("    <file>{path}</file>\n"))
                .collect::<String>();
            format!(
                r#"<package pkgid="{}" name="{}" arch="x86_64">
    <version epoch="0" ver="{}" rel="{}"/>
{files}  </package>
"#,
                package.checksum, package.name, package.version, package.release
            )
        })
        .collect::<String>();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<filelists xmlns="http://linux.duke.edu/metadata/filelists" packages="{}">
{packages}</filelists>
"#,
        packages.matches("<package ").count()
    )
    .into_bytes()
}

fn write_metadata(
    directory: &Path,
    repository: &str,
    packages: &[PackageFixture<'_>],
) -> (PathBuf, PathBuf) {
    let primary = directory.join(format!("{repository}-primary.xml.gz"));
    let filelists = directory.join(format!("{repository}-filelists.xml.zst"));
    let mut gzip = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::default());
    gzip.write_all(&primary_xml(packages)).unwrap();
    fs::write(&primary, gzip.finish().unwrap()).unwrap();
    fs::write(
        &filelists,
        zstd::stream::encode_all(filelists_xml(packages).as_slice(), 1).unwrap(),
    )
    .unwrap();
    (primary, filelists)
}

fn source_snapshot(repository: &str, primary: &Path, filelists: &Path) -> SourceSnapshotV1 {
    let primary_bytes = fs::read(primary).unwrap();
    let filelists_bytes = fs::read(filelists).unwrap();
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
        repository_identity: repository.to_string(),
        stream: SourceStreamV1 {
            kind: SourceStreamKindV1::Release,
            identity: "44".to_string(),
        },
        stream_binding_sha256: digest('1'),
        parser_projection_version: 1,
        provenance: SourceProvenanceV1 {
            ecosystem: SourceEcosystemV1::Rpm,
            metadata_url: "https://metadata.example.test/fedora".to_string(),
            content_url: Some("https://content.example.test/fedora".to_string()),
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
        authenticated_objects: vec![
            SourceMetadataObjectV1 {
                role: SourceMetadataObjectRoleV1::RpmPrimary,
                source_path: format!("repodata/{repository}-primary.xml.gz"),
                sha256: crate::hash::sha256(&primary_bytes),
                size: primary_bytes.len() as u64,
            },
            SourceMetadataObjectV1 {
                role: SourceMetadataObjectRoleV1::RpmFilelists,
                source_path: format!("repodata/{repository}-filelists.xml.zst"),
                sha256: crate::hash::sha256(&filelists_bytes),
                size: filelists_bytes.len() as u64,
            },
        ],
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
        profile: "fedora-44".to_string(),
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

fn fixture(
    directory: &Path,
    conflicting_duplicate: bool,
) -> (Vec<(PathBuf, PathBuf)>, Vec<SourceSnapshotV1>) {
    let alpha_checksum = digest('a');
    let shared_checksum = digest('b');
    let beta_checksum = digest('c');
    let conflict_checksum = digest('d');
    let alpha_format = r#"
    <rpm:provides>
      <rpm:entry name="alpha" flags="EQ" epoch="0" ver="1.2" rel="3.fc44"/>
      <rpm:entry name="virtual-alpha" flags="GE" epoch="0" ver="2" rel="1"/>
    </rpm:provides>
    <rpm:requires>
      <rpm:entry name="glibc" flags="GE" epoch="0" ver="2.40" rel="1.fc44"/>
      <rpm:entry name="(feature-a or feature-b)"/>
      <rpm:entry name="setup" pre="1"/>
    </rpm:requires>
    <rpm:recommends><rpm:entry name="recommended"/></rpm:recommends>
    <rpm:suggests><rpm:entry name="suggested"/></rpm:suggests>
    <rpm:supplements><rpm:entry name="(alpha if desktop)"/></rpm:supplements>
    <rpm:enhances><rpm:entry name="enhanced"/></rpm:enhances>
    <rpm:conflicts><rpm:entry name="old-alpha" flags="LT" epoch="0" ver="1" rel="0"/></rpm:conflicts>
    <rpm:obsoletes><rpm:entry name="older-alpha"/></rpm:obsoletes>
    <file>/usr/bin/alpha</file>"#;
    let mut alpha = PackageFixture::simple("alpha", &alpha_checksum);
    alpha.version = "1.2";
    alpha.release = "3.fc44";
    alpha.size = 123;
    alpha.format = alpha_format;
    alpha.files = &["/usr/bin/alpha", "/usr/share/alpha/data"];
    let shared_core = PackageFixture::simple("shared", &shared_checksum);
    let core_packages = [alpha, shared_core.clone()];
    let core = write_metadata(directory, "fedora-core", &core_packages);

    let beta = PackageFixture::simple("beta", &beta_checksum);
    let mut shared_updates = shared_core;
    if conflicting_duplicate {
        shared_updates.checksum = &conflict_checksum;
    }
    let updates_packages = [beta, shared_updates];
    let updates = write_metadata(directory, "fedora-updates", &updates_packages);
    let metadata = vec![core, updates];
    let snapshots = metadata
        .iter()
        .zip(["fedora-core", "fedora-updates"])
        .map(|((primary, filelists), repository)| source_snapshot(repository, primary, filelists))
        .collect();
    (metadata, snapshots)
}

fn inputs<'a>(
    snapshots: &'a [SourceSnapshotV1],
    metadata: &'a [(PathBuf, PathBuf)],
) -> Vec<RpmParityMemberInput<'a>> {
    snapshots
        .iter()
        .zip(metadata)
        .map(
            |(source_snapshot, (primary, filelists))| RpmParityMemberInput {
                source_snapshot,
                primary,
                filelists,
            },
        )
        .collect()
}

#[test]
fn producer_projects_complete_typed_rpm_facts_and_reopens_bundle() {
    let directory = tempfile::tempdir().unwrap();
    let (metadata, snapshots) = fixture(directory.path(), false);
    let profile = profile(&snapshots);
    let output = directory.path().join("oracle");

    let manifest =
        produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &output).unwrap();

    assert_eq!(manifest.implementation.name, "libsolv");
    assert_eq!(manifest.implementation.version, "0.7.36");
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
    assert_eq!(alpha.version, "1.2-3.fc44");
    assert_eq!(alpha.checksum, format!("sha256:{}", digest('a')));
    assert_eq!(alpha.size, 123);
    assert_eq!(alpha.architecture.as_deref(), Some("x86_64"));
    assert!(alpha.provides.iter().any(|provide| {
        provide.capability == "/usr/share/alpha/data" && provide.kind == "file"
    }));
    assert!(alpha.provides.iter().any(|provide| {
        provide.capability == "virtual-alpha"
            && provide.version.as_deref() == Some("2-1")
            && provide.version_relation == Some(ProvideVersionRelation::GreaterOrEqual)
    }));
    for kind in [
        "depends",
        "pre_depends",
        "recommends",
        "suggests",
        "supplements",
        "enhances",
        "conflict",
        "obsolete",
    ] {
        assert!(
            alpha
                .requirement_groups
                .iter()
                .any(|group| group.kind == kind),
            "missing RPM relation {kind}"
        );
    }
    let rich = alpha
        .requirement_groups
        .iter()
        .find(|group| group.native_text.as_deref() == Some("(feature-a or feature-b)"))
        .unwrap();
    assert!(rich.expression_json.contains("\"operator\":\"or\""));
    let shared = packages
        .iter()
        .find(|package| package.name == "shared")
        .unwrap();
    assert_eq!(shared.member_ordinal, 0);
    assert_eq!(shared.repository_identity, "fedora-core");
}

#[test]
fn producer_rejects_changed_authenticated_rpmmd_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let (metadata, snapshots) = fixture(directory.path(), false);
    let profile = profile(&snapshots);
    fs::OpenOptions::new()
        .append(true)
        .open(&metadata[0].0)
        .unwrap()
        .write_all(b"changed")
        .unwrap();

    let error = produce_rpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ChecksumMismatch { .. }));
}

#[test]
fn producer_rejects_conflicting_exact_identity() {
    let directory = tempfile::tempdir().unwrap();
    let (metadata, snapshots) = fixture(directory.path(), true);
    let profile = profile(&snapshots);

    let error = produce_rpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(matches!(error, Error::ConflictError(_)));
    assert!(error.to_string().contains("contradictory package identity"));
}

#[test]
fn producer_requires_exact_rpm_metadata_roles() {
    let directory = tempfile::tempdir().unwrap();
    let (metadata, mut snapshots) = fixture(directory.path(), false);
    snapshots[0].authenticated_objects.pop();
    let profile = profile(&snapshots);

    let error = produce_rpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &directory.path().join("oracle"),
    )
    .unwrap_err();

    assert!(error.to_string().contains("exactly primary and filelists"));
}
