// crates/conary-core/src/repository/catalog/parity/rpm/tests.rs

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::{Compression, GzBuilder};

use super::ffi::SolvResolution;
use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, NativeResolutionNotInstallableReasonV1,
    NativeResolutionOutcomeV1, NativeResolutionSurveyNativeExplanationV1,
    NativeUnresolvedDependencyV1, PROFILE_REVISION_SCHEMA_V3, ProfileSourceMemberV2,
    SOURCE_SNAPSHOT_SCHEMA_V1, SourceProvenanceV1, SourceStreamKindV1, SourceStreamV1,
    native_requirement_group_sha256, verify_native_resolution_oracle_bundle,
};
use crate::repository::supported_profiles::ProfileSourceRole;
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};

mod split_provides;

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

#[derive(Clone, Copy)]
enum SplitProvideFileCoverage {
    Missing,
    Exact,
    Descendant,
    LexicalPrefix,
    ForeignDescendant,
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
        parser_projection_version: crate::repository::catalog::SOURCE_CATALOG_PROJECTION_VERSION_V2,
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
        schema_version: PROFILE_REVISION_SCHEMA_V3,
        profile: "fedora-44".to_string(),
        target_architecture:
            crate::repository::supported_profiles::ProfileTargetArchitecture::X86_64,
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
    split_provide_file_coverage: SplitProvideFileCoverage,
) -> (Vec<(PathBuf, PathBuf)>, Vec<SourceSnapshotV1>) {
    let alpha_checksum = digest('a');
    let shared_checksum = digest('b');
    let beta_checksum = digest('c');
    let conflict_checksum = digest('d');
    let alpha_format = r#"
    <rpm:provides>
      <rpm:entry name="alpha" flags="EQ" epoch="0" ver="1.2" rel="3.fc44"/>
      <rpm:entry name="virtual-alpha" flags="GE" epoch="0" ver="2" rel="1"/>
      <rpm:entry name="cmocka-test:/usr/lib/cmake/cmocka"/>
    </rpm:provides>
    <rpm:requires>
      <rpm:entry name="glibc" flags="GE" epoch="0" ver="2.40" rel="1.fc44"/>
      <rpm:entry name="(feature-a or feature-b)"/>
      <rpm:entry name="((python3.14dist(semver) >= 3.0.2 with python3.14dist(semver) &lt; 3.1) with python3.14dist(semver) >= 3.0.2)"/>
      <rpm:entry name="(python3.14dist(semver) >= 3.0.2 with python3.14dist(semver) &lt; 3.1 with python3.14dist(semver) >= 3.0.2)"/>
      <rpm:entry name="setup" pre="1"/>
    </rpm:requires>
    <rpm:recommends><rpm:entry name="recommended"/></rpm:recommends>
    <rpm:suggests>
      <rpm:entry name="suggested"/>
      <rpm:entry name="(nvidia-query-resource-opengl-libs(x86-32) = :1.0.0-23.fc44 if libGL(x86-32))"/>
    </rpm:suggests>
    <rpm:supplements><rpm:entry name="(alpha unless desktop)"/></rpm:supplements>
    <rpm:enhances><rpm:entry name="enhanced"/></rpm:enhances>
    <rpm:conflicts><rpm:entry name="old-alpha" flags="LT" epoch="0" ver="1" rel="0"/></rpm:conflicts>
    <rpm:obsoletes><rpm:entry name="older-alpha"/></rpm:obsoletes>
    <file>/usr/bin/alpha</file>"#;
    let mut alpha = PackageFixture::simple("alpha", &alpha_checksum);
    alpha.version = "1.2";
    alpha.release = "3.fc44";
    alpha.size = 123;
    alpha.format = alpha_format;
    alpha.files = match split_provide_file_coverage {
        SplitProvideFileCoverage::Missing | SplitProvideFileCoverage::ForeignDescendant => {
            &["/usr/bin/alpha", "/usr/share/alpha/data"]
        }
        SplitProvideFileCoverage::Exact => &[
            "/usr/bin/alpha",
            "/usr/lib/cmake/cmocka",
            "/usr/share/alpha/data",
        ],
        SplitProvideFileCoverage::Descendant => &[
            "/usr/bin/alpha",
            "/usr/lib/cmake/cmocka/cmocka-config.cmake",
            "/usr/share/alpha/data",
        ],
        SplitProvideFileCoverage::LexicalPrefix => &[
            "/usr/bin/alpha",
            "/usr/lib/cmake/cmockable/cmocka-config.cmake",
            "/usr/share/alpha/data",
        ],
    };
    let shared_core = PackageFixture::simple("shared", &shared_checksum);
    let core_packages = [alpha, shared_core.clone()];
    let core = write_metadata(directory, "fedora-core", &core_packages);

    let mut beta = PackageFixture::simple("beta", &beta_checksum);
    if matches!(
        split_provide_file_coverage,
        SplitProvideFileCoverage::ForeignDescendant
    ) {
        beta.files = &["/usr/lib/cmake/cmocka/cmocka-config.cmake"];
    }
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
    let (metadata, snapshots) = fixture(directory.path(), false, SplitProvideFileCoverage::Exact);
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
    assert!(alpha.provides.iter().any(|provide| {
        provide.capability == "cmocka-test:/usr/lib/cmake/cmocka" && provide.kind == "generic"
    }));
    assert!(alpha.provides.iter().any(|provide| {
        provide.capability == "/usr/lib/cmake/cmocka" && provide.kind == "file"
    }));
    assert_eq!(
        alpha
            .requirement_groups
            .iter()
            .filter(|group| group.kind == "supplements")
            .count(),
        1,
        "libsolv split-provides machinery must not become a source supplement"
    );
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
    let left_nested = alpha
        .requirement_groups
        .iter()
        .find(|group| {
            group.native_text.as_deref()
                == Some(
                    "((python3.14dist(semver) >= 3.0.2 with python3.14dist(semver) < 3.1) with python3.14dist(semver) >= 3.0.2)",
                )
        })
        .unwrap();
    let flat = alpha
        .requirement_groups
        .iter()
        .find(|group| {
            group.native_text.as_deref()
                == Some(
                    "(python3.14dist(semver) >= 3.0.2 with python3.14dist(semver) < 3.1 with python3.14dist(semver) >= 3.0.2)",
                )
        })
        .unwrap();
    assert_ne!(left_nested.expression_json, flat.expression_json);
    let empty_epoch = alpha
        .requirement_groups
        .iter()
        .find(|group| {
            group.native_text.as_deref()
                == Some(
                    "(nvidia-query-resource-opengl-libs(x86-32) = 1.0.0-23.fc44 if libGL(x86-32))",
                )
        })
        .unwrap();
    assert!(empty_epoch.expression_json.contains("= 1.0.0-23.fc44"));
    let shared = packages
        .iter()
        .find(|package| package.name == "shared")
        .unwrap();
    assert_eq!(shared.member_ordinal, 0);
    assert_eq!(shared.repository_identity, "fedora-core");
}

