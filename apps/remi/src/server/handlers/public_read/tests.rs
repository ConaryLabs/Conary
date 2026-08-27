// apps/remi/src/server/handlers/public_read/tests.rs

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tokio::sync::RwLock;
use tower::ServiceExt;

use super::*;
use crate::server::catalog_authority::test_support::{ActiveCatalogFixture, package};
use crate::server::search::PackageSearchDoc;
use crate::server::{SearchEngine, ServerConfig, ServerState, create_router};

fn search_document(distro: &str, name: &str) -> PackageSearchDoc {
    PackageSearchDoc {
        name: name.to_string(),
        version: "1.0".to_string(),
        release: Some("1".to_string()),
        distro: distro.to_string(),
        architecture: Some("x86_64".to_string()),
        description: Some(format!("{distro} candidate package")),
        requirement_terms: None,
        size: 1024,
        converted: false,
        source_kind: None,
    }
}

async fn app(
    fixture: &ActiveCatalogFixture,
    search_engine: SearchEngine,
) -> (tempfile::TempDir, Router) {
    let runtime = tempfile::tempdir().unwrap();
    let chunk_dir = runtime.path().join("chunks");
    let cache_dir = runtime.path().join("cache");
    let catalog_candidate_dir = runtime.path().join("catalog-candidates");
    std::fs::create_dir_all(&chunk_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::create_dir_all(&catalog_candidate_dir).unwrap();
    let mut state = ServerState::new(ServerConfig {
        db_path: fixture.db_path().to_path_buf(),
        chunk_dir,
        cache_dir,
        catalog_dir: fixture.catalog_dir().to_path_buf(),
        catalog_candidate_dir,
        enable_rate_limit: false,
        enable_audit_log: false,
        enable_bloom_filter: false,
        web_root: None,
        ..ServerConfig::default()
    })
    .unwrap();
    state.search_engine = Some(Arc::new(search_engine));
    let router = create_router(Arc::new(RwLock::new(state))).await;
    (runtime, router)
}

async fn get(app: &Router, path: &str) -> axum::response::Response {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 49152))));
    app.clone().oneshot(request).await.unwrap()
}

async fn json(response: axum::response::Response) -> Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn every_package_read_surface_returns_typed_503_without_an_active_universe() {
    let fixture = ActiveCatalogFixture::new();
    fixture.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "htop",
            "3.4.1",
            "1",
            Some("x86_64"),
            1024,
            "fedora-htop",
        )],
    );
    fixture.activate(
        "solus",
        2,
        vec![package(
            "solus",
            "htop",
            "3.4.1-1",
            "",
            Some("x86_64"),
            1024,
            "solus-htop",
        )],
    );
    let search_dir = tempfile::tempdir().unwrap();
    let engine = SearchEngine::new(search_dir.path()).unwrap();
    engine
        .index_package(&search_document("solus", "htop"))
        .unwrap();
    let (_runtime, app) = app(&fixture, engine).await;

    for path in [
        "/v1/fedora/packages/htop",
        "/v1/fedora/packages/htop/download",
        "/v1/packages/fedora/htop",
        "/v1/packages/fedora/htop/versions",
        "/v1/packages/fedora/htop/dependencies",
        "/v1/packages/fedora/htop/rdepends",
        "/v1/index/fedora/htop",
        "/v1/search?q=htop",
        "/v1/suggest?prefix=ht",
        "/v1/stats/popular",
        "/v1/stats/recent",
        "/v1/stats/overview",
    ] {
        let response = get(&app, path).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(
            response.headers()["x-conary-error"],
            "PUBLIC_UNIVERSE_UNAVAILABLE",
            "{path}"
        );
        let body = json(response).await;
        assert_eq!(body["code"], "PUBLIC_UNIVERSE_UNAVAILABLE", "{path}");
        assert_eq!(body["reason"], "no_active_universe", "{path}");
    }
}

