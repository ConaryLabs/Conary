// apps/conary/src/commands/install/batch/tests.rs
use super::*;
use crate::commands::PackageFormatType;
use conary_core::db::models::{
    ChangesetStatus, ConfigFile, ConfigSource, FileEntry, InstalledRequirementGroup, Trove,
    TroveType,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use std::collections::BTreeMap;

// Both children need an explicit path: `tests` is itself loaded through a
// `#[path]` attribute, so its submodules resolve against `batch/` rather than
// `batch/tests/`. Without this, `mod witness_universe;` silently binds to the
// source module of the same name instead of the test file.
#[path = "tests/mutation_lock.rs"]
mod mutation_lock;
#[path = "tests/witness_universe.rs"]
mod witness_universe;

fn payload_node(kind: PayloadNodeKind, mode: u32) -> PayloadNode {
    PayloadNode {
        kind,
        mode,
        user: PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::geteuid() }),
        },
        group: PayloadIdentity::Numeric {
            id: u64::from(unsafe { libc::getegid() }),
        },
        mtime: PayloadTimestamp::UNIX_EPOCH,
        xattrs: BTreeMap::new(),
    }
}

fn extracted_regular(path: &str, content: &[u8], permissions: u32) -> PackagePayloadFile {
    PackagePayloadFile::new(
        path.to_string(),
        payload_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            libc::S_IFREG | permissions,
        ),
        Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(content),
            size: content.len() as u64,
        }),
        Some(
            conary_core::packages::payload::ReopenablePayload::from_in_memory_bytes(
                std::sync::Arc::<[u8]>::from(content),
            ),
        ),
    )
    .unwrap()
}

fn extracted_symlink(path: &str, target: &str) -> PackagePayloadFile {
    PackagePayloadFile::new(
        path.to_string(),
        payload_node(
            PayloadNodeKind::Symlink {
                target: target.to_string(),
            },
            libc::S_IFLNK | 0o777,
        ),
        None,
        None,
    )
    .unwrap()
}

fn installed_regular_file(
    db_path: &std::path::Path,
    path: &str,
    content: &[u8],
    permissions: u32,
    trove_id: i64,
) -> FileEntry {
    let runtime_root =
        conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.to_path_buf());
    let sha256 = conary_core::filesystem::CasStore::new(runtime_root.objects_dir())
        .unwrap()
        .store(content)
        .unwrap();
    FileEntry::new(
        path.to_string(),
        ResolvedPayloadNode::from_numeric_source(payload_node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            libc::S_IFREG | permissions,
        ))
        .unwrap(),
        Some(PayloadContentAuthority {
            sha256,
            size: content.len() as u64,
        }),
        trove_id,
    )
}

fn installed_directory(path: &str, permissions: u32, trove_id: i64) -> FileEntry {
    FileEntry::new(
        path.to_string(),
        ResolvedPayloadNode::from_numeric_source(payload_node(
            PayloadNodeKind::Directory,
            libc::S_IFDIR | permissions,
        ))
        .unwrap(),
        None,
        trove_id,
    )
}

#[test]
fn test_batch_plan_detects_cross_package_conflict() {
    // Create two packages that both try to install /usr/bin/foo
    let pkg1 = PreparedPackage {
        name: "pkg1".to_string(),
        version: "1.0".to_string(),
        semantics: InstallSemantics::native_package(PackageFormatType::Rpm),
        package_release: None,
        debian_multi_arch: None,
        architecture: Some("x86_64".to_string()),
        description: None,
        extracted_files: vec![extracted_regular("/usr/bin/foo", b"pkg1 content", 0o755)],
        repository_enrollments: Vec::new(),
        requirements: Vec::new(),
        provides: Vec::new(),
        relations: Vec::new(),
        relation_removals: Vec::new(),
        relation_deconfigurations: Vec::new(),
        config_declarations: Vec::new(),
        install_reason: InstallReason::Explicit,
        selection_reason: "Test".to_string(),
        is_upgrade: false,
        old_trove: None,
        installed_components: vec![ComponentType::Runtime],
        classified_files: HashMap::new(),
        installed_component_names: None,
        component_names_by_path: None,
        repository_provenance: None,
        native_lifecycle_state: NativeLifecycleInstallState::default(),
        ccs: None,
    };

    let pkg2 = PreparedPackage {
        name: "pkg2".to_string(),
        version: "1.0".to_string(),
        semantics: InstallSemantics::native_package(PackageFormatType::Rpm),
        package_release: None,
        debian_multi_arch: None,
        architecture: Some("x86_64".to_string()),
        description: None,
        extracted_files: vec![extracted_regular(
            "/usr/bin/foo", // Same path!
            b"pkg2 content",
            0o755,
        )],
        repository_enrollments: Vec::new(),
        requirements: Vec::new(),
        provides: Vec::new(),
        relations: Vec::new(),
        relation_removals: Vec::new(),
        relation_deconfigurations: Vec::new(),
        config_declarations: Vec::new(),
        install_reason: InstallReason::Explicit,
        selection_reason: "Test".to_string(),
        is_upgrade: false,
        old_trove: None,
        installed_components: vec![ComponentType::Runtime],
        classified_files: HashMap::new(),
        installed_component_names: None,
        component_names_by_path: None,
        repository_provenance: None,
        native_lifecycle_state: NativeLifecycleInstallState::default(),
        ccs: None,
    };

    let installer = BatchInstaller::new("/tmp/test.db", SandboxMode::Always);
    let conn = rusqlite::Connection::open_in_memory().unwrap();

    let plan = installer.plan_batch(&[pkg1, pkg2], &conn).unwrap();

    assert_eq!(plan.conflicts.len(), 1);
    match &plan.conflicts[0] {
        BatchConflict::CrossPackageConflict {
            path,
            package1,
            package2,
            ..
        } => {
            assert_eq!(path, "/usr/bin/foo");
            assert!(package1 == "pkg1" || package1 == "pkg2");
            assert!(package2 == "pkg1" || package2 == "pkg2");
            assert_ne!(package1, package2);
            assert_eq!(
                plan.conflicts[0].to_string(),
                "/usr/bin/foo: conflict between pkg1 and pkg2: content digests differ"
            );
        }
    }
}

