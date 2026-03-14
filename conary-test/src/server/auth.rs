// conary-test/src/server/auth.rs

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

/// Axum middleware that enforces `Authorization: Bearer <token>` on every
/// request. The expected token is stored in an `Arc<String>` and captured
/// by the closure passed to `axum::middleware::from_fn`.
async fn bearer_auth(expected: Arc<String>, request: Request<Body>, next: Next) -> Response {
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if value.starts_with("Bearer ") => {
            let token = &value[7..];
            if token == expected.as_str() {
                next.run(request).await
            } else {
                unauthorized()
            }
        }
        _ => unauthorized(),
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({"error": "unauthorized"})),
    )
        .into_response()
}

/// Wrap a router with bearer token authentication middleware.
///
/// Every request routed through the returned router must include a valid
/// `Authorization: Bearer <token>` header. Requests with missing or
/// incorrect tokens receive a 401 JSON response.
pub fn with_auth<S: Clone + Send + Sync + 'static>(router: Router<S>, token: String) -> Router<S> {
    let token = Arc::new(token);
    router.layer(axum::middleware::from_fn(move |req, next| {
        let token = Arc::clone(&token);
        async move { bearer_auth(token, req, next).await }
    }))
}

#[cfg(test)]
mod tests {
    use crate::server::routes;
    use crate::test_fixtures;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    const TEST_TOKEN: &str = "test-secret-token";

    fn authed_router() -> axum::Router {
        routes::create_router(test_fixtures::test_app_state(), Some(TEST_TOKEN.to_string()))
    }

    #[tokio::test]
    async fn request_without_token_returns_401() {
        let app = authed_router();
        let req = Request::builder()
            .uri("/v1/suites")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "unauthorized");
    }

    #[tokio::test]
    async fn request_with_wrong_token_returns_401() {
        let app = authed_router();
        let req = Request::builder()
            .uri("/v1/suites")
            .header("authorization", "Bearer wrong-token")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn request_with_correct_token_returns_200() {
        let app = authed_router();
        let req = Request::builder()
            .uri("/v1/distros")
            .header("authorization", format!("Bearer {TEST_TOKEN}"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_endpoint_works_without_token() {
        let app = authed_router();
        let req = Request::builder()
            .uri("/v1/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mcp_endpoint_requires_token() {
        let app = authed_router();
        let req = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
