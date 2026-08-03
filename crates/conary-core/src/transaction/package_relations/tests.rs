// conary-core/src/transaction/package_relations/tests.rs

use super::*;
use crate::db::models::TroveType;
use crate::db::testing::create_test_db;
use crate::error::Error;
use crate::packages::traits::PackageFile;
use crate::repository::dependency_model::ProvidedCapability;
use crate::repository::dependency_model::RepositoryRequirementGroup;
use crate::repository::package_relation::parse_native_relation;
use crate::repository::requirement::parse_native_requirement;

struct TestPackage {
    name: String,
    version: String,
    provides: Vec<ProvidedCapability>,
    relations: Vec<RepositoryRequirementGroup>,
}

impl PackageFormat for TestPackage {
    fn parse(_path: &str) -> Result<Self> {
        Err(Error::ParseError("not used".to_string()))
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn version_scheme(&self) -> VersionScheme {
        VersionScheme::Rpm
    }

    fn architecture(&self) -> Option<&str> {
        None
    }

    fn description(&self) -> Option<&str> {
        None
    }

    fn files(&self) -> &[PackageFile] {
        &[]
    }

    fn requirements(&self) -> &[RepositoryRequirementGroup] {
        &[]
    }

    fn resolution_capabilities(&self) -> Result<Vec<ProvidedCapability>> {
        Ok(self.provides.clone())
    }

    fn relations(&self) -> &[RepositoryRequirementGroup] {
        &self.relations
    }

    fn package_payload(&self) -> Result<crate::packages::PackagePayload> {
        Ok(crate::packages::PackagePayload::default())
    }

    fn to_trove(&self) -> Trove {
        Trove::new(
            self.name.clone(),
            self.version.clone(),
            TroveType::Package,
            crate::repository::versioning::VersionScheme::Conary,
        )
    }
}

fn install_trove(conn: &Connection, name: &str, version: &str, scheme: VersionScheme) -> i64 {
    let mut trove = Trove::new(
        name.to_string(),
        version.to_string(),
        TroveType::Package,
        scheme,
    );
    trove.architecture = Some(
        if scheme == VersionScheme::Debian {
            "amd64"
        } else {
            "x86_64"
        }
        .to_string(),
    );
    trove.debian_multi_arch = (scheme == VersionScheme::Debian)
        .then_some(crate::repository::dependency_model::DebianMultiArch::No);
    trove.insert(conn).unwrap()
}

fn incoming(relation: RepositoryRequirementGroup) -> TestPackage {
    TestPackage {
        name: "newpkg".to_string(),
        version: "2.0-1".to_string(),
        provides: Vec::new(),
        relations: vec![relation],
    }
}

#[test]
fn unversioned_cross_format_conflict_is_a_constraint_removal() {
    let (_temp, conn) = create_test_db();
    let old_id = install_trove(&conn, "oldpkg", "1.0-1", VersionScheme::Rpm);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Debian,
        "oldpkg",
    )
    .unwrap();

    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Debian).unwrap();

    assert_eq!(plan.removals.len(), 1);
    assert_eq!(plan.removals[0].trove_id, old_id);
    assert_eq!(
        plan.removals[0].mode,
        PackageRelationRemovalMode::Constraint
    );
}

#[test]
fn versioned_relation_does_not_compare_across_native_schemes() {
    let (_temp, conn) = create_test_db();
    install_trove(&conn, "oldpkg", "1.0-1", VersionScheme::Rpm);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Replace,
        VersionScheme::Debian,
        "oldpkg (<< 9.0)",
    )
    .unwrap();

    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Debian).unwrap();

    assert!(plan.removals.is_empty());
}

#[test]
fn replace_requests_explicit_ownership_transfer() {
    let (_temp, conn) = create_test_db();
    let old_id = install_trove(&conn, "oldpkg", "1.0-1", VersionScheme::Debian);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Replace,
        VersionScheme::Debian,
        "oldpkg (<< 2.0)",
    )
    .unwrap();

    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Debian).unwrap();

    assert_eq!(plan.removals[0].trove_id, old_id);
    assert_eq!(
        plan.removals[0].mode,
        PackageRelationRemovalMode::OwnershipTransfer
    );
}