#[test]
fn test_prepared_package_to_trove() {
    let pkg = PreparedPackage {
        name: "test-pkg".to_string(),
        version: "1.2.3".to_string(),
        semantics: InstallSemantics::native_package(PackageFormatType::Deb),
        package_release: None,
        debian_multi_arch: Some(conary_core::repository::dependency_model::DebianMultiArch::No),
        architecture: Some("amd64".to_string()),
        description: Some("Test package".to_string()),
        extracted_files: Vec::new(),
        repository_enrollments: Vec::new(),
        requirements: Vec::new(),
        provides: Vec::new(),
        relations: Vec::new(),
        relation_removals: Vec::new(),
        relation_deconfigurations: Vec::new(),
        config_declarations: Vec::new(),
        install_reason: InstallReason::Dependency,
        selection_reason: "selected from the exact nginx dependency closure".to_string(),
        is_upgrade: false,
        old_trove: None,
        installed_components: Vec::new(),
        classified_files: HashMap::new(),
        installed_component_names: None,
        component_names_by_path: None,
        repository_provenance: None,
        native_lifecycle_state: super::super::NativeLifecycleInstallState::default(),
        ccs: None,
    };

    assert!(pkg.native_lifecycle_state.bundle_to_persist.is_none());

    let trove = pkg.to_trove(42).unwrap();

    assert_eq!(trove.name, "test-pkg");
    assert_eq!(trove.version, "1.2.3");
    assert_eq!(trove.architecture, Some("amd64".to_string()));
    assert_eq!(trove.installed_by_changeset_id, Some(42));
    assert_eq!(
        trove.install_reason,
        conary_core::db::models::InstallReason::Dependency
    );
    assert_eq!(
        trove.selection_reason.as_deref(),
        Some("selected from the exact nginx dependency closure")
    );
}

#[test]
fn prepared_package_to_trove_preserves_matching_repository_provenance() {
    let mut pkg = PreparedPackage {
        name: "dep-pkg".to_string(),
        version: "2.0.0-1".to_string(),
        semantics: InstallSemantics::native_package(PackageFormatType::Arch),
        package_release: None,
        debian_multi_arch: None,
        architecture: Some("x86_64".to_string()),
        description: Some("Repo dependency".to_string()),
        extracted_files: Vec::new(),
        repository_enrollments: Vec::new(),
        requirements: Vec::new(),
        provides: Vec::new(),
        relations: Vec::new(),
        relation_removals: Vec::new(),
        relation_deconfigurations: Vec::new(),
        config_declarations: Vec::new(),
        install_reason: InstallReason::Dependency,
        selection_reason: "selected from the exact parent closure".to_string(),
        is_upgrade: false,
        old_trove: None,
        installed_components: Vec::new(),
        classified_files: HashMap::new(),
        installed_component_names: None,
        component_names_by_path: None,
        repository_provenance: Some(RepositoryInstallProvenance {
            repository_id: 9,
            source_profile: Some("arch".to_string()),
            version_scheme: conary_core::repository::versioning::VersionScheme::Arch,
            source_kind: conary_core::repository::RepositorySourceKind::Native,
        }),
        native_lifecycle_state: NativeLifecycleInstallState::default(),
        ccs: None,
    };

    let trove = pkg.to_trove(42).unwrap();

    assert_eq!(
        trove.install_source,
        conary_core::db::models::InstallSource::Repository
    );
    assert_eq!(trove.installed_from_repository_id, Some(9));
    assert_eq!(trove.source_profile.as_deref(), Some("arch"));
    assert_eq!(
        trove.version_scheme,
        conary_core::repository::versioning::VersionScheme::Arch
    );

    pkg.repository_provenance.as_mut().unwrap().version_scheme =
        conary_core::repository::versioning::VersionScheme::Rpm;
    let error = pkg.to_trove(42).unwrap_err().to_string();
    assert!(error.contains("declares rpm versioning"), "{error}");
    assert!(error.contains("owns arch versioning"), "{error}");
}

fn prepared_test_package(name: &str, path: &str, content: &[u8]) -> PreparedPackage {
    PreparedPackage {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        semantics: InstallSemantics::native_package(PackageFormatType::Rpm),
        package_release: None,
        debian_multi_arch: None,
        architecture: Some("x86_64".to_string()),
        description: None,
        extracted_files: vec![extracted_regular(path, content, 0o755)],
        repository_enrollments: Vec::new(),
        requirements: Vec::new(),
        provides: vec![crate::commands::test_helpers::exact_package_self_provider(
            name,
            "1.0.0",
            conary_core::repository::versioning::VersionScheme::Rpm,
        )],
        relations: Vec::new(),
        relation_removals: Vec::new(),
        relation_deconfigurations: Vec::new(),
        config_declarations: Vec::new(),
        install_reason: InstallReason::Explicit,
        selection_reason: "Required by wording that must not control ownership".to_string(),
        is_upgrade: false,
        old_trove: None,
        installed_components: vec![ComponentType::Runtime],
        classified_files: HashMap::from([(ComponentType::Runtime, vec![path.to_string()])]),
        installed_component_names: None,
        component_names_by_path: None,
        repository_provenance: None,
        native_lifecycle_state: NativeLifecycleInstallState::default(),
        ccs: None,
    }
}

fn prepared_repository_package(
    name: &str,
    version: &str,
    endpoint: &str,
    key: &[u8],
) -> PreparedPackage {
    let declaration_path = "/etc/yum.repos.d/browser.repo";
    let key_path = "/etc/pki/rpm-gpg/browser.gpg";
    let declaration = format!(
        "[browser]\nbaseurl={endpoint}\ngpgcheck=1\nrepo_gpgcheck=1\ngpgkey=file://{key_path}\n"
    );
    let mut package = prepared_test_package(name, declaration_path, declaration.as_bytes());
    package.version = version.to_string();
    package.extracted_files[0].node.mode = libc::S_IFREG | 0o644;
    package
        .extracted_files
        .push(extracted_regular(key_path, key, 0o644));
    package.classified_files.insert(
        ComponentType::Runtime,
        vec![declaration_path.to_string(), key_path.to_string()],
    );
    package.provides = vec![crate::commands::test_helpers::exact_package_self_provider(
        name,
        version,
        conary_core::repository::versioning::VersionScheme::Rpm,
    )];
    package.repository_enrollments =
        conary_core::repository::enrollment::derive::derive_rpm_repository_enrollments(
            &package.extracted_files,
            "x86_64",
            Some("fedora-44"),
        )
        .unwrap();
    package
}

fn repository_certificate() -> Vec<u8> {
    use sequoia_openpgp::cert::prelude::CertBuilder;
    use sequoia_openpgp::serialize::Serialize;
    let (certificate, _) = CertBuilder::new()
        .add_userid("batch repository fixture")
        .generate()
        .unwrap();
    let mut bytes = Vec::new();
    certificate.serialize(&mut bytes).unwrap();
    bytes
}

#[test]
fn batch_install_and_update_replace_repository_authority_atomically() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let db_path_string = db_path.to_string_lossy().into_owned();
    let key = repository_certificate();
    let first = prepared_repository_package(
        "browser-repository-release",
        "1",
        "https://repo.example/one/$basearch",
        &key,
    );

    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![first])
        .unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let old_trove = Trove::find_by_name(&conn, "browser-repository-release")
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(
        conary_core::db::models::Repository::find_by_name(&conn, "browser")
            .unwrap()
            .unwrap()
            .url,
        "https://repo.example/one/x86_64"
    );
    drop(conn);

    let mut second = prepared_repository_package(
        "browser-repository-release",
        "2",
        "https://repo.example/two/$basearch",
        &key,
    );
    second.is_upgrade = true;
    second.old_trove = Some(Box::new(old_trove));
    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![second])
        .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        conary_core::db::models::Repository::find_by_name(&conn, "browser")
            .unwrap()
            .unwrap()
            .url,
        "https://repo.example/two/x86_64"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM package_repository_enrollments
             WHERE owner_kind = 'package'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
    drop(conn);

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(crate::commands::remove::cmd_remove(
            "browser-repository-release",
            &db_path_string,
            None,
            Some("x86_64".to_string()),
            SandboxMode::Always,
            false,
        ))
        .unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        conary_core::db::models::Repository::find_by_name(&conn, "browser")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM package_repository_enrollments",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );
}

