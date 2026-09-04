// apps/remi/src/server/routes/tests.rs

use super::*;
use axum::body::Body;
use std::net::Ipv4Addr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

#[test]
fn test_cors_layer_restricted_no_origins() {
    let config = ServerConfig::default();
    // Default config has empty cors_allowed_origins
    let _cors = create_cors_layer(&config, true);
    // Just verify it doesn't panic
}

#[test]
fn test_cors_layer_restricted_with_origins() {
    let config = ServerConfig {
        cors_allowed_origins: vec!["https://example.com".to_string()],
        ..ServerConfig::default()
    };
    let _cors = create_cors_layer(&config, true);
    // Just verify it doesn't panic
}

#[test]
fn test_cors_layer_public() {
    let config = ServerConfig::default();
    let _cors = create_cors_layer(&config, false);
    // Public CORS should be permissive
}

#[test]
fn request_body_limit_is_16_mib() {
    assert_eq!(request_body_limit_bytes(), 16 * 1024 * 1024);
}

#[test]
fn test_is_cloudflare_ip() {
    // Known Cloudflare IPs
    assert!(is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(104, 16, 0, 1))));
    assert!(is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(172, 64, 0, 1))));
    assert!(is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(162, 158, 0, 1))));

    // Non-Cloudflare IPs
    assert!(!is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(
        192, 168, 1, 1
    ))));
    assert!(!is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    assert!(!is_cloudflare_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
}

#[test]
fn test_parse_cidr() {
    let (network, prefix) = parse_cidr("192.168.1.0/24").unwrap();
    assert_eq!(prefix, 24);
    assert_eq!(network, u32::from(Ipv4Addr::new(192, 168, 1, 0)));

    let (network, prefix) = parse_cidr("10.0.0.0/8").unwrap();
    assert_eq!(prefix, 8);
    assert_eq!(network, u32::from(Ipv4Addr::new(10, 0, 0, 0)));
}

#[test]
fn test_extract_client_ip_direct() {
    let headers = HeaderMap::new();
    let conn_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));

    let result = extract_client_ip(&headers, &conn_ip, None);
    assert_eq!(result, conn_ip);
}

#[test]
fn test_extract_client_ip_cf_header() {
    let mut headers = HeaderMap::new();
    headers.insert("CF-Connecting-IP", "203.0.113.50".parse().unwrap());

    // From Cloudflare IP
    let cf_ip = IpAddr::V4(Ipv4Addr::new(104, 16, 0, 1));
    let result = extract_client_ip(&headers, &cf_ip, None);
    assert_eq!(result, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50)));

    // From non-Cloudflare IP (should ignore CF header)
    let non_cf_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
    let result = extract_client_ip(&headers, &non_cf_ip, None);
    assert_eq!(result, non_cf_ip);
}

#[test]
fn test_extract_client_ip_trusted_header() {
    let mut headers = HeaderMap::new();
    headers.insert("X-Real-IP", "10.20.30.40".parse().unwrap());

    let conn_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
    let result = extract_client_ip(&headers, &conn_ip, Some("X-Real-IP"));
    assert_eq!(result, IpAddr::V4(Ipv4Addr::new(10, 20, 30, 40)));
}

#[test]
fn test_extract_client_ip_forwarded_for() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "X-Forwarded-For",
        "203.0.113.50, 198.51.100.1, 192.0.2.1".parse().unwrap(),
    );

    let conn_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
    let result = extract_client_ip(&headers, &conn_ip, Some("X-Forwarded-For"));
    // Should take the first IP (original client)
    assert_eq!(result, IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50)));
}

#[tokio::test]
async fn obsolete_metadata_routes_are_absent_while_owned_routes_remain() {
    let temp = tempfile::TempDir::new().unwrap();
    let db_path = temp.path().join("remi.db");
    let chunk_dir = temp.path().join("chunks");
    let cache_dir = temp.path().join("cache");
    std::fs::create_dir_all(&chunk_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    conary_core::db::init(&db_path).unwrap();

    let state = Arc::new(RwLock::new(
        ServerState::new(ServerConfig {
            db_path,
            chunk_dir,
            cache_dir,
            enable_rate_limit: false,
            enable_audit_log: false,
            ..ServerConfig::default()
        })
        .expect("test server state"),
    ));
    let app = create_router(state).await;

    async fn request(app: Router, uri: &str) -> Response {
        let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 49152))));
        app.oneshot(request).await.unwrap()
    }

    let response = request(app.clone(), "/v1/not-supported/metadata.sig").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = request(app.clone(), "/v1/fedora/metadata.sig").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.is_empty(), "removed route must not run a handler");

    for uri in ["/v1/not-supported/metadata", "/v1/fedora/metadata"] {
        let response = request(app.clone(), uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            body.is_empty(),
            "removed route {uri} must not run a handler"
        );
    }

    let response = request(app, "/v1/not-supported/tuf/root.json").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
