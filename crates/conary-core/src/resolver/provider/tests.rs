// conary-core/src/resolver/provider/tests.rs

use super::*;
use crate::db;
use crate::db::models::{
    CanonicalPackage, InstalledRequirementGroup, RepologyCacheEntry, Repository, RepositoryPackage,
    RepositoryProvide, RepositoryRequirement, RepositoryRequirementGroup, Trove,
};
use crate::repository::dependency_model::{
    RepositoryRequirementClause, RepositoryRequirementExpression,
};
use crate::repository::resolution_policy::{ResolutionPolicy, SelectionMode};
use crate::repository::versioning::RepoVersionConstraint;
use crate::resolver::identity::ProvidedCapability;
use crate::version::VersionConstraint;
use futures::executor::block_on;
use resolvo::{DependencyProvider, SolverCache};

fn setup_test_db() -> (tempfile::TempDir, rusqlite::Connection) {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    db::init(&db_path).unwrap();
    let conn = db::open(&db_path).unwrap();
    (temp_dir, conn)
}

fn atom_expression(name: &str) -> RepositoryRequirementExpression {
    RepositoryRequirementExpression::Atom(RepositoryRequirementClause::name_only(name.to_string()))
}

fn versioned_expression(name: &str, constraint: &str) -> RepositoryRequirementExpression {
    RepositoryRequirementExpression::Atom(RepositoryRequirementClause::versioned(
        name.to_string(),
        constraint.to_string(),
    ))
}

fn expression_json(expression: &RepositoryRequirementExpression) -> String {
    serde_json::to_string(expression).unwrap()
}

fn insert_installed_requirement(
    conn: &rusqlite::Connection,
    trove_id: i64,
    scheme: VersionScheme,
    native: &str,
) {
    let requirement = crate::repository::requirement::parse_native_requirement(
        crate::repository::dependency_model::RepositoryRequirementKind::Depends,
        scheme,
        native,
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(conn, trove_id, scheme, &[requirement]).unwrap();
}

/// Helper to build a test `PackageIdentity` for installed packages.
fn installed_identity(
    name: &str,
    version: &str,
    scheme: VersionScheme,
    trove_id: Option<i64>,
) -> PackageIdentity {
    PackageIdentity {
        repo_package_id: None,
        name: name.to_string(),
        version: version.to_string(),
        package_release: None,
        architecture: None,
        version_scheme: scheme,
        repository_id: None,
        repository_name: String::new(),
        repository_distro: None,
        repository_priority: 0,
        canonical_id: None,
        canonical_name: None,
        installed_trove_id: trove_id,
        installed_pinned: false,
        provided_capabilities: Vec::new(),
    }
}

/// Helper to build a test `PackageIdentity` for repo packages.
fn repo_identity(
    name: &str,
    version: &str,
    scheme: VersionScheme,
    repo_package_id: Option<i64>,
) -> PackageIdentity {
    PackageIdentity {
        repo_package_id,
        name: name.to_string(),
        version: version.to_string(),
        package_release: None,
        architecture: None,
        version_scheme: scheme,
        repository_id: None,
        repository_name: String::new(),
        repository_distro: None,
        repository_priority: 0,
        canonical_id: None,
        canonical_name: None,
        installed_trove_id: None,
        installed_pinned: false,
        provided_capabilities: Vec::new(),
    }
}

#[test]
fn resolver_uses_loaded_pin_authority_and_excludes_uncompiled_candidates() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);
    let name = provider.intern_name("fixture").unwrap();
    let mut installed = installed_identity("fixture", "1", VersionScheme::Rpm, Some(7));
    installed.installed_pinned = true;
    let installed_id = provider.add_solvable(installed).unwrap();
    provider
        .add_solvable(repo_identity("fixture", "2", VersionScheme::Rpm, Some(8)))
        .unwrap();

    let candidates = block_on(provider.get_candidates(name)).unwrap();
    assert_eq!(candidates.locked, Some(installed_id));
    assert!(matches!(
        block_on(provider.get_dependencies(installed_id)),
        resolvo::Dependencies::Unknown(_)
    ));

    provider.intern_all_dependency_version_sets().unwrap();
    assert!(matches!(
        block_on(provider.get_dependencies(installed_id)),
        resolvo::Dependencies::Known(_)
    ));
}

