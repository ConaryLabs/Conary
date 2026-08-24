// crates/conary-core/src/repository/catalog/parity/rpm/tests.rs

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::{Compression, GzBuilder};

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, NativeResolutionOutcomeV1, NativeUnresolvedDependencyV1,
    PROFILE_REVISION_SCHEMA_V2, ProfileSourceMemberV2, SOURCE_SNAPSHOT_SCHEMA_V1,
    SourceProvenanceV1, SourceStreamKindV1, SourceStreamV1, native_requirement_group_sha256,
    verify_native_resolution_oracle_bundle,
};
use crate::repository::supported_profiles::ProfileSourceRole;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

#[derive(Clone)]
struct PackageFixture<'a> {
    name: &'a str,
    architecture: &'a str,
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
            architecture: "x86_64",
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
  <arch>{architecture}</arch>
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
        architecture = package.architecture,
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
                r#"<package pkgid="{}" name="{}" arch="{}">
    <version epoch="0" ver="{}" rel="{}"/>
{files}  </package>
"#,
                package.checksum,
                package.name,
                package.architecture,
                package.version,
                package.release
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

#[test]
fn resolution_producer_emits_complete_closures_and_typed_unresolved_groups() {
    let directory = tempfile::tempdir().unwrap();
    let (metadata, snapshots) = fixture(directory.path(), false);
    let profile = profile(&snapshots);
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();
    let resolution_output = directory.path().join("resolution-oracle");

    let manifest = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &resolution_output,
    )
    .unwrap();

    assert_eq!(manifest.implementation.name, "libsolv");
    assert_eq!(manifest.implementation.version, "0.7.36");
    assert_eq!(manifest.artifact.counts.roots, 3);
    assert_eq!(manifest.artifact.counts.resolved_roots, 2);
    assert_eq!(manifest.artifact.counts.unresolved_roots, 1);
    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let reader =
        verify_native_resolution_oracle_bundle(&resolution_output, &profile, &package_reader)
            .unwrap();
    let mut roots = Vec::new();
    reader
        .for_each_root(|root| {
            roots.push(root);
            Ok(())
        })
        .unwrap();
    let packages = {
        let mut packages = Vec::new();
        package_reader
            .for_each_package(|package| {
                packages.push(package);
                Ok(())
            })
            .unwrap();
        packages
    };
    let alpha = packages
        .iter()
        .find(|package| package.name == "alpha")
        .unwrap();
    let alpha_root = roots
        .iter()
        .find(|root| root.root_package_key_sha256 == alpha.package_key_sha256)
        .unwrap();
    let NativeResolutionOutcomeV1::Unresolved { dependencies } = &alpha_root.outcome else {
        panic!("alpha must retain typed unresolved requirements");
    };
    assert!(!dependencies.is_empty());
    assert!(
        dependencies
            .iter()
            .all(|dependency| dependency.requiring_package_key_sha256 == alpha.package_key_sha256)
    );
    let strong_groups = alpha
        .requirement_groups
        .iter()
        .filter(|group| group.kind == "depends" || group.kind == "pre_depends")
        .map(native_requirement_group_sha256)
        .collect::<Result<std::collections::BTreeSet<_>>>()
        .unwrap();
    assert!(!dependencies.is_empty());
    assert!(
        dependencies
            .iter()
            .all(|dependency| { strong_groups.contains(&dependency.requirement_group_sha256) })
    );
    for package in packages.iter().filter(|package| package.name != "alpha") {
        let root = roots
            .iter()
            .find(|root| root.root_package_key_sha256 == package.package_key_sha256)
            .unwrap();
        assert_eq!(
            root.outcome,
            NativeResolutionOutcomeV1::Resolved {
                closure_package_keys_sha256: vec![package.package_key_sha256.clone()]
            }
        );
    }
}

