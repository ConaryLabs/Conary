// apps/conaryd/src/daemon/routes/errors.rs
//! API error conversion helpers for daemon routes.

use crate::daemon::DaemonError;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use std::fmt::Display;

pub(super) const INTERNAL_ERROR_DETAIL: &str = "An internal daemon error occurred";

/// Error response wrapper for RFC 7807 format
pub struct ApiError(pub(super) Box<DaemonError>);

impl From<DaemonError> for ApiError {
    fn from(err: DaemonError) -> Self {
        ApiError(Box::new(err))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        let body = Json(&*self.0);

        (status, [("content-type", "application/problem+json")], body).into_response()
    }
}

/// Result type for API handlers
pub type ApiResult<T> = Result<T, ApiError>;

pub(super) fn not_found_error(resource: &str, identifier: &str) -> ApiError {
    ApiError(Box::new(DaemonError::not_found(&format!(
        "{} '{}'",
        resource, identifier
    ))))
}

pub(super) fn bad_request_error(message: &str) -> ApiError {
    ApiError(Box::new(DaemonError::bad_request(message)))
}

pub(super) fn not_implemented_error(detail: &str) -> ApiError {
    ApiError(Box::new(DaemonError::new(
        "not_implemented",
        "Not Implemented",
        501,
        detail,
    )))
}

pub(super) fn internal_error(message: &str) -> DaemonError {
    tracing::error!("{message}");
    DaemonError::internal(INTERNAL_ERROR_DETAIL)
}

pub(super) fn internal_error_with(context: &str, error: impl Display) -> DaemonError {
    tracing::error!(error = %error, "{context}");
    DaemonError::internal(INTERNAL_ERROR_DETAIL)
}

pub(super) fn internal_api_error(context: &str, error: impl Display) -> ApiError {
    ApiError(Box::new(internal_error_with(context, error)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_response() {
        let err = DaemonError::not_found("package nginx");
        let api_err = ApiError::from(err);

        // Just verify it can be converted to a response
        let _ = api_err.into_response();
    }
}
