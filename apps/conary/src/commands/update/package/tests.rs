// conary/src/commands/update/package/tests.rs

use super::*;
use crate::commands::test_helpers::create_test_db;
use conary_core::ccs::builder::{CcsBuilder, write_signed_current_ccs_package};
use conary_core::ccs::manifest::{CcsManifest, Platform};
use conary_core::ccs::native_lifecycle::{
    LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_V1, NativeInvocation, NativeLifecycleBundle,
    NativeLifecycleEntry, NativeLifecycleEntryKind, RpmCriticality, RpmProgram, RpmRuntimeMetadata,
    ScriptletFidelity, SourceFormat, TransactionOrder, VersionScheme,
};
use conary_core::db::models::{
    Changeset, ChangesetStatus, DistroPin, InstallSource, PackageDelta, PackageResolution,
    PrimaryStrategy, Repository, ResolutionStrategy, Trove, TroveType,
};
use conary_core::filesystem::{CasStore, object_path};
use conary_core::repository::resolution_policy::ResolutionPolicy;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

fn build_test_ccs_package_with_bundle(
    dir: &Path,
    name: &str,
    version: &str,
    native_lifecycle: Option<NativeLifecycleBundle>,
) -> PathBuf {
    let source_dir = dir.join("src");
    std::fs::create_dir_all(source_dir.join("usr/bin")).unwrap();
    std::fs::write(
        source_dir.join("usr/bin").join(name),
        format!("#!/bin/sh\necho {name} {version}\n"),
    )
    .unwrap();

    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.package.version_scheme = conary_core::repository::versioning::VersionScheme::Rpm;
    manifest.package.platform = Some(Platform {
        os: "linux".to_string(),
        arch: Some("x86_64".to_string()),
        libc: "gnu".to_string(),
        abi: None,
    });
    manifest.native_lifecycle = native_lifecycle;

    let result = CcsBuilder::new(manifest, &source_dir).build().unwrap();
    let package_path = dir.join(format!("{name}-{version}.ccs"));
    let signing_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    write_signed_current_ccs_package(&result, &package_path, &signing_key, true).unwrap();
    package_path
}

fn rpm_upgrade_bundle(package: &str, version: &str) -> NativeLifecycleBundle {
    let entry = rpm_upgrade_entry();
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: conary_core::ccs::native_lifecycle::NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora-rhel".to_string(),
        source_distro: Some("fedora".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package.to_string(),
        source_version: version.to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "remi".to_string(),
        conversion_tool_version: "0.8.0".to_string(),
        conversion_policy: "goal6-update-test".to_string(),
        evidence_digest: Some(conary_core::hash::sha256_prefixed(
            format!("{package}-{version}-typed-rpm-upgrade").as_bytes(),
        )),
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![entry],
    }
}

fn rpm_upgrade_entry() -> NativeLifecycleEntry {
    let body = "print('rpm-upgrade-new-pre')\n";
    NativeLifecycleEntry {
        id: "rpm:%pre".to_string(),
        native_slot: "%pre".to_string(),
        kind: NativeLifecycleEntryKind::Executable,
        phase: LifecyclePath::PreUpgrade,
        lifecycle_paths: vec!["upgrade:new-pre".to_string()],
        interpreter: "<lua>".to_string(),
        interpreter_args: Vec::new(),
        body_sha256: conary_core::hash::sha256_prefixed(body.as_bytes()),
        body: body.to_string(),
        body_encoding: None,
        native_invocation: NativeInvocation::default(),
        transaction_order: TransactionOrder {
            position: "before-payload".to_string(),
            before: vec!["payload".to_string()],
            after: Vec::new(),
        },
        timeout_ms: 30_000,
        sandbox: None,
        capabilities: Vec::new(),
        evidence_digest: None,
        source_evidence_refs: Vec::new(),
        rpm_trigger: None,
        rpm_runtime: Some(RpmRuntimeMetadata {
            program: RpmProgram::EmbeddedLua,
            body_transforms: Vec::new(),
            critical: true,
            criticality: RpmCriticality::Header,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: Vec::new(),
            macro_context: Default::default(),
            header_context: Default::default(),
            package_rpm_version: None,
        }),
        rpm_sysusers: None,
        deb_maintainer: None,
        arch_install: None,
        arch_hook: None,
        residual_lifecycle: None,
    }
}

fn serve_test_file(file_path: PathBuf) -> (String, std::thread::JoinHandle<()>) {
    let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
    let bytes = std::fs::read(&file_path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            stream.write_all(headers.as_bytes()).unwrap();
            stream.write_all(&bytes).unwrap();
        }
    });
    (format!("http://{addr}/{filename}"), handle)
}