#[test]
fn installed_conflict_removes_its_installed_declarer() {
    let (_temp, conn) = create_test_db();
    let old_id = install_trove(&conn, "oldpkg", "1.0-1", VersionScheme::Rpm);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "newpkg",
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(&conn, old_id, VersionScheme::Rpm, &[relation])
        .unwrap();
    let incoming = TestPackage {
        name: "newpkg".to_string(),
        version: "2.0-1".to_string(),
        provides: Vec::new(),
        relations: Vec::new(),
    };

    let plan = plan_package_relations(&conn, &incoming, VersionScheme::Debian).unwrap();

    assert_eq!(plan.removals.len(), 1);
    assert_eq!(plan.removals[0].trove_id, old_id);
    assert_eq!(
        plan.removals[0].mode,
        PackageRelationRemovalMode::Constraint
    );
}

#[test]
fn rich_rpm_conflict_uses_exact_minimum_removal_set() {
    let (_temp, conn) = create_test_db();
    let first = install_trove(&conn, "old-a", "1", VersionScheme::Rpm);
    install_trove(&conn, "old-b", "1", VersionScheme::Rpm);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "(old-a and old-b)",
    )
    .unwrap();

    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Rpm).unwrap();

    assert_eq!(plan.removals.len(), 1);
    assert_eq!(plan.removals[0].trove_id, first);
}

#[test]
fn rpm_with_requires_both_capabilities_from_one_package() {
    let (_temp, conn) = create_test_db();
    let combined = install_trove(&conn, "combined", "1", VersionScheme::Rpm);
    let separate_a = install_trove(&conn, "separate-a", "1", VersionScheme::Rpm);
    let separate_b = install_trove(&conn, "separate-b", "1", VersionScheme::Rpm);
    for (trove_id, capability) in [
        (combined, "cap-a"),
        (combined, "cap-b"),
        (separate_a, "cap-a"),
        (separate_b, "cap-b"),
    ] {
        let mut provide =
            ProvideEntry::new(trove_id, capability.to_string(), None, VersionScheme::Rpm);
        provide.insert(&conn).unwrap();
    }
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "(cap-a with cap-b)",
    )
    .unwrap();

    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Rpm).unwrap();

    assert_eq!(plan.removals.len(), 1);
    assert_eq!(plan.removals[0].trove_id, combined);
}

#[test]
fn installed_rich_conflict_is_evaluated_with_incoming_and_installed_provides() {
    let (_temp, conn) = create_test_db();
    let declaring = install_trove(&conn, "oldpkg", "1", VersionScheme::Rpm);
    install_trove(&conn, "helper", "1", VersionScheme::Rpm);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "(newpkg and helper)",
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(&conn, declaring, VersionScheme::Rpm, &[relation])
        .unwrap();
    let incoming = TestPackage {
        name: "newpkg".to_string(),
        version: "2".to_string(),
        provides: Vec::new(),
        relations: Vec::new(),
    };

    let plan = plan_package_relations(&conn, &incoming, VersionScheme::Rpm).unwrap();

    assert_eq!(plan.removals.len(), 1);
    assert_eq!(plan.removals[0].trove_id, declaring);
}

#[test]
fn atomic_batch_attributes_installed_rich_conflict_to_all_causal_packages() {
    let (_temp, conn) = create_test_db();
    let declaring = install_trove(&conn, "oldpkg", "1", VersionScheme::Rpm);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "(new-a and new-b)",
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(&conn, declaring, VersionScheme::Rpm, &[relation])
        .unwrap();
    let first = TestPackage {
        name: "new-a".to_string(),
        version: "1".to_string(),
        provides: Vec::new(),
        relations: Vec::new(),
    };
    let second = TestPackage {
        name: "new-b".to_string(),
        version: "1".to_string(),
        provides: Vec::new(),
        relations: Vec::new(),
    };
    let incoming = [
        IncomingPackageRelations {
            name: first.name(),
            version: first.version(),
            architecture: first.architecture(),
            version_scheme: VersionScheme::Rpm,
            provides: &first.provides,
            relations: first.relations(),
        },
        IncomingPackageRelations {
            name: second.name(),
            version: second.version(),
            architecture: second.architecture(),
            version_scheme: VersionScheme::Rpm,
            provides: &second.provides,
            relations: second.relations(),
        },
    ];

    let plan = plan_package_relation_batch_facts(&conn, &incoming).unwrap();

    assert_eq!(plan.removals.len(), 1);
    assert_eq!(plan.removals[0].trove_id, declaring);
    assert_eq!(
        plan.removals[0].incoming_packages,
        vec!["new-a".to_string(), "new-b".to_string()]
    );
    assert!(plan.removals[0].ownership_transfer_packages.is_empty());
}

