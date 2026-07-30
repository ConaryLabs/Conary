// apps/conary/src/commands/install/conversion/tests/dependencies.rs

use super::*;

fn write_signed_ccs_fixture(
    directory: &std::path::Path,
    name: &str,
    mut manifest: CcsManifest,
    payload_path: &str,
    payload: &[u8],
    signing_key: &SigningKeyPair,
) -> std::path::PathBuf {
    let package_path = directory.join(format!("{name}.ccs"));
    let payload_hash = hash::sha256(payload);
    let files = vec![regular_ccs_file(
        payload_path,
        payload_hash.clone(),
        payload.len() as u64,
        0o755,
    )];
    manifest.components.default = vec!["runtime".to_string()];
    let result = BuildResult {
        manifest,
        components: HashMap::from([(
            "runtime".to_string(),
            ComponentData {
                name: "runtime".to_string(),
                files: files.clone(),
                hash: "runtime".to_string(),
                size: payload.len() as u64,
            },
        )]),
        files: files.clone(),
        payloads: conary_core::ccs::builder::payloads_from_bounded_memory_for_tests(
            &files,
            HashMap::from([(payload_hash, payload.to_vec())]),
        )
        .unwrap(),
        total_size: payload.len() as u64,
        chunked: false,
        chunk_stats: None,
    };
    write_signed_current_ccs_package(&result, &package_path, signing_key, false).unwrap();
    package_path
}