#[test]
fn diagnostic_metadata_dependency_json_is_not_solver_authority() {
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "kernel".to_string(),
        "6.19.6-200.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:deadbeef".to_string(),
        1,
        "https://example.invalid/kernel.rpm".to_string(),
    );
    pkg.metadata = Some(
        serde_json::json!({
            "dependencies": [
                "kernel-core-uname-r = 6.19.6-200.fc44.x86_64",
                "coreutils >= 9.7"
            ]
        })
        .to_string(),
    );
    pkg.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider
        .load_repo_packages_for_names(&["kernel".to_string()])
        .unwrap();

    assert!(
        provider.dependencies.values().all(|deps| deps.is_empty()),
        "only normalized repository requirement rows may drive SAT"
    );
}

#[test]
fn load_repo_packages_uses_normalized_repository_requirements() {
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new("arch-core".to_string(), "https://example.invalid".into());
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "ripgrep".to_string(),
        "14.1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:deadbeef".to_string(),
        1,
        "https://example.invalid/ripgrep.pkg.tar.zst".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let repo_package_id = pkg.id.unwrap();

    let mut group = RepositoryRequirementGroup::new(
        repo_package_id,
        "depends".to_string(),
        "hard".to_string(),
        expression_json(&versioned_expression("glibc", ">= 2.39")),
    );
    group.native_text = Some("glibc >= 2.39".to_string());
    group.insert(&conn).unwrap();

    let mut requirement = RepositoryRequirement::new(
        repo_package_id,
        group.id.unwrap(),
        "glibc".to_string(),
        Some(">= 2.39".to_string()),
        "package".to_string(),
        "runtime".to_string(),
        Some("glibc >= 2.39".to_string()),
    );
    requirement.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider
        .load_repo_packages_for_names(&["ripgrep".to_string()])
        .unwrap();

    let deps = provider
        .dependencies
        .values()
        .find(|deps| deps.iter().any(|d| d.single_name() == Some("glibc")))
        .cloned()
        .unwrap();

    assert_eq!(deps.len(), 1);
    let (name, constraint) = deps[0].as_single().expect("expected Single dep");
    assert_eq!(name, "glibc");
    assert_eq!(
        *constraint,
        ConaryConstraint::Repository {
            scheme: VersionScheme::Arch,
            constraint: RepoVersionConstraint::GreaterOrEqual("2.39".to_string()),
            raw: Some(">= 2.39".to_string()),
        }
    );
}

#[test]
fn repository_clause_without_expression_group_is_rejected() {
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new("arch-core".to_string(), "https://example.invalid".into());
    let repo_id = repo.insert(&conn).unwrap();
    let mut pkg = RepositoryPackage::new(
        repo_id,
        "ripgrep".to_string(),
        "14.1.0-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:deadbeef".to_string(),
        1,
        "https://example.invalid/ripgrep.pkg.tar.zst".to_string(),
    );
    pkg.insert(&conn).unwrap();

    let error = RepositoryRequirement::new(
        pkg.id.unwrap(),
        0,
        "glibc".to_string(),
        Some(">= 2.39".to_string()),
        "package".to_string(),
        "runtime".to_string(),
        Some("glibc >= 2.39".to_string()),
    )
    .insert(&conn)
    .unwrap_err();
    assert!(
        error.to_string().contains("has no authoritative group"),
        "{error}"
    );
}

#[test]
fn load_repo_packages_uses_normalized_repository_provides() {
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new("fedora-remi".to_string(), "https://example.invalid".into());
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "kernel-core".to_string(),
        "6.19.6-200.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:deadbeef".to_string(),
        1,
        "https://example.invalid/kernel-core.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let repo_package_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        repo_package_id,
        "kernel-core-uname-r".to_string(),
        Some("6.19.6-200.fc44.x86_64".to_string()),
        "package".to_string(),
        Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64".to_string()),
        VersionScheme::Rpm,
    );
    provide.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider
        .load_repo_packages_for_names(&["kernel-core".to_string()])
        .unwrap();

    let loaded = provider
        .solvables
        .iter()
        .find(|pkg| pkg.name == "kernel-core" && pkg.repo_package_id == Some(repo_package_id))
        .unwrap();

    assert!(loaded.provided_capabilities.iter().any(|capability| {
        capability.name == "kernel-core-uname-r"
            && capability.version == Some("6.19.6-200.fc44.x86_64".to_string())
            && capability.version_scheme == VersionScheme::Rpm
    }));
}