#[test]
fn atomic_batch_rejects_conflicting_co_selected_package() {
    let (_temp, conn) = create_test_db();
    let first = TestPackage {
        name: "new-a".to_string(),
        version: "1".to_string(),
        provides: Vec::new(),
        relations: vec![
            parse_native_relation(
                RepositoryRequirementKind::Conflict,
                VersionScheme::Rpm,
                "new-b",
            )
            .unwrap(),
        ],
    };
    let second = TestPackage {
        name: "new-b".to_string(),
        version: "1".to_string(),
        provides: Vec::new(),
        relations: Vec::new(),
    };
    let incoming = [
        IncomingPackageRelations {
            name: first.name(),
            version: first.version(),
            architecture: first.architecture(),
            version_scheme: VersionScheme::Rpm,
            provides: &first.provides,
            relations: first.relations(),
        },
        IncomingPackageRelations {
            name: second.name(),
            version: second.version(),
            architecture: second.architecture(),
            version_scheme: VersionScheme::Rpm,
            provides: &second.provides,
            relations: second.relations(),
        },
    ];

    let error = plan_package_relation_batch_facts(&conn, &incoming).unwrap_err();

    assert!(error.to_string().contains("co-selected package"));
}

#[test]
fn incoming_package_does_not_own_a_preexisting_installed_conflict() {
    let (_temp, conn) = create_test_db();
    let declaring = install_trove(&conn, "oldpkg", "1", VersionScheme::Rpm);
    install_trove(&conn, "helper", "1", VersionScheme::Rpm);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Rpm,
        "helper",
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(&conn, declaring, VersionScheme::Rpm, &[relation])
        .unwrap();
    let incoming = TestPackage {
        name: "newpkg".to_string(),
        version: "2".to_string(),
        provides: Vec::new(),
        relations: Vec::new(),
    };

    let plan = plan_package_relations(&conn, &incoming, VersionScheme::Rpm).unwrap();

    assert!(plan.removals.is_empty());
}

#[test]
fn relation_removal_rejects_pinned_target() {
    let (_temp, conn) = create_test_db();
    let old_id = install_trove(&conn, "oldpkg", "1", VersionScheme::Rpm);
    conn.execute("UPDATE troves SET pinned = 1 WHERE id = ?1", [old_id])
        .unwrap();
    let relation = parse_native_relation(
        RepositoryRequirementKind::Obsolete,
        VersionScheme::Rpm,
        "oldpkg",
    )
    .unwrap();
    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Rpm).unwrap();

    let error = validate_package_relation_plan(&conn, &plan).unwrap_err();

    assert!(error.to_string().contains("pinned package oldpkg 1"));
}

#[test]
fn relation_removal_plans_exact_deconfiguration_for_broken_dependent() {
    let (_temp, conn) = create_test_db();
    install_trove(&conn, "oldpkg", "1", VersionScheme::Rpm);
    let consumer = install_trove(&conn, "consumer", "1", VersionScheme::Rpm);
    let dependency = parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Rpm,
        "oldpkg",
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(&conn, consumer, VersionScheme::Rpm, &[dependency])
        .unwrap();
    let relation = parse_native_relation(
        RepositoryRequirementKind::Obsolete,
        VersionScheme::Rpm,
        "oldpkg",
    )
    .unwrap();
    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Rpm).unwrap();

    validate_package_relation_plan(&conn, &plan).unwrap();

    assert_eq!(plan.deconfigurations.len(), 1);
    let deconfiguration = &plan.deconfigurations[0];
    assert_eq!(deconfiguration.package.trove_id, consumer);
    assert_eq!(deconfiguration.triggering_incoming.package_name, "newpkg");
    assert_eq!(deconfiguration.dependency_depth, 1);
    assert!(matches!(
        &deconfiguration.cause,
        PackageRelationDeconfigurationCause::Dependency {
            kind: RepositoryRequirementKind::Depends,
            removing_conflict: Some(conflict),
            ..
        } if conflict.package_name == "oldpkg" && conflict.package_version == "1"
    ));
}

#[test]
fn breaks_deconfigures_matching_package_without_removing_it() {
    let (_temp, conn) = create_test_db();
    let old_id = install_trove(&conn, "oldpkg", "1.0-1", VersionScheme::Debian);
    let relation = parse_native_relation(
        RepositoryRequirementKind::Breaks,
        VersionScheme::Debian,
        "oldpkg (<< 2.0)",
    )
    .unwrap();

    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Debian).unwrap();

    assert!(plan.removals.is_empty());
    assert_eq!(plan.deconfigurations.len(), 1);
    assert_eq!(plan.deconfigurations[0].package.trove_id, old_id);
    assert!(matches!(
        plan.deconfigurations[0].cause,
        PackageRelationDeconfigurationCause::Breaks { .. }
    ));
}

