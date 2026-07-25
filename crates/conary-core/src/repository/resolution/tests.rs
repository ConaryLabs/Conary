// conary-core/src/repository/resolution/tests.rs

mod tests {
    use super::*;
    use crate::db::testing::create_test_db;

    fn create_test_repo(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('test-repo', 'https://example.com', 1, 10)",
            [],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn create_test_package(conn: &Connection, repo_id: i64, name: &str, version: &str) -> i64 {
        conn.execute(
            "INSERT INTO repository_packages
             (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, ?2, ?3, 'sha256:abc123', 1024, 'https://example.com/pkg.rpm', 'rpm')",
            rusqlite::params![repo_id, name, version],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn test_delegate_context_depth_limit() {
        let mut ctx = DelegateContext::new();

        for i in 0..MAX_DELEGATE_DEPTH {
            ctx.enter(&format!("label{}", i)).unwrap();
        }

        // Should fail at max depth
        let result = ctx.enter("too_deep");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too deep"));
    }

    #[test]
    fn test_delegate_context_cycle_detection() {
        let mut ctx = DelegateContext::new();

        ctx.enter("label_a").unwrap();
        ctx.enter("label_b").unwrap();

        // Should fail on cycle
        let result = ctx.enter("label_a");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn test_delegate_context_exit_unwinds_state() {
        let mut ctx = DelegateContext::new();

        ctx.enter("label_a").unwrap();
        ctx.enter("label_b").unwrap();

        // Exit label_b -- depth and visited should unwind
        ctx.exit("label_b");

        // Re-entering label_b should succeed (no longer visited)
        ctx.enter("label_b").unwrap();

        // Exit both
        ctx.exit("label_b");
        ctx.exit("label_a");

        // After full unwind, depth should be 0 and entering label_a works
        ctx.enter("label_a").unwrap();
        assert_eq!(ctx.depth, 1);
    }

    #[test]
    fn test_delegate_context_exit_allows_reuse_after_failure() {
        let mut ctx = DelegateContext::new();

        // Fill up to max depth
        for i in 0..MAX_DELEGATE_DEPTH {
            ctx.enter(&format!("label{}", i)).unwrap();
        }

        // Should fail at max depth
        assert!(ctx.enter("overflow").is_err());

        // Exit one level -- should allow a new entry
        ctx.exit(&format!("label{}", MAX_DELEGATE_DEPTH - 1));
        ctx.enter("replacement").unwrap();
    }

    #[test]
    fn test_resolution_options_to_selection_options() {
        let res_options = ResolutionOptions {
            version: Some("1.0.0".to_string()),
            repository: Some("test-repo".to_string()),
            architecture: Some("x86_64".to_string()),
            output_dir: None,
            skip_installed: false,
            policy: None,
            is_root: false,
            primary_flavor: None,
        };

        let sel_options = res_options.to_selection_options();
        assert_eq!(sel_options.version, Some("1.0.0".to_string()));
        assert_eq!(sel_options.repository, Some("test-repo".to_string()));
        assert_eq!(sel_options.architecture, Some("x86_64".to_string()));
    }

    #[test]
    fn test_get_strategies_uses_selected_repository_package() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        // Find package
        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "nginx", &SelectionOptions::default())
                .unwrap();

        // No route means the exact package row selected above is used.
        let resolver = PackageResolver::new(&conn);
        let options = ResolutionOptions::default();
        let strategies = resolver.get_strategies(&pkg_with_repo, &options).unwrap();

        assert_eq!(strategies.len(), 1);
        assert!(matches!(
            strategies[0],
            ResolutionStrategy::RepositoryPackage { .. }
        ));
    }

    #[test]
    fn test_get_strategies_from_routing_table() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        // Create routing entry
        let mut resolution = PackageResolution::remi(
            repo_id,
            "nginx".to_string(),
            "https://remi.example.com".to_string(),
            "fedora".to_string(),
        );
        resolution.insert(&conn).unwrap();

        // Find package
        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "nginx", &SelectionOptions::default())
                .unwrap();

        // Get strategies - should use routing entry
        let resolver = PackageResolver::new(&conn);
        let options = ResolutionOptions::default();
        let strategies = resolver.get_strategies(&pkg_with_repo, &options).unwrap();

        assert_eq!(strategies.len(), 1);
        assert!(matches!(strategies[0], ResolutionStrategy::Remi { .. }));
    }