#[test]
fn canonical_rpm_text_preserves_left_nested_and_flat_with_trees() {
    let left_text = "((python3.14dist(semver) >= 3.0.2 with python3.14dist(semver) < 3.1) with python3.14dist(semver) >= 3.0.2)";
    let flat_text = "(python3.14dist(semver) >= 3.0.2 with python3.14dist(semver) < 3.1 with python3.14dist(semver) >= 3.0.2)";
    let left = crate::repository::rpm_dependency::parse_rpm_dependency(
        RepositoryRequirementKind::Depends,
        left_text,
    )
    .unwrap();
    let flat = crate::repository::rpm_dependency::parse_rpm_dependency(
        RepositoryRequirementKind::Depends,
        flat_text,
    )
    .unwrap();

    assert_ne!(left, flat);
    assert_eq!(canonical_rpm_dependency_text(&left), left_text);
    assert_eq!(canonical_rpm_dependency_text(&flat), flat_text);

    let error = project_decoded_requirement(
        RepositoryRequirementKind::Depends,
        flat_text.to_string(),
        left,
    )
    .unwrap_err();
    assert!(matches!(error, Error::ConflictError(_)));
    assert!(error.to_string().contains("typed relation tree disagrees"));
}

#[test]
fn native_rpm_evr_canonicalizes_source_empty_and_zero_epochs() {
    assert_eq!(
        canonical_rpm_evr(":1.0.0-23.fc44").unwrap(),
        "1.0.0-23.fc44"
    );
    assert_eq!(
        canonical_rpm_evr("0:1.0.0-23.fc44").unwrap(),
        "1.0.0-23.fc44"
    );
    assert_eq!(
        canonical_rpm_evr("2:1.0.0-23.fc44").unwrap(),
        "2:1.0.0-23.fc44"
    );
    assert!(canonical_rpm_evr(":1:1.0.0-23.fc44").is_err());
}

