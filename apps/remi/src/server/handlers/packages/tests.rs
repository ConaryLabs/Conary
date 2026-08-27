// apps/remi/src/server/handlers/packages/tests.rs
use super::*;
use crate::server::catalog_authority::test_support::{
    ActiveCatalogFixture, package as catalog_package,
};
use crate::server::conversion::test_support::seed_repository_conversion_source;
use crate::server::native_publish::test_support::seed_native_publication;
use base64::Engine as _;
use conary_core::ccs::convert::ScriptletBundleSummary;
use conary_core::db::models::{
    CONVERSION_VERSION, ChunkAccess, ConvertedPackage, NativeSourceEcosystem, NativeSourceStream,
    Repository, RepositoryPackage, RepositoryPolicyScope, RepositorySourcePolicy,
    RepositoryUpdateMode,
};
use conary_core::repository::trust::openpgp::PreparedOpenPgpTrust;
use conary_core::repository::versioning::VersionScheme;
use conary_core::repository::{OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy};
use sequoia_openpgp::cert::prelude::CertBuilder;
use sequoia_openpgp::serialize::Serialize as _;
use sha2::{Digest, Sha256};
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use tokio::sync::RwLock;

fn create_test_db() -> (tempfile::NamedTempFile, rusqlite::Connection) {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let conn = rusqlite::Connection::open(temp_file.path()).unwrap();
    conary_core::db::schema::ensure_current(&conn).unwrap();
    (temp_file, conn)
}