fn table_count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn package_specific_update_requires_selector_for_ambiguous_variants() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();

    for arch in ["x86_64", "aarch64"] {
        let mut trove = Trove::new(
            "demo".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            conary_core::repository::versioning::VersionScheme::Conary,
        );
        trove.architecture = Some(arch.to_string());
        trove.insert(&conn).unwrap();
    }

    let err = installed_troves_for_update(&conn, Some("demo".to_string()), None, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("Multiple installed variants"), "{err}");
    assert!(err.contains("--arch"), "{err}");

    let selected = installed_troves_for_update(
        &conn,
        Some("demo".to_string()),
        Some("1.0.0".to_string()),
        Some("aarch64".to_string()),
    )
    .unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].architecture.as_deref(), Some("aarch64"));
}

#[test]
fn update_selector_without_package_refuses() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();

    let err = installed_troves_for_update(&conn, None, None, Some("x86_64".to_string()))
        .unwrap_err()
        .to_string();

    assert!(err.contains("A package name is required"), "{err}");
}

#[tokio::test]
async fn update_executes_typed_rpm_lifecycle_and_commits_changeset() {
    let (_temp, db_path) = create_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let package_path = build_test_ccs_package_with_bundle(
        package_dir.path(),
        "vim",
        "2.0.0",
        Some(rpm_upgrade_bundle("vim", "2.0.0")),
    );
    let package_bytes = std::fs::read(&package_path).unwrap();
    let package_checksum = conary_core::hash::sha256(&package_bytes);
    let package_size = i64::try_from(package_bytes.len()).unwrap();
    let (package_url, _server_handle) = serve_test_file(package_path);

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    DistroPin::set(&conn, "fedora-44", "strict").unwrap();
    let mut repo = Repository::new("fedora-test".to_string(), package_url.clone());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install vim-1.0.0".to_string());
        let changeset_id = changeset.insert(tx)?;
        let mut installed = Trove::new_with_source(
            "vim".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-44".to_string());
        installed.installed_from_repository_id = Some(repo_id);
        installed.installed_by_changeset_id = Some(changeset_id);
        installed.insert(tx)?;
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "vim".to_string(),
        "2.0.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        package_checksum.clone(),
        package_size,
        package_url.clone(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.distro = Some("fedora-44".to_string());
    repo_pkg.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: package_url,
            checksum: package_checksum,
            delta_base: None,
        }],
    );
    resolution.version = Some("2.0.0".to_string());
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.insert(&conn).unwrap();

    let before_changesets = table_count(&conn, "changesets");
    drop(conn);

    cmd_update(
        Some("vim".to_string()),
        &db_path,
        root.path().to_str().unwrap(),
        false,
        false,
        SandboxMode::Always,
        None,
        true,
        None,
        Some("x86_64".to_string()),
    )
    .await
    .expect("typed RPM lifecycle update should execute");

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert!(
        table_count(&conn, "changesets") > before_changesets,
        "successful typed lifecycle update must commit a changeset"
    );
    let installed_versions = Trove::find_by_name(&conn, "vim")
        .unwrap()
        .into_iter()
        .filter(|trove| trove.trove_type == TroveType::Package)
        .map(|trove| trove.version)
        .collect::<Vec<_>>();
    assert_eq!(installed_versions, vec!["2.0.0".to_string()]);
}

#[tokio::test]
async fn static_ccs_update_verifies_signature_before_lifecycle_execution_preflight() {
    let (_temp, db_path) = create_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let package_path = build_test_ccs_package_with_bundle(
        package_dir.path(),
        "vim",
        "2.0.0",
        Some(rpm_upgrade_bundle("vim", "2.0.0")),
    );
    let package_bytes = std::fs::read(&package_path).unwrap();
    let package_checksum = conary_core::hash::sha256(&package_bytes);
    let package_size = i64::try_from(package_bytes.len()).unwrap();
    let (package_url, _server_handle) = serve_test_file(package_path);

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    DistroPin::set(&conn, "fedora-44", "strict").unwrap();
    let mut repo = Repository::new("static-test".to_string(), package_url.clone());
    repo.default_strategy = Some("static".to_string());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install vim-1.0.0".to_string());
        let changeset_id = changeset.insert(tx)?;
        let mut installed = Trove::new_with_source(
            "vim".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-44".to_string());
        installed.installed_from_repository_id = Some(repo_id);
        installed.installed_by_changeset_id = Some(changeset_id);
        installed.insert(tx)?;
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "vim".to_string(),
        "2.0.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        package_checksum.clone(),
        package_size,
        package_url.clone(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.distro = Some("fedora-44".to_string());
    repo_pkg.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: package_url,
            checksum: package_checksum,
            delta_base: None,
        }],
    );
    resolution.version = Some("2.0.0".to_string());
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.insert(&conn).unwrap();

    let before_changesets = table_count(&conn, "changesets");
    drop(conn);

    let err = cmd_update(
        Some("vim".to_string()),
        &db_path,
        root.path().to_str().unwrap(),
        false,
        false,
        SandboxMode::Always,
        None,
        true,
        None,
        Some("x86_64".to_string()),
    )
    .await
    .expect_err("static unsigned update must fail before CCS preflight parses scriptlets");
    let message = format!("{err:?}");
    assert!(
        message.contains("Static repository package signature verification failed"),
        "{message}"
    );

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert_eq!(
        table_count(&conn, "changesets"),
        before_changesets,
        "static signature refusal must happen before update changeset insertion"
    );
}