    #[tokio::test]
    async fn binary_routing_does_not_bypass_native_repository_trust() {
        let (_db_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        conn.execute(
            "UPDATE repositories SET default_strategy = 'static' WHERE id = ?1",
            [repo_id],
        )
        .unwrap();
        let pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("routed-local.ccs");
        let output = temp.path().join("downloads");
        let content = b"local route table payload is not TUF-pinned";
        std::fs::write(&source, content).unwrap();
        conn.execute(
            "UPDATE repository_packages SET size = ?1 WHERE id = ?2",
            rusqlite::params![i64::try_from(content.len()).unwrap(), pkg_id],
        )
        .unwrap();

        let mut resolution = PackageResolution::binary(
            repo_id,
            "nginx".to_string(),
            source.to_str().unwrap().to_string(),
            crate::hash::sha256(content),
        );
        resolution.insert(&conn).unwrap();

        let error = resolve_package(
            &conn,
            "nginx",
            &ResolutionOptions {
                output_dir: Some(output),
                ..ResolutionOptions::default()
            },
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("has no ecosystem-native trust policy"),
            "expected authority-less binary routing to fail before download, got: {error}"
        );
    }

    #[test]
    fn test_effective_remi_version_defaults_to_selected_repo_version() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "nginx", &SelectionOptions::default())
                .unwrap();

