// conary-core/src/repository/sync/tests/native.rs

#[test]
fn test_persist_native_sync_rows_writes_normalized_capabilities() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "arch-core".to_string(),
        "https://example.com/arch".to_string(),
    );
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
    let count = persist_native_sync_rows(&conn, &mut repo, synced_packages).unwrap();
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
    persist_native_sync_rows(&conn, &mut repo, synced_packages).unwrap();

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
fn test_remi_sparse_entry_builds_normalized_capabilities() {
    let entry = RemiSparseResolutionVersionEntry {
        version: "6.19.6-200.fc44".to_string(),
        release: None,
        architecture: Some("x86_64".to_string()),
        provides: vec![
            RemiProvide {
                capability: "kernel-core".to_string(),
                version: Some("6.19.6-200.fc44".to_string()),
                version_relation: Some(
                    crate::repository::dependency_model::ProvideVersionRelation::Equal,
                ),
                kind: "package".to_string(),
                raw: Some("kernel-core = 6.19.6-200.fc44".to_string()),
                version_scheme: VersionScheme::Rpm,
                architecture_qualifier:
                    crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
            },
            RemiProvide {
                capability: "kernel-core-uname-r".to_string(),
                version: Some("6.19.6-200.fc44.x86_64".to_string()),
                version_relation: Some(
                    crate::repository::dependency_model::ProvideVersionRelation::Equal,
                ),
                kind: "package".to_string(),
                raw: Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64".to_string()),
                version_scheme: VersionScheme::Rpm,
                architecture_qualifier:
                    crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
            },
            RemiProvide {
                capability: "/usr/bin/kernel-install".to_string(),
                version: None,
                version_relation: None,
                kind: "file".to_string(),
                raw: None,
                version_scheme: VersionScheme::Rpm,
                architecture_qualifier:
                    crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
            },
        ],
        requirement_groups: vec![
            RemiRequirementGroup {
                kind: "depends".to_string(),
                behavior: "hard".to_string(),
                description: None,
                native_text: Some(
                    "kernel-modules-core-uname-r = 6.19.6-200.fc44.x86_64".to_string(),
                ),
                expression_json: serde_json::to_string(
                    &dep_model::RepositoryRequirementExpression::Atom(
                        dep_model::RepositoryRequirementClause::versioned(
                            "kernel-modules-core-uname-r".to_string(),
                            "= 6.19.6-200.fc44.x86_64".to_string(),
                        ),
                    ),
                )
                .unwrap(),
                clauses: vec![RemiRequirement {
                    capability: "kernel-modules-core-uname-r".to_string(),
                    version_constraint: Some("= 6.19.6-200.fc44.x86_64".to_string()),
                    kind: "package".to_string(),
                    dependency_type: "runtime".to_string(),
                    raw: Some("kernel-modules-core-uname-r = 6.19.6-200.fc44.x86_64".to_string()),
                }],
            },
            RemiRequirementGroup {
                kind: "depends".to_string(),
                behavior: "hard".to_string(),
                description: None,
                native_text: Some("glibc >= 2.39".to_string()),
                expression_json: serde_json::to_string(
                    &dep_model::RepositoryRequirementExpression::Atom(
                        dep_model::RepositoryRequirementClause::versioned(
                            "glibc".to_string(),
                            ">= 2.39".to_string(),
                        ),
                    ),
                )
                .unwrap(),
                clauses: vec![RemiRequirement {
                    capability: "glibc".to_string(),
                    version_constraint: Some(">= 2.39".to_string()),
                    kind: "package".to_string(),
                    dependency_type: "runtime".to_string(),
                    raw: Some("glibc >= 2.39".to_string()),
                }],
            },
        ],
        size: 4096,
        metadata: None,
    };

    let row = remi_sync_row(
        7,
        "https://remi.conary.io".to_string(),
        "fedora-44".to_string(),
        "kernel-core".to_string(),
        entry,
    )
    .unwrap();

    assert_eq!(row.package.size, 4096);
    assert!(row.provides.iter().any(|provide| {
        provide.capability == "kernel-core-uname-r"
            && provide.version.as_deref() == Some("6.19.6-200.fc44.x86_64")
            && provide.raw.as_deref() == Some("kernel-core-uname-r = 6.19.6-200.fc44.x86_64")
    }));
    assert!(row.provides.iter().any(|provide| {
        provide.capability == "/usr/bin/kernel-install"
            && provide.kind == "file"
            && provide.version.is_none()
            && provide.raw.is_none()
    }));
    let requirements = row.requirement_group_clauses.concat();
    assert!(requirements.iter().any(|requirement| {
        requirement.capability == "kernel-modules-core-uname-r"
            && requirement.version_constraint.as_deref() == Some("= 6.19.6-200.fc44.x86_64")
            && requirement.raw.as_deref()
                == Some("kernel-modules-core-uname-r = 6.19.6-200.fc44.x86_64")
    }));
    assert!(requirements.iter().any(|requirement| {
        requirement.capability == "glibc"
            && requirement.version_constraint.as_deref() == Some(">= 2.39")
    }));
}