#[test]
fn dependency_deconfiguration_is_ordered_dependents_before_providers() {
    let (_temp, conn) = create_test_db();
    install_trove(&conn, "oldpkg", "1", VersionScheme::Debian);
    let middle = install_trove(&conn, "middle", "1", VersionScheme::Debian);
    let leaf = install_trove(&conn, "leaf", "1", VersionScheme::Debian);
    for (trove_id, dependency) in [(middle, "oldpkg"), (leaf, "middle")] {
        let requirement = parse_native_requirement(
            RepositoryRequirementKind::Depends,
            VersionScheme::Debian,
            dependency,
        )
        .unwrap();
        InstalledRequirementGroup::insert_groups(
            &conn,
            trove_id,
            VersionScheme::Debian,
            &[requirement],
        )
        .unwrap();
    }
    let relation = parse_native_relation(
        RepositoryRequirementKind::Conflict,
        VersionScheme::Debian,
        "oldpkg",
    )
    .unwrap();

    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Debian).unwrap();

    assert_eq!(plan.deconfigurations.len(), 2);
    assert_eq!(plan.deconfigurations[0].package.package_name, "leaf");
    assert_eq!(plan.deconfigurations[0].dependency_depth, 2);
    assert_eq!(plan.deconfigurations[1].package.package_name, "middle");
    assert_eq!(plan.deconfigurations[1].dependency_depth, 1);
    for deconfiguration in &plan.deconfigurations {
        assert!(matches!(
            &deconfiguration.cause,
            PackageRelationDeconfigurationCause::Dependency {
                removing_conflict: Some(conflict),
                ..
            } if conflict.package_name == "oldpkg"
        ));
    }
}

#[test]
fn incoming_provider_prevents_unnecessary_deconfiguration() {
    let (_temp, conn) = create_test_db();
    let old = install_trove(&conn, "oldpkg", "1", VersionScheme::Debian);
    let consumer = install_trove(&conn, "consumer", "1", VersionScheme::Debian);
    let dependency = parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Debian,
        "virtual-api",
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(&conn, consumer, VersionScheme::Debian, &[dependency])
        .unwrap();
    let mut old_provide =
        ProvideEntry::new(old, "virtual-api".to_string(), None, VersionScheme::Debian);
    old_provide.insert(&conn).unwrap();
    let relation = parse_native_relation(
        RepositoryRequirementKind::Replace,
        VersionScheme::Debian,
        "oldpkg",
    )
    .unwrap();
    let package = TestPackage {
        name: "newpkg".to_string(),
        version: "2".to_string(),
        provides: vec![ProvidedCapability {
            kind: crate::repository::dependency_model::RepositoryCapabilityKind::Virtual,
            name: "virtual-api".to_string(),
            version: None,
            version_relation: None,
            version_scheme: VersionScheme::Debian,
            architecture_qualifier: Default::default(),
            provenance: crate::repository::dependency_model::CapabilityProvenance::AuthorDeclared,
        }],
        relations: vec![relation],
    };

    let plan = plan_package_relations(&conn, &package, VersionScheme::Debian).unwrap();

    assert_eq!(plan.removals.len(), 1);
    assert!(plan.deconfigurations.is_empty());
}

#[test]
fn exact_relation_target_keeps_dependent_satisfied_by_other_instance() {
    let (_temp, conn) = create_test_db();
    let old = install_trove(&conn, "oldpkg", "1", VersionScheme::Rpm);
    let current = install_trove(&conn, "oldpkg", "2", VersionScheme::Rpm);
    let consumer = install_trove(&conn, "consumer", "1", VersionScheme::Rpm);
    let dependency = parse_native_requirement(
        RepositoryRequirementKind::Depends,
        VersionScheme::Rpm,
        "oldpkg",
    )
    .unwrap();
    InstalledRequirementGroup::insert_groups(&conn, consumer, VersionScheme::Rpm, &[dependency])
        .unwrap();
    let relation = parse_native_relation(
        RepositoryRequirementKind::Obsolete,
        VersionScheme::Rpm,
        "oldpkg < 2",
    )
    .unwrap();
    let plan = plan_package_relations(&conn, &incoming(relation), VersionScheme::Rpm).unwrap();

    validate_package_relation_plan(&conn, &plan).unwrap();

    assert_eq!(plan.removals.len(), 1);
    assert_eq!(plan.removals[0].trove_id, old);
    assert_ne!(plan.removals[0].trove_id, current);
}