#[test]
fn test_intern_name_roundtrip() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);

    let id1 = provider.intern_name("nginx").unwrap();
    let id2 = provider.intern_name("nginx").unwrap();
    let id3 = provider.intern_name("curl").unwrap();

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
    assert_eq!(provider.names[id1.0 as usize], "nginx");
    assert_eq!(provider.names[id3.0 as usize], "curl");
}

#[test]
fn test_version_set_filtering() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);

    let name_id = provider.intern_name("lib").unwrap();
    let constraint = VersionConstraint::parse(">= 2.0.0").unwrap();
    let vs_id = provider.intern_version_set(name_id, constraint).unwrap();

    // Add candidates at different versions
    let s1 = provider
        .add_solvable(installed_identity("lib", "1.0.0", VersionScheme::Rpm, None))
        .unwrap();
    let s2 = provider
        .add_solvable(installed_identity("lib", "2.0.0", VersionScheme::Rpm, None))
        .unwrap();
    let s3 = provider
        .add_solvable(installed_identity("lib", "3.0.0", VersionScheme::Rpm, None))
        .unwrap();

    let candidates = [s1, s2, s3];

    // Test the filtering logic directly
    let (_, ref constraint) = provider.version_sets[vs_id.0 as usize];
    let matching: Vec<SolvableId> = candidates
        .iter()
        .copied()
        .filter(|&sid| {
            let pkg = &provider.solvables[sid.0 as usize];
            constraint_matches_package(constraint, &pkg.version, pkg.version_scheme)
                .expect("valid test versions")
        })
        .collect();

    assert_eq!(matching.len(), 2);
    assert!(matching.contains(&s2));
    assert!(matching.contains(&s3));
    assert!(!matching.contains(&s1));
}

#[test]
fn test_favored_installed() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);

    let name_id = provider.intern_name("nginx").unwrap();

    // Add an installed version
    let installed = provider
        .add_solvable(installed_identity(
            "nginx",
            "1.0.0",
            VersionScheme::Rpm,
            Some(42),
        ))
        .unwrap();

    // Add a repo version
    let _repo = provider
        .add_solvable(repo_identity(
            "nginx",
            "2.0.0",
            VersionScheme::Rpm,
            Some(100),
        ))
        .unwrap();

    // Test candidates lookup logic directly
    let candidates = provider.solvables_for_name(name_id);
    let favored = provider.installed_solvable_for_name(name_id);

    assert_eq!(candidates.len(), 2);
    assert_eq!(favored, Some(installed));
}

#[test]
fn test_unknown_version_set_ids_are_rejected_before_solver_callbacks() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);

    let err = provider
        .intern_version_set(
            NameId(u32::MAX),
            VersionConstraint::parse("*").expect("valid test constraint"),
        )
        .expect_err("unknown name IDs must be rejected");
    assert!(err.to_string().contains("unknown resolver name ID"));

    let err = provider
        .intern_version_set_union(vec![VersionSetId(u32::MAX)])
        .expect_err("unknown version-set IDs must be rejected");
    assert!(err.to_string().contains("unknown version-set ID"));
}

#[test]
fn test_corrupt_solvable_name_index_is_a_typed_invariant_error() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);
    provider
        .add_solvable(installed_identity(
            "pkg",
            "1.0",
            VersionScheme::Rpm,
            Some(1),
        ))
        .unwrap();
    provider
        .name_to_solvable_ids
        .get_mut("pkg")
        .unwrap()
        .push(SolvableId(99));

    let err = provider
        .intern_all_dependency_version_sets()
        .expect_err("a corrupt solver index must be rejected before SAT");
    assert!(err.to_string().contains("unknown candidate ID 99"));
}

