// conary-core/src/repository/declarations/takeover/tests.rs

use super::*;
use crate::repository::{OpenPgpTrustRoot, RpmMetadataAuthority};
use openpgp::cert::prelude::CertBuilder;
use openpgp::serialize::Serialize;
use sequoia_openpgp as openpgp;
use std::os::unix::fs::PermissionsExt;

fn certificate() -> (Vec<u8>, String) {
    let (certificate, _) = CertBuilder::new()
        .add_userid("Native takeover fixture")
        .generate()
        .unwrap();
    let fingerprint = certificate.fingerprint().to_hex();
    let mut bytes = Vec::new();
    certificate.serialize(&mut bytes).unwrap();
    (bytes, fingerprint)
}

fn armored_certificate() -> (String, String) {
    let (certificate, _) = CertBuilder::new()
        .add_userid("Embedded takeover fixture")
        .generate()
        .unwrap();
    let fingerprint = certificate.fingerprint().to_hex();
    let mut bytes = Vec::new();
    certificate.armored().serialize(&mut bytes).unwrap();
    (String::from_utf8(bytes).unwrap(), fingerprint)
}

fn fixture() -> (
    tempfile::TempDir,
    Connection,
    NativeRepositoryTakeoverManifest,
) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("etc/yum.repos.d")).unwrap();
    fs::create_dir_all(root.path().join("etc/pki/rpm-gpg")).unwrap();
    let (certificate, fingerprint) = certificate();
    let key_path = root.path().join("etc/pki/rpm-gpg/VENDOR");
    fs::write(&key_path, certificate).unwrap();
    let declaration_path = root.path().join("etc/yum.repos.d/vendor.repo");
    fs::write(
        &declaration_path,
        "[vendor]\nenabled=1\npriority=40\nmetalink=https://mirrors.example/metalink\ngpgcheck=1\nrepo_gpgcheck=0\ngpgkey=file:///etc/pki/rpm-gpg/VENDOR\n",
    )
    .unwrap();
    fs::set_permissions(&declaration_path, fs::Permissions::from_mode(0o644)).unwrap();
    let db_path = root.path().join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();
    let declarations = discover_selected_root(root.path()).unwrap();
    let reference = plan_selected_root_trust(&declarations).repositories[0]
        .repository
        .clone();
    let manifest = NativeRepositoryTakeoverManifest {
        schema: TAKEOVER_MANIFEST_SCHEMA,
        repositories: vec![TakeoverRepositoryInput {
            declaration: reference,
            name: "vendor".to_string(),
            source_identity: "vendor-rpm".to_string(),
            repository_identity: "vendor".to_string(),
            scope: TakeoverPolicyScope::Repository {
                identity: "vendor".to_string(),
            },
            stream: TakeoverSourceStream::Release {
                identity: "stable".to_string(),
            },
            update: TakeoverUpdatePolicy::Follow,
            metadata_url: "https://packages.example/repo".to_string(),
            content_url: Some("https://packages.example/content".to_string()),
            parser: RepositoryParserConfig::Rpm {
                architecture: "x86_64".to_string(),
            },
            trust: RepositoryTrustPolicy::Rpm {
                metadata: RpmMetadataAuthority::Metalink {
                    url: "https://mirrors.example/metalink".to_string(),
                },
                package_keys: vec![OpenPgpTrustRoot {
                    url: url::Url::from_file_path(key_path).unwrap().to_string(),
                    fingerprint,
                }],
            },
            enabled: true,
            priority: 40,
            metadata_expire: 3600,
            source_profile: None,
        }],
    };
    (root, conn, manifest)
}

#[test]
fn preview_is_deterministic_serializable_and_rejects_unknown_input() {
    let (root, conn, manifest) = fixture();
    let first = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    let second = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert_eq!(first, second);
    assert!(first.blockers.is_empty());
    assert_eq!(first.sha256().unwrap(), second.sha256().unwrap());
    let encoded = serde_json::to_string(&manifest).unwrap();
    let malformed = encoded.replacen("{", "{\"future\":true,", 1);
    assert!(serde_json::from_str::<NativeRepositoryTakeoverManifest>(&malformed).is_err());
}