fn prepared_test_hardlink_package(
    name: &str,
    target: &str,
    edge: &str,
    identity: &str,
) -> PreparedPackage {
    let mut package = prepared_test_package(name, target, b"shared");
    package.extracted_files[0].node.kind = PayloadNodeKind::Regular {
        hardlink_identity: Some(identity.to_string()),
    };
    let mut edge_node = package.extracted_files[0].node.clone();
    edge_node.kind = PayloadNodeKind::Hardlink {
        target: target.to_string(),
        identity: identity.to_string(),
    };
    package
        .extracted_files
        .push(PackagePayloadFile::new(edge.to_string(), edge_node, None, None).unwrap());
    package
        .classified_files
        .get_mut(&ComponentType::Runtime)
        .unwrap()
        .push(edge.to_string());
    package
}

#[test]
fn batch_plan_accepts_source_compatible_identical_rpm_regular_files() {
    let first = prepared_test_package("first", "/usr/share/licenses/shared", b"license");
    let second = prepared_test_package("second", "/usr/share/licenses/shared", b"license");
    let installer = BatchInstaller::new("/tmp/test.db", SandboxMode::Always);
    let conn = rusqlite::Connection::open_in_memory().unwrap();

    let plan = installer.plan_batch(&[first, second], &conn).unwrap();

    assert!(plan.conflicts.is_empty(), "{:?}", plan.conflicts);
    assert_eq!(plan.total_files, 2);
}

#[test]
fn batch_install_persists_two_rpm_claims_on_one_identical_regular_anchor() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let db_path_string = db_path.to_string_lossy().into_owned();
    let first = prepared_test_package("first", "/usr/share/licenses/shared", b"license");
    let second = prepared_test_package("second", "/usr/share/licenses/shared", b"license");

    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![first, second])
        .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let claims =
        conary_core::db::models::PayloadClaim::find_by_path(&conn, "/usr/share/licenses/shared")
            .unwrap();
    assert_eq!(claims.len(), 2);
    assert!(
        claims.iter().all(|claim| {
            claim.sharing_policy == conary_core::payload::PayloadSharingPolicy::Rpm
        })
    );
    let anchor = FileEntry::find_by_path(&conn, "/usr/share/licenses/shared")
        .unwrap()
        .unwrap();
    assert_eq!(
        claims
            .iter()
            .filter(|claim| claim.trove_id == anchor.trove_id)
            .count(),
        1
    );
    for claim in claims {
        let payload =
            conary_core::db::models::PackagePayloadOwnership::load(&conn, claim.trove_id).unwrap();
        assert_eq!(payload.lifecycle_paths(), &["/usr/share/licenses/shared"]);
    }
}

#[test]
fn batch_install_extends_a_compatible_shared_rpm_hardlink_target() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let db_path_string = db_path.to_string_lossy().into_owned();
    let target = "/usr/share/shared-target";
    let mut first =
        prepared_test_hardlink_package("first", target, "/usr/share/first-edge", "rpm:1:7");
    let mut second =
        prepared_test_hardlink_package("second", target, "/usr/share/second-edge", "rpm:9:42");
    first.extracted_files.reverse();
    second.extracted_files.reverse();

    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![first, second])
        .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        conary_core::db::models::PayloadClaim::find_by_path(&conn, target)
            .unwrap()
            .len(),
        2
    );
    let identity = format!("path:{target}");
    assert!(matches!(
        FileEntry::find_by_path(&conn, target)
            .unwrap()
            .unwrap()
            .node
            .source
            .kind,
        PayloadNodeKind::Regular {
            hardlink_identity: Some(actual)
        } if actual == identity
    ));
    for path in ["/usr/share/first-edge", "/usr/share/second-edge"] {
        assert_eq!(
            FileEntry::find_by_path(&conn, path)
                .unwrap()
                .unwrap()
                .node
                .source
                .kind,
            PayloadNodeKind::Hardlink {
                target: target.to_string(),
                identity: identity.clone()
            }
        );
    }
}

fn prepared_test_symlink_package(name: &str, path: &str, target: &str) -> PreparedPackage {
    let mut package = prepared_test_package(name, path, &[]);
    package.extracted_files[0] = extracted_symlink(path, target);
    package
}

fn depends_on(name: &str) -> conary_core::repository::dependency_model::RepositoryRequirementGroup {
    conary_core::repository::dependency_model::RepositoryRequirementGroup::simple(
        conary_core::repository::dependency_model::RepositoryRequirementKind::Depends,
        conary_core::repository::dependency_model::RepositoryRequirementClause::name_only(
            name.to_string(),
        ),
    )
}

fn pre_depends_on(
    name: &str,
) -> conary_core::repository::dependency_model::RepositoryRequirementGroup {
    conary_core::repository::dependency_model::RepositoryRequirementGroup::simple(
        conary_core::repository::dependency_model::RepositoryRequirementKind::PreDepends,
        conary_core::repository::dependency_model::RepositoryRequirementClause::name_only(
            name.to_string(),
        ),
    )
}

fn empty_test_db() -> (tempfile::TempDir, rusqlite::Connection) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    (temp, conn)
}

#[test]
fn exact_requirement_order_handles_dependency_chains_deeper_than_two() {
    let (_temp, conn) = empty_test_db();
    let mut a = prepared_test_package("a", "/a", b"a");
    let mut b = prepared_test_package("b", "/b", b"b");
    let mut c = prepared_test_package("c", "/c", b"c");
    let mut d = prepared_test_package("d", "/d", b"d");
    let mut root = prepared_test_package("root", "/root", b"root");
    for dependency in [&mut a, &mut b, &mut c, &mut d] {
        dependency.install_reason = InstallReason::Dependency;
    }
    b.requirements = vec![depends_on("a")];
    c.requirements = vec![depends_on("b")];
    d.requirements = vec![depends_on("c")];
    root.requirements = vec![depends_on("d")];
    let mut packages = vec![root, c, a, d, b];

    super::ordering::order_packages_for_transaction(&conn, &mut packages).unwrap();

    assert_eq!(
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "c", "d", "root"]
    );
}

#[test]
fn exact_requirement_order_collapses_cycles_before_downstream_dependents() {
    let (_temp, conn) = empty_test_db();
    let mut a = prepared_test_package("a", "/a", b"a");
    let mut b = prepared_test_package("b", "/b", b"b");
    let mut root = prepared_test_package("root", "/root", b"root");
    a.install_reason = InstallReason::Dependency;
    b.install_reason = InstallReason::Dependency;
    a.requirements = vec![depends_on("b")];
    b.requirements = vec![depends_on("a")];
    root.requirements = vec![depends_on("a")];
    let mut packages = vec![root, b, a];

    super::ordering::order_packages_for_transaction(&conn, &mut packages).unwrap();

    assert_eq!(
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "root"]
    );
}

