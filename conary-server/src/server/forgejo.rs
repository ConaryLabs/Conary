// conary-server/src/server/forgejo.rs
//! Shared Forgejo API client.
//!
//! Centralises the duplicated `forgejo_get` / `forgejo_post` helpers that were
//! previously defined independently in `handlers/admin.rs` and `mcp.rs`.
//! Both modules now call into this single implementation and map
//! `ForgejoError` to their own error types (axum `Response` or `McpError`).

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::server::ServerState;

/// Forgejo repository path used in all API calls.
pub const FORGEJO_REPO_PATH: &str = "/repos/peter/Conary";

/// Per-request timeout for Forgejo API calls (30 seconds).
///
/// The shared `http_client` may have a different timeout configured for
/// upstream chunk fetches. This per-request timeout prevents a hung
/// Forgejo server from blocking callers indefinitely.
const FORGEJO_TIMEOUT: Duration = Duration::from_secs(30);

/// Error returned by Forgejo API helpers.
///
/// Carries an optional HTTP status code from the upstream response so that
/// callers can choose an appropriate status for their own error response
/// (e.g. 502 Bad Gateway when the upstream returned a non-success status).
#[derive(Debug, thiserror::Error)]
#[error("{}", ForgejoError::format_message(*.status, message))]
pub struct ForgejoError {
    /// HTTP status code from the Forgejo response, if one was received.
    pub status: Option<u16>,
    /// Human-readable error message.
    pub message: String,
}

impl ForgejoError {
    fn format_message(status: Option<u16>, message: &str) -> String {
        match status {
            Some(code) => format!("Forgejo error (HTTP {code}): {message}"),
            None => format!("Forgejo error: {message}"),
        }
    }
}

/// Extract the Forgejo base URL, API token, and HTTP client from shared state.
///
/// Performs a single `state.read().await` so the lock is held as briefly as
/// possible.
async fn get_config(
    state: &Arc<RwLock<ServerState>>,
) -> Result<(String, String, reqwest::Client), ForgejoError> {
    let s = state.read().await;
    let base = s.forgejo_url.as_ref().ok_or_else(|| ForgejoError {
        status: None,
        message: "Forgejo not configured".to_string(),
    })?;
    let token = s.forgejo_token.clone().unwrap_or_default();
    let client = s.http_client.clone();
    Ok((base.trim_end_matches('/').to_string(), token, client))
}

/// Build a full Forgejo API URL from a base URL and a path.
///
/// `path` should start with `/` (e.g. `"{FORGEJO_REPO_PATH}/actions/workflows"`).
fn api_url(base: &str, path: &str) -> String {
    format!("{base}/api/v1{path}")
}

/// Send a GET request to the Forgejo API and return the response body text.
pub async fn get(state: &Arc<RwLock<ServerState>>, path: &str) -> Result<String, ForgejoError> {
    let (base, token, client) = get_config(state).await?;
    let url = api_url(&base, path);

    let resp = client
        .get(&url)
        .header("Authorization", format!("token {token}"))
        .timeout(FORGEJO_TIMEOUT)
        .send()
        .await
        .map_err(|e| ForgejoError {
            status: None,
            message: format!("Forgejo unreachable: {e}"),
        })?;

    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        return Err(ForgejoError {
            status: Some(code),
            message: format!("Forgejo returned {code}"),
        });
    }

    resp.text().await.map_err(|e| ForgejoError {
        status: None,
        message: format!("Response error: {e}"),
    })
}

/// Send a POST request to the Forgejo API and return the response body text.
///
/// If `body` is `Some`, it is sent as a JSON payload.  If `None`, an empty
/// JSON object `{}` is sent (Forgejo requires a Content-Type even for
/// endpoints that ignore the body).
///
/// A `204 No Content` response is normalised to the string `{"status":"ok"}`
/// so callers always receive valid JSON.
pub async fn post(
    state: &Arc<RwLock<ServerState>>,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<String, ForgejoError> {
    let (base, token, client) = get_config(state).await?;
    let url = api_url(&base, path);

    let default_body = serde_json::json!({});
    let payload = body.unwrap_or(&default_body);

    let resp = client
        .post(&url)
        .header("Authorization", format!("token {token}"))
        .json(payload)
        .timeout(FORGEJO_TIMEOUT)
        .send()
        .await
        .map_err(|e| ForgejoError {
            status: None,
            message: format!("Forgejo unreachable: {e}"),
        })?;

    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        return Err(ForgejoError {
            status: Some(code),
            message: format!("Forgejo returned {code}"),
        });
    }

    // Some Forgejo POSTs return 204 No Content
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(r#"{"status":"ok"}"#.to_string());
    }

    resp.text().await.map_err(|e| ForgejoError {
        status: None,
        message: format!("Response error: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_url() {
        assert_eq!(
            api_url(
                "https://forge.example.com",
                "/repos/owner/repo/actions/workflows"
            ),
            "https://forge.example.com/api/v1/repos/owner/repo/actions/workflows"
        );
    }

    #[test]
    fn test_api_url_trailing_slash_stripped() {
        // `get_config` strips trailing slashes from the base, so `api_url`
        // receives a clean base.  Verify the concatenation is correct.
        let base = "https://forge.example.com";
        assert_eq!(
            api_url(base, &format!("{FORGEJO_REPO_PATH}/mirror-sync")),
            "https://forge.example.com/api/v1/repos/peter/Conary/mirror-sync"
        );
    }

    #[test]
    fn test_forgejo_error_display_with_status() {
        let err = ForgejoError {
            status: Some(404),
            message: "not found".to_string(),
        };
        assert_eq!(err.to_string(), "Forgejo error (HTTP 404): not found");
    }

    #[test]
    fn test_forgejo_error_display_without_status() {
        let err = ForgejoError {
            status: None,
            message: "connection refused".to_string(),
        };
        assert_eq!(err.to_string(), "Forgejo error: connection refused");
    }
}