#[test]
fn preview_envelope_binds_the_complete_typed_preview() {
    let (root, conn, manifest) = fixture();
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    let expected_sha256 = preview.sha256().unwrap();
    let envelope = NativeRepositoryTakeoverPreviewEnvelope::new(preview.clone()).unwrap();

    assert_eq!(envelope.schema, TAKEOVER_PREVIEW_ENVELOPE_SCHEMA);
    assert_eq!(envelope.preview_sha256, expected_sha256);
    assert_eq!(envelope.preview, preview);

    let encoded = serde_json::to_vec(&envelope).unwrap();
    let decoded: NativeRepositoryTakeoverPreviewEnvelope =
        serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, envelope);
}

#[test]
fn ambiguous_enabled_trust_blocks_without_database_or_filesystem_changes() {
    let (root, conn, mut manifest) = fixture();
    let declaration_path = root.path().join("etc/yum.repos.d/vendor.repo");
    fs::write(
        &declaration_path,
        "[vendor]\nenabled=1\nbaseurl=https://packages.example/repo\n",
    )
    .unwrap();
    let declarations = discover_selected_root(root.path()).unwrap();
    manifest.repositories[0].declaration = plan_selected_root_trust(&declarations).repositories[0]
        .repository
        .clone();
    let before = fs::read(&declaration_path).unwrap();
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(
        preview
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TakeoverBlocker::EnabledTrust { .. }))
    );
    assert!(
        apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
            .is_err()
    );
    assert_eq!(fs::read(&declaration_path).unwrap(), before);
    assert!(Repository::find_by_name(&conn, "vendor").unwrap().is_none());
}

#[test]
fn enrollment_cannot_substitute_unrelated_trust_for_imported_evidence() {
    let (root, conn, mut manifest) = fixture();
    let (_, unrelated_fingerprint) = certificate();
    let RepositoryTrustPolicy::Rpm { package_keys, .. } = &mut manifest.repositories[0].trust
    else {
        unreachable!("fixture is RPM")
    };
    package_keys[0].fingerprint = unrelated_fingerprint;

    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(preview.blockers.iter().any(|blocker| matches!(
        blocker,
        TakeoverBlocker::TrustPolicyMismatch { name, .. } if name == "vendor"
    )));
    assert!(apply_native_repository_takeover(
        &conn,
        root.path(),
        &manifest,
        &preview.sha256().unwrap(),
    )
    .is_err());
    assert!(Repository::find_by_name(&conn, "vendor").unwrap().is_none());
}

#[test]
fn complete_enrollment_includes_disabled_repositories_and_exact_products() {
    let (root, conn, mut manifest) = fixture();
    let declaration_path = root.path().join("etc/yum.repos.d/vendor.repo");
    fs::write(
        &declaration_path,
        "[vendor]\nenabled=0\npriority=40\nmetalink=https://mirrors.example/metalink\ngpgcheck=1\nrepo_gpgcheck=0\ngpgkey=file:///etc/pki/rpm-gpg/VENDOR\n",
    )
    .unwrap();
    let declarations = discover_selected_root(root.path()).unwrap();
    manifest.repositories[0].declaration = plan_selected_root_trust(&declarations).repositories[0]
        .repository
        .clone();
    manifest.repositories[0].enabled = false;

    let mut missing = manifest.clone();
    missing.repositories.clear();
    let preview = preview_native_repository_takeover(&conn, root.path(), &missing).unwrap();
    assert!(
        preview
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, TakeoverBlocker::MissingEnrollment { .. }))
    );

    let mut duplicate = manifest;
    let mut second = duplicate.repositories[0].clone();
    second.name = "vendor-copy".to_string();
    second.source_identity = "vendor-rpm-copy".to_string();
    second.repository_identity = "vendor-copy".to_string();
    second.scope = TakeoverPolicyScope::Repository {
        identity: "vendor-copy".to_string(),
    };
    duplicate.repositories.push(second);
    let preview = preview_native_repository_takeover(&conn, root.path(), &duplicate).unwrap();
    assert!(preview.blockers.iter().any(|blocker| matches!(
        blocker,
        TakeoverBlocker::DuplicateEnrollmentProduct { names, .. }
            if names == &["vendor".to_string(), "vendor-copy".to_string()]
    )));
}