#[test]
fn strong_requirement_orders_provider_first_inside_mixed_cycle() {
    let (_temp, conn) = empty_test_db();
    let mut filesystem = prepared_test_package("filesystem", "/filesystem", b"filesystem");
    let mut setup = prepared_test_package("setup", "/setup", b"setup");
    filesystem.install_reason = InstallReason::Dependency;
    setup.install_reason = InstallReason::Dependency;
    filesystem.requirements = vec![pre_depends_on("setup")];
    setup.requirements = vec![depends_on("filesystem")];
    let mut packages = vec![filesystem, setup];

    super::ordering::order_packages_for_transaction(&conn, &mut packages).unwrap();

    assert_eq!(
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        vec!["setup", "filesystem"]
    );
}

#[test]
fn every_acyclic_strong_edge_wins_inside_an_ordinary_dependency_cycle() {
    let (_temp, conn) = empty_test_db();
    let mut a = prepared_test_package("a", "/a", b"a");
    let mut b = prepared_test_package("b", "/b", b"b");
    let mut c = prepared_test_package("c", "/c", b"c");
    let mut d = prepared_test_package("d", "/d", b"d");
    for dependency in [&mut a, &mut b, &mut c, &mut d] {
        dependency.install_reason = InstallReason::Dependency;
    }
    a.requirements = vec![depends_on("d")];
    b.requirements = vec![pre_depends_on("a")];
    c.requirements = vec![pre_depends_on("b")];
    d.requirements = vec![depends_on("c")];
    let mut packages = vec![d, c, b, a];

    super::ordering::order_packages_for_transaction(&conn, &mut packages).unwrap();

    let names = packages
        .iter()
        .map(|package| package.name.as_str())
        .collect::<Vec<_>>();
    let position = |name| {
        names
            .iter()
            .position(|candidate| *candidate == name)
            .unwrap()
    };
    assert!(position("a") < position("b"), "{names:?}");
    assert!(position("b") < position("c"), "{names:?}");
}

#[test]
fn irreducible_strong_requirement_cycle_uses_stable_fallback() {
    let (_temp, conn) = empty_test_db();
    let mut a = prepared_test_package("a", "/a", b"a");
    let mut b = prepared_test_package("b", "/b", b"b");
    a.install_reason = InstallReason::Dependency;
    b.install_reason = InstallReason::Dependency;
    a.requirements = vec![pre_depends_on("b")];
    b.requirements = vec![pre_depends_on("a")];
    let mut packages = vec![b, a];

    super::ordering::order_packages_for_transaction(&conn, &mut packages).unwrap();

    assert_eq!(
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
}

#[test]
fn no_current_generation_batch_materializes_db_state_and_publishes_selected_root() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    let seeded_trove = crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

    // The fixture requirement must be satisfiable: since #345, a single-package
    // batch's requirements are certified against the end state like every other
    // batch's, so the seeded runtime has to provide what the fixture depends
    // on. The materialization assertions below are about batch state, not
    // about requirement semantics, so the provider is seeded directly.
    let conn = conary_core::db::open(&db_path).unwrap();
    conary_core::db::models::ProvideEntry::new(
        seeded_trove,
        "libfixture".to_string(),
        Some("2.0.0".to_string()),
        conary_core::repository::versioning::VersionScheme::Rpm,
    )
    .insert(&conn)
    .unwrap();
    drop(conn);

    let db_path_string = db_path.to_string_lossy().into_owned();
    let mut package =
        prepared_test_package("batch-fixture", "/usr/bin/batch-fixture", b"batch-selected");
    package.requirements = vec![package_requirement_fixture()];
    let installer = BatchInstaller::new(&db_path_string, SandboxMode::Always);

    installer.install_batch(vec![package]).unwrap();

    assert!(!root.join("usr/bin/batch-fixture").exists());
    let conn = conary_core::db::open(&db_path).unwrap();
    let file = FileEntry::find_by_path(&conn, "/usr/bin/batch-fixture")
        .unwrap()
        .expect("batch file should be recorded in DB");
    let owner = Trove::find_by_id(&conn, file.trove_id)
        .unwrap()
        .expect("batch file owner should exist");
    assert_eq!(owner.name, "batch-fixture");
    assert_eq!(owner.install_reason, InstallReason::Explicit);
    assert_eq!(
        owner.selection_reason.as_deref(),
        Some("Required by wording that must not control ownership")
    );
    let requirements = InstalledRequirementGroup::find_by_trove(&conn, owner.id.unwrap()).unwrap();
    assert_eq!(requirements.len(), 1);
    assert_eq!(
        requirements[0].kind,
        conary_core::repository::dependency_model::RepositoryRequirementKind::Depends
    );
    assert_eq!(
        requirements[0].version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
    assert_eq!(requirements[0].requirement, package_requirement_fixture());
    let changesets = conary_core::db::models::Changeset::list_all(&conn).unwrap();
    assert_eq!(changesets.len(), 1);
    assert_eq!(changesets[0].status, ChangesetStatus::Applied);
    let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path);
    let publication_debts =
        conary_core::db::models::GenerationPublication::pending_recoverable(&conn).unwrap();
    assert!(
        conary_core::generation::mount::current_generation(runtime_root.root())
            .unwrap()
            .is_some(),
        "{publication_debts:#?}"
    );
}

#[test]
fn generation_batch_executes_graph_in_selected_root_and_publishes_final_state() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let boot = temp.path().join("boot");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&boot).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

    let db_path_string = db_path.to_string_lossy().into_owned();
    let package =
        prepared_test_package("generation-batch", "/usr/lib/generation-batch", b"selected");
    let installer = BatchInstaller::new(&db_path_string, SandboxMode::Always);

    installer.install_batch(vec![package]).unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        FileEntry::find_by_path(&conn, "/usr/lib/generation-batch")
            .unwrap()
            .is_some()
    );
    let runtime_root = conary_core::runtime_root::ConaryRuntimeRoot::from_db_path(db_path.clone());
    let publication_debts =
        conary_core::db::models::GenerationPublication::pending_recoverable(&conn).unwrap();
    assert!(
        conary_core::generation::mount::current_generation(runtime_root.root())
            .unwrap()
            .is_some(),
        "{publication_debts:#?}"
    );
    let sessions = runtime_root.root().join("selected-root-sessions");
    assert!(!sessions.exists() || std::fs::read_dir(sessions).unwrap().next().is_none());
}

fn package_requirement_fixture()
-> conary_core::repository::dependency_model::RepositoryRequirementGroup {
    conary_core::repository::dependency_model::RepositoryRequirementGroup::simple(
        conary_core::repository::dependency_model::RepositoryRequirementKind::Depends,
        conary_core::repository::dependency_model::RepositoryRequirementClause::versioned(
            "libfixture".to_string(),
            ">= 2".to_string(),
        ),
    )
}

