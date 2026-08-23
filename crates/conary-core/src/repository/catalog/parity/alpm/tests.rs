// crates/conary-core/src/repository/catalog/parity/alpm/tests.rs

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::{Compression, GzBuilder};

use super::*;
use crate::repository::catalog::{
    CatalogArtifactV1, CatalogCountsV1, NATIVE_PARITY_PACKAGE_FILE_NAME, NativeParityOracleWriter,
    NativeResolutionOutcomeV1, PROFILE_REVISION_SCHEMA_V2, ProfileSourceMemberV2,
    SOURCE_SNAPSHOT_SCHEMA_V1, SourceMetadataObjectV1, SourceProvenanceV1, SourceStreamKindV1,
    SourceStreamV1, native_requirement_group_sha256, verify_native_resolution_oracle_bundle,
    write_native_parity_oracle_manifest,
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

fn resolution_fixture_databases(
    directory: &Path,
    conflicting_closure: bool,
) -> (Vec<PathBuf>, Vec<SourceSnapshotV1>) {
    let core = directory.join("core-resolution.db");
    let extra = directory.join("extra-resolution.db");
    let digests = ('0'..='9').map(digest).collect::<Vec<_>>();

    let leaf = PackageFixture::new("leaf", &digests[0]);
    let mut middle = PackageFixture::new("middle", &digests[1]);
    middle.depends = &["leaf>=1"];
    let mut provider_core = PackageFixture::new("provider-core", &digests[2]);
    provider_core.provides = &["virtual-cap=1.0"];
    let mut root = PackageFixture::new("root", &digests[3]);
    root.depends = &["middle>=1", "virtual-cap>=1"];
    let mut broken = PackageFixture::new("broken", &digests[4]);
    broken.depends = &["missing>=1"];
    let mut broken_child = PackageFixture::new("broken-child", &digests[5]);
    broken_child.depends = &["transitive-missing>=2"];
    let mut broken_root = PackageFixture::new("broken-root", &digests[6]);
    broken_root.depends = &["broken-child"];
    let mut multi_v1 = PackageFixture::new("multi", &digests[7]);
    multi_v1.version = "1.0-1";
    if conflicting_closure {
        middle.conflicts = &["leaf"];
    }
    write_database(
        &core,
        &[
            leaf,
            middle,
            provider_core,
            root,
            broken,
            broken_child,
            broken_root,
            multi_v1,
        ],
    );

    let mut provider_extra = PackageFixture::new("provider-extra", &digests[8]);
    provider_extra.provides = &["virtual-cap=1.0"];
    let mut multi_v2 = PackageFixture::new("multi", &digests[9]);
    multi_v2.version = "2.0-1";
    write_database(&extra, &[provider_extra, multi_v2]);

    let snapshots = vec![
        source_snapshot("arch-core-x86_64", &core),
        source_snapshot("arch-extra-x86_64", &extra),
    ];
    (vec![core, extra], snapshots)
}

#[test]
fn resolution_producer_emits_exact_closure_precedence_versions_and_unresolved_groups() {
    let directory = tempfile::tempdir().unwrap();
    let (databases, snapshots) = resolution_fixture_databases(directory.path(), false);
    let mut profile = profile(&snapshots);
    profile.counts.packages = 10;
    let package_output = directory.path().join("package-oracle");
    produce_alpm_parity_oracle(&profile, &inputs(&snapshots, &databases), &package_output).unwrap();

    let resolution_output = directory.path().join("resolution-oracle");
    let manifest = produce_alpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &databases),
        &package_output,
        "x86_64",
        &resolution_output,
    )
    .unwrap();

    assert_eq!(manifest.implementation.name, "libalpm");
    assert_eq!(manifest.implementation.version, alpm::version());
    assert_eq!(
        manifest.implementation.projection_schema,
        ALPM_RESOLUTION_PROJECTION_SCHEMA_V1
    );
    assert_eq!(manifest.policy.architecture, "x86_64");
    assert_eq!(manifest.artifact.counts.roots, 10);
    assert_eq!(manifest.artifact.counts.unresolved_roots, 3);

    let package_reader = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();
    let mut package_by_name = std::collections::BTreeMap::<String, Vec<_>>::new();
    package_reader
        .for_each_package(|package| {
            package_by_name
                .entry(package.name.clone())
                .or_default()
                .push(package);
            Ok(())
        })
        .unwrap();
    let resolution_reader =
        verify_native_resolution_oracle_bundle(&resolution_output, &profile, &package_reader)
            .unwrap();
    let mut roots = std::collections::BTreeMap::new();
    resolution_reader
        .for_each_root(|root| {
            roots.insert(root.root_package_key_sha256.clone(), root);
            Ok(())
        })
        .unwrap();

    let root = &package_by_name["root"][0];
    let NativeResolutionOutcomeV1::Resolved {
        closure_package_keys_sha256,
    } = &roots[&root.package_key_sha256].outcome
    else {
        panic!("root must resolve");
    };
    for name in ["root", "middle", "leaf", "provider-core"] {
        assert!(
            closure_package_keys_sha256.contains(&package_by_name[name][0].package_key_sha256),
            "root closure omitted {name}"
        );
    }
    assert!(
        !closure_package_keys_sha256
            .contains(&package_by_name["provider-extra"][0].package_key_sha256),
        "libalpm must preserve profile database precedence for virtual providers"
    );

    let broken = &package_by_name["broken"][0];
    let NativeResolutionOutcomeV1::Unresolved { dependencies } =
        &roots[&broken.package_key_sha256].outcome
    else {
        panic!("broken must remain typed unresolved");
    };
    assert_eq!(dependencies.len(), 1);
    assert_eq!(
        dependencies[0].requiring_package_key_sha256,
        broken.package_key_sha256
    );
    let missing_group = broken
        .requirement_groups
        .iter()
        .find(|group| group.native_text.as_deref() == Some("missing>=1"))
        .unwrap();
    assert_eq!(
        dependencies[0].requirement_group_sha256,
        native_requirement_group_sha256(missing_group).unwrap()
    );

    let broken_root = &package_by_name["broken-root"][0];
    let broken_child = &package_by_name["broken-child"][0];
    let NativeResolutionOutcomeV1::Unresolved { dependencies } =
        &roots[&broken_root.package_key_sha256].outcome
    else {
        panic!("transitively broken root must remain typed unresolved");
    };
    assert_eq!(dependencies.len(), 2);
    let transitive_group = broken_child
        .requirement_groups
        .iter()
        .find(|group| group.native_text.as_deref() == Some("transitive-missing>=2"))
        .unwrap();
    let transitive_group_sha256 = native_requirement_group_sha256(transitive_group).unwrap();
    assert!(dependencies.iter().any(|dependency| {
        dependency.requiring_package_key_sha256 == broken_child.package_key_sha256
            && dependency.requirement_group_sha256 == transitive_group_sha256
    }));

    let mut multi = package_by_name.remove("multi").unwrap();
    multi.sort_by(|left, right| left.version.cmp(&right.version));
    assert_eq!(multi.len(), 2);
    for package in multi {
        let NativeResolutionOutcomeV1::Resolved {
            closure_package_keys_sha256,
        } = &roots[&package.package_key_sha256].outcome
        else {
            panic!("each exact multi version must resolve independently");
        };
        assert!(closure_package_keys_sha256.contains(&package.package_key_sha256));
    }
}

