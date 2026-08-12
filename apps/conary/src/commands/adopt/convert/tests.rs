// apps/conary/src/commands/adopt/convert/tests.rs

use super::*;
use conary_core::db::models::{
    NativeSourceEcosystem, NativeSourceStream, PackagePayloadEntry, RepositoryPolicyScope,
    RepositorySourcePolicy, RepositoryUpdateMode, Trove, TroveType,
};
use conary_core::packages::traits::{ExtractedFile, PackageFile};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadTimestamp,
};
use conary_core::repository::dependency_model::RepositoryRequirementGroup;
use conary_core::repository::versioning::VersionScheme;
use conary_core::repository::{
    ArchKeyringFormat, ArchKeyringTrust, ArchSigLevel, OpenPgpTrustRoot, RepositoryParserConfig,
    RepositoryTrustPolicy, RpmMetadataAuthority,
};
use std::collections::BTreeMap;

#[path = "tests/acquisition.rs"]
mod acquisition;
#[path = "tests/lifecycle.rs"]
mod lifecycle;

#[derive(Clone)]
struct ExactPackage {
    name: String,
    version: String,
    architecture: String,
    scheme: VersionScheme,
    files: Vec<PackageFile>,
    extracted: Vec<ExtractedFile>,
}

impl PackageFormat for ExactPackage {
    fn parse(_path: &str) -> conary_core::Result<Self> {
        unreachable!("the exact package test double is constructed directly")
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn version_scheme(&self) -> VersionScheme {
        self.scheme
    }

    fn architecture(&self) -> Option<&str> {
        Some(&self.architecture)
    }

    fn description(&self) -> Option<&str> {
        None
    }

    fn files(&self) -> &[PackageFile] {
        &self.files
    }

    fn requirements(&self) -> &[RepositoryRequirementGroup] {
        &[]
    }

    fn package_payload(&self) -> conary_core::Result<conary_core::packages::PackagePayload> {
        conary_core::packages::PackagePayload::from_extracted_in_memory(self.extracted.clone())
    }

    fn to_trove(&self) -> Trove {
        Trove::new(
            self.name.clone(),
            self.version.clone(),
            TroveType::Package,
            self.scheme,
        )
    }
}

fn node(kind: PayloadNodeKind, permissions: u32) -> PayloadNode {
    let node_type = match kind {
        PayloadNodeKind::Directory => libc::S_IFDIR,
        PayloadNodeKind::Symlink { .. } => libc::S_IFLNK,
        _ => libc::S_IFREG,
    };
    PayloadNode {
        kind,
        mode: node_type | permissions,
        user: PayloadIdentity::Numeric { id: 0 },
        group: PayloadIdentity::Numeric { id: 0 },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    }
}

fn package_for(scheme: VersionScheme) -> ExactPackage {
    let content = b"exact payload".to_vec();
    let authority = PayloadContentAuthority {
        sha256: conary_core::hash::sha256(&content),
        size: content.len() as u64,
    };
    let nodes = vec![
        (
            "/usr/lib/exact".to_string(),
            node(PayloadNodeKind::Directory, 0o755),
            None,
            Vec::new(),
        ),
        (
            "/usr/lib/exact/tool".to_string(),
            node(
                PayloadNodeKind::Regular {
                    hardlink_identity: Some("source-group".to_string()),
                },
                0o755,
            ),
            Some(authority.clone()),
            content,
        ),
        (
            "/usr/lib/exact/tool-link".to_string(),
            node(
                PayloadNodeKind::Hardlink {
                    target: "/usr/lib/exact/tool".to_string(),
                    identity: "source-group".to_string(),
                },
                0o755,
            ),
            None,
            Vec::new(),
        ),
        (
            "/usr/bin/exact".to_string(),
            node(
                PayloadNodeKind::Symlink {
                    target: "../lib/exact/tool".to_string(),
                },
                0o777,
            ),
            None,
            Vec::new(),
        ),
    ];
    ExactPackage {
        name: "exact".to_string(),
        version: "1.2.3-4".to_string(),
        architecture: "x86_64".to_string(),
        scheme,
        files: nodes
            .iter()
            .map(|(path, node, authority, _)| PackageFile {
                path: path.clone(),
                node: node.clone(),
                content: authority.clone(),
            })
            .collect(),
        extracted: nodes
            .into_iter()
            .map(|(path, node, authority, content)| ExtractedFile {
                path,
                node,
                content,
                content_authority: authority,
            })
            .collect(),
    }
}

fn installed_entries(
    package: &ExactPackage,
    format: PackageFormatType,
) -> Vec<PackagePayloadEntry> {
    let payload = package.package_payload().unwrap();
    let resolved = resolve_native_payload_nodes(
        Path::new("/"),
        payload.files().iter().map(|file| file.node.clone()),
        format,
    )
    .unwrap();
    payload
        .files()
        .iter()
        .zip(resolved)
        .map(|(file, node)| PackagePayloadEntry {
            path: file.path.clone(),
            node,
            content: file.content_authority.clone(),
            trove_id: 7,
            installed_at: None,
            component_id: None,
            materialized_file_id: None,
        })
        .collect()
}

fn identity_for(scheme: VersionScheme) -> InstalledPackageIdentity {
    match scheme {
        VersionScheme::Rpm => InstalledPackageIdentity::rpm(
            "exact-1.2.3-4.x86_64",
            "exact",
            None,
            "1.2.3",
            "4",
            "x86_64",
        )
        .unwrap(),
        VersionScheme::Debian => InstalledPackageIdentity::dpkg(
            "exact:x86_64",
            "exact",
            "1.2.3-4",
            "x86_64",
            conary_core::repository::dependency_model::DebianMultiArch::No,
        )
        .unwrap(),
        VersionScheme::Arch => {
            InstalledPackageIdentity::pacman("exact", "exact", "1.2.3-4", "x86_64").unwrap()
        }
        other => panic!("unsupported native test scheme {other:?}"),
    }
}

fn format_for(scheme: VersionScheme) -> PackageFormatType {
    match scheme {
        VersionScheme::Rpm => PackageFormatType::Rpm,
        VersionScheme::Debian => PackageFormatType::Deb,
        VersionScheme::Arch => PackageFormatType::Arch,
        other => panic!("unsupported native test scheme {other:?}"),
    }
}

fn insert_repository_package(
    conn: &rusqlite::Connection,
    scheme: VersionScheme,
    name: &str,
    priority: i32,
) {
    let ecosystem = match scheme {
        VersionScheme::Rpm => NativeSourceEcosystem::Rpm,
        VersionScheme::Debian => NativeSourceEcosystem::Deb,
        VersionScheme::Arch => NativeSourceEcosystem::Alpm,
        other => panic!("unsupported native test scheme {other:?}"),
    };
    let root = OpenPgpTrustRoot {
        url: format!("https://keys.example.test/{name}.gpg"),
        fingerprint: "A".repeat(40),
    };
    let mut repository = Repository::new(
        name.to_string(),
        format!("https://packages.example.test/{name}"),
    );
    repository.priority = priority;
    match ecosystem {
        NativeSourceEcosystem::Rpm => {
            repository
                .set_parser_config(RepositoryParserConfig::Rpm {
                    architecture: "x86_64".to_string(),
                })
                .unwrap();
            repository
                .set_trust_policy(RepositoryTrustPolicy::Rpm {
                    metadata: RpmMetadataAuthority::OpenPgp {
                        keys: vec![root.clone()],
                    },
                    package_keys: vec![root],
                })
                .unwrap();
        }
        NativeSourceEcosystem::Deb => {
            repository
                .set_parser_config(RepositoryParserConfig::Deb {
                    distribution: "stable".to_string(),
                    component: "main".to_string(),
                    architecture: "x86_64".to_string(),
                })
                .unwrap();
            repository
                .set_trust_policy(RepositoryTrustPolicy::Debian {
                    release_keys: vec![root],
                })
                .unwrap();
        }
        NativeSourceEcosystem::Alpm => {
            repository
                .set_parser_config(RepositoryParserConfig::Arch {
                    database: "core".to_string(),
                })
                .unwrap();
            repository
                .set_trust_policy(RepositoryTrustPolicy::Arch {
                    keyring: ArchKeyringTrust {
                        url: "https://keys.example.test/archlinux-keyring.pkg.tar.zst".to_string(),
                        format: ArchKeyringFormat::AlpmPackageZstd,
                        master_fingerprints: vec!["A".repeat(40)],
                        packager_key_threshold: 1,
                    },
                    sig_level: ArchSigLevel::distribution_default(),
                })
                .unwrap();
        }
    }
    let repository_identity = format!("test:{name}");
    let policy = RepositorySourcePolicy::new(
        "test-source",
        RepositoryPolicyScope::repository(repository_identity.clone()).unwrap(),
        ecosystem,
        NativeSourceStream::channel("stable").unwrap(),
        RepositoryUpdateMode::Follow,
    )
    .unwrap();
    repository
        .set_native_source_policy(policy, repository_identity, None)
        .unwrap();
    let repository_id = repository.insert(conn).unwrap();
    let mut package = RepositoryPackage::new(
        repository_id,
        "exact".to_string(),
        "1.2.3-4".to_string(),
        scheme,
        "a".repeat(64),
        1,
        format!("https://packages.example.test/{name}/exact.pkg"),
    );
    package.architecture = Some("x86_64".to_string());
    package.insert(conn).unwrap();
}

#[test]
fn payload_paths_are_normalized_without_accepting_dot_segments() {
    assert_eq!(
        canonical_payload_path("usr/bin/tool").unwrap(),
        "/usr/bin/tool"
    );
    assert_eq!(
        canonical_payload_path("/etc/tool.conf").unwrap(),
        "/etc/tool.conf"
    );
    assert!(canonical_payload_path("usr/../bin/tool").is_err());
    assert!(canonical_payload_path("./usr/bin/tool").is_err());
    assert!(canonical_payload_path("").is_err());
}

#[test]
fn artifact_cache_identity_requires_an_exact_sha256() {
    let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert_eq!(exact_sha256(digest).unwrap(), digest);
    assert_eq!(exact_sha256(&format!("sha256:{digest}")).unwrap(), digest);
    assert!(exact_sha256("sha256:abcd").is_err());
}

#[test]
fn conversion_requires_complete_adopted_payload_authority() {
    let error =
        require_conversion_payload_authority("exact", &InstallSource::AdoptedTrack).unwrap_err();
    assert!(error.to_string().contains("adopt exact --full"), "{error}");
    require_conversion_payload_authority("exact", &InstallSource::AdoptedFull).unwrap();
}

#[test]
fn conversion_rejects_bytes_other_than_the_selected_repository_artifact() {
    let expected = format!("sha256:{}", "a".repeat(64));
    let actual = format!("sha256:{}", "b".repeat(64));
    let error = validate_converted_source_checksum(&actual, &expected).unwrap_err();
    assert!(error.to_string().contains(&expected), "{error}");
    assert!(error.to_string().contains(&actual), "{error}");
    validate_converted_source_checksum(&expected, &expected).unwrap();
}

#[test]
fn hardlink_comparison_uses_topology_not_capture_specific_identity() {
    let left = PayloadNodeKind::Hardlink {
        target: "/usr/lib/tool".to_string(),
        identity: "artifact-group-1".to_string(),
    };
    let right = PayloadNodeKind::Hardlink {
        target: "/usr/lib/tool".to_string(),
        identity: "adopted-group-9".to_string(),
    };
    assert_eq!(comparable_kind(&left, true), comparable_kind(&right, true));
}

#[test]
fn hardlink_equivalence_does_not_depend_on_the_selected_primary_path() {
    let package = package_for(VersionScheme::Rpm);
    let format = PackageFormatType::Rpm;
    let mut installed = installed_entries(&package, format);
    let primary = installed
        .iter_mut()
        .find(|entry| entry.path == "/usr/lib/exact/tool")
        .unwrap();
    let content = primary.content.take();
    primary.node.source.kind = PayloadNodeKind::Hardlink {
        target: "/usr/lib/exact/tool-link".to_string(),
        identity: "adopted-group".to_string(),
    };
    let alternate = installed
        .iter_mut()
        .find(|entry| entry.path == "/usr/lib/exact/tool-link")
        .unwrap();
    alternate.node.source.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some("adopted-group".to_string()),
    };
    alternate.content = content;