#[test]
fn test_remi_sparse_entry_preserves_trusted_security_advisory() {
    let entry = RemiSparseResolutionVersionEntry {
        version: "3.2.1-1.fc44".to_string(),
        release: None,
        architecture: Some("x86_64".to_string()),
        provides: vec![RemiProvide {
            capability: "openssl".to_string(),
            version: Some("3.2.1-1.fc44".to_string()),
            version_relation: Some(
                crate::repository::dependency_model::ProvideVersionRelation::Equal,
            ),
            kind: "package".to_string(),
            raw: Some("openssl".to_string()),
            version_scheme: VersionScheme::Rpm,
            architecture_qualifier:
                crate::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
        }],
        requirement_groups: Vec::new(),
        size: 4096,
        metadata: Some(json!({
            "security_advisory": {
                "id": "FEDORA-2026-0001",
                "source": "remi",
                "source_trust": "trusted",
                "severity": "critical",
                "cves": ["CVE-2026-0001", "CVE-2026-0002"],
                "fixed_version": "3.2.1-1.fc44",
                "url": "https://security.example.test/FEDORA-2026-0001"
            }
        })),
    };

    let row = remi_sync_row(
        7,
        "https://remi.conary.io".to_string(),
        "fedora-44".to_string(),
        "openssl".to_string(),
        entry,
    )
    .unwrap();

    assert!(row.package.is_security_update);
    assert_eq!(row.package.severity.as_deref(), Some("critical"));
    assert_eq!(
        row.package.cve_ids.as_deref(),
        Some("CVE-2026-0001,CVE-2026-0002")
    );
    assert_eq!(
        row.package.advisory_id.as_deref(),
        Some("FEDORA-2026-0001")
    );
    assert_eq!(
        row.package.advisory_url.as_deref(),
        Some("https://security.example.test/FEDORA-2026-0001")
    );

    let metadata: serde_json::Value =
        serde_json::from_str(row.package.metadata.as_deref().unwrap()).unwrap();
    assert_eq!(
        metadata["security_advisory"]["fixed_version"],
        "3.2.1-1.fc44"
    );
    assert_eq!(
        metadata["security_advisory"]["source_trust"],
        "trusted"
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
    persist_native_sync_rows(&conn, &mut repo, synced).unwrap();

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
    persist_native_sync_rows(&conn, &mut repo, synced).unwrap();

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
    persist_native_sync_rows(&conn, &mut repo, synced).unwrap();

    let stored = RepositoryPackage::find_by_repository(&conn, repo_id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].source_profile.as_deref(), Some("arch"));
    assert_eq!(stored[0].version_scheme, VersionScheme::Arch);
}

#[test]
fn test_sync_persists_requirement_groups_with_alternatives() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    let mut repo = Repository::new(
        "debian-main".to_string(),
        "https://example.com/debian".to_string(),
    );
    repo.source_profile = Some("ubuntu-26.04".to_string());
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
    persist_native_sync_rows(&conn, &mut repo, synced).unwrap();

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
    persist_native_sync_rows(&conn, &mut repo, synced).unwrap();

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