fn create_test_state(
    root: &tempfile::TempDir,
    catalogs: &ActiveCatalogFixture,
) -> Arc<RwLock<ServerState>> {
    let db_path = catalogs.db_path().to_path_buf();
    let chunk_dir = root.path().join("chunks");
    let cache_dir = root.path().join("cache");
    std::fs::create_dir_all(&chunk_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();

    let repository_keys_dir = root.path().join("repository-keys");
    let profile_keys_dir = repository_keys_dir.join("ubuntu-26.04");
    std::fs::create_dir_all(&profile_keys_dir).unwrap();
    std::fs::set_permissions(&repository_keys_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&profile_keys_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let private_key = profile_keys_dir.join("targets.private");
    let public_key = profile_keys_dir.join("targets.public");
    conary_core::ccs::SigningKeyPair::generate()
        .with_key_id("targets")
        .save_to_files(&private_key, &public_key)
        .unwrap();
    std::fs::set_permissions(&private_key, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::set_permissions(&public_key, std::fs::Permissions::from_mode(0o644)).unwrap();

    let mut state = ServerState::new(crate::server::ServerConfig {
        db_path,
        chunk_dir,
        cache_dir,
        catalog_dir: catalogs.catalog_dir().to_path_buf(),
        max_concurrent_conversions: 1,
        enable_rate_limit: false,
        enable_audit_log: false,
        enable_bloom_filter: false,
        release_publish: crate::server::config::ReleasePublishSection {
            repository_keys_dir: Some(repository_keys_dir),
            trusted_build_attestation_signers: Vec::new(),
        },
        ..crate::server::ServerConfig::default()
    })
    .unwrap();
    state.publication_readiness = crate::server::readiness::PublicationReadiness {
        repository: crate::server::readiness::PublicationPhaseState::Complete,
        canonical: crate::server::readiness::PublicationPhaseState::Complete,
    };
    state.conversion_service = state
        .conversion_service
        .clone()
        .with_catalog_authority(catalogs.authority().clone());
    Arc::new(RwLock::new(state))
}

fn selection(profile: &str, profile_revision_sha256: &str) -> ProfileRevisionSelection {
    ProfileRevisionSelection {
        source_profile: profile.to_string(),
        profile_revision_sha256: profile_revision_sha256.to_string(),
    }
}

async fn install_catalog_authority(
    state: &Arc<RwLock<ServerState>>,
    authority: &crate::server::catalog_authority::CatalogAuthority,
) {
    let mut state = state.write().await;
    state.catalog_authority = authority.clone();
    state.conversion_service = state
        .conversion_service
        .clone()
        .with_catalog_authority(authority.clone());
}

async fn response_json(response: Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn seed_current_conversion(
    db_path: &std::path::Path,
    ccs_path: &std::path::Path,
    name: &str,
    version: &str,
    profile_revision_sha256: &str,
) {
    let converted =
        seed_repository_source(db_path, ccs_path, name, version, profile_revision_sha256);
    publish_current_conversion(db_path, ccs_path, converted);
}

fn seed_repository_source(
    db_path: &std::path::Path,
    ccs_path: &std::path::Path,
    name: &str,
    version: &str,
    profile_revision_sha256: &str,
) -> ConvertedPackage {
    let conn = conary_core::db::open(db_path).unwrap();
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        profile_revision_sha256.to_string(),
        name.to_string(),
        version.to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &transport,
        3,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    seed_repository_conversion_source(&conn, &mut converted);
    converted
}

fn publish_current_conversion(
    db_path: &std::path::Path,
    ccs_path: &std::path::Path,
    mut converted: ConvertedPackage,
) {
    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::write(ccs_path, b"ccs").unwrap();
    let conn = conary_core::db::open(db_path).unwrap();
    converted.insert_with_conversion_pin(&conn, 1).unwrap();
}

async fn poll_job(state: &Arc<RwLock<ServerState>>, job_id: JobId) -> serde_json::Value {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let response = crate::server::handlers::jobs::get_job_status(
                State(Arc::clone(state)),
                Path(job_id.to_string()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = response_json(response).await;
            if matches!(
                body["status"].as_str(),
                Some("ready" | "failed" | "cancelled")
            ) {
                return body;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("conversion job should reach a terminal state")
}

fn append_tar_file<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    path: &str,
    mode: u32,
    body: &[u8],
) {
    let mut header = tar::Header::new_gnu();
    header.set_path(path).unwrap();
    header.set_size(body.len() as u64);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    archive.append(&header, Cursor::new(body)).unwrap();
}

fn gzip_tar(path: &str, mode: u32, body: &[u8]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    append_tar_file(&mut archive, path, mode, body);
    archive
        .into_inner()
        .unwrap()
        .finish()
        .expect("finish test tarball")
}

fn debian_fixture(root: &std::path::Path) -> std::path::PathBuf {
    let components = root.join("deb-components");
    std::fs::create_dir_all(&components).unwrap();
    let binary = components.join("debian-binary");
    let control = components.join("control.tar.gz");
    let data = components.join("data.tar.gz");
    std::fs::write(&binary, b"2.0\n").unwrap();
    std::fs::write(
        &control,
        gzip_tar(
            "control",
            0o644,
            b"Package: demo\nVersion: 1.0\nArchitecture: amd64\nMaintainer: Test <test@example.invalid>\nDescription: retry fixture\n",
        ),
    )
    .unwrap();
    std::fs::write(&data, gzip_tar("usr/bin/demo", 0o755, b"hello")).unwrap();

    let package = root.join("demo_1.0_amd64.deb");
    let mut archive = ar::Builder::new(std::fs::File::create(&package).unwrap());
    archive
        .append_file(b"debian-binary", &mut std::fs::File::open(binary).unwrap())
        .unwrap();
    archive
        .append_file(
            b"control.tar.gz",
            &mut std::fs::File::open(control).unwrap(),
        )
        .unwrap();
    archive
        .append_file(b"data.tar.gz", &mut std::fs::File::open(data).unwrap())
        .unwrap();
    package
}

async fn serve_package_once(package: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            package.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&package).await.unwrap();
    });
    format!("http://{address}/demo_1.0_amd64.deb")
}

async fn populate_debian_repository(db_path: &std::path::Path, package_url: String, body: &[u8]) {
    let (certificate, _) = CertBuilder::new()
        .add_userid("remi-test@example.invalid")
        .generate()
        .unwrap();
    let mut certificate_bytes = Vec::new();
    certificate.serialize(&mut certificate_bytes).unwrap();
    let policy = RepositoryTrustPolicy::Debian {
        release_keys: vec![
            OpenPgpTrustRoot::new(
                format!(
                    "data:application/pgp-keys;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(certificate_bytes)
                ),
                certificate.fingerprint().to_hex(),
            )
            .unwrap(),
        ],
    };
    let source_repository_name = "conversion-fixture-ubuntu-26.04-source";
    let key_material_identity = "ubuntu-resolute-security-main-amd64";
    let key_material_repository_name = {
        let conn = conary_core::db::open(db_path).unwrap();
        conn.query_row(
            "SELECT name FROM repositories
             WHERE source_profile = 'ubuntu-26.04' AND repository_identity = ?1",
            rusqlite::params![key_material_identity],
            |row| row.get::<_, String>(0),
        )
        .expect("fixture has one exact operational repository for prepared key material")
    };
    PreparedOpenPgpTrust::prepare(
        &key_material_repository_name,
        &conary_core::db::paths::keyring_dir(&db_path.display().to_string()),
        &policy,
    )
    .await
    .unwrap();

    let conn = conary_core::db::open(db_path).unwrap();
    let parser_config = RepositoryParserConfig::Deb {
        distribution: "noble".to_string(),
        component: "main".to_string(),
        architecture: "amd64".to_string(),
    };
    let mut repository = Repository::new(source_repository_name.to_string(), package_url.clone());
    repository.source_profile = Some("ubuntu-26.04".to_string());
    repository.set_parser_config(parser_config.clone()).unwrap();
    repository.set_trust_policy(policy.clone()).unwrap();
    let repository_identity = "noble:main:amd64";
    repository
        .set_native_source_policy(
            RepositorySourcePolicy::new(
                "ubuntu-26.04-test",
                RepositoryPolicyScope::repository(repository_identity).unwrap(),
                NativeSourceEcosystem::Deb,
                NativeSourceStream::release("noble").unwrap(),
                RepositoryUpdateMode::Follow,
            )
            .unwrap(),
            repository_identity,
            None,
        )
        .unwrap();
    let repository_id = repository.insert(&conn).unwrap();
    let checksum = format!("sha256:{}", hex::encode(Sha256::digest(body)));
    let mut package = RepositoryPackage::new(
        repository_id,
        "demo".to_string(),
        "1.0".to_string(),
        VersionScheme::Debian,
        checksum,
        body.len() as i64,
        package_url.clone(),
    );
    package.architecture = Some("amd64".to_string());
    package.source_profile = Some("ubuntu-26.04".to_string());
    package.insert(&conn).unwrap();
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
fn converted_manifest_includes_typed_lifecycle_summary() {
    let catalogs = ActiveCatalogFixture::new();
    let profile_revision_sha256 = catalogs.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "pkg",
            "1.0",
            "1",
            Some("x86_64"),
            3,
            "pkg-source",
        )],
    );
    let db_path = catalogs.db_path().to_path_buf();
    let ccs_path = db_path.with_file_name("pkg-1.0-x86_64.ccs");
    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::write(&ccs_path, b"ccs").unwrap();

    let conn = catalogs.connection();
    let mut transport =
        crate::server::conversion::test_support::test_transport(&["sha256:chunk".to_string()]);
    transport.objects[0].size = 3;
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        profile_revision_sha256.clone(),
        "pkg".to_string(),
        "1.0".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &transport,
        3,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    let summary = ScriptletBundleSummary {
        scriptlet_fidelity: "native-free".to_string(),
        ..ScriptletBundleSummary::default()
    };
    converted.set_scriptlet_metadata(&summary).unwrap();
    seed_repository_conversion_source(&conn, &mut converted);
    converted.insert_with_conversion_pin(&conn, 1).unwrap();
    ChunkAccess::new("sha256:chunk".to_string(), 3)
        .upsert(&conn)
        .unwrap();

    let manifest = match check_converted(
        &db_path,
        &profile_revision_sha256,
        "fedora",
        "pkg",
        Some("1.0"),
        Some("x86_64"),
    )
    .unwrap()
    {
        ConvertedManifestLookup::Ready(manifest) => manifest,
        _ => panic!("public converted row should return a manifest"),
    };
    let json = serde_json::to_string(&manifest).unwrap();

    let scriptlets = manifest.scriptlets.as_ref().unwrap();
    assert_eq!(scriptlets.scriptlet_fidelity, "native-free");
    assert_eq!(manifest.transport.objects[0].size, 3);
    assert_eq!(manifest.transport.objects[0].sha256, "sha256:chunk");
    assert!(!json.contains("publication_status"));
}

#[test]
fn malformed_current_conversion_metadata_is_an_internal_data_error() {
    let catalogs = ActiveCatalogFixture::new();
    let profile_revision_sha256 = catalogs.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "pkg",
            "1.0",
            "1",
            Some("x86_64"),
            8,
            "pkg-source",
        )],
    );
    let db_path = catalogs.db_path().to_path_buf();
    let conn = catalogs.connection();
    let ccs_path = db_path.with_file_name("pkg.ccs");
    std::fs::write(&ccs_path, b"fake ccs").unwrap();

    let transport = crate::server::conversion::test_support::test_transport(&["abc".to_string()]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        profile_revision_sha256.clone(),
        "pkg".to_string(),
        "1.0".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:source".to_string(),
        &transport,
        8,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    converted
        .set_scriptlet_metadata(&ScriptletBundleSummary::default())
        .unwrap();
    seed_repository_conversion_source(&conn, &mut converted);
    converted.insert_with_conversion_pin(&conn, 1).unwrap();
    conn.execute(
        "UPDATE converted_packages SET scriptlet_summary_json = '{broken' WHERE original_checksum = ?1",
        ["sha256:source"],
    )
    .unwrap();

    assert!(
        check_converted(
            &db_path,
            &profile_revision_sha256,
            "fedora",
            "pkg",
            Some("1.0"),
            None,
        )
        .is_err()
    );
    assert!(
        converted_ccs_path_for_download(
            &db_path,
            &profile_revision_sha256,
            "pkg",
            Some("1.0"),
            None,
        )
        .is_err()
    );
}

