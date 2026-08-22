// conary-core/src/repository/sync/tests/native.rs

#[path = "native/source_config.rs"]
mod source_config;

fn configure_native_test_repository(repo: &mut Repository, ecosystem: NativeSourceEcosystem) {
    source_config::configure(repo, ecosystem);
}

fn configure_native_test_repository_with_policy(
    repo: &mut Repository,
    ecosystem: NativeSourceEcosystem,
    update_mode: RepositoryUpdateMode,
    pinned_snapshot: Option<AuthenticatedSnapshotIdentity>,
) {
    source_config::configure_with_policy(repo, ecosystem, update_mode, pinned_snapshot);
}

fn authenticated_snapshot(value: &[u8]) -> AuthenticatedSnapshotIdentity {
    AuthenticatedSnapshotIdentity::for_bytes(value)
}

#[test]
fn pin_refusal_leaves_native_rows_and_sync_state_untouched() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let pinned = authenticated_snapshot(b"pinned-root");
    let mut repo = Repository::new(
        "pinned-rpm".to_string(),
        "https://example.com/pinned-rpm".to_string(),
    );
    repo.source_profile = Some("fedora-44".to_string());
    configure_native_test_repository_with_policy(
        &mut repo,
        NativeSourceEcosystem::Rpm,
        RepositoryUpdateMode::Pin,
        Some(pinned.clone()),
    );
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();
    let repository_url = repo.url.clone();
    let package = |checksum: &str| {
        synced_package_row(
            repo_id,
            Some("fedora-44"),
            &repository_url,
            None,
            PackageMetadata::new(
                "bash".to_string(),
                "5.3-1".to_string(),
                checksum.to_string(),
                1,
                "https://example.com/pinned-rpm/bash.rpm".to_string(),
                RepositoryDependencyFlavor::Rpm,
                VersionScheme::Rpm,
            ),
        )
    };
    persist_native_sync_rows(&conn, &mut repo, vec![package("old")], pinned.clone()).unwrap();
    let before = Repository::find_by_id(&conn, repo_id).unwrap().unwrap();

    let error = persist_native_sync_rows(
        &conn,
        &mut repo,
        vec![package("new")],
        authenticated_snapshot(b"different-root"),
    )
    .unwrap_err();
    assert!(matches!(error, Error::TrustError(_)));
    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].checksum, "sha256:old");
    let after = Repository::find_by_id(&conn, repo_id).unwrap().unwrap();
    assert_eq!(after.last_checked_at, before.last_checked_at);
    assert_eq!(after.last_changed_at, before.last_changed_at);
    assert_eq!(after.last_validated_at, before.last_validated_at);
    assert_eq!(after.last_published_at, before.last_published_at);
}