fn rpm_pretransaction_absence_bundle(
    package_name: &str,
    package_version: &str,
    absent_path: &str,
) -> conary_core::ccs::native_lifecycle::NativeLifecycleBundle {
    use conary_core::ccs::native_lifecycle::{
        LifecyclePath, NATIVE_LIFECYCLE_SCHEMA_REVISION, NATIVE_LIFECYCLE_SCHEMA_V1,
        NativeInvocation, NativeLifecycleBundle, NativeLifecycleEntry, NativeLifecycleEntryKind,
        RpmCriticality, RpmProgram, RpmRuntimeMetadata, ScriptletFidelity, SourceFormat,
        TransactionOrder, VersionScheme,
    };

    let body = format!(
        "assert(#rpm.glob({absent_path:?}) == 0, 'dependency payload was visible during root pre-transaction')\n"
    );
    NativeLifecycleBundle {
        schema: NATIVE_LIFECYCLE_SCHEMA_V1.to_string(),
        schema_revision: NATIVE_LIFECYCLE_SCHEMA_REVISION,
        source_format: SourceFormat::Rpm,
        source_family: "fedora".to_string(),
        source_profile: Some("fedora-44".to_string()),
        source_release: Some("44".to_string()),
        source_arch: Some("x86_64".to_string()),
        source_package: package_name.to_string(),
        source_version: package_version.to_string(),
        source_checksum: None,
        version_scheme: VersionScheme::Rpm,
        conversion_tool: "test".to_string(),
        conversion_tool_version: "1".to_string(),
        conversion_policy: "transaction-order-regression".to_string(),
        evidence_digest: None,
        scriptlet_fidelity: ScriptletFidelity::NativeLifecycle,
        entries: vec![NativeLifecycleEntry {
            id: "rpm:%pretrans".to_string(),
            native_slot: "%pretrans".to_string(),
            kind: NativeLifecycleEntryKind::Executable,
            phase: LifecyclePath::PreTransaction,
            lifecycle_paths: vec![LifecyclePath::PreTransaction.as_str().to_string()],
            interpreter: "<lua>".to_string(),
            interpreter_args: Vec::new(),
            body_sha256: hash::sha256_prefixed(body.as_bytes()),
            body,
            body_encoding: None,
            native_invocation: NativeInvocation::default(),
            transaction_order: TransactionOrder {
                position: "pre-transaction".to_string(),
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
        }],
    }
}

fn verified_rpm_ccs(name: &str, version: &str, architecture: &str) -> CcsPackage {
    let temp_dir = tempfile::tempdir().unwrap();
    let signing_key = SigningKeyPair::generate().with_key_id("dependency-identity");
    let mut manifest = CcsManifest::new_minimal(name, version);
    manifest.package.version_scheme = conary_core::repository::versioning::VersionScheme::Rpm;
    manifest.package.platform.as_mut().unwrap().arch = Some(architecture.to_string());
    let package_path =
        write_runtime_signed_ccs_package(temp_dir.path(), name, manifest, &signing_key);
    let verified = conary_core::ccs::verify::verify_package(
        &package_path,
        &conary_core::ccs::TrustPolicy::strict(vec![signing_key.public_key_base64()]),
    )
    .unwrap();
    CcsPackage::from_verified_archive(package_path.to_str().unwrap(), &verified).unwrap()
}

fn selected_rpm_dependency() -> SatPackage {
    SatPackage {
        name: "dbus-broker".to_string(),
        version: "37-8.fc44".to_string(),
        package_release: None,
        architecture: Some("x86_64".to_string()),
        version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
        repo_package_id: Some(41),
        repository_id: Some(7),
        repository_name: Some("supported-host-fedora-44".to_string()),
        installed_trove_id: None,
        source: SatSource::Repository,
    }
}

fn selected_remi_provenance() -> RepositoryInstallProvenance {
    RepositoryInstallProvenance {
        repository_id: 7,
        source_profile: Some("fedora-44".to_string()),
        version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
        source_kind: RepositorySourceKind::Remi,
    }
}

#[test]
fn verified_remi_ccs_matches_exact_selected_source_identity() {
    let package = verified_rpm_ccs("dbus-broker", "37-8.fc44", "x86_64");
    let selected = selected_rpm_dependency();

    validate_selected_repository_ccs_identity(
        &package,
        &selected,
        Some(&selected_remi_provenance()),
    )
    .expect("signed CCS identity must match its exact selected Remi row");
}

#[test]
fn pending_remi_root_matches_selected_source_identity_without_ccs_release() {
    let package = verified_rpm_ccs("dbus-broker", "37-8.fc44", "x86_64");
    let selected = selected_rpm_dependency();

    assert!(
        PendingCcsProvider::from_package(&package).matches(&selected),
        "the pending root must provide its own source identity when the Remi row omits the CCS build release"
    );
}

#[test]
fn pending_static_root_release_stays_exact() {
    let package = verified_rpm_ccs("dbus-broker", "37-8.fc44", "x86_64");
    let pending = PendingCcsProvider::from_package(&package);
    let mut selected = selected_rpm_dependency();

    selected.package_release = package.package_release().map(str::to_string);
    assert!(
        pending.matches(&selected),
        "an explicitly selected matching CCS release must identify the pending root"
    );

    selected.package_release = Some("2".to_string());
    assert!(
        !pending.matches(&selected),
        "an explicitly selected mismatched CCS release must not identify the pending root"
    );
}

#[test]
fn verified_remi_ccs_rejects_any_selected_identity_drift() {
    let package = verified_rpm_ccs("dbus-broker", "37-8.fc44", "x86_64");
    let provenance = selected_remi_provenance();

    let mut mismatches = Vec::new();

    let mut name = selected_rpm_dependency();
    name.name = "dbus-daemon".to_string();
    mismatches.push(name);

    let mut version = selected_rpm_dependency();
    version.version = "37-9.fc44".to_string();
    mismatches.push(version);

    let mut architecture = selected_rpm_dependency();
    architecture.architecture = Some("aarch64".to_string());
    mismatches.push(architecture);

    let mut scheme = selected_rpm_dependency();
    scheme.version_scheme = conary_core::repository::versioning::VersionScheme::Debian;
    mismatches.push(scheme);

    let mut repository = selected_rpm_dependency();
    repository.repository_id = Some(8);
    mismatches.push(repository);

    for selected in mismatches {
        let error =
            validate_selected_repository_ccs_identity(&package, &selected, Some(&provenance))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match SAT-selected repository row"),
            "identity drift must fail before mutation: {error:#}"
        );
    }
}