#[test]
fn producer_rejects_changed_authenticated_rpmmd_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let (metadata, snapshots) = fixture(directory.path(), false, SplitProvideFileCoverage::Exact);
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
    let (metadata, snapshots) = fixture(directory.path(), true, SplitProvideFileCoverage::Exact);
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
    let (metadata, mut snapshots) =
        fixture(directory.path(), false, SplitProvideFileCoverage::Exact);
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
    let (metadata, snapshots) = fixture(directory.path(), false, SplitProvideFileCoverage::Exact);
    let profile = profile(&snapshots);
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();
    let resolution_output = directory.path().join("resolution-oracle");
    resolution::reset_explanation_builds();

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
    assert_eq!(
        resolution::explanation_builds(),
        0,
        "resolved and typed-unresolved RPM roots must not build survey evidence"
    );
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
fn resolution_producer_projects_existing_name_with_only_wrong_version() {
    const SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP: i32 = 0x102;

    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="wrong-version-root" flags="EQ" epoch="0" ver="1.0" rel="1.fc44"/></rpm:provides>
    <rpm:requires><rpm:entry name="wrong-version-target" flags="EQ" epoch="0" ver="2.0" rel="1.fc44"/></rpm:requires>"#;
    let target_format = r#"
    <rpm:provides><rpm:entry name="wrong-version-target" flags="EQ" epoch="0" ver="3.0" rel="1.fc44"/></rpm:provides>"#;
    let mut root = PackageFixture::simple("wrong-version-root", &checksums[0]);
    root.format = root_format;
    let mut target = PackageFixture::simple("wrong-version-target", &checksums[1]);
    target.version = "3.0";
    target.format = target_format;
    let metadata = write_metadata(directory.path(), "fedora", &[root, target]);

    let mut pool = SolvPool::create().unwrap();
    pool.load("fedora", &metadata.0, &metadata.1, 0, 30_000)
        .unwrap();
    pool.set_architecture("x86_64").unwrap();
    let root_index = (0..pool.package_count())
        .find(|index| pool.package(*index).unwrap().name().unwrap() == "wrong-version-root")
        .unwrap();
    let SolvResolution::Unresolved(problems) = pool.solve(root_index).unwrap() else {
        panic!("wrong-version-root must be unresolved");
    };
    assert!(
        problems
            .iter()
            .flat_map(|problem| &problem.rules)
            .any(|rule| {
                rule.rule_type == SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP
                    && pool.dependency(rule.dependency).unwrap().text().unwrap()
                        == "wrong-version-target = 2.0-1.fc44"
            })
    );

    let snapshots = vec![source_snapshot("fedora", &metadata.0, &metadata.1)];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 2;
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(
        &profile,
        &inputs(&snapshots, &[metadata.clone()]),
        &package_output,
    )
    .unwrap();
    let resolution_output = directory.path().join("resolution-oracle");
    produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &[metadata]),
        &package_output,
        "x86_64",
        &resolution_output,
    )
    .unwrap();

    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut root = None;
    package_reader
        .for_each_package(|package| {
            if package.name == "wrong-version-root" {
                root = Some(package);
            }
            Ok(())
        })
        .unwrap();
    let root = root.unwrap();
    let group = root
        .requirement_groups
        .iter()
        .find(|group| group.native_text.as_deref() == Some("wrong-version-target = 2.0-1.fc44"))
        .unwrap();
    let expected = NativeResolutionOutcomeV1::Unresolved {
        dependencies: vec![NativeUnresolvedDependencyV1 {
            requiring_package_key_sha256: root.package_key_sha256.clone(),
            requirement_group_sha256: native_requirement_group_sha256(group).unwrap(),
        }],
    };
    let resolution_reader =
        verify_native_resolution_oracle_bundle(&resolution_output, &profile, &package_reader)
            .unwrap();
    let mut observed = None;
    resolution_reader
        .for_each_root(|candidate| {
            if candidate.root_package_key_sha256 == root.package_key_sha256 {
                observed = Some(candidate.outcome);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(observed, Some(expected));
}

#[test]
fn resolution_producer_projects_strict_priority_blocked_dependency() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b', 'c', 'd'].map(digest);
    let high_root_format = r#"
    <rpm:provides><rpm:entry name="strict-root" flags="EQ" epoch="0" ver="2.0" rel="1.fc44"/></rpm:provides>
    <rpm:requires><rpm:entry name="strict-runtime" flags="EQ" epoch="0" ver="2.0" rel="1.fc44"/></rpm:requires>"#;
    let high_runtime_format = r#"
    <rpm:provides><rpm:entry name="strict-runtime" flags="EQ" epoch="0" ver="2.0" rel="1.fc44"/></rpm:provides>"#;
    let low_root_format = r#"
    <rpm:provides><rpm:entry name="strict-root" flags="EQ" epoch="0" ver="1.0" rel="1.fc44"/></rpm:provides>
    <rpm:requires><rpm:entry name="strict-runtime" flags="EQ" epoch="0" ver="1.0" rel="1.fc44"/></rpm:requires>"#;
    let low_runtime_format = r#"
    <rpm:provides><rpm:entry name="strict-runtime" flags="EQ" epoch="0" ver="1.0" rel="1.fc44"/></rpm:provides>"#;

    let mut high_root = PackageFixture::simple("strict-root", &checksums[0]);
    high_root.version = "2.0";
    high_root.format = high_root_format;
    let mut high_runtime = PackageFixture::simple("strict-runtime", &checksums[1]);
    high_runtime.version = "2.0";
    high_runtime.format = high_runtime_format;
    let high = write_metadata(
        directory.path(),
        "fedora-updates",
        &[high_root, high_runtime],
    );

    let mut low_root = PackageFixture::simple("strict-root", &checksums[2]);
    low_root.format = low_root_format;
    let mut low_runtime = PackageFixture::simple("strict-runtime", &checksums[3]);
    low_runtime.format = low_runtime_format;
    let low = write_metadata(directory.path(), "fedora-base", &[low_root, low_runtime]);

    let metadata = vec![high, low];
    let snapshots = metadata
        .iter()
        .zip(["fedora-updates", "fedora-base"])
        .map(|((primary, filelists), repository)| source_snapshot(repository, primary, filelists))
        .collect::<Vec<_>>();
    let mut profile = profile(&snapshots);
    profile.counts.packages = 4;
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

    assert_eq!(manifest.implementation.projection_schema, 4);
    assert_eq!(manifest.artifact.counts.roots, 4);
    assert_eq!(manifest.artifact.counts.resolved_roots, 3);
    assert_eq!(manifest.artifact.counts.unresolved_roots, 1);

    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut low_root = None;
    package_reader
        .for_each_package(|package| {
            if package.name == "strict-root" && package.version == "1.0-1.fc44" {
                low_root = Some(package);
            }
            Ok(())
        })
        .unwrap();
    let low_root = low_root.unwrap();
    let blocked_group = low_root
        .requirement_groups
        .iter()
        .find(|group| group.native_text.as_deref() == Some("strict-runtime = 1.0-1.fc44"))
        .unwrap();
    let expected = NativeResolutionOutcomeV1::Unresolved {
        dependencies: vec![NativeUnresolvedDependencyV1 {
            requiring_package_key_sha256: low_root.package_key_sha256.clone(),
            requirement_group_sha256: native_requirement_group_sha256(blocked_group).unwrap(),
        }],
    };
    let resolution_reader =
        verify_native_resolution_oracle_bundle(&resolution_output, &profile, &package_reader)
            .unwrap();
    let mut observed = None;
    resolution_reader
        .for_each_root(|root| {
            if root.root_package_key_sha256 == low_root.package_key_sha256 {
                observed = Some(root.outcome);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(observed.unwrap(), expected);
}

#[test]
fn resolution_producer_excludes_strict_priority_multilib_root() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b', 'c', 'd', 'e', 'f'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="postgresql"/></rpm:provides>
    <rpm:requires>
      <rpm:entry name="strict-runtime" flags="EQ" epoch="0" ver="1" rel="1.fc44"/>
      <rpm:entry name="libpq.so.private18-5"/>
      <rpm:entry name="libpq.so.private18-5()(64bit)"/>
    </rpm:requires>"#;
    let strict_high_format = r#"
    <rpm:provides><rpm:entry name="strict-runtime" flags="EQ" epoch="0" ver="2" rel="1.fc44"/></rpm:provides>"#;
    let strict_low_format = r#"
    <rpm:provides><rpm:entry name="strict-runtime" flags="EQ" epoch="0" ver="1" rel="1.fc44"/></rpm:provides>
    <rpm:requires><rpm:entry name="shadowed-only-missing"/></rpm:requires>"#;
    let private_i686_format = r#"
    <rpm:provides>
      <rpm:entry name="postgresql-private-libs"/>
      <rpm:entry name="postgresql-private-libs-any"/>
      <rpm:entry name="libpq.so.private18-5"/>
    </rpm:provides>
    <rpm:conflicts><rpm:entry name="postgresql-private-libs-any"/></rpm:conflicts>"#;
    let private_x86_64_format = r#"
    <rpm:provides>
      <rpm:entry name="postgresql-private-libs"/>
      <rpm:entry name="postgresql-private-libs-any"/>
      <rpm:entry name="libpq.so.private18-5()(64bit)"/>
    </rpm:provides>
    <rpm:conflicts><rpm:entry name="postgresql-private-libs-any"/></rpm:conflicts>"#;

    let mut strict_high = PackageFixture::simple("strict-runtime", &checksums[0]);
    strict_high.version = "2";
    strict_high.format = strict_high_format;
    let mut updated_i686 = PackageFixture::simple("postgresql-private-libs", &checksums[1]);
    updated_i686.architecture = "i686";
    updated_i686.version = "18.4";
    updated_i686.format = private_i686_format;
    let mut updated_x86_64 = PackageFixture::simple("postgresql-private-libs", &checksums[2]);
    updated_x86_64.version = "18.4";
    updated_x86_64.format = private_x86_64_format;
    let updates = write_metadata(
        directory.path(),
        "fedora-updates",
        &[strict_high, updated_i686, updated_x86_64],
    );

    let mut root = PackageFixture::simple("postgresql", &checksums[3]);
    root.architecture = "i686";
    root.version = "18.3";
    root.format = root_format;
    let mut strict_low = PackageFixture::simple("strict-runtime", &checksums[4]);
    strict_low.format = strict_low_format;
    let mut private_low = PackageFixture::simple("postgresql-private-libs", &checksums[5]);
    private_low.architecture = "i686";
    private_low.version = "18.3";
    private_low.format = private_i686_format;
    let base = write_metadata(
        directory.path(),
        "fedora-base",
        &[root, strict_low, private_low],
    );

    let metadata = vec![updates, base];
    let snapshots = metadata
        .iter()
        .zip(["fedora-updates", "fedora-base"])
        .map(|((primary, filelists), repository)| source_snapshot(repository, primary, filelists))
        .collect::<Vec<_>>();
    let mut profile = profile(&snapshots);
    profile.counts.packages = 6;
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

    assert_eq!(manifest.implementation.projection_schema, 4);
    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut root = None;
    package_reader
        .for_each_package(|package| {
            if package.name == "postgresql" {
                root = Some(package);
            }
            Ok(())
        })
        .unwrap();
    let root = root.unwrap();
    let expected = NativeResolutionOutcomeV1::NotInstallable {
        reason: NativeResolutionNotInstallableReasonV1::ArchitectureExcluded,
    };
    let resolution_reader =
        verify_native_resolution_oracle_bundle(&resolution_output, &profile, &package_reader)
            .unwrap();
    let mut observed = None;
    resolution_reader
        .for_each_root(|candidate| {
            if candidate.root_package_key_sha256 == root.package_key_sha256 {
                observed = Some(candidate.outcome);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(observed.unwrap(), expected);
}

#[test]
fn resolution_producer_rejects_strict_provider_conflict_without_residual_probe() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b', 'c', 'd', 'e', 'f'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="shadow-chain-root"/></rpm:provides>
    <rpm:requires>
      <rpm:entry name="shared-shadow-runtime"/>
      <rpm:entry name="visible-shadow-blocker"/>
    </rpm:requires>"#;
    let high_shadow_format = r#"<rpm:provides><rpm:entry name="shadow-runtime"/></rpm:provides>"#;
    let visible_format = r#"
    <rpm:provides>
      <rpm:entry name="visible-shadow-runtime"/>
      <rpm:entry name="shared-shadow-runtime"/>
    </rpm:provides>
    <rpm:conflicts><rpm:entry name="visible-shadow-blocker"/></rpm:conflicts>"#;
    let helper_format = r#"
    <rpm:provides><rpm:entry name="shadow-chain-helper"/></rpm:provides>
    <rpm:requires><rpm:entry name="shadow-chain-terminal-missing"/></rpm:requires>"#;
    let low_shadow_format = r#"
    <rpm:provides>
      <rpm:entry name="shadow-runtime"/>
      <rpm:entry name="shared-shadow-runtime"/>
    </rpm:provides>
    <rpm:requires><rpm:entry name="shadow-chain-helper"/></rpm:requires>"#;

    let mut high_shadow = PackageFixture::simple("shadow-runtime", &checksums[0]);
    high_shadow.version = "2";
    high_shadow.format = high_shadow_format;
    let mut visible = PackageFixture::simple("visible-shadow-runtime", &checksums[1]);
    visible.format = visible_format;
    let mut helper = PackageFixture::simple("shadow-chain-helper", &checksums[2]);
    helper.format = helper_format;
    let blocker = PackageFixture::simple("visible-shadow-blocker", &checksums[3]);
    let updates = write_metadata(
        directory.path(),
        "fedora-updates",
        &[high_shadow, visible, helper, blocker],
    );

    let mut root = PackageFixture::simple("shadow-chain-root", &checksums[4]);
    root.format = root_format;
    let mut low_shadow = PackageFixture::simple("shadow-runtime", &checksums[5]);
    low_shadow.format = low_shadow_format;
    let base = write_metadata(directory.path(), "fedora-base", &[root, low_shadow]);

    let metadata = vec![updates, base];
    let snapshots = metadata
        .iter()
        .zip(["fedora-updates", "fedora-base"])
        .map(|((primary, filelists), repository)| source_snapshot(repository, primary, filelists))
        .collect::<Vec<_>>();
    let mut profile = profile(&snapshots);
    profile.counts.packages = 6;
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();

    let survey = produce_rpm_resolution_survey(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &directory
            .path()
            .join("strict-residual-conflict-survey.json"),
    )
    .unwrap();
    let failure = survey
        .failures
        .iter()
        .find(|failure| failure.name == "shadow-chain-root")
        .unwrap();
    let NativeResolutionSurveyNativeExplanationV1::Rpm { problems } = &failure.native_explanation
    else {
        panic!("RPM survey failure must carry libsolv problems");
    };
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0]
            .rules
            .iter()
            .any(|rule| rule.rule_type_numeric == 0xd00)
    );
    assert!(
        problems[0]
            .rules
            .iter()
            .any(|rule| rule.rule_type_numeric == 0x105)
    );
    let conflict = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &directory.path().join("strict-residual-conflict-resolution"),
    )
    .unwrap_err();
    assert!(matches!(conflict, Error::ConflictError(_)));
    assert!(conflict.to_string().contains("problem rule 0x105"));
}

#[test]
fn resolution_producer_rejects_root_conflict_inside_strict_priority_problem() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b', 'c', 'd'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="strict-conflict-root"/></rpm:provides>
    <rpm:requires><rpm:entry name="shared-strict-conflict-runtime"/></rpm:requires>
    <rpm:conflicts><rpm:entry name="visible-strict-conflict-runtime"/></rpm:conflicts>"#;
    let high_shadow_format =
        r#"<rpm:provides><rpm:entry name="shadowed-strict-conflict-runtime"/></rpm:provides>"#;
    let visible_format = r#"
    <rpm:provides>
      <rpm:entry name="visible-strict-conflict-runtime"/>
      <rpm:entry name="shared-strict-conflict-runtime"/>
    </rpm:provides>"#;
    let low_shadow_format = r#"
    <rpm:provides>
      <rpm:entry name="shadowed-strict-conflict-runtime"/>
      <rpm:entry name="shared-strict-conflict-runtime"/>
    </rpm:provides>"#;

    let mut high_shadow = PackageFixture::simple("shadowed-strict-conflict-runtime", &checksums[0]);
    high_shadow.version = "2";
    high_shadow.format = high_shadow_format;
    let mut visible = PackageFixture::simple("visible-strict-conflict-runtime", &checksums[1]);
    visible.format = visible_format;
    let updates = write_metadata(directory.path(), "fedora-updates", &[high_shadow, visible]);

    let mut root = PackageFixture::simple("strict-conflict-root", &checksums[2]);
    root.format = root_format;
    let mut low_shadow = PackageFixture::simple("shadowed-strict-conflict-runtime", &checksums[3]);
    low_shadow.format = low_shadow_format;
    let base = write_metadata(directory.path(), "fedora-base", &[root, low_shadow]);

    let metadata = vec![updates, base];
    let snapshots = metadata
        .iter()
        .zip(["fedora-updates", "fedora-base"])
        .map(|((primary, filelists), repository)| source_snapshot(repository, primary, filelists))
        .collect::<Vec<_>>();
    let mut profile = profile(&snapshots);
    profile.counts.packages = 4;
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();

    let conflict = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &directory.path().join("strict-root-conflict-resolution"),
    )
    .unwrap_err();
    assert!(matches!(conflict, Error::ConflictError(_)));
    assert!(conflict.to_string().contains("problem rule 0x105"));
}