#[tokio::test]
async fn update_delta_candidate_executes_typed_rpm_lifecycle() {
    let (_temp, db_path) = create_test_db();
    let root = tempfile::tempdir().unwrap();
    let package_dir = tempfile::tempdir().unwrap();
    let _guard = crate::commands::composefs_ops::test_mount_skip_guard();

    let package_path = build_test_ccs_package_with_bundle(
        package_dir.path(),
        "vim",
        "2.0.0",
        Some(rpm_upgrade_bundle("vim", "2.0.0")),
    );
    let package_bytes = std::fs::read(&package_path).unwrap();
    let package_checksum = conary_core::hash::sha256(&package_bytes);
    let package_size = i64::try_from(package_bytes.len()).unwrap();
    let (package_url, _server_handle) = serve_test_file(package_path);

    let mut conn = crate::commands::open_db(&db_path).unwrap();
    DistroPin::set(&conn, "fedora-44", "strict").unwrap();
    let mut repo = Repository::new("fedora-test".to_string(), package_url.clone());
    repo.default_strategy_distro = Some("fedora-44".to_string());
    let repo_id = repo.insert(&conn).unwrap();

    conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = Changeset::new("Install vim-1.0.0".to_string());
        let changeset_id = changeset.insert(tx)?;
        let mut installed = Trove::new_with_source(
            "vim".to_string(),
            "1.0.0".to_string(),
            TroveType::Package,
            InstallSource::Repository,
            conary_core::repository::versioning::VersionScheme::Rpm,
        );
        installed.architecture = Some("x86_64".to_string());
        installed.source_distro = Some("fedora-44".to_string());
        installed.installed_from_repository_id = Some(repo_id);
        installed.installed_by_changeset_id = Some(changeset_id);
        installed.insert(tx)?;
        changeset.update_status(tx, ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(())
    })
    .unwrap();

    let mut repo_pkg = RepositoryPackage::new(
        repo_id,
        "vim".to_string(),
        "2.0.0".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        package_checksum.clone(),
        package_size,
        package_url.clone(),
    );
    repo_pkg.architecture = Some("x86_64".to_string());
    repo_pkg.distro = Some("fedora-44".to_string());
    repo_pkg.insert(&conn).unwrap();

    let mut resolution = PackageResolution::new(
        repo_id,
        "vim".to_string(),
        vec![ResolutionStrategy::Binary {
            url: package_url,
            checksum: package_checksum,
            delta_base: None,
        }],
    );
    resolution.version = Some("2.0.0".to_string());
    resolution.primary_strategy = PrimaryStrategy::Binary;
    resolution.insert(&conn).unwrap();

    let from_hash = conary_core::hash::sha256(b"old-package-placeholder");
    let to_hash = conary_core::hash::sha256(b"new-package-placeholder");
    for hash in [&from_hash, &to_hash] {
        conn.execute(
            "INSERT INTO file_contents (sha256_hash, content_path, size) VALUES (?1, ?2, 0)",
            rusqlite::params![hash, format!("objects/{hash}")],
        )
        .unwrap();
    }

    let mut delta = PackageDelta::new(
        "vim".to_string(),
        "1.0.0".to_string(),
        "2.0.0".to_string(),
        from_hash,
        to_hash,
        "http://127.0.0.1:9/vim.delta".to_string(),
        1,
        conary_core::hash::sha256(b"unused-delta"),
        package_size,
    );
    delta.insert(&conn).unwrap();

    let before_changesets = table_count(&conn, "changesets");
    drop(conn);

    cmd_update(
        Some("vim".to_string()),
        &db_path,
        root.path().to_str().unwrap(),
        false,
        false,
        SandboxMode::Always,
        None,
        true,
        None,
        Some("x86_64".to_string()),
    )
    .await
    .expect("delta-selected typed RPM lifecycle update should execute");

    let conn = crate::commands::open_db(&db_path).unwrap();
    assert!(
        table_count(&conn, "changesets") > before_changesets,
        "successful delta-selected lifecycle update must commit a changeset"
    );
    let installed_versions = Trove::find_by_name(&conn, "vim")
        .unwrap()
        .into_iter()
        .filter(|trove| trove.trove_type == TroveType::Package)
        .map(|trove| trove.version)
        .collect::<Vec<_>>();
    assert_eq!(installed_versions, vec!["2.0.0".to_string()]);
}