#[test]
fn resolution_producer_uses_precedence_variants_files_rich_and_strong_requirements() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b', 'c', 'd', 'e', 'f', '1', '2', '3'].map(digest);
    let app_format = r#"
    <rpm:provides><rpm:entry name="app"/></rpm:provides>
    <rpm:requires>
      <rpm:entry name="virtual-provider"/>
      <rpm:entry name="/usr/bin/file-tool"/>
      <rpm:entry name="helper" flags="GE" epoch="0" ver="1" rel="0"/>
      <rpm:entry name="(feature-a or feature-b)"/>
      <rpm:entry name="setup" pre="1"/>
    </rpm:requires>
    <rpm:recommends><rpm:entry name="weak-only"/></rpm:recommends>"#;
    let provider_high_format = r#"
    <rpm:provides>
      <rpm:entry name="provider-high"/>
      <rpm:entry name="virtual-provider"/>
    </rpm:provides>"#;
    let file_tool_format = r#"<rpm:provides><rpm:entry name="file-tool"/></rpm:provides>"#;
    let helper_high_format = r#"
    <rpm:provides><rpm:entry name="helper" flags="EQ" epoch="0" ver="2" rel="1.fc44"/></rpm:provides>
    <rpm:requires><rpm:entry name="leaf"/></rpm:requires>"#;
    let leaf_format = r#"<rpm:provides><rpm:entry name="leaf"/></rpm:provides>"#;
    let feature_format = r#"<rpm:provides><rpm:entry name="feature-b"/></rpm:provides>"#;
    let setup_format = r#"<rpm:provides><rpm:entry name="setup"/></rpm:provides>"#;
    let provider_low_format = r#"
    <rpm:provides>
      <rpm:entry name="provider-low"/>
      <rpm:entry name="virtual-provider"/>
    </rpm:provides>"#;
    let helper_low_format = r#"
    <rpm:provides><rpm:entry name="helper" flags="EQ" epoch="0" ver="9" rel="1.fc44"/></rpm:provides>"#;

    let mut app = PackageFixture::simple("app", &checksums[0]);
    app.format = app_format;
    let mut provider_high = PackageFixture::simple("provider-high", &checksums[1]);
    provider_high.format = provider_high_format;
    let mut file_tool = PackageFixture::simple("file-tool", &checksums[2]);
    file_tool.format = file_tool_format;
    file_tool.files = &["/usr/bin/file-tool"];
    let mut helper_high = PackageFixture::simple("helper", &checksums[3]);
    helper_high.version = "2";
    helper_high.format = helper_high_format;
    let mut leaf = PackageFixture::simple("leaf", &checksums[4]);
    leaf.format = leaf_format;
    let mut feature = PackageFixture::simple("feature-b", &checksums[5]);
    feature.format = feature_format;
    let mut setup = PackageFixture::simple("setup", &checksums[6]);
    setup.format = setup_format;
    let high_packages = [
        app,
        provider_high,
        file_tool,
        helper_high,
        leaf,
        feature,
        setup,
    ];
    let high = write_metadata(directory.path(), "fedora-high", &high_packages);

    let mut provider_low = PackageFixture::simple("provider-low", &checksums[7]);
    provider_low.format = provider_low_format;
    let mut helper_low = PackageFixture::simple("helper", &checksums[8]);
    helper_low.version = "9";
    helper_low.format = helper_low_format;
    let low_packages = [provider_low, helper_low];
    let low = write_metadata(directory.path(), "fedora-low", &low_packages);
    let metadata = vec![high, low];
    let snapshots = metadata
        .iter()
        .zip(["fedora-high", "fedora-low"])
        .map(|((primary, filelists), repository)| source_snapshot(repository, primary, filelists))
        .collect::<Vec<_>>();
    let mut profile = profile(&snapshots);
    profile.counts.packages = 9;
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();
    let resolution_output = directory.path().join("resolution-oracle");
    let manifest = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &resolution_output,
    )
    .unwrap();

    assert_eq!(manifest.artifact.counts.roots, 9);
    assert_eq!(manifest.artifact.counts.resolved_roots, 9);
    assert_eq!(manifest.artifact.counts.unresolved_roots, 0);
    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut packages = Vec::new();
    package_reader
        .for_each_package(|package| {
            packages.push(package);
            Ok(())
        })
        .unwrap();
    let package_key = |name: &str, version: Option<&str>| {
        packages
            .iter()
            .find(|package| {
                package.name == name && version.is_none_or(|version| package.version == version)
            })
            .unwrap()
            .package_key_sha256
            .clone()
    };
    let app_key = package_key("app", None);
    let resolution_reader =
        verify_native_resolution_oracle_bundle(&resolution_output, &profile, &package_reader)
            .unwrap();
    let mut app_outcome = None;
    resolution_reader
        .for_each_root(|root| {
            if root.root_package_key_sha256 == app_key {
                app_outcome = Some(root.outcome);
            }
            Ok(())
        })
        .unwrap();
    let NativeResolutionOutcomeV1::Resolved {
        closure_package_keys_sha256,
    } = app_outcome.unwrap()
    else {
        panic!("app must resolve");
    };
    let closure = closure_package_keys_sha256
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    for key in [
        package_key("app", None),
        package_key("provider-high", None),
        package_key("file-tool", None),
        package_key("helper", Some("2-1.fc44")),
        package_key("leaf", None),
        package_key("feature-b", None),
        package_key("setup", None),
    ] {
        assert!(
            closure.contains(&key),
            "missing expected closure package {key}"
        );
    }
    assert!(!closure.contains(&package_key("provider-low", None)));
    assert!(!closure.contains(&package_key("helper", Some("9-1.fc44"))));
    assert_eq!(closure.len(), 7, "weak requirements must not enter closure");
}