#[test]
fn test_canonical_map_deserialization_and_persist() {
    use crate::db::models::{CanonicalMappingAuthority, CanonicalPackage, PackageImplementation};

    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    // Simulate the JSON response from GET /v1/canonical/map
    let json = json!({
        "schema_version": 1,
        "revision": 1,
        "generated_at": "2026-03-16T00:00:00Z",
        "entries": [
            {
                "canonical": "firefox",
                "kind": "package",
                "implementations": {
                    "fedora-44": "firefox",
                    "ubuntu-26.04": "firefox-esr",
                    "arch": "firefox"
                }
            },
            {
                "canonical": "openssl",
                "kind": "package",
                "implementations": {
                    "fedora-44": "openssl",
                    "ubuntu-26.04": "libssl3"
                }
            }
        ]
    });

    let map = crate::canonical::exchange::parse_snapshot(json.to_string().as_bytes()).unwrap();
    assert_eq!(map.entries.len(), 2);

    assert_eq!(persist_canonical_map(&conn, &map).unwrap(), 2);

    // Verify canonical packages were persisted
    let firefox = CanonicalPackage::find_by_name(&conn, "firefox")
        .unwrap()
        .unwrap();
    assert_eq!(firefox.kind, "package");

    let openssl = CanonicalPackage::find_by_name(&conn, "openssl")
        .unwrap()
        .unwrap();
    assert_eq!(openssl.kind, "package");

    // Verify implementations
    let ff_impls = PackageImplementation::find_by_canonical(&conn, firefox.id.unwrap()).unwrap();
    assert_eq!(ff_impls.len(), 3);
    let debian_impl = ff_impls
        .iter()
        .find(|i| i.distro == "ubuntu-26.04")
        .unwrap();
    assert_eq!(debian_impl.distro_name, "firefox-esr");
    assert_eq!(debian_impl.source, CanonicalMappingAuthority::Remi);

    let ssl_impls = PackageImplementation::find_by_canonical(&conn, openssl.id.unwrap()).unwrap();
    assert_eq!(ssl_impls.len(), 2);
    let ubuntu_impl = ssl_impls
        .iter()
        .find(|i| i.distro == "ubuntu-26.04")
        .unwrap();
    assert_eq!(ubuntu_impl.distro_name, "libssl3");

    // Second ingest is idempotent -- no duplicate rows
    persist_canonical_map(&conn, &map).unwrap();

    let ff_impls2 = PackageImplementation::find_by_canonical(&conn, firefox.id.unwrap()).unwrap();
    assert_eq!(
        ff_impls2.len(),
        3,
        "No duplicate implementations after re-ingest"
    );
}

#[test]
fn test_link_canonical_ids_populates_from_implementations() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('firefox-web', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO package_implementations (canonical_id, distro, distro_name, source)
             VALUES (?1, 'fedora-44', 'firefox', 'contract')",
        [canonical_id],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority, source_profile)
            VALUES ('fedora-44', 'https://example.com', 1, 10, 'fedora-44')",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();

    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme)
             VALUES (?1, 'firefox', '125.0', 'sha256:abc', 1024, 'https://example.com/firefox.rpm', 'rpm')",
            [repo_id],
        )
        .unwrap();
    let pkg_id = conn.last_insert_rowid();

    let count = link_canonical_ids(&conn, repo_id).unwrap();
    assert_eq!(count, 1);

    let cid: Option<i64> = conn
        .query_row(
            "SELECT canonical_id FROM repository_packages WHERE id = ?1",
            [pkg_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cid, Some(canonical_id));
}

#[test]
fn test_link_canonical_ids_skips_already_linked() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('test', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority)
             VALUES ('test-repo', 'https://example.com', 1, 10)",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();

    conn.execute(
            "INSERT INTO repository_packages (repository_id, name, version, checksum, size, download_url, version_scheme, canonical_id)
             VALUES (?1, 'test-pkg', '1.0', 'sha256:x', 100, 'https://example.com/x', 'rpm', ?2)",
            rusqlite::params![repo_id, canonical_id],
        )
        .unwrap();

    let count = link_canonical_ids(&conn, repo_id).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn repository_name_never_substitutes_for_source_profile_authority() {
    let conn = Connection::open_in_memory().unwrap();
    ensure_current(&conn).unwrap();

    conn.execute(
        "INSERT INTO canonical_packages (name, kind) VALUES ('firefox-web', 'package')",
        [],
    )
    .unwrap();
    let canonical_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO package_implementations (canonical_id, distro, distro_name, source)
         VALUES (?1, 'fedora-44', 'firefox', 'contract')",
        [canonical_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repositories (name, url, enabled, priority)
         VALUES ('fedora-44', 'https://example.com', 1, 10)",
        [],
    )
    .unwrap();
    let repo_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO repository_packages
         (repository_id, name, version, checksum, size, download_url, version_scheme)
         VALUES (?1, 'firefox', '125.0', 'sha256:abc', 1024,
                 'https://example.com/firefox.rpm', 'rpm')",
        [repo_id],
    )
    .unwrap();
    let package_id = conn.last_insert_rowid();

    assert_eq!(link_canonical_ids(&conn, repo_id).unwrap(), 0);
    let linked: Option<i64> = conn
        .query_row(
            "SELECT canonical_id FROM repository_packages WHERE id = ?1",
            [package_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(linked, None);
}