#[test]
fn selected_root_batch_records_declared_config_metadata() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

    let db_path_string = db_path.to_string_lossy().into_owned();
    let mut package = prepared_test_package(
        "batch-config-fixture",
        "/etc/batch-config-fixture.conf",
        b"managed=true\n",
    );
    package.config_declarations = vec![
        conary_core::packages::config_authority::SourceConfigDeclaration::Rpm(
            conary_core::packages::rpm::authority::RpmConfigDeclaration {
                header_index: 0,
                path: "/etc/batch-config-fixture.conf".to_string(),
                noreplace: true,
                ghost: false,
                missing_ok: false,
                payload: conary_core::packages::config_authority::ConfigPayloadAssociation::Matched,
            },
        ),
    ];
    let installer = BatchInstaller::new(&db_path_string, SandboxMode::Always);

    installer.install_batch(vec![package]).unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let file = FileEntry::find_by_path(&conn, "/etc/batch-config-fixture.conf")
        .unwrap()
        .expect("batch config file should be recorded in DB");
    let config = ConfigFile::find_by_path(&conn, "/etc/batch-config-fixture.conf")
        .unwrap()
        .expect("declared batch config file should be tracked");
    assert_eq!(config.file_id, file.id);
    assert_eq!(
        config.original_hash.as_deref(),
        Some(file.content.as_ref().unwrap().sha256.as_str())
    );
    assert_eq!(
        config.current_hash.as_deref(),
        Some(file.content.as_ref().unwrap().sha256.as_str())
    );
    assert_eq!(config.source, ConfigSource::Rpm);
    assert!(config.noreplace);
}

#[test]
fn selected_root_batch_publishes_symlink_from_cas() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

    let db_path_string = db_path.to_string_lossy().into_owned();
    let package =
        prepared_test_symlink_package("batch-link-fixture", "/usr/bin/batch-link", "batch-target");
    let installer = BatchInstaller::new(&db_path_string, SandboxMode::Always);

    installer.install_batch(vec![package]).unwrap();

    assert!(!root.join("usr/bin/batch-link").exists());
    let conn = conary_core::db::open(&db_path).unwrap();
    let file = FileEntry::find_by_path(&conn, "/usr/bin/batch-link")
        .unwrap()
        .expect("batch symlink should be recorded in DB");
    assert_eq!(
        file.node.source.kind,
        PayloadNodeKind::Symlink {
            target: "batch-target".to_string()
        }
    );
    assert!(file.content.is_none());
}

#[test]
fn selected_root_batch_conflict_preflight_runs_before_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    let live_file = root.join("usr/bin/batch-fixture");
    let marker = root.join("batch-pre-scriptlet-ran");
    std::fs::create_dir_all(live_file.parent().unwrap()).unwrap();
    std::fs::write(&live_file, "owned elsewhere").unwrap();
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    let mut other_trove = Trove::new(
        "other-owner".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let other_trove_id = other_trove.insert(&conn).unwrap();
    for path in ["/usr", "/usr/bin"] {
        let mut directory = installed_directory(path, 0o755, other_trove_id);
        directory.insert(&conn).unwrap();
    }
    let mut existing = installed_regular_file(
        &db_path,
        "/usr/bin/batch-fixture",
        b"owned elsewhere",
        0o755,
        other_trove_id,
    );
    existing.insert(&conn).unwrap();

    let package = prepared_test_package("batch-fixture", "/usr/bin/batch-fixture", b"replacement");
    let db_path_string = db_path.to_string_lossy().into_owned();
    let installer = BatchInstaller::new(&db_path_string, SandboxMode::Always);

    let error = installer.install_batch(vec![package]).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("Path /usr/bin/batch-fixture from batch-fixture is incompatible with package other-owner: sharing policies differ ('rpm' versus 'exclusive')"),
        "{error:#}"
    );
    assert!(!marker.exists(), "pre-install scriptlet must not run");
    assert_eq!(
        std::fs::read_to_string(live_file).unwrap(),
        "owned elsewhere"
    );
}

/// Sign one CCS carrying `paths` as its complete runtime payload.
///
/// The manifest declares no capability naming a payload path, which is how a
/// converted artifact reaches the transaction: source headers own `Provides`,
/// and the payload alone owns file ownership.
fn signed_rpm_ccs_package(
    temp_dir: &std::path::Path,
    name: &str,
    paths: &[&str],
) -> conary_core::ccs::CcsPackage {
    let signing_key =
        conary_core::ccs::SigningKeyPair::generate().with_key_id("payload-file-provider");
    let mut manifest = conary_core::ccs::CcsManifest::new_minimal(name, "1.0.0");
    manifest.package.version_scheme = conary_core::repository::versioning::VersionScheme::Rpm;
    manifest.components.default = vec!["runtime".to_string()];

    let mut entries = Vec::new();
    let mut blobs = HashMap::new();
    let mut total_size = 0;
    for path in paths {
        let content = format!("payload for {path}").into_bytes();
        let sha256 = conary_core::hash::sha256(&content);
        total_size += content.len() as u64;
        entries.push(conary_core::ccs::FileEntry {
            path: (*path).to_string(),
            node: payload_node(
                PayloadNodeKind::Regular {
                    hardlink_identity: None,
                },
                libc::S_IFREG | 0o755,
            ),
            content: Some(PayloadContentAuthority {
                sha256: sha256.clone(),
                size: content.len() as u64,
            }),
            component: "runtime".to_string(),
            chunks: None,
        });
        blobs.insert(sha256, content);
    }

    let result = conary_core::ccs::BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            conary_core::ccs::ComponentData {
                name: "runtime".to_string(),
                files: entries.clone(),
                hash: "runtime".to_string(),
                size: total_size,
            },
        )]),
        files: entries.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &entries, blobs,
        )
        .unwrap(),
        total_size,
        chunked: false,
        chunk_stats: None,
    };
    let package_path = temp_dir.join(format!("{name}.ccs"));
    conary_core::ccs::builder::write_signed_current_ccs_package(
        &result,
        &package_path,
        &signing_key,
        false,
    )
    .unwrap();
    let verified = conary_core::ccs::verify::verify_package(
        &package_path,
        &conary_core::ccs::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
    )
    .unwrap();
    conary_core::ccs::CcsPackage::from_verified_archive(package_path.to_str().unwrap(), &verified)
        .unwrap()
}

fn prepare_signed_ccs_dependency(
    temp_dir: &std::path::Path,
    db_path: &std::path::Path,
    name: &str,
    paths: &[&str],
) -> PreparedPackage {
    let package = signed_rpm_ccs_package(temp_dir, name, paths);
    prepare_ccs_package_for_batch(
        &package,
        db_path.to_str().unwrap(),
        InstallReason::Dependency,
        "required by the exact selected transaction",
        false,
        InstallIntent::PackageChange,
        None,
    )
    .unwrap()
}

fn ccs_test_db(temp_dir: &std::path::Path) -> (std::path::PathBuf, rusqlite::Connection) {
    let db_path = temp_dir.join("conary.db");
    conary_core::db::init(&db_path).unwrap();
    let conn = conary_core::db::open(&db_path).unwrap();
    (db_path, conn)
}