#[test]
fn parser_and_existing_ownership_conflicts_leave_authority_unchanged() {
    let (root, conn, mut manifest) = fixture();
    let release_keys = match &manifest.repositories[0].trust {
        RepositoryTrustPolicy::Rpm { package_keys, .. } => package_keys.clone(),
        _ => unreachable!("fixture is RPM"),
    };
    manifest.repositories[0].parser = RepositoryParserConfig::Deb {
        distribution: "stable".to_string(),
        component: "main".to_string(),
        architecture: "amd64".to_string(),
    };
    manifest.repositories[0].trust = RepositoryTrustPolicy::Debian { release_keys };
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(preview.blockers.iter().any(|blocker| matches!(
        blocker,
        TakeoverBlocker::ParserFormatMismatch {
            expected: RepositoryFormat::Fedora,
            requested: RepositoryFormat::Debian,
            ..
        }
    )));

    manifest.repositories[0].parser = RepositoryParserConfig::Rpm {
        architecture: "x86_64".to_string(),
    };
    let declarations = discover_selected_root(root.path()).unwrap();
    let trust = plan_selected_root_trust(&declarations);
    let expected = expected_trust_policy(
        root.path(),
        &declarations,
        &trust.repositories[0],
        RepositoryFormat::Fedora,
    )
    .unwrap()
    .unwrap();
    manifest.repositories[0].trust = expected;
    let mut existing = Repository::new(
        "vendor".to_string(),
        "https://operator.example/repo".to_string(),
    );
    let existing_id = existing.insert(&conn).unwrap();
    let declaration_path = root.path().join("etc/yum.repos.d/vendor.repo");
    let before = fs::read(&declaration_path).unwrap();
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(preview.blockers.iter().any(|blocker| matches!(
        blocker,
        TakeoverBlocker::ExistingRepository { name, ownership }
            if name == "vendor" && ownership == "operator"
    )));
    assert!(apply_native_repository_takeover(
        &conn,
        root.path(),
        &manifest,
        &preview.sha256().unwrap(),
    )
    .is_err());
    assert_eq!(fs::read(declaration_path).unwrap(), before);
    let loaded = Repository::find_by_id(&conn, existing_id)
        .unwrap()
        .expect("operator repository remains");
    assert_eq!(loaded.name, existing.name);
    assert_eq!(loaded.url, existing.url);
    assert_eq!(loaded.managed_by, RepositoryOwnership::Operator);
    let takeover_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM native_repository_takeovers",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(takeover_count, 0);
}