#[test]
fn converted_ccs_path_for_download_rejects_stale_conversion_records() {
    let catalogs = ActiveCatalogFixture::new();
    let profile_revision_sha256 = catalogs.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "p11-kit-trust",
            "0.25.8-1.fc44",
            "1",
            Some("x86_64"),
            17,
            "stale-source",
        )],
    );
    let db_path = catalogs.db_path().to_path_buf();

    let ccs_path = db_path.with_file_name("p11-kit-trust-0.25.8-1.fc44-x86_64.ccs");
    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::write(&ccs_path, b"stale ccs payload").unwrap();

    let conn = catalogs.connection();
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        profile_revision_sha256.clone(),
        "p11-kit-trust".to_string(),
        "0.25.8-1.fc44".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        "sha256:stale".to_string(),
        &transport,
        17,
        "sha256:content".to_string(),
        ccs_path.to_string_lossy().to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    converted.conversion_version = CONVERSION_VERSION - 1;
    converted.insert(&conn).unwrap();

    let resolved = converted_ccs_path_for_download(
        &db_path,
        &profile_revision_sha256,
        "p11-kit-trust",
        Some("0.25.8-1.fc44"),
        Some("x86_64"),
    )
    .unwrap();

    assert!(matches!(resolved, ConvertedDownloadLookup::Missing));
}