#[test]
fn prepared_package_publishes_every_payload_path_as_a_file_provider() {
    let temp = tempfile::tempdir().unwrap();
    let (db_path, _conn) = ccs_test_db(temp.path());
    let paths = ["/usr/bin/sh", "/usr/share/doc/bash/README"];

    let prepared = prepare_signed_ccs_dependency(temp.path(), &db_path, "bash", &paths);

    assert!(
        !prepared.extracted_files.is_empty(),
        "the fixture must carry a payload"
    );
    for file in &prepared.extracted_files {
        assert!(
            prepared.provides.iter().any(|provide| {
                provide.kind
                    == conary_core::repository::dependency_model::RepositoryCapabilityKind::File
                    && provide.name == file.path
            }),
            "payload path {} is not published as a file provider: {:?}",
            file.path,
            prepared
                .provides
                .iter()
                .map(|provide| provide.name.as_str())
                .collect::<Vec<_>>()
        );
    }
    assert!(
        prepared
            .provides
            .iter()
            .any(|provide| provide.name == "bash"),
        "materialized paths must not displace the exact package self-provider"
    );
}

#[test]
fn payload_file_provider_satisfies_an_exact_path_requirement_and_orders_provider_first() {
    let temp = tempfile::tempdir().unwrap();
    let (db_path, conn) = ccs_test_db(temp.path());
    let dependent_of = |requirement: &str| {
        let mut dependent =
            prepared_test_package("systemd-udev", "/usr/lib/udev/rules.d/99.rules", b"rule");
        dependent.requirements = vec![depends_on(requirement)];
        dependent
    };

    // A converted artifact reduced to its declared header capabilities cannot
    // satisfy the path dependency the solver resolved from repository metadata.
    let mut declared_only =
        prepare_signed_ccs_dependency(temp.path(), &db_path, "bash", &["/usr/bin/sh"]);
    declared_only.provides.retain(|provide| {
        provide.kind != conary_core::repository::dependency_model::RepositoryCapabilityKind::File
    });
    let error = super::ordering::order_packages_for_transaction(
        &conn,
        &mut vec![dependent_of("/usr/bin/sh"), declared_only],
    )
    .unwrap_err();
    assert!(
        error.to_string().contains(
            "selected package transaction does not satisfy exact depends requirement for systemd-udev"
        ),
        "{error:#}"
    );

    let shell = prepare_signed_ccs_dependency(temp.path(), &db_path, "bash", &["/usr/bin/sh"]);
    let mut packages = vec![dependent_of("/usr/bin/sh"), shell];

    super::ordering::order_packages_for_transaction(&conn, &mut packages).unwrap();

    assert_eq!(
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        vec!["bash", "systemd-udev"]
    );
}

#[test]
fn a_single_package_rejects_an_unsatisfiable_requirement_with_the_ordered_batches_diagnostic() {
    let (_temp, conn) = empty_test_db();
    let unsatisfiable = || {
        let mut package = prepared_test_package("needy", "/usr/bin/needy", b"needy");
        package.requirements = vec![depends_on("/usr/bin/never-provided")];
        package
    };

    // The ordered path already fails this batch, naming the dependent and the
    // exact requirement. A lone package must fail with the identical typed
    // diagnostic: one owner for every batch size, so the two paths cannot
    // drift. The solver never ran here -- the batch is prepared directly and
    // handed to transaction validation, which is the only authority between
    // the packages and the commit.
    let mut ordered = vec![
        unsatisfiable(),
        prepared_test_package("other", "/usr/bin/other", b"other"),
    ];
    let ordered_error =
        super::ordering::order_packages_for_transaction(&conn, &mut ordered).unwrap_err();
    let mut lone = vec![unsatisfiable()];
    let lone_error = super::ordering::order_packages_for_transaction(&conn, &mut lone).unwrap_err();

    assert!(
        ordered_error.to_string().contains(
            "selected package transaction does not satisfy exact depends requirement for needy"
        ),
        "{ordered_error:#}"
    );
    assert!(
        lone_error.to_string().contains(
            "selected package transaction does not satisfy exact depends requirement for needy"
        ),
        "{lone_error:#}"
    );
    assert_eq!(
        format!("{lone_error:#}"),
        format!("{ordered_error:#}"),
        "a single-package batch must fail with the same typed diagnostic as an ordered batch"
    );
}

#[test]
fn a_lone_package_install_batch_rejects_an_unsatisfiable_requirement_at_validation() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(&root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let db_path_string = db_path.to_string_lossy().into_owned();
    let mut package = prepared_test_package("lone-tool", "/usr/bin/lone-tool", b"tool");
    package.requirements = vec![depends_on("/usr/bin/never-provided")];

    // Nothing upstream (no solver, no repository) ran on this package: the
    // batch installer is the first authority to see it, so this is exactly the
    // single-package install the len<2 early return used to wave through.
    let error = BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![package])
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(
        message.contains(
            "selected package transaction does not satisfy exact depends requirement for lone-tool"
        ),
        "{message}"
    );
    let conn = conary_core::db::open(&db_path).unwrap();
    assert!(
        conary_core::db::models::Changeset::list_all(&conn)
            .unwrap()
            .is_empty(),
        "validation must fail before any changeset applies"
    );
}

#[test]
fn a_promised_path_witnesses_a_dependency_the_provider_ships_no_content_for() {
    let (_temp, conn) = empty_test_db();
    let dependent_of = |requirement: &str| {
        let mut dependent = prepared_test_package("krb5-libs", "/usr/lib64/libkrb5.so.3", b"krb5");
        dependent.requirements = vec![depends_on(requirement)];
        dependent
    };
    let promised = "/etc/crypto-policies/back-ends/krb5.config";

    // Before the package publishes its promise, nothing in the transaction can
    // witness a path the provider owns but never ships.
    let mut without_promise = vec![
        dependent_of(promised),
        prepared_test_package(
            "crypto-policies",
            "/usr/share/crypto-policies/DEFAULT",
            b"policy",
        ),
    ];
    let error =
        super::ordering::order_packages_for_transaction(&conn, &mut without_promise).unwrap_err();
    assert!(
        error.to_string().contains(
            "selected package transaction does not satisfy exact depends requirement for krb5-libs"
        ),
        "{error:#}"
    );

    let mut provider = prepared_test_package(
        "crypto-policies",
        "/usr/share/crypto-policies/DEFAULT",
        b"policy",
    );
    provider.install_reason = InstallReason::Dependency;
    provider.provides.push(
        conary_core::repository::dependency_model::ProvidedCapability::promised_path(
            conary_core::repository::dependency_model::SourcePackageFormat::Rpm,
            promised,
        ),
    );
    let mut packages = vec![dependent_of(promised), provider];

    super::ordering::order_packages_for_transaction(&conn, &mut packages).unwrap();

    assert_eq!(
        packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<Vec<_>>(),
        vec!["crypto-policies", "krb5-libs"],
        "the package that promises the path must be ordered before its dependent"
    );
}