#[test]
fn test_persist_native_sync_rows_writes_normalized_capabilities() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "arch-core".to_string(),
        "https://example.com/arch".to_string(),
    );
    configure_native_test_repository(&mut repo, NativeSourceEcosystem::Alpm);
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg_meta = PackageMetadata::new(
        "ripgrep".to_string(),
        "14.1.0-1".to_string(),
        "abc123".to_string(),
        1234,
        "https://example.com/arch/pool/ripgrep.pkg.tar.zst".to_string(),
        RepositoryDependencyFlavor::Arch,
        VersionScheme::Arch,
    );
    pkg_meta.architecture = Some("x86_64".to_string());
    pkg_meta.requirements = vec![
        dep_model::RepositoryRequirementGroup::simple(
            RepositoryRequirementKind::Depends,
            dep_model::RepositoryRequirementClause::versioned(
                "glibc".to_string(),
                ">= 2.39".to_string(),
            ),
        )
        .with_native_text("glibc >= 2.39".to_string()),
    ];
    let mut rg_provide =
        dep_model::RepositoryProvide::virtual_cap("rg".to_string(), Some("14.1.0-1".to_string()));
    rg_provide.native_text = Some("rg=14.1.0-1".to_string());
    pkg_meta.provides = vec![
        dep_model::RepositoryProvide::package_name(
            "ripgrep".to_string(),
            Some("14.1.0-1".to_string()),
        ),
        rg_provide,
    ];

    let provides = normalized_repository_capabilities(&pkg_meta);
    let (requirement_groups, requirement_group_clauses) =
        convert_requirement_groups(0, &pkg_meta.requirements);
    let package = RepositoryPackage::new(
        repo_id,
        pkg_meta.name.clone(),
        pkg_meta.version.clone(),
        crate::repository::versioning::VersionScheme::Arch,
        pkg_meta.checksum.clone(),
        pkg_meta.size as i64,
        pkg_meta.download_url.clone(),
    );
    let synced_packages = vec![SyncedPackageRow {
        package,
        provides,
        requirement_groups,
        requirement_group_clauses,
    }];
    let count = persist_native_sync_rows(
        &conn,
        &mut repo,
        synced_packages,
        authenticated_snapshot(b"arch-capabilities"),
    )
    .unwrap();
    assert_eq!(count, 1);

    let stored_packages = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored_packages.len(), 1);
    let repository_package_id = stored_packages[0].id.unwrap();

    let stored_provides =
        RepositoryProvide::find_by_repository_package(&conn, repository_package_id).unwrap();
    assert_eq!(stored_provides.len(), 2);
    assert!(stored_provides.iter().any(|provide| {
        provide.capability == "ripgrep"
            && provide.version.as_deref() == Some("14.1.0-1")
            && provide.raw.is_none()
    }));
    assert!(stored_provides.iter().any(|provide| {
        provide.capability == "rg"
            && provide.version.as_deref() == Some("14.1.0-1")
            && provide.raw.as_deref() == Some("rg=14.1.0-1")
    }));

    let stored_requirements =
        RepositoryRequirement::find_by_repository_package(&conn, repository_package_id).unwrap();
    assert_eq!(stored_requirements.len(), 1);
    assert_eq!(stored_requirements[0].capability, "glibc");
    assert_eq!(
        stored_requirements[0].version_constraint.as_deref(),
        Some(">= 2.39")
    );
    assert_eq!(stored_requirements[0].dependency_type, "runtime");
    assert_eq!(stored_requirements[0].raw, None);

    let stored_groups =
        DbRequirementGroup::find_by_repository_package(&conn, repository_package_id).unwrap();
    assert_eq!(stored_groups.len(), 1);
    assert_eq!(
        stored_groups[0].native_text.as_deref(),
        Some("glibc >= 2.39")
    );
}

#[test]
fn native_sync_persists_generator_selected_rpm_file_providers_without_reclassification() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "fedora-44".to_string(),
        "https://example.com/fedora".to_string(),
    );
    repo.source_profile = Some("fedora-44".to_string());
    configure_native_test_repository(&mut repo, NativeSourceEcosystem::Rpm);
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let mut pkg_meta = PackageMetadata::new(
        "bash".to_string(),
        "5.3.3-2.fc44".to_string(),
        "abc123".to_string(),
        1024,
        "https://example.com/fedora/bash.rpm".to_string(),
        RepositoryDependencyFlavor::Rpm,
        VersionScheme::Rpm,
    );
    pkg_meta.architecture = Some("x86_64".to_string());
    pkg_meta.provides = vec![
        dep_model::RepositoryProvide::package_name(
            "bash".to_string(),
            Some("5.3.3-2.fc44".to_string()),
        ),
        dep_model::RepositoryProvide::file("/usr/bin/bash".to_string()),
    ];

    let provides = normalized_repository_capabilities(&pkg_meta);
    let synced_packages = vec![SyncedPackageRow {
        package: {
            let mut package = RepositoryPackage::new(
                repo_id,
                pkg_meta.name,
                pkg_meta.version,
                VersionScheme::Rpm,
                pkg_meta.checksum,
                pkg_meta.size as i64,
                pkg_meta.download_url,
            );
            package.architecture = pkg_meta.architecture;
            package.source_profile = Some("fedora-44".to_string());
            package
        },
        provides,
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    }];
    persist_native_sync_rows(
        &conn,
        &mut repo,
        synced_packages,
        authenticated_snapshot(b"rpm-files"),
    )
    .unwrap();

    let package = RepositoryPackage::find_by_repository(&conn, repo_id)
        .unwrap()
        .pop()
        .unwrap();
    let stored = RepositoryProvide::find_by_repository_package(&conn, package.id.unwrap()).unwrap();
    let file = stored
        .iter()
        .find(|provide| provide.capability == "/usr/bin/bash")
        .expect("persisted file provider");
    assert_eq!(file.kind, "file");
    assert_eq!(file.version, None);
    assert_eq!(file.raw, None);
    assert_eq!(file.version_scheme, VersionScheme::Rpm);
}