#[test]
fn resolution_producer_projects_rich_required_helper_terminal_missing_edge() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="chain-root"/></rpm:provides>
    <rpm:requires><rpm:entry name="(chain-helper or unavailable-chain-alternative)"/></rpm:requires>"#;
    let helper_format = r#"
    <rpm:provides><rpm:entry name="chain-helper"/></rpm:provides>
    <rpm:requires><rpm:entry name="absent-capability"/></rpm:requires>"#;
    let mut root = PackageFixture::simple("chain-root", &checksums[0]);
    root.format = root_format;
    let mut helper = PackageFixture::simple("chain-helper", &checksums[1]);
    helper.format = helper_format;
    let metadata = vec![write_metadata(
        directory.path(),
        "fedora-base",
        &[root, helper],
    )];
    let snapshots = vec![source_snapshot(
        "fedora-base",
        &metadata[0].0,
        &metadata[0].1,
    )];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 2;

    let observed = resolve_named_root(&directory, &profile, &snapshots, &metadata, "chain-root");
    let helper = observed.package("chain-helper");
    let terminal_group = helper
        .requirement_groups
        .iter()
        .find(|group| group.native_text.as_deref() == Some("absent-capability"))
        .unwrap();
    assert_eq!(
        observed.outcome,
        NativeResolutionOutcomeV1::Unresolved {
            dependencies: vec![NativeUnresolvedDependencyV1 {
                requiring_package_key_sha256: helper.package_key_sha256.clone(),
                requirement_group_sha256: native_requirement_group_sha256(terminal_group).unwrap(),
            }],
        }
    );
}