    validate_payload_entries(&package, format, &installed).unwrap();
}

#[test]
fn exact_identity_and_payload_equivalence_cover_all_native_formats() {
    for scheme in [
        VersionScheme::Rpm,
        VersionScheme::Debian,
        VersionScheme::Arch,
    ] {
        let package = package_for(scheme);
        let format = format_for(scheme);
        validate_native_identity(&package, format, &identity_for(scheme)).unwrap();
        validate_payload_entries(&package, format, &installed_entries(&package, format)).unwrap();
    }
}

#[test]
fn identity_mismatch_is_typed_for_all_native_formats() {
    for scheme in [
        VersionScheme::Rpm,
        VersionScheme::Debian,
        VersionScheme::Arch,
    ] {
        let mut package = package_for(scheme);
        package.architecture = "aarch64".to_string();
        let error = validate_native_identity(&package, format_for(scheme), &identity_for(scheme))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("resolved artifact identity mismatch for architecture"),
            "unexpected {scheme:?} identity error: {error}"
        );
    }
}

#[test]
fn payload_drift_is_path_specific_for_all_native_formats() {
    for scheme in [
        VersionScheme::Rpm,
        VersionScheme::Debian,
        VersionScheme::Arch,
    ] {
        let package = package_for(scheme);
        let format = format_for(scheme);
        let mut installed = installed_entries(&package, format);
        let drifted = installed
            .iter_mut()
            .find(|entry| entry.path == "/usr/lib/exact/tool")
            .unwrap();
        drifted.content.as_mut().unwrap().sha256 = "f".repeat(64);

        let error = validate_payload_entries(&package, format, &installed).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("/usr/lib/exact/tool"), "{text}");
        assert!(text.contains("content size or SHA-256 differs"), "{text}");
    }
}