#[test]
fn test_sort_candidates_version_descending() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);

    let s1 = provider
        .add_solvable(installed_identity("pkg", "1.0.0", VersionScheme::Rpm, None))
        .unwrap();
    let s2 = provider
        .add_solvable(installed_identity("pkg", "3.0.0", VersionScheme::Rpm, None))
        .unwrap();
    let s3 = provider
        .add_solvable(installed_identity(
            "pkg",
            "2.0.0",
            VersionScheme::Rpm,
            Some(1),
        ))
        .unwrap();

    // Test sort logic directly
    let mut solvables = [s1, s2, s3];
    solvables.sort_by(|a, b| {
        let pkg_a = &provider.solvables[a.0 as usize];
        let pkg_b = &provider.solvables[b.0 as usize];
        let version_cmp = matching::compare_package_versions_desc(
            &pkg_a.version,
            pkg_a.version_scheme,
            &pkg_b.version,
            pkg_b.version_scheme,
        )
        .expect("valid test versions");
        if version_cmp != std::cmp::Ordering::Equal {
            return version_cmp;
        }
        let a_installed = pkg_a.installed_trove_id.is_some();
        let b_installed = pkg_b.installed_trove_id.is_some();
        b_installed.cmp(&a_installed)
    });

    // Should be: 3.0.0 (repo), 2.0.0 (installed), 1.0.0 (repo)
    assert_eq!(solvables[0], s2); // 3.0.0
    assert_eq!(solvables[1], s3); // 2.0.0 (installed)
    assert_eq!(solvables[2], s1); // 1.0.0
}

#[test]
fn sort_candidates_prefers_latest_signal_when_policy_requests_it() {
    let (_dir, conn) = setup_test_db();
    let fresh = chrono::Utc::now().to_rfc3339();

    let mut canonical = CanonicalPackage::new("python".to_string(), "package".to_string());
    let canonical_id = canonical.insert(&conn).unwrap();

    let mut fedora_repo = Repository::new(
        "fedora-remi".to_string(),
        "https://example.invalid".to_string(),
    );
    fedora_repo.priority = 20;
    fedora_repo.default_strategy_distro = Some("fedora".to_string());
    let fedora_repo_id = fedora_repo.insert(&conn).unwrap();

    let mut arch_repo = Repository::new(
        "arch-core".to_string(),
        "https://example.invalid".to_string(),
    );
    arch_repo.priority = 5;
    arch_repo.default_strategy_distro = Some("arch".to_string());
    let arch_repo_id = arch_repo.insert(&conn).unwrap();

    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'python', '3.12.2-1.fc44', 'sha256:fedora', 1, 'https://example.invalid/python-fedora.rpm', 'rpm', ?2)",
            rusqlite::params![fedora_repo_id, canonical_id],
        )
        .unwrap();
    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'python', '3.13.0-1', 'sha256:arch', 1, 'https://example.invalid/python-arch.pkg.tar.zst', 'arch', ?2)",
            rusqlite::params![arch_repo_id, canonical_id],
        )
        .unwrap();

    RepologyCacheEntry::insert_or_replace(
        &conn,
        &RepologyCacheEntry {
            project_name: "python".into(),
            distro: "fedora".into(),
            distro_name: "python".into(),
            version: Some("3.12.2".into()),
            status: Some("outdated".into()),
            fetched_at: fresh.clone(),
        },
    )
    .unwrap();
    RepologyCacheEntry::insert_or_replace(
        &conn,
        &RepologyCacheEntry {
            project_name: "python".into(),
            distro: "arch".into(),
            distro_name: "python".into(),
            version: Some("3.13.0".into()),
            status: Some("newest".into()),
            fetched_at: fresh,
        },
    )
    .unwrap();

    let mut provider = ConaryProvider::new_with_policy(
        &conn,
        ResolutionPolicy::new().with_selection_mode(SelectionMode::Latest),
    );
    provider
        .load_repo_packages_for_names(&["python".to_string()])
        .unwrap();

    let name_id = provider.intern_name("python").unwrap();
    let mut solvables = provider.solvables_for_name(name_id);
    assert_eq!(solvables.len(), 2);

    let cache = SolverCache::new(provider);
    block_on(cache.provider().sort_candidates(&cache, &mut solvables));

    assert_eq!(
        cache.provider().get_solvable(solvables[0]).repository_name,
        "arch-core"
    );
}

