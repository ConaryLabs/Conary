// apps/remi/src/server/routes/public.rs
//! Public Remi router assembly and readiness endpoints.

use super::*;
use std::path::PathBuf;
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};

/// Create the main public application router
///
/// This router handles:
/// - Health checks
/// - Package metadata and downloads
/// - Chunk serving
/// - Federation discovery
/// - Job status polling
///
/// Admin endpoints are NOT included here - they're on a separate listener.
pub async fn create_router(state: Arc<RwLock<ServerState>>) -> Router {
    let config = {
        let guard = state.read().await;
        guard.config.clone()
    };

    let rate_limiter = Arc::new(RateLimiter::new(
        config.rate_limit_rps,
        config.rate_limit_burst,
    ));

    if config.enable_rate_limit {
        let cleanup_limiter = Arc::clone(&rate_limiter);
        tokio::spawn(async move {
            let cleanup_interval = std::time::Duration::from_secs(300);
            let max_age = std::time::Duration::from_secs(300);
            loop {
                tokio::time::sleep(cleanup_interval).await;
                cleanup_limiter.cleanup(max_age).await;
            }
        });
    }

    let public_cors = create_cors_layer(&config, false);
    let restricted_cors = create_cors_layer(&config, true);
    let compression = CompressionLayer::new().compress_when(
        DefaultPredicate::new().and(NotForContentType::const_new("application/octet-stream")),
    );

    let chunk_routes = Router::new()
        .route("/v1/chunks/{hash}", head(chunks::head_chunk))
        .route("/v1/chunks/{hash}", get(chunks::get_chunk))
        .route("/v1/chunks/find-missing", post(chunks::find_missing))
        .layer(restricted_cors)
        .with_state(state.clone());

    let public_routes = Router::new()
        .route("/health", get(health_check))
        .route("/health/ready", get(readiness_check))
        .route("/v1/federation/directory", get(federation::directory))
        .route("/v1/{distro}/packages/{name}", get(packages::get_package))
        .route(
            "/v1/{distro}/packages/{name}/download",
            get(packages::download_package),
        )
        .route(
            "/v1/{distro}/packages/{name}/delta",
            get(packages::get_delta),
        )
        .route("/v1/jobs/{job_id}", get(jobs::get_job_status))
        .route(
            "/v1/recipes/{name}/{version}/download",
            get(recipes::download_recipe_package),
        )
        .route("/v1/index/{distro}/{name}", get(sparse::get_sparse_entry))
        .route("/v1/index/{distro}", get(sparse::list_packages))
        .route("/v1/search", get(search::search_packages))
        .route("/v1/suggest", get(search::suggest_packages))
        .route("/v1/canonical/map", get(canonical::canonical_map))
        .route("/v1/canonical/search", get(canonical::canonical_search))
        .route("/v1/canonical/{name}", get(canonical::canonical_lookup))
        .route("/v1/groups", get(canonical::groups_list))
        .route("/v1/models/{name}", get(models::get_model))
        .route(
            "/v1/models/{name}/signature",
            get(models::get_model_signature),
        )
        .route("/v1/models", get(models::list_models))
        .route(
            "/v1/packages/{distro}/{name}",
            get(detail::get_package_detail),
        )
        .route(
            "/v1/packages/{distro}/{name}/versions",
            get(detail::get_versions),
        )
        .route(
            "/v1/packages/{distro}/{name}/dependencies",
            get(detail::get_dependencies),
        )
        .route(
            "/v1/packages/{distro}/{name}/rdepends",
            get(detail::get_reverse_dependencies),
        )
        .route("/v1/{distro}/tuf/timestamp.json", get(tuf::get_timestamp))
        .route("/v1/{distro}/tuf/snapshot.json", get(tuf::get_snapshot))
        .route("/v1/{distro}/tuf/targets.json", get(tuf::get_targets))
        .route("/v1/{distro}/tuf/root.json", get(tuf::get_root))
        .route("/v1/{distro}/tuf/{version}", get(tuf::get_versioned_root))
        .route("/v1/ccs/conary/latest", get(self_update::get_latest))
        .route("/v1/ccs/conary/versions", get(self_update::get_versions))
        .route(
            "/v1/ccs/conary/{version}",
            get(self_update::get_version_info),
        )
        .route(
            "/v1/ccs/conary/{version}/download",
            get(self_update::download),
        )
        .route(
            "/test-fixtures/{*path}",
            get(artifacts::get_fixture).head(artifacts::head_fixture),
        )
        .route(
            "/test-artifacts/{*path}",
            get(artifacts::get_test_artifact).head(artifacts::head_test_artifact),
        )
        .route(
            "/v1/derivations/probe",
            post(derivations::probe_derivations),
        )
        .route(
            "/v1/derivations/{derivation_id}",
            get(derivations::get_derivation)
                .head(derivations::head_derivation)
                .put(derivations::put_derivation),
        )
        .route("/v1/seeds/latest", get(seeds::get_latest_seed))
        .route("/v1/seeds", get(seeds::list_seeds))
        .route(
            "/v1/seeds/{seed_id}",
            get(seeds::get_seed).put(seeds::put_seed),
        )
        .route("/v1/seeds/{seed_id}/image", get(seeds::get_seed_image))
        .route(
            "/v1/profiles/{profile_hash}",
            get(profiles::get_profile).put(profiles::put_profile),
        )
        .route("/v1/stats/popular", get(detail::get_popular))
        .route("/v1/stats/recent", get(detail::get_recent))
        .route("/v1/stats/overview", get(detail::get_overview))
        .route("/metrics", get(prometheus_metrics))
        .route("/v2/", get(oci::version_check))
        .route("/v2/_catalog", get(oci::catalog))
        .route(
            "/v2/{*path}",
            get(oci::oci_catchall).head(oci::oci_catchall_head),
        )
        .layer(compression)
        .layer(public_cors)
        .with_state(state.clone());

    let web_routes = {
        let state_guard = state.read().await;
        state_guard.config.web_root.as_ref().map(|web_root| {
            let web_root = Arc::new(web_root.clone());
            Router::new().fallback_service(
                ServeDir::new(web_root.as_path())
                    .fallback(get(serve_web_route).with_state(Arc::clone(&web_root))),
            )
        })
    };

    let mut app = Router::new().merge(chunk_routes).merge(public_routes);

    if let Some(web) = web_routes {
        app = app.merge(web);
    }

    if config.enable_rate_limit {
        app = app.route_layer(middleware::from_fn_with_state(
            (rate_limiter, state.clone()),
            rate_limit_middleware,
        ));
    }

    app = app.route_layer(middleware::from_fn_with_state(
        state.clone(),
        ban_middleware,
    ));

    app = app.layer(axum::extract::DefaultBodyLimit::max(
        request_body_limit_bytes(),
    ));

    if config.enable_audit_log {
        app = app.route_layer(middleware::from_fn_with_state(
            state.clone(),
            audit_log_middleware,
        ));
    }

    app
}

