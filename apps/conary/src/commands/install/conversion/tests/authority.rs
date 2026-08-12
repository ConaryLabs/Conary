// src/commands/install/conversion/tests/authority.rs

use super::*;

fn add_bootable_init(package: &mut FakeNativePackage) {
    let content = b"#!/bin/sh\nexec true\n".to_vec();
    let content_authority = PayloadContentAuthority {
        sha256: hash::sha256(&content),
        size: content.len() as u64,
    };
    package.files.push(PackageFile {
        path: "/usr/sbin/init".to_string(),
        node: test_regular_node(0o755),
        content: Some(content_authority.clone()),
    });
    package.extracted_files.push(ExtractedFile {
        path: "/usr/sbin/init".to_string(),
        node: test_regular_node(0o755),
        content,
        content_authority: Some(content_authority),
    });
}

#[tokio::test]
async fn local_conversion_key_is_distinct_from_persisted_native_repository_authority() {
    let _mount_guard = crate::commands::composefs_ops::test_mount_skip_guard();
    let temp_dir = tempfile::tempdir().unwrap();
    let install_root = temp_dir.path().join("root");
    let db_path = temp_dir.path().join("conary.db");
    let db_path_str = db_path.to_str().unwrap();
    let native_path = temp_dir.path().join("nginx.rpm");

    std::fs::create_dir_all(&install_root).unwrap();
    conary_core::db::init(db_path_str).unwrap();
    stage_test_boot_assets(temp_dir.path());
    rpm::PackageBuilder::new("nginx", "1.0.0", "MIT", "x86_64", "RPM fixture")
        .build()
        .unwrap()
        .write_file(&native_path)
        .unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let mut repository = Repository::new(
        "native-source".to_string(),
        "https://native.example.invalid/repo".to_string(),
    );
    let repository_id = repository.insert(&conn).unwrap();
    drop(conn);
    let provenance = RepositoryInstallProvenance {
        repository_id,
        source_identity: Some("fedora-44".to_string()),
        source_profile: Some("fedora-44".to_string()),
        version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
        source_kind: RepositorySourceKind::Native,
    };

    let mut package = FakeNativePackage::nginx();
    add_bootable_init(&mut package);
    let converted = try_convert_to_ccs(
        &package,
        &native_path,
        PackageFormatType::Rpm,
        db_path_str,
        &test_resolution_policy().with_primary_source_identity("fedora-44"),
        Some("fedora-44"),
    )
    .await
    .unwrap();
    let ConversionResult::Converted {
        ccs_path,
        temp_dir: _conversion_dir,
        pending_record,
        signing_public_key,
    } = converted
    else {
        panic!("native conversion unexpectedly skipped");
    };

    let wrong_key = SigningKeyPair::generate().public_key_base64();
    let error = verify_ccs_package_authority(
        db_path_str,
        Path::new(&ccs_path),
        &CcsEnvelopeAuthority::ExactKey(wrong_key),
        Some(&provenance),
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("package signer is not trusted"),
        "wrong exact conversion key must fail even with repository provenance: {error:#}"
    );

    let trove_id = install_ccs_artifact(CcsArtifactInstallOptions {
        ccs_path: &ccs_path,
        db_path: db_path_str,
        root: install_root.to_str().unwrap(),
        dry_run: false,
        sandbox_mode: SandboxMode::Always,
        no_deps: true,
        allow_downgrade: false,
        intent: InstallIntent::PackageChange,
        yes: true,
        envelope_authority: CcsEnvelopeAuthority::ExactKey(signing_public_key),
        repository_provenance: Some(provenance),
        resolution_policy: test_resolution_policy(),
    })
    .await
    .unwrap()
    .expect("converted package install must persist a trove");
    pending_record.persist(db_path_str, trove_id).unwrap();

    let conn = conary_core::db::open(db_path_str).unwrap();
    let installed = Trove::find_by_id(&conn, trove_id).unwrap().unwrap();
    assert_eq!(installed.installed_from_repository_id, Some(repository_id));
    assert_eq!(installed.source_profile.as_deref(), Some("fedora-44"));
    assert_eq!(
        installed.version_scheme,
        conary_core::repository::versioning::VersionScheme::Rpm
    );
}