#[test]
fn explicit_static_ccs_release_must_match_selected_row() {
    let package = verified_rpm_ccs("dbus-broker", "37-8.fc44", "x86_64");
    let mut selected = selected_rpm_dependency();
    selected.package_release = Some("2".to_string());

    let error = validate_selected_repository_ccs_identity(
        &package,
        &selected,
        Some(&selected_remi_provenance()),
    )
    .unwrap_err();

    assert!(
        error.to_string().contains("package release"),
        "an explicitly selected CCS release must remain exact: {error:#}"
    );
}

#[test]
fn native_dependency_must_match_every_selected_identity_field() {
    let selected = selected_rpm_dependency();
    let provenance = RepositoryInstallProvenance {
        source_kind: RepositorySourceKind::Native,
        ..selected_remi_provenance()
    };

    validate_selected_repository_native_identity(
        "dbus-broker",
        "37-8.fc44",
        None,
        Some("x86_64"),
        conary_core::repository::versioning::VersionScheme::Rpm,
        &selected,
        &provenance,
    )
    .expect("the exact native artifact identity must match its selected row");

    let mismatches = [
        validate_selected_repository_native_identity(
            "dbus-daemon",
            "37-8.fc44",
            None,
            Some("x86_64"),
            conary_core::repository::versioning::VersionScheme::Rpm,
            &selected,
            &provenance,
        ),
        validate_selected_repository_native_identity(
            "dbus-broker",
            "37-9.fc44",
            None,
            Some("x86_64"),
            conary_core::repository::versioning::VersionScheme::Rpm,
            &selected,
            &provenance,
        ),
        validate_selected_repository_native_identity(
            "dbus-broker",
            "37-8.fc44",
            Some("2"),
            Some("x86_64"),
            conary_core::repository::versioning::VersionScheme::Rpm,
            &selected,
            &provenance,
        ),
        validate_selected_repository_native_identity(
            "dbus-broker",
            "37-8.fc44",
            None,
            Some("aarch64"),
            conary_core::repository::versioning::VersionScheme::Rpm,
            &selected,
            &provenance,
        ),
        validate_selected_repository_native_identity(
            "dbus-broker",
            "37-8.fc44",
            None,
            Some("x86_64"),
            conary_core::repository::versioning::VersionScheme::Debian,
            &selected,
            &provenance,
        ),
    ];

    for mismatch in mismatches {
        let error = mismatch.expect_err("every selected native identity field is authoritative");
        assert!(
            error
                .to_string()
                .contains("does not match SAT-selected repository row"),
            "{error:#}"
        );
    }
}