#[tokio::test]
async fn detail_index_search_and_stats_share_one_universe_and_exclude_candidates() {
    let fixture = ActiveCatalogFixture::new();
    let fedora_revision = fixture.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "htop",
            "3.4.1",
            "1",
            Some("x86_64"),
            1024,
            "fedora-htop",
        )],
    );
    fixture.activate(
        "solus",
        2,
        vec![package(
            "solus",
            "htop",
            "99.0-1",
            "",
            Some("x86_64"),
            1024,
            "solus-htop",
        )],
    );
    let conn = fixture.connection();
    conn.execute(
        "INSERT INTO download_counts (
             source_profile, package_name, total_count, count_30d, count_7d
         ) VALUES ('fedora-44', 'htop', 3, 2, 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO download_counts (
             source_profile, package_name, total_count, count_30d, count_7d
         ) VALUES ('solus', 'htop', 100, 100, 100)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO download_stats (
             source_profile, package_name, package_version, downloaded_at
         ) VALUES ('fedora-44', 'htop', '3.4.1', '2026-08-27 01:00:00')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO download_stats (
             source_profile, package_name, package_version, downloaded_at
         ) VALUES ('solus', 'htop', '99.0-1', '2026-08-27 02:00:00')",
        [],
    )
    .unwrap();
    drop(conn);
    let universe_revision = fixture.activate_universe(1);
    let universe = PublicUniverseSnapshot::load(fixture.db_path())
        .unwrap()
        .unwrap();
    assert!(universe.profile("solus").is_none());

    let search_dir = tempfile::tempdir().unwrap();
    let engine = SearchEngine::new(search_dir.path()).unwrap();
    engine
        .index_package(&search_document("solus", "stale-candidate"))
        .unwrap();
    engine
        .rebuild_from_universe(fixture.db_path(), fixture.authority(), &universe)
        .unwrap();
    let (_runtime, app) = app(&fixture, engine).await;

    let paths = [
        "/v1/packages/fedora/htop",
        "/v1/index/fedora/htop",
        "/v1/search?q=htop&distro=fedora",
        "/v1/stats/overview",
        "/v1/stats/popular?limit=1",
        "/v1/stats/recent?limit=1",
    ];
    for path in paths {
        let response = get(&app, path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers()[UNIVERSE_REVISION_HEADER],
            universe_revision,
            "{path}"
        );
        if path.contains("fedora") {
            assert_eq!(
                response.headers()[PROFILE_REVISION_HEADER],
                fedora_revision,
                "{path}"
            );
        }
        let body = json(response).await;
        if path.starts_with("/v1/search") {
            assert_eq!(body["results"].as_array().unwrap().len(), 1);
            assert_eq!(body["results"][0]["distro"], "fedora");
        } else if path == "/v1/stats/overview" {
            assert_eq!(body["total_downloads"], 3);
            assert_eq!(body["downloads_30d"], 2);
        } else if path.starts_with("/v1/stats/") {
            assert_eq!(body.as_array().unwrap().len(), 1);
            assert_eq!(body[0]["distro"], "fedora");
        }
    }

    let candidate = get(&app, "/v1/search?q=htop&distro=solus").await;
    assert_eq!(candidate.status(), StatusCode::BAD_REQUEST);

    fixture.activate(
        "fedora-44",
        3,
        vec![package(
            "fedora-44",
            "new-package",
            "2.0",
            "1",
            Some("x86_64"),
            2048,
            "fedora-new",
        )],
    );
    let replacement_universe = fixture.activate_universe(2);

    let detail = get(&app, "/v1/packages/fedora/new-package").await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(
        detail.headers()[UNIVERSE_REVISION_HEADER],
        replacement_universe
    );

    let stale_search = get(&app, "/v1/search?q=htop&distro=fedora").await;
    assert_eq!(stale_search.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        json(stale_search).await["reason"],
        "search_index_revision_mismatch"
    );
}

#[tokio::test]
async fn supported_profile_absent_from_the_active_universe_is_typed_unavailability() {
    let fixture = ActiveCatalogFixture::new();
    fixture.activate(
        "fedora-44",
        1,
        vec![package(
            "fedora-44",
            "htop",
            "3.4.1",
            "1",
            Some("x86_64"),
            1024,
            "fedora-htop",
        )],
    );
    fixture.activate_universe(1);
    let universe = PublicUniverseSnapshot::load(fixture.db_path())
        .unwrap()
        .unwrap();
    let search_dir = tempfile::tempdir().unwrap();
    let engine = SearchEngine::new(search_dir.path()).unwrap();
    engine
        .rebuild_from_universe(fixture.db_path(), fixture.authority(), &universe)
        .unwrap();
    let (_runtime, app) = app(&fixture, engine).await;

    let response = get(&app, "/v1/packages/ubuntu/htop").await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = json(response).await;
    assert_eq!(body["code"], "PUBLIC_UNIVERSE_UNAVAILABLE");
    assert_eq!(body["reason"], "profile_not_in_universe");
    assert_eq!(body["profile"], "ubuntu-26.04");
}
