// conary-core/src/repository/sync/tests/native/requirements.rs

use super::*;

#[test]
fn test_sync_persists_requirement_groups_with_alternatives() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "debian-main".to_string(),
        "https://example.com/debian".to_string(),
    );
    repo.source_profile = Some("ubuntu-26.04".to_string());
    configure_native_test_repository(&mut repo, NativeSourceEcosystem::Deb);
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    // Simulate a Debian package with an OR dependency: default-mta | mail-transport-agent
    let or_group = dep_model::RepositoryRequirementGroup::alternatives(
        RepositoryRequirementKind::Depends,
        vec![
            dep_model::RepositoryRequirementClause::name_only("default-mta".to_string()),
            dep_model::RepositoryRequirementClause::name_only("mail-transport-agent".to_string()),
        ],
    )
    .with_native_text("default-mta | mail-transport-agent".to_string());

    let simple_group = dep_model::RepositoryRequirementGroup::simple(
        RepositoryRequirementKind::Depends,
        dep_model::RepositoryRequirementClause::versioned(
            "libc6".to_string(),
            ">= 2.34".to_string(),
        ),
    );

    let mut pkg_meta = PackageMetadata::new(
        "postfix".to_string(),
        "3.9.1-1".to_string(),
        "aabbcc".to_string(),
        4096,
        "https://example.com/debian/postfix.deb".to_string(),
        RepositoryDependencyFlavor::Deb,
        VersionScheme::Debian,
    );
    pkg_meta.requirements = vec![or_group, simple_group];

    let provides = normalized_repository_capabilities(&pkg_meta);
    let (req_groups, req_group_clauses) = convert_requirement_groups(0, &pkg_meta.requirements);

    let synced = vec![SyncedPackageRow {
        package: {
            let mut p = RepositoryPackage::new(
                repo_id,
                pkg_meta.name.clone(),
                pkg_meta.version.clone(),
                pkg_meta.version_scheme,
                pkg_meta.checksum.clone(),
                pkg_meta.size as i64,
                pkg_meta.download_url.clone(),
            );
            p.source_profile = Some("ubuntu-26.04".to_string());
            p
        },
        provides,
        requirement_groups: req_groups,
        requirement_group_clauses: req_group_clauses,
    }];
    persist_native_sync_rows(
        &conn,
        &mut repo,
        synced,
        authenticated_snapshot(b"deb-requirements"),
    )
    .unwrap();

    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored.len(), 1);
    let pkg_id = stored[0].id.unwrap();

    // Verify requirement groups were persisted
    let groups = DbRequirementGroup::find_by_repository_package(&conn, pkg_id).unwrap();
    assert_eq!(groups.len(), 2);

    // First group: OR alternative
    let or = groups
        .iter()
        .find(|g| g.native_text.as_deref() == Some("default-mta | mail-transport-agent"));
    assert!(or.is_some(), "OR group should be persisted");
    let or = or.unwrap();
    assert_eq!(or.kind, "depends");
    assert_eq!(or.behavior, "hard");

    // Verify the OR group has two clauses
    let or_clauses = RepositoryRequirement::find_by_group(&conn, or.id.unwrap()).unwrap();
    assert_eq!(or_clauses.len(), 2);
    assert!(or_clauses.iter().any(|c| c.capability == "default-mta"));
    assert!(
        or_clauses
            .iter()
            .any(|c| c.capability == "mail-transport-agent")
    );

    // Second group: simple versioned dependency
    let simple = groups.iter().find(|g| g.native_text.is_none());
    assert!(simple.is_some(), "simple group should be persisted");
    let simple = simple.unwrap();
    let simple_clauses = RepositoryRequirement::find_by_group(&conn, simple.id.unwrap()).unwrap();
    assert_eq!(simple_clauses.len(), 1);
    assert_eq!(simple_clauses[0].capability, "libc6");
    assert_eq!(
        simple_clauses[0].version_constraint.as_deref(),
        Some(">= 2.34")
    );
}
#[test]
fn test_sync_persists_conditional_requirement_behavior() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "fedora".to_string(),
        "https://example.com/fedora".to_string(),
    );
    repo.source_profile = Some("fedora-44".to_string());
    configure_native_test_repository(&mut repo, NativeSourceEcosystem::Rpm);
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    // Simulate a conditional RPM rich dependency
    let conditional_group = dep_model::RepositoryRequirementGroup::simple(
        RepositoryRequirementKind::Depends,
        dep_model::RepositoryRequirementClause::versioned(
            "systemd".to_string(),
            ">= 255".to_string(),
        ),
    )
    .with_behavior(ConditionalRequirementBehavior::Conditional)
    .with_native_text("(systemd >= 255 if systemd-resolved)".to_string());

    let mut pkg_meta = PackageMetadata::new(
        "resolved-client".to_string(),
        "1.0-1.fc44".to_string(),
        "ff00ff".to_string(),
        256,
        "https://example.com/fedora/resolved-client.rpm".to_string(),
        RepositoryDependencyFlavor::Rpm,
        VersionScheme::Rpm,
    );
    pkg_meta.requirements = vec![conditional_group];

    let provides = normalized_repository_capabilities(&pkg_meta);
    let (req_groups, req_group_clauses) = convert_requirement_groups(0, &pkg_meta.requirements);

    let synced = vec![SyncedPackageRow {
        package: {
            let mut p = RepositoryPackage::new(
                repo_id,
                pkg_meta.name.clone(),
                pkg_meta.version.clone(),
                pkg_meta.version_scheme,
                pkg_meta.checksum.clone(),
                pkg_meta.size as i64,
                pkg_meta.download_url.clone(),
            );
            p.source_profile = Some("fedora-44".to_string());
            p
        },
        provides,
        requirement_groups: req_groups,
        requirement_group_clauses: req_group_clauses,
    }];
    persist_native_sync_rows(
        &conn,
        &mut repo,
        synced,
        authenticated_snapshot(b"rpm-conditional"),
    )
    .unwrap();

    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    let pkg_id = stored[0].id.unwrap();

    let groups = DbRequirementGroup::find_by_repository_package(&conn, pkg_id).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].kind, "depends");
    assert_eq!(groups[0].behavior, "conditional");
    assert_eq!(
        groups[0].native_text.as_deref(),
        Some("(systemd >= 255 if systemd-resolved)")
    );

    let clauses = RepositoryRequirement::find_by_group(&conn, groups[0].id.unwrap()).unwrap();
    assert_eq!(clauses.len(), 1);
    assert_eq!(clauses[0].capability, "systemd");
    assert_eq!(clauses[0].version_constraint.as_deref(), Some(">= 255"));
}