#[tokio::test]
async fn existing_pending_job_response_reports_pending() {
    let root = tempfile::tempdir().unwrap();
    let catalogs = ActiveCatalogFixture::new();
    let revision = catalogs.activate("fedora-44", 1, Vec::new());
    catalogs.activate_universe(1);
    let state = create_test_state(&root, &catalogs);
    let query = PackageQuery {
        version: None,
        release: None,
        arch: None,
    };
    let job_key = conversion_job_key("fedora", "curl", &query, &revision);
    let job_id = state
        .write()
        .await
        .job_manager
        .create_job(job_key, "fedora".into(), "curl".into(), None, None)
        .unwrap();

    let response = get_package(
        State(Arc::clone(&state)),
        Path(("fedora".into(), "curl".into())),
        Query(query),
    )
    .await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = response_json(response).await;
    assert_eq!(body["status"], "pending");
    assert_eq!(body["job_id"], job_id.to_string());
}

#[tokio::test]
async fn failed_job_stays_pollable_while_unpopulated_repository_returns_typed_503() {
    let root = tempfile::tempdir().unwrap();
    let catalogs = ActiveCatalogFixture::new();
    let revision = catalogs.activate("fedora-44", 1, Vec::new());
    catalogs.activate_universe(1);
    let state = create_test_state(&root, &catalogs);
    install_catalog_authority(&state, catalogs.authority()).await;
    let query = PackageQuery {
        version: None,
        release: None,
        arch: None,
    };
    let job_key = conversion_job_key("fedora", "curl", &query, &revision);
    let failed_id = {
        let mut state = state.write().await;
        let id = state
            .job_manager
            .create_job(job_key, "fedora".into(), "curl".into(), None, None)
            .unwrap();
        state
            .job_manager
            .update_status(&id, JobStatus::Failed("repository not ready".into()));
        id
    };

    let response = get_package(
        State(Arc::clone(&state)),
        Path(("fedora".into(), "curl".into())),
        Query(query),
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(response).await;
    assert_eq!(body["code"], "REPOSITORY_NOT_READY");
    assert_eq!(body["profile"], "fedora-44");
    assert_eq!(body["reason"], "profile_empty");
    assert!(matches!(
        state
            .read()
            .await
            .job_manager
            .get_job(&failed_id)
            .map(|job| &job.status),
        Some(JobStatus::Failed(error)) if error == "repository not ready"
    ));
}

#[tokio::test]
async fn ready_job_after_initial_cache_miss_is_revalidated_before_new_work() {
    let root = tempfile::tempdir().unwrap();
    let catalogs = ActiveCatalogFixture::new();
    let profile_revision_sha256 = catalogs.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "curl",
            "1.0",
            "1",
            Some("x86_64"),
            3,
            "curl-source",
        )],
    );
    let state = create_test_state(&root, &catalogs);
    install_catalog_authority(&state, catalogs.authority()).await;
    let query = PackageQuery {
        version: Some("1.0".into()),
        release: None,
        arch: Some("x86_64".into()),
    };
    let job_key = conversion_job_key("fedora", "curl", &query, &profile_revision_sha256);
    let ready_id = {
        let mut state = state.write().await;
        let id = state
            .job_manager
            .create_job(
                job_key.clone(),
                "fedora".into(),
                "curl".into(),
                query.version.clone(),
                query.arch.clone(),
            )
            .unwrap();
        state.job_manager.update_status(&id, JobStatus::Ready);
        id
    };
    let db_path = catalogs.db_path().to_path_buf();
    seed_current_conversion(
        &db_path,
        &db_path.with_file_name("curl.ccs"),
        "curl",
        "1.0",
        &profile_revision_sha256,
    );

    // Enter after the request's first cache lookup to reproduce a conversion
    // becoming ready in the gap before job-key coordination.
    let response = start_conversion_after_cache_miss(
        Arc::clone(&state),
        &db_path,
        "fedora",
        "curl",
        &query,
        &selection("fedora-44", &profile_revision_sha256),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["name"], "curl");
    assert_eq!(
        state
            .read()
            .await
            .job_manager
            .get_job_by_key(&job_key)
            .unwrap()
            .id,
        ready_id,
        "the committed ready job must not be replaced by duplicate work"
    );
}