/// Public path prefixes the package-index SPA resolves in the browser.
///
/// `web/` is a SvelteKit app built with the static adapter in fallback mode:
/// one `index.html` plus its assets, with `/packages/{distro}/{name}` and the
/// rest of the map routed client-side. Those paths have no file on disk, so the
/// server must hand them the entry document.
///
/// The list is explicit because the wildcard fallback it replaces answered
/// *every* unknown path with that document -- `conary-repo.toml`,
/// `metadata/root.json`, anything -- as 200 `text/html`. A client probing for a
/// file's presence got "yes" for every path it asked about, which is how a
/// `conary repo add` against this host tried to parse the SPA page as TOML
/// (issue #155). A path that is neither a file nor a declared route now fails
/// closed.
///
/// This list must agree with `web/src/routes/` in both directions, and
/// `spa_route_prefixes_match_declared_web_routes` below enforces that by
/// reading the route tree: a route added there and missing here would 404 on
/// reload, and an entry left here after a route is deleted would keep answering
/// 200 for a path the app no longer has.
const SPA_ROUTE_PREFIXES: &[&str] = &["/about", "/packages", "/search", "/stats"];

/// Whether `path` is served by the SPA entry document rather than by a file.
///
/// `/` is served by `ServeDir` itself (directory index), so it is not listed
/// here; a prefix matches its own path and anything nested beneath it.
fn is_spa_route(path: &str) -> bool {
    SPA_ROUTE_PREFIXES.iter().any(|route| {
        path == *route
            || path
                .strip_prefix(route)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Serve the SPA entry document for a declared client-side route, and a typed
/// 404 for everything else `ServeDir` could not find on disk.
async fn serve_web_route(State(web_root): State<Arc<PathBuf>>, request: Request<Body>) -> Response {
    if !is_spa_route(request.uri().path()) {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not found"})),
        )
            .into_response();
    }

    match ServeFile::new(web_root.join("index.html"))
        .oneshot(request)
        .await
    {
        Ok(response) => response.into_response(),
        Err(infallible) => match infallible {},
    }
}

async fn health_check() -> &'static str {
    "OK"
}

/// Report whether this server can actually answer requests.
///
/// Every probe fails closed: an absent database, a wrong schema revision, a
/// missing directory, insufficient free space, or a probe that could not run at
/// all all produce 503. Deploy verification and the release pipeline consume
/// this endpoint, so it must never report readiness it has not established.
/// The evaluation itself lives in `server::readiness`, where each failure mode
/// is covered by a focused test.
async fn readiness_check(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> Response {
    let inputs = {
        let state_guard = state.read().await;
        let config = &state_guard.config;
        ReadinessInputs {
            db_path: config.db_path.clone(),
            chunk_dir: config.chunk_dir.clone(),
            cache_dir: config.cache_dir.clone(),
            min_free_bytes: config.readiness_min_free_bytes,
        }
    };

    // Probes open a database and stat filesystems; keep them off the runtime.
    let report = match tokio::task::spawn_blocking(move || readiness::evaluate(&inputs)).await {
        Ok(report) => report,
        Err(error) => {
            // The probe task itself failed, so readiness is unknown. Unknown is
            // not ready.
            tracing::error!("readiness probe task failed: {error}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "readiness probe did not complete",
            )
                .into_response();
        }
    };

    if report.ready {
        (StatusCode::OK, Json(report)).into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(report)).into_response()
    }
}

pub(super) async fn prometheus_metrics(
    axum::extract::State(state): axum::extract::State<Arc<RwLock<ServerState>>>,
) -> String {
    let state = state.read().await;
    state.metrics.to_prometheus()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::ServerConfig;
    use axum::extract::ConnectInfo;
    use std::collections::BTreeSet;
    use std::net::SocketAddr;
    use std::path::Path;

    /// Build the public router over a web root holding the SPA entry document
    /// and one real static asset.
    async fn web_app() -> (tempfile::TempDir, Router) {
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("remi.db");
        let chunk_dir = temp.path().join("chunks");
        let cache_dir = temp.path().join("cache");
        let web_root = temp.path().join("web");
        std::fs::create_dir_all(&chunk_dir).unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::create_dir_all(web_root.join("brand")).unwrap();
        conary_core::db::init(&db_path).unwrap();

        std::fs::write(
            web_root.join("index.html"),
            "<!doctype html><title>Conary package index</title>",
        )
        .unwrap();
        std::fs::write(web_root.join("brand/conary-mark.svg"), "<svg/>").unwrap();

        let state = Arc::new(RwLock::new(
            ServerState::new(ServerConfig {
                db_path,
                chunk_dir,
                cache_dir,
                web_root: Some(web_root),
                enable_rate_limit: false,
                enable_audit_log: false,
                enable_bloom_filter: false,
                ..ServerConfig::default()
            })
            .expect("test server state"),
        ));

        (temp, create_router(state).await)
    }

    async fn get_path(app: &Router, uri: &str) -> (StatusCode, Option<String>, String) {
        let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 49152))));
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|value| value.to_str().unwrap().to_string());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, content_type, String::from_utf8_lossy(&body).into())
    }

    /// A path that is neither a file nor a declared SPA route must say so. The
    /// probes below are the ones that actually misfired against the public
    /// host: a static-repository identity file and a TUF metadata target.
    #[tokio::test]
    async fn unknown_public_paths_are_not_answered_with_the_spa_document() {
        let (_temp, app) = web_app().await;

        for path in [
            "/definitely-not-a-real-path",
            "/conary-repo.toml",
            "/metadata/root.json",
            "/packages.json",
            "/aboutish",
            "/v1/no-such-route",
        ] {
            let (status, content_type, body) = get_path(&app, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path} must 404");
            let content_type = content_type.unwrap_or_default();
            assert!(
                !content_type.starts_with("text/html"),
                "{path} answered {content_type}: {body}"
            );
            assert!(
                !body.contains("<!doctype html>"),
                "{path} served the SPA document: {body}"
            );
        }
    }

    /// The SPA still works: its entry path, its declared routes, and the dynamic
    /// package routes beneath them all reach the entry document.
    #[tokio::test]
    async fn declared_spa_routes_serve_the_entry_document() {
        let (_temp, app) = web_app().await;

        for path in [
            "/",
            "/about",
            "/packages",
            "/packages/fedora",
            "/packages/fedora/qemu-img",
            "/search",
            "/stats",
        ] {
            let (status, content_type, body) = get_path(&app, path).await;
            assert_eq!(status, StatusCode::OK, "{path} must serve the SPA");
            assert!(
                content_type.unwrap_or_default().starts_with("text/html"),
                "{path} must serve the SPA as html"
            );
            assert!(body.contains("Conary package index"), "{path}: {body}");
        }
    }

    /// A real file under the web root is still served by `ServeDir`, with its
    /// own type rather than the entry document's.
    #[tokio::test]
    async fn static_assets_under_the_web_root_still_serve() {
        let (_temp, app) = web_app().await;

        let (status, content_type, body) = get_path(&app, "/brand/conary-mark.svg").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type.as_deref(), Some("image/svg+xml"));
        assert_eq!(body, "<svg/>");
    }

    /// The API is unaffected: owned routes under `/v1` keep answering, and a
    /// removed API route stays an empty 404 rather than falling through to the
    /// web fallback.
    #[tokio::test]
    async fn api_routes_are_unaffected_by_the_web_fallback() {
        let (_temp, app) = web_app().await;

        let (status, _, body) = get_path(&app, "/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "OK");

        let (status, content_type, _) = get_path(&app, "/v1/federation/directory").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            content_type
                .unwrap_or_default()
                .starts_with("application/json"),
            "federation directory must stay JSON"
        );

        let (status, content_type, body) = get_path(&app, "/v1/not-supported/metadata").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            !content_type.unwrap_or_default().starts_with("text/html"),
            "removed API route must not be answered by the SPA"
        );
        assert!(!body.contains("Conary package index"));
    }

    /// `SPA_ROUTE_PREFIXES` is the server's copy of a fact the SPA owns, so the
    /// route tree itself decides whether that copy is still true.
    ///
    /// Every top-level directory under `web/src/routes/` is one client-side
    /// route, and every entry in the const must still have a directory. The
    /// comparison is a set equality so both failures name themselves: a new
    /// SvelteKit route fails here instead of 404-ing in production, and a
    /// removed one cannot leave a stale prefix answering 200.
    ///
    /// Nested `[param]` directories do not appear here -- they live under their
    /// parent, which the prefix match already covers. A top-level `[param]` or
    /// `(group)` directory would fail this assertion, which is the right
    /// outcome: neither maps to a plain prefix, so both need a decision rather
    /// than a silent extra entry.
    #[test]
    fn spa_route_prefixes_match_declared_web_routes() {
        let routes_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/routes");
        let entries = std::fs::read_dir(&routes_dir).unwrap_or_else(|error| {
            panic!(
                "read the SPA route tree at {}: {error}. \
                 This contract cannot be checked without it, and an unchecked \
                 SPA_ROUTE_PREFIXES is how an unserved route reaches production.",
                routes_dir.display()
            )
        });

        let mut declared = BTreeSet::new();
        for entry in entries {
            let entry = entry.expect("read SPA route directory entry");
            // `+page.svelte`, `+layout.svelte`, and `+error.svelte` at this
            // level are the root route, which `ServeDir` serves as the
            // directory index rather than through the prefix list.
            if entry.file_type().expect("stat SPA route entry").is_dir() {
                declared.insert(format!("/{}", entry.file_name().to_string_lossy()));
            }
        }

        assert!(
            !declared.is_empty(),
            "found no route directories under {} -- the SPA declares several, \
             so this is a broken path rather than an empty contract",
            routes_dir.display()
        );

        let configured: BTreeSet<String> = SPA_ROUTE_PREFIXES
            .iter()
            .map(|r| (*r).to_string())
            .collect();
        assert_eq!(
            configured,
            declared,
            "SPA_ROUTE_PREFIXES and web/src/routes/ disagree; \
             only in the const: {:?}, only in the route tree: {:?}",
            configured.difference(&declared).collect::<Vec<_>>(),
            declared.difference(&configured).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn spa_route_matching_is_prefix_bounded() {
        assert!(is_spa_route("/packages"));
        assert!(is_spa_route("/packages/"));
        assert!(is_spa_route("/packages/fedora/qemu-img"));
        assert!(!is_spa_route("/packages.json"));
        assert!(!is_spa_route("/packagesish"));
        assert!(!is_spa_route("/"));
        assert!(!is_spa_route("/v1/search"));
    }
}