#[test]
fn update_repository_install_provenance_uses_selected_package_metadata() {
    let mut repo = Repository::new(
        "slice-d-local-update".to_string(),
        "https://example.test/slice-d".to_string(),
    );
    repo.default_strategy_distro = Some("fedora".to_string());
    repo.id = Some(42);

    let mut package = RepositoryPackage::new(
        42,
        "phase4-runtime-fixture".to_string(),
        "1.0.1-1".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "sha256:fixture".to_string(),
        123,
        "https://example.test/phase4-runtime-fixture-1.0.1.rpm".to_string(),
    );
    package.architecture = Some("x86_64".to_string());
    package.distro = Some("fedora".to_string());

    let provenance = repository_install_provenance_from_package(&package, &repo).unwrap();

    assert_eq!(provenance.repository_id, 42);
    assert_eq!(provenance.source_distro.as_deref(), Some("fedora"));
    assert_eq!(
        provenance.version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
}

#[test]
fn selected_update_resolution_bypasses_local_cas_shortcut() {
    let temp = tempfile::tempdir().unwrap();
    let keyring_dir = temp.path().join("keyrings");
    let repo = Repository::new(
        "slice-d-source-switch".to_string(),
        "https://example.test/slice-d".to_string(),
    );
    let mut package = RepositoryPackage::new(
        42,
        "phase4-runtime-fixture".to_string(),
        "1.0.1-1".to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        "sha256:fixture".to_string(),
        123,
        "https://example.test/phase4-runtime-fixture-1.0.1.rpm".to_string(),
    );
    package.architecture = Some("x86_64".to_string());

    let options = resolution_options_for_selected_update(
        &package,
        &repo,
        temp.path(),
        &keyring_dir,
        &ResolutionPolicy::new(),
        Some(RepositoryDependencyFlavor::Rpm),
    );

    assert!(options.skip_installed);
    assert_eq!(options.version.as_deref(), Some("1.0.1-1"));
    assert_eq!(options.repository.as_deref(), Some("slice-d-source-switch"));
    assert_eq!(options.architecture.as_deref(), Some("x86_64"));
}

#[test]
fn partial_update_failure_message_is_not_clean_success() {
    let failures = vec![UpdatePackageFailure {
        package: "broken".to_string(),
        version: "2.0.0".to_string(),
        reason: "resolver failed".to_string(),
    }];

    let message = update_required_failure_message(&failures, 2).unwrap();

    assert!(message.contains("1 of 2"));
    assert!(message.contains("broken"));
    assert!(!message.contains("All packages are up to date"));
}

#[test]
fn delta_result_uses_verified_cas_retrieval() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cas = CasStore::new(temp_dir.path()).unwrap();
    let expected_hash = conary_core::hash::sha256(b"expected-bytes");
    let corrupted_path = object_path(temp_dir.path(), &expected_hash).unwrap();
    std::fs::create_dir_all(corrupted_path.parent().unwrap()).unwrap();
    std::fs::write(&corrupted_path, b"corrupted-bytes").unwrap();

    assert!(read_delta_result_from_cas(&cas, &expected_hash).is_err());
}

#[test]
fn mark_pending_changeset_rolled_back_updates_pending_rows() {
    let (_temp, db_path) = create_test_db();
    let mut conn = rusqlite::Connection::open(&db_path).unwrap();
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = conary_core::db::models::Changeset::new("test update".to_string());
        changeset.insert(tx)
    })
    .unwrap();

    assert!(mark_pending_changeset_rolled_back(&mut conn, changeset_id).unwrap());

    let changeset = conary_core::db::models::Changeset::find_by_id(&conn, changeset_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        changeset.status,
        conary_core::db::models::ChangesetStatus::RolledBack
    );
}

#[test]
fn mark_pending_changeset_rolled_back_leaves_applied_rows_alone() {
    let (_temp, db_path) = create_test_db();
    let mut conn = rusqlite::Connection::open(&db_path).unwrap();
    let changeset_id = conary_core::db::transaction(&mut conn, |tx| {
        let mut changeset = conary_core::db::models::Changeset::new("applied update".to_string());
        let id = changeset.insert(tx)?;
        changeset.update_status(tx, conary_core::db::models::ChangesetStatus::Applied)?;
        Ok::<_, conary_core::Error>(id)
    })
    .unwrap();

    assert!(!mark_pending_changeset_rolled_back(&mut conn, changeset_id).unwrap());

    let changeset = conary_core::db::models::Changeset::find_by_id(&conn, changeset_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        changeset.status,
        conary_core::db::models::ChangesetStatus::Applied
    );
}