#[tokio::test]
async fn missing_ready_artifact_creates_one_replacement_and_deduplicates_it() {
    let root = tempfile::tempdir().unwrap();
    let catalogs = ActiveCatalogFixture::new();
    let profile_revision_sha256 = catalogs.activate("fedora-44", 1, Vec::new());
    let state = create_test_state(&root, &catalogs);
    install_catalog_authority(&state, catalogs.authority()).await;
    let query = PackageQuery {
        version: Some("1.0".into()),
        release: None,
        arch: Some("x86_64".into()),
    };
    let job_key = conversion_job_key("fedora", "curl", &query, &profile_revision_sha256);
    let db_path = catalogs.db_path().to_path_buf();
    let ready_id = {
        let mut state = state.write().await;
        let id = state
            .job_manager
            .create_job(
                job_key,
                "fedora".into(),
                "curl".into(),
                query.version.clone(),
                query.arch.clone(),
            )
            .unwrap();
        state.job_manager.update_status(&id, JobStatus::Ready);
        id
    };
    let semaphore = state.read().await.job_manager.semaphore();
    let permit = semaphore.acquire_owned().await.unwrap();

    let first = start_conversion_after_cache_miss(
        Arc::clone(&state),
        &db_path,
        "fedora",
        "curl",
        &query,
        &selection("fedora-44", &profile_revision_sha256),
    )
    .await;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let replacement_id: JobId = response_json(first).await["job_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(replacement_id, ready_id);

    let second = start_conversion_after_cache_miss(
        Arc::clone(&state),
        &db_path,
        "fedora",
        "curl",
        &query,
        &selection("fedora-44", &profile_revision_sha256),
    )
    .await;
    assert_eq!(second.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(second).await["job_id"],
        replacement_id.to_string()
    );

    drop(permit);
    assert_eq!(poll_job(&state, replacement_id).await["status"], "failed");
}

#[tokio::test]
async fn collected_ready_mapping_revalidates_persisted_artifact_before_new_work() {
    let root = tempfile::tempdir().unwrap();
    let catalogs = ActiveCatalogFixture::new();
    let profile_revision_sha256 = catalogs.activate(
        "fedora-44",
        1,
        vec![catalog_package(
            "fedora-44",
            "curl",
            "1.0",
            "1",
            Some("x86_64"),
            3,
            "curl-source",
        )],
    );
    let state = create_test_state(&root, &catalogs);
    install_catalog_authority(&state, catalogs.authority()).await;
    let query = PackageQuery {
        version: Some("1.0".into()),
        release: None,
        arch: Some("x86_64".into()),
    };
    let db_path = catalogs.db_path().to_path_buf();
    seed_current_conversion(
        &db_path,
        &db_path.with_file_name("curl.ccs"),
        "curl",
        "1.0",
        &profile_revision_sha256,
    );

    // Enter after an earlier cache miss with no retained key mapping, as if a
    // just-completed ready polling record was evicted for capacity.
    let response = start_conversion_after_cache_miss(
        Arc::clone(&state),
        &db_path,
        "fedora",
        "curl",
        &query,
        &selection("fedora-44", &profile_revision_sha256),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["name"], "curl");
    assert!(
        state
            .read()
            .await
            .job_manager
            .get_job_by_key(&conversion_job_key(
                "fedora",
                "curl",
                &query,
                &profile_revision_sha256,
            ))
            .is_none()
    );
}

#[tokio::test]
async fn package_route_waits_for_repository_population_without_creating_failed_job() {
    let root = tempfile::tempdir().unwrap();
    let catalogs = ActiveCatalogFixture::new();
    let empty_revision = catalogs.activate("ubuntu-26.04", 1, Vec::new());
    catalogs.activate_universe(1);
    let state = create_test_state(&root, &catalogs);
    install_catalog_authority(&state, catalogs.authority()).await;
    let query = || PackageQuery {
        version: Some("1.0".into()),
        release: None,
        arch: Some("amd64".into()),
    };

    let coordinator = state.read().await.publication_coordinator.clone();
    let held_publication = coordinator.lock_owned().await;
    let first_state = Arc::clone(&state);
    let mut first = tokio::spawn(async move {
        get_package(
            State(first_state),
            Path(("ubuntu".into(), "demo".into())),
            Query(query()),
        )
        .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut first)
            .await
            .is_err(),
        "package admission bypassed the publication coordinator"
    );
    drop(held_publication);
    let first = tokio::time::timeout(std::time::Duration::from_secs(2), first)
        .await
        .expect("package admission resumed after publication released")
        .expect("package request completed");
    assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(first.headers().get(header::RETRY_AFTER).unwrap(), "30");
    let first_body = response_json(first).await;
    assert_eq!(first_body["code"], "REPOSITORY_NOT_READY");
    assert_eq!(first_body["profile"], "ubuntu-26.04");
    assert_eq!(first_body["reason"], "profile_empty");
    assert!(
        state
            .read()
            .await
            .job_manager
            .get_job_by_key(&conversion_job_key(
                "ubuntu",
                "demo",
                &query(),
                &empty_revision,
            ))
            .is_none(),
        "a pre-ready request must not create a doomed conversion job"
    );

    let db_path = state.read().await.config.db_path.clone();
    let fixture = debian_fixture(root.path());
    let package = std::fs::read(fixture).unwrap();
    let package_url = serve_package_once(package.clone()).await;
    populate_debian_repository(&db_path, package_url.clone(), &package).await;
    let mut catalog_entry = catalog_package(
        "ubuntu-26.04",
        "demo",
        "1.0",
        "1",
        Some("amd64"),
        u64::try_from(package.len()).unwrap(),
        "ubuntu-demo",
    );
    catalog_entry.download_url = package_url.clone();
    catalog_entry.checksum = format!("sha256:{}", hex::encode(Sha256::digest(&package)));
    let package_checksum = catalog_entry.checksum.clone();
    let profile_revision_sha256 = catalogs.activate("ubuntu-26.04", 2, vec![catalog_entry]);
    catalogs.activate_universe(2);
    let semaphore = state.read().await.job_manager.semaphore();
    let permit = semaphore.acquire_owned().await.unwrap();

    let retry = get_package(
        State(Arc::clone(&state)),
        Path(("ubuntu".into(), "demo".into())),
        Query(query()),
    )
    .await;
    assert_eq!(retry.status(), StatusCode::ACCEPTED);
    let retry_body = response_json(retry).await;
    assert_eq!(retry_body["status"], "pending");
    let retry_id: JobId = retry_body["job_id"].as_str().unwrap().parse().unwrap();

    // Let the request reserve a job before installing a cache hit. The
    // semaphore keeps the worker pending while this exact-revision evidence
    // is prepared, so the test still exercises the post-admission job path.
    let state_conn = conary_core::db::open(&db_path).unwrap();
    let ccs_path = root.path().join("cache/packages/demo.ccs");
    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::write(&ccs_path, b"ccs").unwrap();
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = ConvertedPackage::new_repository(
        "ubuntu-26.04".to_string(),
        profile_revision_sha256,
        "demo".to_string(),
        "1.0".to_string(),
        "amd64".to_string(),
        "deb".to_string(),
        package_checksum,
        &transport,
        3,
        "sha256:converted-content".to_string(),
        ccs_path.to_string_lossy().to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    converted
        .insert_with_conversion_pin(&state_conn, 2)
        .unwrap();

    drop(permit);

    let ready = poll_job(&state, retry_id).await;
    assert_eq!(ready["status"], "ready", "{ready}");
    assert!(ready["manifest"].is_object());
    assert_eq!(ready["manifest"]["name"], "demo");
    assert_eq!(ready["manifest"]["version"], "1.0");
}