fn promised(path: &str) -> conary_core::repository::dependency_model::ProvidedCapability {
    conary_core::repository::dependency_model::ProvidedCapability::promised_path(
        conary_core::repository::dependency_model::SourcePackageFormat::Rpm,
        path,
    )
}

#[test]
fn a_transaction_fails_when_a_witness_used_promised_path_is_never_materialized() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(temp.path().join("root")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let db_path_string = db_path.to_string_lossy().into_owned();
    let promised_path = "/etc/crypto-policies/back-ends/krb5.config";

    // The dependent is satisfied only through the promise, so the transaction
    // certified an edge against a path that must then exist.
    let mut provider = prepared_test_package(
        "crypto-policies",
        "/usr/share/crypto-policies/DEFAULT/krb5.txt",
        b"policy",
    );
    provider.install_reason = InstallReason::Dependency;
    provider.provides.push(promised(promised_path));
    let mut dependent = prepared_test_package("krb5-libs", "/usr/lib64/libkrb5.so.3", b"krb5");
    dependent.requirements = vec![depends_on(promised_path)];
    let installer = BatchInstaller::new(&db_path_string, SandboxMode::Always);

    let error = installer
        .install_batch(vec![dependent, provider])
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(
        message.contains(&format!(
            "did not materialize promised path {promised_path}"
        )),
        "{message}"
    );
    assert!(message.contains("crypto-policies"), "{message}");
    // The diagnostic must name the edge that relied on the promise.
    assert!(
        message.contains("krb5-libs depends on it through"),
        "{message}"
    );
}

#[test]
fn an_unused_promised_path_never_fails_the_transaction() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(temp.path().join("root")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let db_path_string = db_path.to_string_lossy().into_owned();

    // The real case this pins: Fedora's setup package owns /etc/fstab as a
    // %ghost and never creates it -- an OS installer does. Nothing in the
    // transaction depends on it, so the transaction has nothing to prove.
    let mut setup = prepared_test_package("setup", "/etc/profile", b"profile");
    setup.provides.push(promised("/etc/fstab"));
    setup.provides.push(promised("/run/motd"));
    // setup is a retained witness for a real edge, so the promises are in
    // scope for consideration -- they are simply not what holds the edge up.
    let mut other = prepared_test_package("filesystem", "/usr/bin/filesystem-marker", b"marker");
    other.requirements = vec![depends_on("setup")];
    let installer = BatchInstaller::new(&db_path_string, SandboxMode::Always);

    installer.install_batch(vec![setup, other]).unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        conary_core::db::models::Changeset::list_all(&conn).unwrap()[0].status,
        ChangesetStatus::Applied
    );
}

#[test]
fn a_witness_used_promise_already_present_satisfies_the_post_condition() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    std::fs::create_dir_all(temp.path().join("root")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let db_path_string = db_path.to_string_lossy().into_owned();
    let promised_path = "/usr/share/promised/marker.conf";

    // The post-condition asks whether the path exists in the assembled root,
    // not who produced it, so a path another batch member ships satisfies it.
    let mut promiser = prepared_test_package("promiser", "/usr/bin/promiser", b"payload");
    promiser.provides.push(promised(promised_path));
    let producer = prepared_test_package("producer", promised_path, b"materialized");
    let mut dependent = prepared_test_package("dependent", "/usr/bin/dependent", b"dependent");
    dependent.requirements = vec![depends_on(promised_path)];
    let installer = BatchInstaller::new(&db_path_string, SandboxMode::Always);

    installer
        .install_batch(vec![dependent, producer, promiser])
        .unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        conary_core::db::models::Changeset::list_all(&conn).unwrap()[0].status,
        ChangesetStatus::Applied
    );
}

/// A database with a bootable runtime and one completed prior transaction.
///
/// Whatever that transaction installed is exactly what a later batch inherits:
/// promises held by packages the later transaction never sees among its own
/// members.
fn db_with_prior_transaction(
    temp: &std::path::Path,
    prior: Vec<PreparedPackage>,
) -> (std::path::PathBuf, String) {
    let db_path = temp.join("conary.db");
    std::fs::create_dir_all(temp.join("root")).unwrap();
    conary_core::db::init(&db_path).unwrap();
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);
    let db_path_string = db_path.to_string_lossy().into_owned();
    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(prior)
        .expect("the prior transaction must install");
    (db_path, db_path_string)
}

fn crypto_policies_with_promise(promised_path: &str) -> PreparedPackage {
    let mut holder = prepared_test_package(
        "crypto-policies",
        "/usr/share/crypto-policies/DEFAULT/krb5.txt",
        b"policy",
    );
    holder.provides.push(promised(promised_path));
    holder
}

fn every_changeset_applied(db_path: &std::path::Path) -> bool {
    let conn = conary_core::db::open(db_path).unwrap();
    conary_core::db::models::Changeset::list_all(&conn)
        .unwrap()
        .iter()
        .all(|changeset| changeset.status == ChangesetStatus::Applied)
}

#[test]
fn a_later_transaction_verifies_a_promise_it_relied_on_from_an_installed_package() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let promised_path = "/etc/crypto-policies/back-ends/krb5.config";
    let later_batch = || {
        let mut dependent = prepared_test_package("krb5-libs", "/usr/lib64/libkrb5.so.3", b"krb5");
        dependent.requirements = vec![depends_on(promised_path)];
        vec![dependent]
    };

    // The promise materialized during the earlier transaction, so the path the
    // later one leans on is really there and the batch applies. The producer
    // declares no capability for the path, so the installed promise is the only
    // witness the requirement has.
    let kept = tempfile::tempdir().unwrap();
    let (kept_db, kept_db_string) = db_with_prior_transaction(
        kept.path(),
        vec![
            crypto_policies_with_promise(promised_path),
            prepared_test_package("materializer", promised_path, b"generated"),
        ],
    );

    BatchInstaller::new(&kept_db_string, SandboxMode::Always)
        .install_batch(later_batch())
        .unwrap();

    assert!(
        every_changeset_applied(&kept_db),
        "a present promise must not fail the transaction that relies on it"
    );

    // Same reasoning, same reliance, but the path was never created. Scoping
    // the post-condition to the current batch left nothing to re-ask the disk.
    let broken = tempfile::tempdir().unwrap();
    let (_broken_db, broken_db_string) = db_with_prior_transaction(
        broken.path(),
        vec![crypto_policies_with_promise(promised_path)],
    );

    let error = BatchInstaller::new(&broken_db_string, SandboxMode::Always)
        .install_batch(later_batch())
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(
        message.contains(&format!(
            "installed package crypto-policies-1.0.0 no longer holds promised path {promised_path}"
        )),
        "{message}"
    );
    // The remedy differs from a batch member that never materialized its own
    // promise, so the diagnostic names the installed holder to repair.
    assert!(
        message.contains("repair or reinstall crypto-policies"),
        "{message}"
    );
    assert!(
        message.contains("krb5-libs depends on it through"),
        "{message}"
    );
}