#[test]
fn resolution_producer_projects_reachable_helper_terminal_missing_file_edge() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="file-chain-root"/></rpm:provides>
    <rpm:requires><rpm:entry name="/usr/libexec/file-chain-helper"/></rpm:requires>"#;
    let helper_format = r#"
    <rpm:provides><rpm:entry name="file-chain-helper"/></rpm:provides>
    <rpm:requires><rpm:entry name="/usr/libexec/absent-chain-runtime"/></rpm:requires>"#;
    let mut root = PackageFixture::simple("file-chain-root", &checksums[0]);
    root.format = root_format;
    let mut helper = PackageFixture::simple("file-chain-helper", &checksums[1]);
    helper.format = helper_format;
    helper.files = &["/usr/libexec/file-chain-helper"];
    let metadata = vec![write_metadata(
        directory.path(),
        "fedora-base",
        &[root, helper],
    )];
    let snapshots = vec![source_snapshot(
        "fedora-base",
        &metadata[0].0,
        &metadata[0].1,
    )];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 2;

    let observed = resolve_named_root(
        &directory,
        &profile,
        &snapshots,
        &metadata,
        "file-chain-root",
    );
    let helper = observed.package("file-chain-helper");
    let terminal_group = helper
        .requirement_groups
        .iter()
        .find(|group| group.native_text.as_deref() == Some("/usr/libexec/absent-chain-runtime"))
        .unwrap();
    assert_eq!(
        observed.outcome,
        NativeResolutionOutcomeV1::Unresolved {
            dependencies: vec![NativeUnresolvedDependencyV1 {
                requiring_package_key_sha256: helper.package_key_sha256.clone(),
                requirement_group_sha256: native_requirement_group_sha256(terminal_group).unwrap(),
            }],
        }
    );
}