        let options = ResolutionOptions::default();
        assert_eq!(
            effective_remi_version(&pkg_with_repo, &options),
            Some("1.24.0")
        );
    }

    #[test]
    fn test_effective_remi_version_prefers_explicit_request() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "nginx", "1.24.0");

        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "nginx", &SelectionOptions::default())
                .unwrap();

        let options = ResolutionOptions {
            version: Some("1.25.0".to_string()),
            ..Default::default()
        };
        assert_eq!(
            effective_remi_version(&pkg_with_repo, &options),
            Some("1.25.0")
        );
    }

    #[test]
    fn repository_source_metadata_uses_selected_repository_package() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "tree", "2.2.1-4.fc44");
        conn.execute(
            "UPDATE repositories SET default_strategy_distro = 'fedora' WHERE id = ?1",
            [repo_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE repository_packages
                SET architecture = 'x86_64', distro = 'fedora', version_scheme = 'rpm'
              WHERE repository_id = ?1 AND name = 'tree'",
            [repo_id],
        )
        .unwrap();

        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "tree", &SelectionOptions::default())
                .unwrap();
        let metadata = repository_source_metadata(&pkg_with_repo);

        assert_eq!(metadata.repository_id, repo_id);
        assert_eq!(metadata.source_distro.as_deref(), Some("fedora"));
        assert_eq!(metadata.version_scheme, VersionScheme::Rpm);
        assert_eq!(metadata.source_kind, RepositorySourceKind::Native);
    }

    #[test]
    fn repository_source_metadata_uses_persisted_arch_scheme() {
        let (_temp, conn) = create_test_db();
        let repo_id = create_test_repo(&conn);
        let _pkg_id = create_test_package(&conn, repo_id, "tree", "2.2.1-4");
        conn.execute(
            "UPDATE repositories SET default_strategy_distro = 'arch' WHERE id = ?1",
            [repo_id],
        )
        .unwrap();
        conn.execute(
            "UPDATE repository_packages SET version_scheme = 'arch' WHERE repository_id = ?1",
            [repo_id],
        )
        .unwrap();

        let pkg_with_repo =
            PackageSelector::find_best_package(&conn, "tree", &SelectionOptions::default())
                .unwrap();
        let metadata = repository_source_metadata(&pkg_with_repo);

        assert_eq!(metadata.source_distro.as_deref(), Some("arch"));
        assert_eq!(metadata.version_scheme, VersionScheme::Arch);
        assert_eq!(metadata.source_kind, RepositorySourceKind::Native);
    }

    #[test]
    fn repository_source_metadata_tags_static_and_remi_sources() {
        let (_temp, conn) = create_test_db();
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy)
             VALUES ('static-repo', 'https://static.example.invalid', 1, 10, 'static')",
            [],
        )
        .unwrap();
        let static_repo_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO repositories (name, url, enabled, priority, default_strategy)
             VALUES ('remi-repo', 'https://remi.example.invalid', 1, 10, 'remi')",
            [],
        )
        .unwrap();
        let remi_repo_id = conn.last_insert_rowid();
        let _static_pkg_id = create_test_package(&conn, static_repo_id, "static-tree", "1.0.0");
        let _remi_pkg_id = create_test_package(&conn, remi_repo_id, "remi-tree", "1.0.0");

        let static_pkg =
            PackageSelector::find_best_package(&conn, "static-tree", &SelectionOptions::default())
                .unwrap();
        let remi_pkg =
            PackageSelector::find_best_package(&conn, "remi-tree", &SelectionOptions::default())
                .unwrap();

        assert_eq!(
            repository_source_metadata(&static_pkg).source_kind,
            RepositorySourceKind::Static
        );
        assert_eq!(
            repository_source_metadata(&remi_pkg).source_kind,
            RepositorySourceKind::Remi
        );
    }

    #[test]
    fn test_package_source_path() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.ccs");
        std::fs::write(&path, "test").unwrap();

        let binary = PackageSource::Binary {
            path: path.clone(),
            _temp_dir: None,
            repository_provenance: None,
        };
        assert_eq!(binary.path(), Some(path.as_path()));

        let ccs = PackageSource::Ccs {
            path: path.clone(),
            _temp_dir: None,
            repository_provenance: None,
        };
        assert_eq!(ccs.path(), Some(path.as_path()));
    }

    #[test]
    fn test_check_installed_not_installed() {
        let (_temp, conn) = create_test_db();
        let resolver = PackageResolver::new(&conn);
        let options = ResolutionOptions::default();

        // Package not installed - should return None
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_check_installed_matching_version() {
        let (_temp, conn) = create_test_db();

        // Insert an installed trove
        conn.execute(
            "INSERT INTO troves (
                 name, version, type, install_source, install_reason, version_scheme
             ) VALUES (
                 'nginx', '1.24.0', 'package', 'repository', 'explicit', 'rpm'
             )",
            [],
        )
        .unwrap();

        let resolver = PackageResolver::new(&conn);

        // No version specified - should match
        let options = ResolutionOptions::default();
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_some());
        if let Some(PackageSource::Installed {
            name,
            version,
            architecture,
            ..
        }) = result
        {
            assert_eq!(name, "nginx");
            assert_eq!(version, "1.24.0");
            assert_eq!(architecture, None);
        } else {
            panic!("Expected typed installed source");
        }

        // Matching version specified - should match
        let options = ResolutionOptions {
            version: Some("1.24.0".to_string()),
            ..Default::default()
        };
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_check_installed_different_version() {
        let (_temp, conn) = create_test_db();

        // Insert an installed trove
        conn.execute(
            "INSERT INTO troves (
                 name, version, type, install_source, install_reason, version_scheme
             ) VALUES (
                 'nginx', '1.24.0', 'package', 'repository', 'explicit', 'rpm'
             )",
            [],
        )
        .unwrap();

        let resolver = PackageResolver::new(&conn);

        // Different version requested - should NOT match
        let options = ResolutionOptions {
            version: Some("1.25.0".to_string()),
            ..Default::default()
        };
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_check_installed_skip_installed() {
        let (_temp, conn) = create_test_db();

        // Insert an installed trove
        conn.execute(
            "INSERT INTO troves (
                 name, version, type, install_source, install_reason, version_scheme
             ) VALUES (
                 'nginx', '1.24.0', 'package', 'repository', 'explicit', 'rpm'
             )",
            [],
        )
        .unwrap();

        let resolver = PackageResolver::new(&conn);

        // The option is consumed by resolve(); the direct helper still reports
        // exact installed state when called explicitly.
        let options = ResolutionOptions {
            skip_installed: true,
            ..Default::default()
        };
        let result = resolver.check_installed("nginx", &options).unwrap();
        assert!(result.is_some()); // Method still finds it
    }

    #[test]
    fn test_check_installed_respects_requested_architecture() {
        let (_temp, conn) = create_test_db();
        conn.execute(
            "INSERT INTO troves (
                name, version, type, architecture, install_source, install_reason, version_scheme
             ) VALUES (
                'nginx', '1.24.0', 'package', 'x86_64', 'repository', 'explicit', 'rpm'
             )",
            [],
        )
        .unwrap();
        let resolver = PackageResolver::new(&conn);

        let mismatched = resolver
            .check_installed(
                "nginx",
                &ResolutionOptions {
                    architecture: Some("aarch64".to_string()),
                    ..ResolutionOptions::default()
                },
            )
            .unwrap();
        assert!(mismatched.is_none());

        let alias_match = resolver
            .check_installed(
                "nginx",
                &ResolutionOptions {
                    architecture: Some("amd64".to_string()),
                    ..ResolutionOptions::default()
                },
            )
            .unwrap();
        assert!(matches!(alias_match, Some(PackageSource::Installed { .. })));
    }
}