#[test]
fn test_display_methods() {
    use resolvo::Interner;

    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);

    let name_id = provider.intern_name("nginx").unwrap();
    let vs_id = provider
        .intern_version_set(name_id, VersionConstraint::parse(">= 1.0.0").unwrap())
        .unwrap();
    let sid = provider
        .add_solvable(installed_identity(
            "nginx",
            "1.24.0",
            VersionScheme::Rpm,
            None,
        ))
        .unwrap();
    let str_id = provider.intern_string("test string").unwrap();

    assert_eq!(provider.display_name(name_id).to_string(), "nginx");
    assert_eq!(provider.display_solvable(sid).to_string(), "nginx=1.24.0");
    assert_eq!(provider.display_version_set(vs_id).to_string(), ">= 1.0.0");
    assert_eq!(provider.display_string(str_id).to_string(), "test string");
    assert_eq!(provider.version_set_name(vs_id), name_id);
    assert_eq!(provider.solvable_name(sid), name_id);
}

#[test]
fn filter_candidates_uses_provide_version_for_virtual_capabilities() {
    let (_dir, conn) = setup_test_db();
    let mut provider = ConaryProvider::new(&conn);

    let capability_name = provider.intern_name("kernel-modules-core-uname-r").unwrap();
    let version_set = provider
        .intern_version_set(
            capability_name,
            VersionConstraint::parse("= 6.19.6").unwrap(),
        )
        .unwrap();

    let mut identity = installed_identity(
        "kernel-modules-core",
        "6.19.6-200.fc44",
        VersionScheme::Rpm,
        None,
    );
    identity.repo_package_id = Some(42);
    identity.provided_capabilities = vec![ProvidedCapability {
        name: "kernel-modules-core-uname-r".to_string(),
        version: Some("6.19.6".to_string()),
        version_scheme: VersionScheme::Rpm,
    }];
    let candidate = provider.add_solvable(identity).unwrap();

    let requested_name = &provider.names[capability_name.0 as usize];
    let (_, constraint) = &provider.version_sets[version_set.0 as usize];
    let matching: Vec<SolvableId> = [candidate]
        .into_iter()
        .filter(|sid| {
            let pkg = &provider.solvables[sid.0 as usize];
            let provided = pkg
                .provided_capabilities
                .iter()
                .find(|capability| capability.name == *requested_name);
            let Some(provided) = provided else {
                return false;
            };
            matching::constraint_matches_provide(
                constraint,
                provided.version.as_deref(),
                provided.version_scheme,
            )
            .expect("valid test capability version")
        })
        .collect();
    assert_eq!(matching, vec![candidate]);
}