#[test]
fn resolution_producer_prefers_visible_provider_edge_over_shadowed_provider() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b', 'c'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="mixed-root"/></rpm:provides>
    <rpm:requires><rpm:entry name="mixed-runtime" flags="GE" epoch="0" ver="1" rel="1.fc44"/></rpm:requires>"#;
    let visible_runtime_format = r#"
    <rpm:provides><rpm:entry name="mixed-runtime" flags="EQ" epoch="0" ver="1" rel="2.fc44"/></rpm:provides>
    <rpm:requires><rpm:entry name="absent-capability"/></rpm:requires>"#;
    let shadowed_runtime_format = r#"
    <rpm:provides><rpm:entry name="mixed-runtime" flags="EQ" epoch="0" ver="1" rel="1.fc44"/></rpm:provides>"#;

    let mut visible_runtime = PackageFixture::simple("mixed-runtime", &checksums[0]);
    visible_runtime.release = "2.fc44";
    visible_runtime.format = visible_runtime_format;
    let updates = write_metadata(directory.path(), "fedora-updates", &[visible_runtime]);

    let mut root = PackageFixture::simple("mixed-root", &checksums[1]);
    root.format = root_format;
    let mut shadowed_runtime = PackageFixture::simple("mixed-runtime", &checksums[2]);
    shadowed_runtime.format = shadowed_runtime_format;
    let base = write_metadata(directory.path(), "fedora-base", &[root, shadowed_runtime]);

    let metadata = vec![updates, base];
    let snapshots = metadata
        .iter()
        .zip(["fedora-updates", "fedora-base"])
        .map(|((primary, filelists), repository)| source_snapshot(repository, primary, filelists))
        .collect::<Vec<_>>();
    let mut profile = profile(&snapshots);
    profile.counts.packages = 3;

    let observed = resolve_named_root(&directory, &profile, &snapshots, &metadata, "mixed-root");
    let visible_runtime = observed.package_with_checksum("mixed-runtime", &checksums[0]);
    let terminal_group = visible_runtime
        .requirement_groups
        .iter()
        .find(|group| group.native_text.as_deref() == Some("absent-capability"))
        .unwrap();
    assert_eq!(
        observed.outcome,
        NativeResolutionOutcomeV1::Unresolved {
            dependencies: vec![NativeUnresolvedDependencyV1 {
                requiring_package_key_sha256: visible_runtime.package_key_sha256.clone(),
                requirement_group_sha256: native_requirement_group_sha256(terminal_group).unwrap(),
            }],
        }
    );
}