#[test]
fn native_sync_does_not_rewrite_exact_conversion_revision_identity() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "fedora-44".to_string(),
        "https://example.com/fedora".to_string(),
    );
    repo.source_profile = Some("fedora-44".to_string());
    configure_native_test_repository(&mut repo, NativeSourceEcosystem::Rpm);
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let synced_row = |include_kernel_install: bool| {
        let mut metadata = PackageMetadata::new(
            "systemd-udev".to_string(),
            "259.5-1.fc44".to_string(),
            "sha256:systemd-udev".to_string(),
            1024,
            "https://example.com/fedora/systemd-udev.rpm".to_string(),
            RepositoryDependencyFlavor::Rpm,
            VersionScheme::Rpm,
        );
        metadata.architecture = Some("x86_64".to_string());
        metadata.provides = vec![dep_model::RepositoryProvide::package_name(
            "systemd-udev".to_string(),
            Some("259.5-1.fc44".to_string()),
        )];
        if include_kernel_install {
            metadata.provides.push(dep_model::RepositoryProvide::file(
                "/usr/bin/kernel-install".to_string(),
            ));
        }
        let provides = normalized_repository_capabilities(&metadata);
        let mut package = RepositoryPackage::new(
            repo_id,
            metadata.name,
            metadata.version,
            VersionScheme::Rpm,
            metadata.checksum,
            metadata.size as i64,
            metadata.download_url,
        );
        package.architecture = metadata.architecture;
        package.source_profile = Some("fedora-44".to_string());
        SyncedPackageRow {
            package,
            provides,
            requirement_groups: Vec::new(),
            requirement_group_clauses: Vec::new(),
        }
    };

    persist_native_sync_rows(
        &conn,
        &mut repo,
        vec![synced_row(false)],
        authenticated_snapshot(b"rpm-before"),
    )
    .unwrap();
    let repository_package_id = RepositoryPackage::find_by_repository(&conn, repo_id)
        .unwrap()
        .pop()
        .unwrap()
        .id
        .unwrap();
    let digest =
        RepositoryProvide::conversion_capabilities_digest(&conn, repository_package_id).unwrap();
    let transport = crate::ccs::transport::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        "a".repeat(64),
        "systemd-udev".to_string(),
        "259.5-1.fc44".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:systemd-udev".to_string(),
        &transport,
        1,
        "sha256:converted".to_string(),
        "/tmp/systemd-udev.ccs".to_string(),
        digest,
    );
    converted.insert(&conn).unwrap();

    persist_native_sync_rows(
        &conn,
        &mut repo,
        vec![synced_row(true)],
        authenticated_snapshot(b"rpm-after"),
    )
    .unwrap();

    assert!(
        ConvertedPackage::find_repository_by_checksum(
            &conn,
            &"a".repeat(64),
            "sha256:systemd-udev"
        )
        .unwrap()
        .is_some(),
        "native metadata rows do not rewrite exact conversion revision identity"
    );
}