#[test]
fn embedded_apt_key_material_becomes_exact_inline_trust_authority() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("etc/apt/sources.list.d")).unwrap();
    let (armor, fingerprint) = armored_certificate();
    let continuation = armor
        .lines()
        .map(|line| {
            if line.is_empty() {
                " .".to_string()
            } else {
                format!(" {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        root.path().join("etc/apt/sources.list.d/vendor.sources"),
        format!(
            "Types: deb\nURIs: https://apt.example\nSuites: stable\nComponents: main\nSigned-By:\n{continuation}\n"
        ),
    )
    .unwrap();
    let declarations = discover_selected_root(root.path()).unwrap();
    let trust = plan_selected_root_trust(&declarations);
    let policy = expected_trust_policy(
        root.path(),
        &declarations,
        &trust.repositories[0],
        RepositoryFormat::Debian,
    )
    .unwrap()
    .unwrap();
    let RepositoryTrustPolicy::Debian { release_keys } = policy else {
        unreachable!("APT evidence produces Debian trust")
    };
    assert_eq!(release_keys[0].fingerprint, fingerprint);
    assert!(
        release_keys[0]
            .url
            .starts_with("data:application/pgp-keys;base64,")
    );
    release_keys[0].validate().unwrap();
}

#[test]
fn apt_enrollment_covers_every_uri_suite_component_product_exactly_once() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("etc/apt/sources.list.d")).unwrap();
    let (armor, _) = armored_certificate();
    let continuation = armor
        .lines()
        .map(|line| {
            if line.is_empty() {
                " .".to_string()
            } else {
                format!(" {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        root.path().join("etc/apt/sources.list.d/vendor.sources"),
        format!(
            "Types: deb\nURIs: https://apt.example\nSuites: stable\nComponents: main updates\nSigned-By:\n{continuation}\n"
        ),
    )
    .unwrap();
    let db_path = root.path().join("conary.db");
    crate::db::init(&db_path).unwrap();
    let conn = crate::db::open(&db_path).unwrap();
    let declarations = discover_selected_root(root.path()).unwrap();
    let trust = plan_selected_root_trust(&declarations);
    let reference = trust.repositories[0].repository.clone();
    let policy = expected_trust_policy(
        root.path(),
        &declarations,
        &trust.repositories[0],
        RepositoryFormat::Debian,
    )
    .unwrap()
    .unwrap();
    let input = |name: &str, component: &str| TakeoverRepositoryInput {
        declaration: reference.clone(),
        name: name.to_string(),
        source_identity: "apt-vendor".to_string(),
        repository_identity: name.to_string(),
        scope: TakeoverPolicyScope::Group {
            identity: "apt-vendor".to_string(),
        },
        stream: TakeoverSourceStream::Release {
            identity: "stable".to_string(),
        },
        update: TakeoverUpdatePolicy::Follow,
        metadata_url: "https://apt.example/".to_string(),
        content_url: None,
        parser: RepositoryParserConfig::Deb {
            distribution: "stable".to_string(),
            component: component.to_string(),
            architecture: "amd64".to_string(),
        },
        trust: policy.clone(),
        enabled: true,
        priority: 40,
        metadata_expire: 3600,
        source_profile: None,
    };
    let mut manifest = NativeRepositoryTakeoverManifest {
        schema: TAKEOVER_MANIFEST_SCHEMA,
        repositories: vec![input("apt-main", "main")],
    };
    let incomplete = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(incomplete.blockers.iter().any(|blocker| matches!(
        blocker,
        TakeoverBlocker::MissingEnrollment { products, .. }
            if products == &vec![NativeEnrollmentProduct::Apt {
                uri: "https://apt.example".to_string(),
                suite: "stable".to_string(),
                component: Some("updates".to_string()),
            }]
    )));

    manifest.repositories.push(input("apt-updates", "updates"));
    let complete = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(complete.blockers.is_empty(), "{:?}", complete.blockers);
    let outcome = apply_native_repository_takeover(
        &conn,
        root.path(),
        &manifest,
        &complete.sha256().unwrap(),
    )
    .unwrap();
    assert_eq!(outcome.repository_ids.len(), 2);
}

#[test]
fn apply_repeat_drift_and_rollback_preserve_exact_authority() {
    let (root, conn, manifest) = fixture();
    let declaration_path = root.path().join("etc/yum.repos.d/vendor.repo");
    let original = fs::read(&declaration_path).unwrap();
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    let digest = preview.sha256().unwrap();
    let applied = apply_native_repository_takeover(&conn, root.path(), &manifest, &digest).unwrap();
    assert_eq!(applied.status, TakeoverApplyStatus::Applied);
    assert_eq!(
        Repository::find_by_name(&conn, "vendor")
            .unwrap()
            .unwrap()
            .managed_by,
        RepositoryOwnership::NativeProjection
    );
    assert!(
        crate::repository::set_repository_enabled(&conn, "vendor", false).is_err(),
        "native projection authority cannot drift through generic repository mutation"
    );
    let mut sync_update = Repository::find_by_name(&conn, "vendor").unwrap().unwrap();
    assert!(sync_update.enabled);
    sync_update.last_sync = Some("2026-08-11T00:00:00Z".to_string());
    sync_update.update(&conn).unwrap();

    let repeated =
        apply_native_repository_takeover(&conn, root.path(), &manifest, &digest).unwrap();
    assert_eq!(repeated.status, TakeoverApplyStatus::AlreadyApplied);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM repositories", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);

    fs::write(&declaration_path, b"drift\n").unwrap();
    let drifted = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(drifted.blockers.iter().any(|blocker| matches!(
        blocker,
        TakeoverBlocker::ProjectionDrift {
            path,
            expected_sha256,
            observed_sha256: Some(_),
        } if path == Path::new("/etc/yum.repos.d/vendor.repo")
            && expected_sha256 == &crate::hash::sha256(&original)
    )));
    assert!(rollback_native_repository_takeover(&conn, root.path()).is_err());
    assert!(Repository::find_by_name(&conn, "vendor").unwrap().is_some());

    fs::write(&declaration_path, &original).unwrap();
    fs::set_permissions(&declaration_path, fs::Permissions::from_mode(0o600)).unwrap();
    let mode_drifted = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    assert!(mode_drifted.blockers.iter().any(|blocker| matches!(
        blocker,
        TakeoverBlocker::ProjectionModeDrift {
            path,
            expected_mode: 0o644,
            observed_mode: Some(0o600),
        } if path == Path::new("/etc/yum.repos.d/vendor.repo")
    )));
    fs::set_permissions(&declaration_path, fs::Permissions::from_mode(0o644)).unwrap();
    rollback_native_repository_takeover(&conn, root.path()).unwrap();
    assert_eq!(fs::read(&declaration_path).unwrap(), original);
    assert!(Repository::find_by_name(&conn, "vendor").unwrap().is_none());
    let takeover_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM native_repository_takeovers",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(takeover_count, 0);
}

#[test]
fn staged_projection_failure_restores_files_and_rolls_back_database() {
    let (root, conn, manifest) = fixture();
    let declaration_path = root.path().join("etc/yum.repos.d/vendor.repo");
    let original = fs::read(&declaration_path).unwrap();
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    let error = apply_native_repository_takeover_with_persist(
        &conn,
        root.path(),
        &manifest,
        &preview.sha256().unwrap(),
        projection::persist_staged_after_one_for_test,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("forced projection persist failure")
    );
    assert_eq!(fs::read(declaration_path).unwrap(), original);
    assert!(Repository::find_by_name(&conn, "vendor").unwrap().is_none());
    let takeover_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM native_repository_takeovers",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(takeover_count, 0);
}

#[test]
fn database_failure_after_exchange_restores_guest_projection_identity_and_bytes() {
    let (root, conn, manifest) = fixture();
    let declaration_path = root.path().join("etc/yum.repos.d/vendor.repo");
    let original = fs::read(&declaration_path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER force_projection_insert_failure
         BEFORE INSERT ON native_repository_projections
         BEGIN
             SELECT RAISE(ABORT, 'forced projection database failure');
         END;",
    )
    .unwrap();
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    let error =
        apply_native_repository_takeover(&conn, root.path(), &manifest, &preview.sha256().unwrap())
            .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("forced projection database failure")
    );
    assert_eq!(fs::read(declaration_path).unwrap(), original);
    assert!(Repository::find_by_name(&conn, "vendor").unwrap().is_none());
    let takeover_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM native_repository_takeovers",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(takeover_count, 0);
}

#[test]
fn atomic_exchange_preserves_concurrent_native_drift_and_rolls_back_database() {
    let (root, conn, manifest) = fixture();
    let declaration_path = root.path().join("etc/yum.repos.d/vendor.repo");
    let preview = preview_native_repository_takeover(&conn, root.path(), &manifest).unwrap();
    let concurrent = b"[vendor]\nenabled=0\n";
    let error = apply_native_repository_takeover_with_persist(
        &conn,
        root.path(),
        &manifest,
        &preview.sha256().unwrap(),
        |staged| {
            fs::write(&declaration_path, concurrent)?;
            persist_staged(staged)
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("changed before atomic exchange"));
    assert_eq!(fs::read(&declaration_path).unwrap(), concurrent);
    assert!(Repository::find_by_name(&conn, "vendor").unwrap().is_none());
    let takeover_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM native_repository_takeovers",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(takeover_count, 0);
}