struct ObservedRoot {
    packages: Vec<crate::repository::catalog::parity::NativeParityPackageV1>,
    outcome: NativeResolutionOutcomeV1,
}

impl ObservedRoot {
    fn package(&self, name: &str) -> &crate::repository::catalog::parity::NativeParityPackageV1 {
        let mut matches = self.packages.iter().filter(|package| package.name == name);
        let package = matches.next().unwrap();
        assert!(matches.next().is_none(), "package name {name} is ambiguous");
        package
    }

    fn package_with_checksum(
        &self,
        name: &str,
        checksum: &str,
    ) -> &crate::repository::catalog::parity::NativeParityPackageV1 {
        let checksum = format!("sha256:{checksum}");
        self.packages
            .iter()
            .find(|package| package.name == name && package.checksum == checksum)
            .unwrap()
    }
}

/// Produce both oracles under schema 4 and return the named root's outcome
/// with every projected package.
fn resolve_named_root(
    directory: &tempfile::TempDir,
    profile: &ProfileRevisionV2,
    snapshots: &[SourceSnapshotV1],
    metadata: &[(PathBuf, PathBuf)],
    root_name: &str,
) -> ObservedRoot {
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(profile, &inputs(snapshots, metadata), &package_output).unwrap();
    let resolution_output = directory.path().join("resolution-oracle");
    let manifest = produce_rpm_resolution_oracle(
        profile,
        &inputs(snapshots, metadata),
        &package_output,
        "x86_64",
        &resolution_output,
    )
    .unwrap();
    assert_eq!(manifest.implementation.projection_schema, 4);
    let package_reader = verify_native_parity_oracle_bundle(&package_output, profile).unwrap();
    let mut packages = Vec::new();
    package_reader
        .for_each_package(|package| {
            packages.push(package);
            Ok(())
        })
        .unwrap();
    let root_key = packages
        .iter()
        .find(|package| package.name == root_name)
        .unwrap()
        .package_key_sha256
        .clone();
    let resolution_reader =
        verify_native_resolution_oracle_bundle(&resolution_output, profile, &package_reader)
            .unwrap();
    let mut outcome = None;
    resolution_reader
        .for_each_root(|candidate| {
            if candidate.root_package_key_sha256 == root_key {
                outcome = Some(candidate.outcome);
            }
            Ok(())
        })
        .unwrap();
    ObservedRoot {
        packages,
        outcome: outcome.unwrap(),
    }
}

