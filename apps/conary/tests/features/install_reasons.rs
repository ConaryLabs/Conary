// conary/tests/features/install_reasons.rs

use super::*;

// =============================================================================
// INSTALL REASON TESTS
// =============================================================================

/// Test InstallReason tracking for autoremove functionality
#[test]
fn test_install_reason_tracking() {
    use conary_core::db::models::{InstallReason, Trove, TroveType};

    let (_dir, _path, mut conn) = common::create_test_db();

    db::transaction(&mut conn, |tx| {
        // Install package explicitly
        let mut explicit_pkg = Trove::new(
            "explicit-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        explicit_pkg.install_reason = InstallReason::Explicit;
        explicit_pkg.insert(tx)?;

        // Install package as dependency
        let mut dep_pkg = Trove::new(
            "dependency-pkg".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        dep_pkg.install_reason = InstallReason::Dependency;
        dep_pkg.insert(tx)?;

        Ok(())
    })
    .unwrap();

    // Verify install reasons are stored correctly
    let explicit = Trove::find_by_name(&conn, "explicit-pkg").unwrap();
    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].install_reason, InstallReason::Explicit);

    let dep = Trove::find_by_name(&conn, "dependency-pkg").unwrap();
    assert_eq!(dep.len(), 1);
    assert_eq!(dep[0].install_reason, InstallReason::Dependency);
}

/// Test install reason queries (equivalent to cmd_query_reason)
#[test]
fn test_install_reason_queries() {
    use conary_core::db::models::{InstallReason, InstallSource, Trove, TroveType};

    let (_temp_dir, db_path) = common::setup_command_test_db();
    let mut conn = db::open(&db_path).unwrap();

    // Set install reasons
    db::transaction(&mut conn, |tx| {
        let nginx = Trove::find_by_name(tx, "nginx")?.pop().unwrap();
        tx.execute(
            "UPDATE troves SET install_reason = ?1 WHERE id = ?2",
            rusqlite::params![InstallReason::Explicit.as_str(), nginx.id],
        )?;

        let openssl = Trove::find_by_name(tx, "openssl")?.pop().unwrap();
        tx.execute(
            "UPDATE troves SET install_reason = ?1 WHERE id = ?2",
            rusqlite::params![InstallReason::Dependency.as_str(), openssl.id],
        )?;

        Ok(())
    })
    .unwrap();

    // Query by install reason
    let explicit: Vec<Trove> = conn
        .prepare("SELECT * FROM troves WHERE install_reason = ?1")
        .unwrap()
        .query_map([InstallReason::Explicit.as_str()], |row| {
            Ok(Trove {
                id: row.get("id")?,
                name: row.get("name")?,
                version: row.get("version")?,
                trove_type: TroveType::Package,
                architecture: row.get("architecture").ok(),
                description: row.get("description").ok(),
                installed_at: row.get("installed_at").ok(),
                installed_by_changeset_id: row.get("installed_by_changeset_id").ok(),
                install_source: InstallSource::Repository,
                install_reason: InstallReason::Explicit,
                selection_reason: None,
                pinned: false,
                flavor_spec: None,
                label_id: None,
                orphan_since: None,
                source_distro: None,
                version_scheme: conary_core::repository::versioning::VersionScheme::Conary,
                native_package_identity: None,
                installed_from_repository_id: None,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert_eq!(explicit.len(), 1);
    assert_eq!(explicit[0].name, "nginx");
}