#[test]
fn an_installed_promise_no_batch_requirement_names_is_never_verified() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();

    // Fedora's setup owns /etc/fstab as a %ghost and never creates it. Being
    // installed does not put it in a later transaction's reach: only a promise
    // that transaction leaned on is its business.
    let mut setup = prepared_test_package("setup", "/etc/profile", b"profile");
    setup.provides.push(promised("/etc/fstab"));
    setup.provides.push(promised("/run/motd"));
    let (db_path, db_path_string) = db_with_prior_transaction(temp.path(), vec![setup]);
    let mut later = prepared_test_package("filesystem", "/usr/bin/filesystem-marker", b"marker");
    later.requirements = vec![depends_on("setup")];

    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![later])
        .unwrap();

    assert!(
        every_changeset_applied(&db_path),
        "an installed promise nothing in the batch requires must not be verified"
    );
}

#[test]
fn an_installed_promise_an_alternative_satisfies_is_never_verified() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let promised_path = "/etc/crypto-policies/back-ends/krb5.config";

    // The requirement names the promised path, but its other alternative is
    // satisfied too, so the transaction never leaned on the path. Naming an
    // atom is not relying on it -- which is why membership in the batch's atom
    // set selects candidates and the counterfactual decides.
    let (db_path, db_path_string) = db_with_prior_transaction(
        temp.path(),
        vec![crypto_policies_with_promise(promised_path)],
    );
    let mut later = prepared_test_package("krb5-libs", "/usr/lib64/libkrb5.so.3", b"krb5");
    later.requirements = vec![
        conary_core::repository::dependency_model::RepositoryRequirementGroup::alternatives(
            conary_core::repository::dependency_model::RepositoryRequirementKind::Depends,
            vec![
                conary_core::repository::dependency_model::RepositoryRequirementClause::name_only(
                    promised_path.to_string(),
                ),
                conary_core::repository::dependency_model::RepositoryRequirementClause::name_only(
                    "crypto-policies".to_string(),
                ),
            ],
        ),
    ];

    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![later])
        .unwrap();

    assert!(
        every_changeset_applied(&db_path),
        "a promise an alternative made dispensable must not be verified"
    );
}

fn package_promising(name: &str, payload: &str, promised_path: &str) -> PreparedPackage {
    let mut package = prepared_test_package(name, payload, b"payload");
    package.provides.push(promised(promised_path));
    package.install_reason = InstallReason::Dependency;
    package
}

fn krb5_depending_on(
    requirement: conary_core::repository::dependency_model::RepositoryRequirementGroup,
) -> PreparedPackage {
    let mut dependent = prepared_test_package("krb5-libs", "/usr/lib64/libkrb5.so.3", b"krb5");
    dependent.requirements = vec![requirement];
    dependent
}

fn any_of(
    first: &str,
    second: &str,
) -> conary_core::repository::dependency_model::RepositoryRequirementGroup {
    conary_core::repository::dependency_model::RepositoryRequirementGroup::alternatives(
        conary_core::repository::dependency_model::RepositoryRequirementKind::Depends,
        vec![
            conary_core::repository::dependency_model::RepositoryRequirementClause::name_only(
                first.to_string(),
            ),
            conary_core::repository::dependency_model::RepositoryRequirementClause::name_only(
                second.to_string(),
            ),
        ],
    )
}

#[test]
fn two_packages_promising_one_path_cannot_alibi_each_other() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let promised_path = "/etc/crypto-policies/back-ends/krb5.config";

    // One installed holder and one incoming holder promise the same path, and
    // nothing creates it. Asking whether either promise alone is load-bearing
    // gets "no" twice -- each is excused by the other -- and an unusable
    // dependency commits. The question has to be asked of the path.
    let (_db_path, db_path_string) = db_with_prior_transaction(
        temp.path(),
        vec![crypto_policies_with_promise(promised_path)],
    );

    let error = BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![
            krb5_depending_on(depends_on(promised_path)),
            package_promising(
                "crypto-policies-extra",
                "/usr/share/crypto-policies/EXTRA",
                promised_path,
            ),
        ])
        .unwrap_err();

    let message = format!("{error:#}");
    // Both holders are named, each with the remedy its own side needs.
    assert!(
        message.contains(&format!(
            "installed package crypto-policies-1.0.0 no longer holds promised path {promised_path}"
        )),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "package crypto-policies-extra-1.0.0 did not materialize promised path {promised_path}"
        )),
        "{message}"
    );
    assert!(
        message.contains("krb5-libs depends on it through"),
        "{message}"
    );
}

#[test]
fn two_packages_promising_one_present_path_satisfy_the_post_condition() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let promised_path = "/etc/crypto-policies/back-ends/krb5.config";

    // Same two holders, but the path is really there. The post-condition asks
    // the disk about the path, not about who is credited with it.
    let (db_path, db_path_string) = db_with_prior_transaction(
        temp.path(),
        vec![
            crypto_policies_with_promise(promised_path),
            prepared_test_package("materializer", promised_path, b"generated"),
        ],
    );

    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![
            krb5_depending_on(depends_on(promised_path)),
            package_promising(
                "crypto-policies-extra",
                "/usr/share/crypto-policies/EXTRA",
                promised_path,
            ),
        ])
        .unwrap();

    assert!(
        every_changeset_applied(&db_path),
        "a present path must satisfy every package that promised it"
    );
}

#[test]
fn an_or_of_two_promised_paths_fails_when_neither_materializes() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let first = "/etc/promise-first";
    let second = "/etc/promise-second";

    // Two promised alternatives excuse each other exactly like two holders of
    // one path do, one level up the expression.
    let (_db_path, db_path_string) = db_with_prior_transaction(
        temp.path(),
        vec![
            package_promising("holder-first", "/usr/share/holder-first", first),
            package_promising("holder-second", "/usr/share/holder-second", second),
        ],
    );

    let error = BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![krb5_depending_on(any_of(first, second))])
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(
        message.contains(&format!(
            "installed package holder-first-1.0.0 no longer holds promised path {first}"
        )),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "installed package holder-second-1.0.0 no longer holds promised path {second}"
        )),
        "{message}"
    );
}

#[test]
fn an_or_of_two_promised_paths_passes_when_one_materializes() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let first = "/etc/promise-first";
    let second = "/etc/promise-second";

    // The requirement is satisfied by either alternative, so one materialized
    // promise is the whole obligation. Demanding both would reject a correct
    // transaction -- the exact over-enforcement #300 removed.
    let (db_path, db_path_string) = db_with_prior_transaction(
        temp.path(),
        vec![
            package_promising("holder-first", "/usr/share/holder-first", first),
            package_promising("holder-second", "/usr/share/holder-second", second),
            prepared_test_package("materializer", second, b"generated"),
        ],
    );

    BatchInstaller::new(&db_path_string, SandboxMode::Always)
        .install_batch(vec![krb5_depending_on(any_of(first, second))])
        .unwrap();

    assert!(
        every_changeset_applied(&db_path),
        "one materialized alternative must satisfy an OR of promised paths"
    );
}