#[test]
fn resolution_producer_rejects_invalid_architecture_and_conflicting_closure() {
    let directory = tempfile::tempdir().unwrap();
    let (databases, snapshots) = resolution_fixture_databases(directory.path(), false);
    let mut wrong_profile = profile(&snapshots);
    wrong_profile.counts.packages = 10;
    let package_output = directory.path().join("package-oracle");
    produce_alpm_parity_oracle(
        &wrong_profile,
        &inputs(&snapshots, &databases),
        &package_output,
    )
    .unwrap();

    let error = produce_alpm_resolution_oracle(
        &wrong_profile,
        &inputs(&snapshots, &databases),
        &package_output,
        "aarch64",
        &directory.path().join("wrong-architecture"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("rejected architecture"));

    let conflict_directory = tempfile::tempdir().unwrap();
    let (databases, snapshots) = resolution_fixture_databases(conflict_directory.path(), true);
    let mut conflict_profile = profile(&snapshots);
    conflict_profile.counts.packages = 10;
    let package_output = conflict_directory.path().join("package-oracle");
    produce_alpm_parity_oracle(
        &conflict_profile,
        &inputs(&snapshots, &databases),
        &package_output,
    )
    .unwrap();
    let error = produce_alpm_resolution_oracle(
        &conflict_profile,
        &inputs(&snapshots, &databases),
        &package_output,
        "x86_64",
        &conflict_directory.path().join("resolution-oracle"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("package conflict"));
}

#[test]
fn resolution_producer_rejects_valid_but_non_native_package_oracle() {
    let directory = tempfile::tempdir().unwrap();
    let (databases, snapshots) = resolution_fixture_databases(directory.path(), false);
    let mut profile = profile(&snapshots);
    profile.counts.packages = 10;
    let package_output = directory.path().join("package-oracle");
    produce_alpm_parity_oracle(&profile, &inputs(&snapshots, &databases), &package_output).unwrap();
    let original = verify_native_parity_oracle_bundle(&package_output, &profile).unwrap();

    let drifted_output = directory.path().join("drifted-package-oracle");
    fs::create_dir(&drifted_output).unwrap();
    let mut writer = NativeParityOracleWriter::create(
        drifted_output.join(NATIVE_PARITY_PACKAGE_FILE_NAME),
        &profile,
        original.manifest().implementation.clone(),
    )
    .unwrap();
    let mut changed = false;
    original
        .for_each_package(|mut package| {
            if !changed {
                package.checksum = format!("sha256:{}", digest('9'));
                changed = true;
            }
            writer.package(&package)
        })
        .unwrap();
    let drifted = writer.finish().unwrap();
    write_native_parity_oracle_manifest(&drifted_output, &drifted).unwrap();
    verify_native_parity_oracle_bundle(&drifted_output, &profile).unwrap();

    let error = produce_alpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &databases),
        &drifted_output,
        "x86_64",
        &directory.path().join("resolution-oracle"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("fresh libalpm projection"));
}

#[test]
fn resolution_producer_rejects_ambiguous_transitive_requiring_identity() {
    let directory = tempfile::tempdir().unwrap();
    let core = directory.path().join("core-ambiguous.db");
    let extra = directory.path().join("extra-ambiguous.db");
    let checksum_a = digest('a');
    let checksum_b = digest('b');
    let checksum_c = digest('c');
    let mut multi_v1 = PackageFixture::new("multi", &checksum_a);
    multi_v1.version = "1.0-1";
    multi_v1.depends = &["missing>=1"];
    let mut root = PackageFixture::new("ambiguous-root", &checksum_b);
    root.depends = &["multi=1.0-1"];
    write_database(&core, &[multi_v1, root]);
    let mut multi_v2 = PackageFixture::new("multi", &checksum_c);
    multi_v2.version = "2.0-1";
    multi_v2.depends = &["missing>=1"];
    write_database(&extra, &[multi_v2]);
    let databases = vec![core, extra];
    let snapshots = vec![
        source_snapshot("arch-core-x86_64", &databases[0]),
        source_snapshot("arch-extra-x86_64", &databases[1]),
    ];
    let mut profile = profile(&snapshots);
    profile.counts.packages = 3;
    let package_output = directory.path().join("package-oracle");
    produce_alpm_parity_oracle(&profile, &inputs(&snapshots, &databases), &package_output).unwrap();

    let error = produce_alpm_resolution_oracle(
        &profile,
        &inputs(&snapshots, &databases),
        &package_output,
        "x86_64",
        &directory.path().join("resolution-oracle"),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("ambiguous across exact packages")
    );
}