#[tokio::test]
async fn repository_ccs_closure_runs_root_pretransaction_before_dependency_payload() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    let install_root = temp.path().join("root");
    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(&db_path).unwrap();
    stage_test_boot_assets(temp.path());

    let dependency_name = "transaction-order-dependency";
    let dependency_version = "1.0-1";
    let dependency_path = "/usr/lib/transaction-order-dependency";
    let repository_key = SigningKeyPair::generate().with_key_id("transaction-order-repository");
    let mut dependency_manifest = CcsManifest::new_minimal(dependency_name, dependency_version);
    dependency_manifest.package.version_scheme =
        conary_core::repository::versioning::VersionScheme::Rpm;
    dependency_manifest.package.platform.as_mut().unwrap().arch = Some("x86_64".to_string());
    let dependency_release = dependency_manifest.package.release.clone();
    let dependency_artifact = write_signed_ccs_fixture(
        temp.path(),
        dependency_name,
        dependency_manifest,
        dependency_path,
        b"dependency payload",
        &repository_key,
    );
    let dependency_bytes = std::fs::read(&dependency_artifact).unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut repository = Repository::new(
        "transaction-order-static".to_string(),
        temp.path().to_string_lossy().into_owned(),
    );
    repository.default_strategy = Some("static".to_string());
    repository.source_profile = Some("fedora-44".to_string());
    let repository_id = repository.insert(&conn).unwrap();
    RepositoryPackageKey::replace_for_repository(
        &conn,
        repository_id,
        &[package_key(
            repository_id,
            &repository_key,
            RepositoryPackageKeyStatus::Active,
        )],
    )
    .unwrap();
    let mut repository_package = conary_core::db::models::RepositoryPackage::new(
        repository_id,
        dependency_name.to_string(),
        dependency_version.to_string(),
        conary_core::repository::versioning::VersionScheme::Rpm,
        hash::sha256(&dependency_bytes),
        dependency_bytes.len() as i64,
        dependency_artifact.to_string_lossy().into_owned(),
    );
    repository_package.package_release = dependency_release;
    repository_package.architecture = Some("x86_64".to_string());
    repository_package.source_profile = Some("fedora-44".to_string());
    let repository_package_id = repository_package.insert(&conn).unwrap();
    let mut package_provide = conary_core::db::models::RepositoryProvide::new(
        repository_package_id,
        dependency_name.to_string(),
        Some(dependency_version.to_string()),
        "package".to_string(),
        None,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    package_provide.insert(&conn).unwrap();
    drop(conn);

    let root_name = "transaction-order-root";
    let root_version = "1.0-1";
    let mut root_manifest = CcsManifest::new_minimal(root_name, root_version);
    root_manifest.package.version_scheme = conary_core::repository::versioning::VersionScheme::Rpm;
    root_manifest.package.platform.as_mut().unwrap().arch = Some("x86_64".to_string());
    root_manifest.requirements = vec![
        conary_core::repository::dependency_model::RepositoryRequirementGroup::simple(
            conary_core::repository::dependency_model::RepositoryRequirementKind::Depends,
            conary_core::repository::dependency_model::RepositoryRequirementClause::name_only(
                dependency_name.to_string(),
            ),
        ),
    ];
    root_manifest.native_lifecycle = Some(rpm_pretransaction_absence_bundle(
        root_name,
        root_version,
        dependency_path,
    ));
    let local_key = crate::commands::ccs::load_or_create_local_dev_key().unwrap();
    let root_artifact = write_signed_ccs_fixture(
        temp.path(),
        root_name,
        root_manifest,
        "/usr/sbin/init",
        b"test init",
        &local_key,
    );

    let installed_root = install_ccs_artifact(CcsArtifactInstallOptions {
        ccs_path: root_artifact.to_str().unwrap(),
        db_path: db_path.to_str().unwrap(),
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::Always,
        no_deps: false,
        allow_downgrade: false,
        intent: InstallIntent::PackageChange,
        yes: true,
        envelope_authority: CcsEnvelopeAuthority::LocalDev,
        repository_provenance: None,
        resolution_policy: test_resolution_policy().with_primary_profile("fedora-44"),
    })
    .await
    .unwrap()
    .expect("the root package must install in the shared closure transaction");

    let conn = conary_core::db::open(&db_path).unwrap();
    assert_eq!(
        Trove::find_by_name(&conn, dependency_name).unwrap().len(),
        1
    );
    assert_eq!(Trove::find_by_name(&conn, root_name).unwrap().len(), 1);
    assert_eq!(
        Trove::find_by_id(&conn, installed_root)
            .unwrap()
            .unwrap()
            .name,
        root_name
    );
    let changesets = conary_core::db::models::Changeset::list_all(&conn).unwrap();
    assert_eq!(
        changesets.len(),
        1,
        "the dependency closure must commit as one changeset"
    );
    assert_eq!(
        changesets[0].status,
        conary_core::db::models::ChangesetStatus::Applied
    );
}