#[test]
fn resolution_producer_rejects_conflicts_architecture_and_input_drift() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="conflict-root"/></rpm:provides>
    <rpm:requires><rpm:entry name="blocker"/></rpm:requires>
    <rpm:conflicts><rpm:entry name="blocker"/></rpm:conflicts>"#;
    let blocker_format = r#"<rpm:provides><rpm:entry name="blocker"/></rpm:provides>"#;
    let mut root = PackageFixture::simple("conflict-root", &checksums[0]);
    root.format = root_format;
    let mut blocker = PackageFixture::simple("blocker", &checksums[1]);
    blocker.format = blocker_format;
    let packages = [root, blocker];
    let metadata = vec![write_metadata(directory.path(), "fedora-core", &packages)];
    let snapshots = vec![source_snapshot(
        "fedora-core",
        &metadata[0].0,
        &metadata[0].1,
    )];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 2;
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();

    let conflict = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &directory.path().join("conflict-resolution"),
    )
    .unwrap_err();
    assert!(matches!(conflict, Error::ConflictError(_)));
    assert!(conflict.to_string().contains("problem rule"));

    let architecture = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "aarch64",
        &directory.path().join("architecture-resolution"),
    )
    .unwrap_err();
    assert!(
        architecture
            .to_string()
            .contains("target architecture 'aarch64'"),
        "{architecture}"
    );

    let package_rows_path = package_output.join(NATIVE_PARITY_PACKAGE_FILE_NAME);
    let package_rows = fs::read(&package_rows_path).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&package_rows_path)
        .unwrap()
        .write_all(b"tampered")
        .unwrap();
    let package_drift = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &directory.path().join("package-drift-resolution"),
    )
    .unwrap_err();
    assert!(matches!(package_drift, Error::ChecksumMismatch { .. }));
    fs::write(&package_rows_path, package_rows).unwrap();

    fs::OpenOptions::new()
        .append(true)
        .open(&metadata[0].0)
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    let drift = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &directory.path().join("drift-resolution"),
    )
    .unwrap_err();
    assert!(matches!(drift, Error::ChecksumMismatch { .. }));
}

#[test]
fn resolution_producer_binds_missing_prerequisite_group_exactly() {
    let directory = tempfile::tempdir().unwrap();
    let checksum = digest('a');
    let prereq_format = r#"
    <rpm:provides><rpm:entry name="prereq-root"/></rpm:provides>
    <rpm:requires><rpm:entry name="missing-setup" pre="1"/></rpm:requires>"#;
    let mut root = PackageFixture::simple("prereq-root", &checksum);
    root.format = prereq_format;
    let metadata = vec![write_metadata(directory.path(), "fedora-core", &[root])];
    let snapshots = vec![source_snapshot(
        "fedora-core",
        &metadata[0].0,
        &metadata[0].1,
    )];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 1;
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();
    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut package = None;
    package_reader
        .for_each_package(|row| {
            package = Some(row);
            Ok(())
        })
        .unwrap();
    let package = package.unwrap();
    let group = package
        .requirement_groups
        .iter()
        .find(|group| group.kind == "pre_depends")
        .unwrap();
    let expected_group = native_requirement_group_sha256(group).unwrap();
    let resolution_output = directory.path().join("resolution-oracle");
    produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &resolution_output,
    )
    .unwrap();
    let resolution_reader =
        verify_native_resolution_oracle_bundle(&resolution_output, &profile, &package_reader)
            .unwrap();
    let mut outcome = None;
    resolution_reader
        .for_each_root(|root| {
            outcome = Some(root.outcome);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        outcome.unwrap(),
        NativeResolutionOutcomeV1::Unresolved {
            dependencies: vec![NativeUnresolvedDependencyV1 {
                requiring_package_key_sha256: package.package_key_sha256,
                requirement_group_sha256: expected_group,
            }]
        }
    );
}
