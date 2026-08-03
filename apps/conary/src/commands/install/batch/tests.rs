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
    crate::commands::test_helpers::seed_test_bootable_runtime(&db_path);

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