#[test]
fn exact_repository_resolution_covers_all_native_formats() {
    for scheme in [
        VersionScheme::Rpm,
        VersionScheme::Debian,
        VersionScheme::Arch,
    ] {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        insert_repository_package(&conn, scheme, "exact-source", 10);

        let selected = resolve_exact_repository_artifact(&conn, &identity_for(scheme)).unwrap();
        assert_eq!(selected.repository.name, "exact-source");
        assert_eq!(selected.package.version, "1.2.3-4");
        assert_eq!(selected.package.architecture.as_deref(), Some("x86_64"));
    }
}

#[test]
fn repository_resolution_reports_unavailable_and_ambiguous_artifacts() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    let identity = identity_for(VersionScheme::Debian);
    let unavailable = resolve_exact_repository_artifact(&conn, &identity).unwrap_err();
    assert!(
        unavailable
            .to_string()
            .contains("no enrolled native repository contains exact artifact"),
        "{unavailable}"
    );

    insert_repository_package(&conn, VersionScheme::Debian, "source-a", 10);
    insert_repository_package(&conn, VersionScheme::Debian, "source-b", 10);
    let ambiguous = resolve_exact_repository_artifact(&conn, &identity).unwrap_err();
    assert!(
        ambiguous
            .to_string()
            .contains("is ambiguous across repositories"),
        "{ambiguous}"
    );
}