#[test]
fn test_json_contract_persists_trusted_advisory_metadata() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "fedora-security".to_string(),
        "https://example.com/fedora".to_string(),
    );
    repo.security_advisory_support = SecurityAdvisorySupport::Supported;
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let metadata: JsonRepositoryMetadata = serde_json::from_value(json!({
        "name": "fedora-security",
        "version": "1",
        "security_advisory_source": {
            "name": "conary-json",
            "trust": "trusted"
        },
        "packages": [
            {
                "name": "openssl",
                "version": "3.2.1-1.fc44",
                "version_scheme": "rpm",
                "architecture": "x86_64",
                "description": "TLS toolkit",
                "checksum": "sha256:openssl-fixed",
                "size": 4096,
                "download_url": "https://example.com/fedora/openssl-3.2.1-1.fc44.ccs",
                "requirements": [],
                "security_advisory": {
                    "id": "FEDORA-2026-0001",
                    "severity": "critical",
                    "cves": ["CVE-2026-0001"],
                    "fixed_version": "3.2.1-1.fc44",
                    "url": "https://security.example.test/FEDORA-2026-0001"
                }
            }
        ]
    }))
    .unwrap();

    let snapshot = json_repository_sync_snapshot(&repo, metadata).unwrap();
    assert_eq!(
        persist_repository_sync_snapshot(&conn, &mut repo, snapshot).unwrap(),
        1
    );

    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored.len(), 1);
    let package = &stored[0];
    assert!(package.is_security_update);
    assert_eq!(package.severity.as_deref(), Some("critical"));
    assert_eq!(package.cve_ids.as_deref(), Some("CVE-2026-0001"));
    assert_eq!(package.advisory_id.as_deref(), Some("FEDORA-2026-0001"));
    assert_eq!(
        package.advisory_url.as_deref(),
        Some("https://security.example.test/FEDORA-2026-0001")
    );

    let package_metadata: serde_json::Value =
        serde_json::from_str(package.metadata.as_deref().unwrap()).unwrap();
    assert_eq!(
        package_metadata["security_advisory"]["fixed_version"],
        "3.2.1-1.fc44"
    );
    assert_eq!(
        package_metadata["security_advisory"]["source"],
        "conary-json"
    );
    assert_eq!(
        package_metadata["security_advisory"]["source_trust"],
        "trusted"
    );
}

#[test]
fn json_contract_keeps_source_version_and_ccs_release_separate() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();
    let mut repo = Repository::new(
        "signed-static".to_string(),
        "https://example.test/static".to_string(),
    );
    repo.source_profile = Some("ubuntu-26.04".to_string());
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();
    let metadata: JsonRepositoryMetadata = serde_json::from_value(json!({
        "name": "signed-static",
        "version": "1",
        "security_advisory_source": null,
        "packages": [{
            "name": "fixture",
            "version": "1.0.0",
            "release": "1",
            "version_scheme": "debian",
            "architecture": "amd64",
            "description": "signed fixture",
            "checksum": "fixture-checksum",
            "size": 42,
            "download_url": "https://example.test/static/fixture-1.0.0-1.ccs",
            "requirements": [],
            "relations": [],
            "delta_from": null,
            "security_advisory": null
        }]
    }))
    .unwrap();

    let snapshot = json_repository_sync_snapshot(&repo, metadata).unwrap();
    persist_repository_sync_snapshot(&conn, &mut repo, snapshot).unwrap();
    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored[0].version, "1.0.0");
    assert_eq!(stored[0].package_release, "1");

    let requirement = crate::repository::requirement::parse_native_requirement(
        crate::repository::dependency_model::RepositoryRequirementKind::Depends,
        VersionScheme::Debian,
        "fixture (= 1.0.0)",
    )
    .unwrap();
    let resolution = crate::resolver::solve_requirement_groups_with_policy(
        &conn,
        &[requirement],
        VersionScheme::Debian,
        &crate::repository::resolution_policy::ResolutionPolicy::new()
            .with_primary_source_identity("ubuntu-26.04"),
    )
    .unwrap();
    assert!(resolution.conflict_message.is_none(), "{resolution:?}");
    assert_eq!(resolution.install_order.len(), 1);
}