#[test]
fn resolution_producer_excludes_cross_machine_provider_from_provider_index() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b', 'c'].map(digest);
    let root_format = r#"
    <rpm:provides><rpm:entry name="inferior-provider-root"/></rpm:provides>
    <rpm:requires><rpm:entry name="inferior-only-capability"/></rpm:requires>"#;
    let inferior_format = r#"
    <rpm:provides>
      <rpm:entry name="inferior-provider"/>
      <rpm:entry name="inferior-only-capability"/>
    </rpm:provides>"#;
    let preferred_format = r#"
    <rpm:provides><rpm:entry name="inferior-provider"/></rpm:provides>
    <rpm:requires><rpm:entry name="missing-preferred-architecture-runtime"/></rpm:requires>"#;

    let mut root = PackageFixture::simple("inferior-provider-root", &checksums[0]);
    root.format = root_format;
    let mut inferior = PackageFixture::simple("inferior-provider", &checksums[1]);
    inferior.architecture = "i686";
    inferior.format = inferior_format;
    let mut preferred = PackageFixture::simple("inferior-provider", &checksums[2]);
    preferred.format = preferred_format;
    let metadata = vec![write_metadata(
        directory.path(),
        "fedora-base",
        &[root, inferior, preferred],
    )];
    let snapshots = vec![source_snapshot(
        "fedora-base",
        &metadata[0].0,
        &metadata[0].1,
    )];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 3;
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();

    let manifest = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &directory.path().join("inferior-provider-resolution"),
    )
    .unwrap();
    assert_eq!(manifest.artifact.counts.not_installable_roots, 1);
    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut root = None;
    package_reader
        .for_each_package(|package| {
            if package.name == "inferior-provider-root" {
                root = Some(package);
            }
            Ok(())
        })
        .unwrap();
    let root = root.unwrap();
    let missing = root.requirement_groups.first().unwrap();
    let resolution_reader = verify_native_resolution_oracle_bundle(
        directory.path().join("inferior-provider-resolution"),
        &profile,
        &package_reader,
    )
    .unwrap();
    let mut outcome = None;
    resolution_reader
        .for_each_root(|candidate| {
            if candidate.root_package_key_sha256 == root.package_key_sha256 {
                outcome = Some(candidate.outcome);
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        outcome.unwrap(),
        NativeResolutionOutcomeV1::Unresolved {
            dependencies: vec![NativeUnresolvedDependencyV1 {
                requiring_package_key_sha256: root.package_key_sha256.clone(),
                requirement_group_sha256: native_requirement_group_sha256(missing).unwrap(),
            }],
        }
    );
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

    let mismatched_output = directory.path().join("architecture-resolution");
    let architecture = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "aarch64",
        &mismatched_output,
    )
    .unwrap_err();
    assert!(matches!(
        architecture,
        Error::ProfileArchitectureMismatch { .. }
    ));
    assert!(!mismatched_output.exists());

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
fn resolution_survey_records_all_failures_rules_and_later_healthy_roots() {
    let directory = tempfile::tempdir().unwrap();
    let checksums = ['a', 'b', 'c', 'd'].map(digest);
    let mut conflict = PackageFixture::simple("conflict-root", &checksums[0]);
    conflict.format = r#"
    <rpm:provides><rpm:entry name="conflict-root"/></rpm:provides>
    <rpm:requires><rpm:entry name="blocker"/></rpm:requires>
    <rpm:conflicts><rpm:entry name="blocker"/></rpm:conflicts>"#;
    let mut blocker = PackageFixture::simple("blocker", &checksums[1]);
    blocker.format = r#"<rpm:provides><rpm:entry name="blocker"/></rpm:provides>"#;
    let mut foreign = PackageFixture::simple("foreign-root", &checksums[2]);
    foreign.architecture = "aarch64";
    let healthy = PackageFixture::simple("healthy-after-failures", &checksums[3]);
    let packages = [conflict, blocker, foreign, healthy];
    let metadata = vec![write_metadata(directory.path(), "fedora-core", &packages)];
    let snapshots = vec![source_snapshot(
        "fedora-core",
        &metadata[0].0,
        &metadata[0].1,
    )];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 4;
    let package_output = directory.path().join("package-oracle");
    produce_rpm_parity_oracle(&profile, &inputs(&snapshots, &metadata), &package_output).unwrap();

    let survey_path = directory.path().join("survey.json");
    let survey = produce_rpm_resolution_survey(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &survey_path,
    )
    .unwrap();

    assert_eq!(survey.counts.roots_walked, 4);
    assert_eq!(survey.counts.resolved_roots, 2);
    assert_eq!(survey.counts.unresolved_roots, 0);
    assert_eq!(survey.counts.not_installable_roots, 1);
    assert_eq!(survey.counts.failed_roots, 1);
    assert_eq!(survey.total_failures, 1);
    assert!(!survey.truncated);
    assert_eq!(survey.failures.len(), 1);
    let conflict_failure = survey
        .failures
        .iter()
        .find(|failure| failure.name == "conflict-root")
        .unwrap();
    let NativeResolutionSurveyNativeExplanationV1::Rpm { problems } =
        &conflict_failure.native_explanation
    else {
        panic!("RPM survey failure must carry libsolv problems");
    };
    assert!(!problems.is_empty());
    let rules = problems
        .iter()
        .flat_map(|problem| &problem.rules)
        .collect::<Vec<_>>();
    assert!(rules.iter().any(|rule| {
        rule.rule_type_numeric == 0x105
            && rule.rule_type_symbolic == "SOLVER_RULE_PKG_CONFLICTS"
            && rule.from.as_ref().is_some_and(|package| {
                package.name == "conflict-root"
                    && package.evr == "1.0-1.fc44"
                    && package.architecture == "x86_64"
            })
            && rule
                .to
                .as_ref()
                .is_some_and(|package| package.name == "blocker")
            && rule.dependency.as_deref() == Some("blocker")
    }));
    let job_rule = rules
        .iter()
        .find(|rule| rule.rule_type_numeric == 0x400)
        .expect("RPM survey explanation must retain the exact-root JOB rule");
    assert_eq!(job_rule.rule_type_symbolic, "SOLVER_RULE_JOB");
    assert_eq!(job_rule.dependency_id, None);
    assert_eq!(job_rule.dependency, None);
    assert_eq!(
        job_rule.dependency_unavailable_reason.as_deref(),
        Some("solver_rule_job_dep_is_job_index")
    );
    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut root_order = Vec::new();
    package_reader
        .for_each_package(|root| {
            root_order.push(root.name);
            Ok(())
        })
        .unwrap();
    let failure_names = survey
        .failures
        .iter()
        .map(|failure| failure.name.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(root_order.iter().enumerate().any(|(index, name)| {
        failure_names.contains(name)
            && root_order[index + 1..]
                .iter()
                .any(|later| !failure_names.contains(later))
    }));
    assert!(!directory.path().join("manifest.json").exists());
    assert!(!directory.path().join("roots.jsonl").exists());

    let strict = produce_rpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &metadata),
        &package_output,
        "x86_64",
        &directory.path().join("strict-resolution"),
    )
    .unwrap_err();
    assert_eq!(strict.to_string(), survey.failures[0].error_message);
    assert!(strict.to_string().contains("problem rule 0x105"));
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