#[test]
fn or_group_loading_from_requirement_groups() {
    use crate::db::models::RepositoryRequirementGroup;

    // Debian OR dependency: default-mta | mail-transport-agent
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new("ubuntu-main".to_string(), "https://example.invalid".into());
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "bsd-mailx".to_string(),
        "8.1.2-0.20220stringe-0ubuntu1".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:mailx".to_string(),
        1,
        "https://example.invalid/bsd-mailx.deb".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    // Create a group for the OR dependency
    let or_expression = RepositoryRequirementExpression::Or(vec![
        atom_expression("default-mta"),
        atom_expression("mail-transport-agent"),
    ]);
    let mut group = RepositoryRequirementGroup::new(
        pkg_id,
        "depends".to_string(),
        "hard".to_string(),
        expression_json(&or_expression),
    );
    group.native_text = Some("default-mta | mail-transport-agent".to_string());
    group.insert(&conn).unwrap();
    let group_id = group.id.unwrap();

    // Insert both alternatives
    let mut clause_a = RepositoryRequirement::new(
        pkg_id,
        group_id,
        "default-mta".to_string(),
        None,
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    clause_a.insert(&conn).unwrap();

    let mut clause_b = RepositoryRequirement::new(
        pkg_id,
        group_id,
        "mail-transport-agent".to_string(),
        None,
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    clause_b.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider
        .load_repo_packages_for_names(&["bsd-mailx".to_string()])
        .unwrap();

    // Find the dependency list for bsd-mailx
    let deps = provider.dependencies.values().next().cloned().unwrap();
    assert_eq!(deps.len(), 1, "should have exactly one (OR) dependency");

    match &deps[0].expression {
        SolverExpression::Or(alternatives) => {
            assert_eq!(alternatives.len(), 2);
            assert_eq!(alternatives[0].atoms()[0].name, "default-mta");
            assert_eq!(alternatives[1].atoms()[0].name, "mail-transport-agent");
        }
        expression => panic!("expected exact OR expression, got {expression:?}"),
    }
}

#[test]
fn conditional_deps_compile_into_solver_requirements() {
    use crate::db::models::RepositoryRequirementGroup;

    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new("fedora-main".to_string(), "https://example.invalid".into());
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "systemd".to_string(),
        "256-1.fc44".to_string(),
        crate::repository::versioning::VersionScheme::Rpm,
        "sha256:systemd".to_string(),
        1,
        "https://example.invalid/systemd.rpm".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    // A hard dependency
    let mut hard_group = RepositoryRequirementGroup::new(
        pkg_id,
        "depends".to_string(),
        "hard".to_string(),
        expression_json(&versioned_expression("glibc", ">= 2.39")),
    );
    hard_group.insert(&conn).unwrap();
    let hard_group_id = hard_group.id.unwrap();

    let mut hard_req = RepositoryRequirement::new(
        pkg_id,
        hard_group_id,
        "glibc".to_string(),
        Some(">= 2.39".to_string()),
        "package".to_string(),
        "runtime".to_string(),
        None,
    );
    hard_req.insert(&conn).unwrap();

    let conditional_expression = RepositoryRequirementExpression::If {
        requirement: Box::new(atom_expression("systemd-boot")),
        condition: Box::new(atom_expression("efi-filesystem")),
        otherwise: None,
    };
    let mut cond_group = RepositoryRequirementGroup::new(
        pkg_id,
        "depends".to_string(),
        "conditional".to_string(),
        expression_json(&conditional_expression),
    );
    cond_group.native_text = Some("(systemd-boot if efi-filesystem)".to_string());
    cond_group.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider
        .load_repo_packages_for_names(&["systemd".to_string()])
        .unwrap();

    let deps = provider.dependencies.values().next().cloned().unwrap();
    assert_eq!(deps.len(), 2);
    let (name, _) = deps[0].as_single().expect("expected Single dep");
    assert_eq!(name, "glibc");
    let conditional_atoms = deps[1]
        .expression
        .atoms()
        .into_iter()
        .map(|atom| atom.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(conditional_atoms, vec!["efi-filesystem", "systemd-boot"]);

    provider.intern_all_dependency_version_sets().unwrap();
    let compiled = provider
        .compiled_dependencies
        .values()
        .next()
        .expect("compiled requirements");
    assert_eq!(compiled.len(), 2);
    assert!(
        compiled
            .iter()
            .any(|requirement| requirement.condition.is_some())
    );
}

#[test]
fn debian_versioned_provide_uses_provide_version_not_package_version() {
    // Package libc6 version 2.39-0ubuntu2 provides libc6 (= 2.39-0ubuntu2)
    // A dep on libc6 (>= 2.38) should match via the provide version, not
    // attempt to parse "2.39-0ubuntu2" as RPM.
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new("ubuntu-main".to_string(), "https://example.invalid".into());
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "libc6".to_string(),
        "2.39-0ubuntu2".to_string(),
        crate::repository::versioning::VersionScheme::Debian,
        "sha256:libc6".to_string(),
        1,
        "https://example.invalid/libc6.deb".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        pkg_id,
        "libc6".to_string(),
        Some("2.39-0ubuntu2".to_string()),
        "package".to_string(),
        Some("libc6 (= 2.39-0ubuntu2)".to_string()),
        VersionScheme::Debian,
    );
    provide.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider
        .load_repo_packages_for_names(&["libc6".to_string()])
        .unwrap();

    let pkg_solvable = provider
        .solvables
        .iter()
        .find(|s| s.name == "libc6" && s.repo_package_id.is_some())
        .unwrap();

    // The provide version should be the raw string
    let provide = pkg_solvable
        .provided_capabilities
        .iter()
        .find(|capability| capability.name == "libc6")
        .unwrap();
    assert_eq!(provide.version, Some("2.39-0ubuntu2".to_string()));
    assert_eq!(provide.version_scheme, VersionScheme::Debian);
}

#[test]
fn arch_versioned_provide_uses_native_scheme() {
    let (_dir, conn) = setup_test_db();

    let mut repo = Repository::new("arch-core".to_string(), "https://example.invalid".into());
    let repo_id = repo.insert(&conn).unwrap();

    let mut pkg = RepositoryPackage::new(
        repo_id,
        "sh".to_string(),
        "5.2.037-1".to_string(),
        crate::repository::versioning::VersionScheme::Arch,
        "sha256:sh".to_string(),
        1,
        "https://example.invalid/sh.pkg.tar.zst".to_string(),
    );
    pkg.insert(&conn).unwrap();
    let pkg_id = pkg.id.unwrap();

    let mut provide = RepositoryProvide::new(
        pkg_id,
        "sh".to_string(),
        Some("5.2.037".to_string()),
        "package".to_string(),
        Some("sh=5.2.037".to_string()),
        VersionScheme::Arch,
    );
    provide.insert(&conn).unwrap();

    let mut provider = ConaryProvider::new(&conn);
    provider
        .load_repo_packages_for_names(&["sh".to_string()])
        .unwrap();

    let pkg_solvable = provider
        .solvables
        .iter()
        .find(|s| s.name == "sh" && s.repo_package_id.is_some())
        .unwrap();

    let provide = pkg_solvable
        .provided_capabilities
        .iter()
        .find(|capability| capability.name == "sh")
        .unwrap();
    assert_eq!(provide.version, Some("5.2.037".to_string()));
    assert_eq!(provide.version_scheme, VersionScheme::Arch);
}

#[test]
fn installed_debian_package_uses_native_version_scheme() {
    use crate::db::models::{Changeset, ChangesetStatus, TroveType};

    let (_dir, conn) = setup_test_db();

    let mut changeset = Changeset::new("Install libc6".to_string());
    changeset.insert(&conn).unwrap();
    changeset
        .update_status(&conn, ChangesetStatus::Applied)
        .unwrap();

    let mut trove = Trove::new(
        "libc6".to_string(),
        "2.39-0ubuntu2".to_string(),
        TroveType::Package,
        VersionScheme::Debian,
    );
    trove.source_distro = Some("ubuntu".to_string());
    let trove_id = trove.insert(&conn).unwrap();

    insert_installed_requirement(&conn, trove_id, VersionScheme::Debian, "libgcc-s1 (>= 3.0)");

    let mut provider = ConaryProvider::new(&conn);
    provider.load_installed_packages().unwrap();

    // Version should use Debian scheme
    let solvable = provider
        .solvables
        .iter()
        .find(|s| s.name == "libc6")
        .unwrap();
    assert_eq!(solvable.version, "2.39-0ubuntu2");
    assert_eq!(solvable.version_scheme, VersionScheme::Debian);

    // Dependencies should use Debian constraint scheme
    let deps = provider.dependencies.values().next().unwrap();
    let (name, constraint) = deps[0].as_single().unwrap();
    assert_eq!(name, "libgcc-s1");
    assert_eq!(
        *constraint,
        ConaryConstraint::Repository {
            scheme: VersionScheme::Debian,
            constraint: RepoVersionConstraint::GreaterOrEqual("3.0".to_string()),
            raw: Some(">= 3.0".to_string()),
        }
    );
}

#[path = "tests/installed_and_canonical.rs"]
mod installed_and_canonical;
