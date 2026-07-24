// apps/remi/src/server/handlers/packages/tests.rs
use super::*;
use crate::server::native_publish::test_support::seed_native_publication;
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{CONVERSION_VERSION, ConvertedPackage};

fn create_test_db() -> (tempfile::NamedTempFile, rusqlite::Connection) {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
    (temp_file, conn)
}

#[test]
fn native_manifest_lookup_prefers_active_native_publication() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(
        &conn,
        "fedora",
        "hello",
        "1.0.0",
        "1",
        "noarch",
        "/tmp/hello.ccs",
    );

    let manifest = native_manifest_for_package(
        temp_file.path(),
        "fedora",
        "hello",
        Some("1.0.0"),
        Some("1"),
        None,
    )
    .unwrap()
    .expect("native manifest");

    assert_eq!(manifest.name, "hello");
    assert_eq!(manifest.version, "1.0.0");
    assert_eq!(manifest.release.as_deref(), Some("1"));
    assert!(manifest.native);
    assert!(!manifest.converted);
}

#[test]
fn native_manifest_lookup_reports_ambiguous_releases() {
    let (temp_file, conn) = create_test_db();
    seed_native_publication(
        &conn,
        "fedora",
        "hello",
        "1.0.0",
        "1",
        "noarch",
        "/tmp/hello-1.ccs",
    );
    seed_native_publication(
        &conn,
        "fedora",
        "hello",
        "1.0.0",
        "2",
        "noarch",
        "/tmp/hello-2.ccs",
    );

    let error = native_manifest_for_package(
        temp_file.path(),
        "fedora",
        "hello",
        Some("1.0.0"),
        None,
        None,
    )
    .unwrap_err();

    assert!(error.to_string().contains("multiple native releases"));
}

#[test]
fn package_publication_manifest_includes_scriptlets_without_private_path() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    conary_core::db::init(&db_path).unwrap();
    let ccs_path = temp.path().join("cache/packages/pkg-1.0-x86_64.ccs");
    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::write(&ccs_path, b"ccs").unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &["sha256:chunk".to_string()],
        3,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
    );
    converted.package_architecture = Some("x86_64".to_string());
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "native-free".to_string(),
        target_compatibility: "compatible".to_string(),
        publication_status: "public".to_string(),
        review_artifact_path: Some("/tmp/private-review-secret".to_string()),
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    converted.insert(&conn).unwrap();

    let manifest =
        match check_converted(&db_path, "fedora", "pkg", Some("1.0"), Some("x86_64")).unwrap() {
            ConvertedManifestLookup::Ready(manifest) => manifest,
            _ => panic!("public converted row should return a manifest"),
        };
    let json = serde_json::to_string(&manifest).unwrap();

    let scriptlets = manifest.scriptlets.as_ref().unwrap();
    assert_eq!(scriptlets.scriptlet_fidelity, "native-free");
    assert!(scriptlets.review_artifact_available);
    assert!(!json.contains("review_artifact_path"));
    assert!(!json.contains("private-review-secret"));
}

#[test]
fn check_converted_returns_review_refusal_for_current_private_row() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
    let ccs_path = temp.path().join("pkg.ccs");
    std::fs::write(&ccs_path, b"fake ccs").unwrap();

    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &["abc".to_string()],
        8,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
    );
    converted
        .set_scriptlet_metadata(&ScriptletBundleSummary {
            publication_status: "private-review".to_string(),
            scriptlet_fidelity: "review-required".to_string(),
            target_compatibility: "review-required".to_string(),
            review_reason_codes: vec!["review-class-debconf".to_string()],
            ..Default::default()
        })
        .unwrap();
    converted.insert(&conn).unwrap();

    let lookup = check_converted(&db_path, "fedora", "pkg", Some("1.0"), None).unwrap();

    assert!(matches!(lookup, ConvertedManifestLookup::ReviewRequired(_)));
}

#[test]
fn converted_download_lookup_refuses_blocked_rows() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("test.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conary_core::db::schema::migrate(&conn).unwrap();
    let ccs_path = temp.path().join("pkg.ccs");
    std::fs::write(&ccs_path, b"fake ccs").unwrap();

    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "pkg".to_string(),
        "1.0".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &["abc".to_string()],
        8,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
    );
    converted
        .set_scriptlet_metadata(&ScriptletBundleSummary {
            publication_status: "blocked".to_string(),
            scriptlet_fidelity: "blocked".to_string(),
            target_compatibility: "blocked".to_string(),
            blocked_reason_codes: vec!["blocked-class-network".to_string()],
            ..Default::default()
        })
        .unwrap();
    converted.insert(&conn).unwrap();

    let lookup =
        converted_ccs_path_for_download(&db_path, "fedora", "pkg", Some("1.0"), None).unwrap();

    assert!(matches!(lookup, ConvertedDownloadLookup::Blocked(_)));
}

#[test]
fn converted_ccs_path_for_download_rejects_stale_conversion_records() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("conary.db");
    conary_core::db::init(&db_path).unwrap();

    let ccs_path = temp
        .path()
        .join("cache/packages/p11-kit-trust-0.25.8-1.fc44-x86_64.ccs");
    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::write(&ccs_path, b"stale ccs payload").unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let mut converted = ConvertedPackage::new_server(
        "fedora".to_string(),
        "p11-kit-trust".to_string(),
        "0.25.8-1.fc44".to_string(),
        "rpm".to_string(),
        "sha256:stale".to_string(),
        &[],
        17,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
    );
    converted.package_architecture = Some("x86_64".to_string());
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.insert(&conn).unwrap();

    let resolved = converted_ccs_path_for_download(
        &db_path,
        "fedora",
        "p11-kit-trust",
        Some("0.25.8-1.fc44"),
        Some("x86_64"),
    )
    .unwrap();

    assert!(matches!(resolved, ConvertedDownloadLookup::Missing));
}