#[tokio::test]
async fn package_downloads_are_not_content_encoded() {
    use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
    use crate::server::conversion::test_support::seed_repository_conversion_source;

    let catalog_fixture = ActiveCatalogFixture::new();
    let db_path = catalog_fixture.db_path().to_path_buf();
    let storage_root = db_path
        .parent()
        .and_then(std::path::Path::parent)
        .expect("fixture storage root")
        .to_path_buf();
    let chunk_dir = storage_root.join("chunks");
    let cache_dir = storage_root.join("cache");
    let ccs_path = cache_dir.join("packages/qemu-img-210.1.0-7.fc44-x86_64.ccs");

    std::fs::create_dir_all(ccs_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&chunk_dir).unwrap();

    let source_checksum = conary_core::hash::sha256(b"route-download-source");
    let profile_revision_sha256 = catalog_fixture.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "qemu-img",
            "2:10.1.0-7.fc44",
            "",
            Some("x86_64"),
            132,
            "route-download-source",
        )],
    );
    catalog_fixture.activate_universe(1);

    let ccs_bytes = [0x1f, 0x8b, 0x08, 0x00]
        .into_iter()
        .chain(std::iter::repeat_n(0x42, 128))
        .collect::<Vec<_>>();
    std::fs::write(&ccs_path, &ccs_bytes).unwrap();

    let conn = conary_core::db::open(&db_path).unwrap();
    let transport = crate::server::conversion::test_support::test_transport(&[]);
    let mut converted = conary_core::db::models::ConvertedPackage::new_repository(
        "fedora-44".to_string(),
        profile_revision_sha256,
        "qemu-img".to_string(),
        "2:10.1.0-7.fc44".to_string(),
        "x86_64".to_string(),
        "rpm".to_string(),
        source_checksum,
        &transport,
        ccs_bytes.len() as i64,
        "sha256:ccs".to_string(),
        ccs_path.to_string_lossy().to_string(),
        conary_core::db::models::EMPTY_REPOSITORY_PROVIDES_DIGEST.to_string(),
    );
    seed_repository_conversion_source(&conn, &mut converted);
    converted.insert_with_conversion_pin(&conn, 1).unwrap();
    drop(conn);

    let conn = conary_core::db::open_fast(&db_path).expect("reopen route fixture database");
    let active_revision = catalog_fixture
        .authority()
        .open_active_profile("fedora-44")
        .expect("open active route fixture")
        .profile_revision_sha256()
        .to_string();
    let stored = conary_core::db::models::ConvertedPackage::find_by_package_identity_with_arch(
        &conn,
        &active_revision,
        "qemu-img",
        Some("2:10.1.0-7.fc44"),
        Some("x86_64"),
    )
    .expect("query exact route fixture")
    .expect("exact route fixture exists");
    assert!(
        stored
            .repository_conversion_is_current_for_revision(&active_revision)
            .expect("validate exact route fixture")
    );
    conary_core::db::models::ConvertedPackage::require_conversion_pin(
        &conn,
        stored.id.expect("persisted route fixture"),
    )
    .expect("route fixture has exact conversion pin");
    stored
        .scriptlet_summary()
        .expect("route fixture has valid lifecycle summary");
    stored
        .repository_artifact()
        .expect("route fixture has complete serving state");
    drop(conn);

    let state = Arc::new(RwLock::new(
        ServerState::new(ServerConfig {
            db_path,
            chunk_dir,
            cache_dir,
            catalog_dir: catalog_fixture.catalog_dir().to_path_buf(),
            enable_rate_limit: false,
            enable_bloom_filter: false,
            ..ServerConfig::default()
        })
        .expect("test server state"),
    ));
    let app = create_router(state).await;

    let mut request = Request::builder()
        .uri("/v1/fedora/packages/qemu-img/download?version=2%3A10.1.0-7.fc44&arch=x86_64")
        .header(header::ACCEPT_ENCODING, "gzip")
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            49152,
        ))));

    let response = app.oneshot(request).await.unwrap();

    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected body: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        !headers.contains_key(header::CONTENT_ENCODING),
        "CCS package downloads are already gzip archives and must not be HTTP content-encoded"
    );

    assert_eq!(body.as_ref(), ccs_bytes.as_slice());
}