#[test]
fn test_json_contract_supported_repo_requires_trusted_advisory_source() {
    let mut repo = Repository::new(
        "fedora-security".to_string(),
        "https://example.com/fedora".to_string(),
    );
    repo.id = Some(42);
    repo.security_advisory_support = SecurityAdvisorySupport::Supported;

    let metadata: JsonRepositoryMetadata = serde_json::from_value(json!({
        "name": "fedora-security",
        "version": "1",
        "packages": [
            {
                "name": "openssl",
                "version": "3.2.1-1.fc44",
                "version_scheme": "rpm",
                "architecture": "x86_64",
                "description": "TLS toolkit",
                "checksum": "sha256:openssl-fixed",
                "size": 4096,
                "download_url": "https://example.com/fedora/openssl-3.2.1-1.fc44.ccs",
                "requirements": [],
                "security_advisory": {
                    "id": "FEDORA-2026-0001",
                    "severity": "critical",
                    "cves": ["CVE-2026-0001"],
                    "fixed_version": "3.2.1-1.fc44"
                }
            }
        ]
    }))
    .unwrap();

    let error = json_repository_sync_snapshot(&repo, metadata).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("trusted security advisory source"),
        "{error}"
    );
}

#[test]
fn test_sync_persists_distro_and_version_scheme() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "fedora-updates".to_string(),
        "https://example.com/fedora".to_string(),
    );
    repo.source_profile = Some("fedora-44".to_string());
    configure_native_test_repository(&mut repo, NativeSourceEcosystem::Rpm);
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let pkg_meta = PackageMetadata::new(
        "bash".to_string(),
        "5.2.37-1.fc44".to_string(),
        "deadbeef".to_string(),
        2048,
        "https://example.com/fedora/bash.rpm".to_string(),
        RepositoryDependencyFlavor::Rpm,
        VersionScheme::Rpm,
    );

    let provides = normalized_repository_capabilities(&pkg_meta);
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
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    }];
    persist_native_sync_rows(
        &conn,
        &mut repo,
        synced,
        authenticated_snapshot(b"rpm-origin"),
    )
    .unwrap();

    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(stored[0].version_scheme, VersionScheme::Rpm);
}

#[test]
fn test_sync_persists_debian_origin_metadata() {
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

    let pkg_meta = PackageMetadata::new(
        "postfix".to_string(),
        "3.9.1-1".to_string(),
        "aabbccdd".to_string(),
        512,
        "https://example.com/debian/postfix.deb".to_string(),
        RepositoryDependencyFlavor::Deb,
        VersionScheme::Debian,
    );

    let provides = normalized_repository_capabilities(&pkg_meta);
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
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    }];
    persist_native_sync_rows(
        &conn,
        &mut repo,
        synced,
        authenticated_snapshot(b"deb-origin"),
    )
    .unwrap();

    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].source_profile.as_deref(),
        Some("ubuntu-26.04")
    );
    assert_eq!(stored[0].version_scheme, VersionScheme::Debian);
}

#[test]
fn test_sync_persists_arch_origin_metadata() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "arch-core".to_string(),
        "https://example.com/arch".to_string(),
    );
    repo.source_profile = Some("arch".to_string());
    configure_native_test_repository(&mut repo, NativeSourceEcosystem::Alpm);
    repo.insert(&conn).unwrap();
    let repo_id = repo.id.unwrap();

    let pkg_meta = PackageMetadata::new(
        "ripgrep".to_string(),
        "14.1.0-1".to_string(),
        "abc123".to_string(),
        1234,
        "https://example.com/arch/ripgrep.pkg.tar.zst".to_string(),
        RepositoryDependencyFlavor::Arch,
        VersionScheme::Arch,
    );

    let provides = normalized_repository_capabilities(&pkg_meta);
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
            p.source_profile = Some("arch".to_string());
            p
        },
        provides,
        requirement_groups: Vec::new(),
        requirement_group_clauses: Vec::new(),
    }];
    persist_native_sync_rows(
        &conn,
        &mut repo,
        synced,
        authenticated_snapshot(b"arch-origin"),
    )
    .unwrap();

    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].source_profile.as_deref(), Some("arch"));
    assert_eq!(stored[0].version_scheme, VersionScheme::Arch);
}

#[path = "native/requirements.rs"]
mod requirements;

#[path = "native/canonical.rs"]
mod canonical;
