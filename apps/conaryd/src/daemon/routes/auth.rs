// apps/conaryd/src/daemon/routes/auth.rs
//! Route-level authorization and visibility helpers.

use super::errors::{ApiError, not_found_error};
use super::types::SharedState;
use crate::daemon::auth::{Action, AuthChecker, PeerCredentials};
use crate::daemon::{DaemonError, DaemonEvent, DaemonJob};
use axum::{
    extract::{Extension, Request, State},
    middleware,
    response::Response,
};
use std::collections::HashMap;

/// Check authorization for a mutating action.
///
/// Extracts `PeerCredentials` from the request extension (injected per-connection
/// in `run_daemon`). Unix socket connections get credentials via `SO_PEERCRED`.
///
/// Returns `Ok(())` if the action is authorized, or an `ApiError` with 403 Forbidden.
pub(super) fn require_auth(
    checker: &AuthChecker,
    creds: &Option<PeerCredentials>,
    action: Action,
) -> Result<(), ApiError> {
    match creds {
        Some(creds) => {
            if !creds.matches_current_process_identity() {
                tracing::warn!(
                    uid = creds.uid,
                    gid = creds.gid,
                    pid = creds.pid,
                    action = ?action,
                    "Authorization denied: stale peer credentials"
                );
                return Err(ApiError(Box::new(DaemonError::forbidden(
                    "Peer credentials are no longer valid for the current process",
                ))));
            }

            if checker.is_allowed(creds, action) {
                Ok(())
            } else {
                tracing::warn!(
                    uid = creds.uid,
                    gid = creds.gid,
                    pid = creds.pid,
                    action = ?action,
                    "Authorization denied"
                );
                Err(ApiError(Box::new(DaemonError::forbidden(&format!(
                    "User (uid={}) is not authorized for {:?}",
                    creds.uid, action
                )))))
            }
        }
        None => {
            // No peer credentials (TCP connection) - deny mutating actions
            tracing::warn!(action = ?action, "Mutating request denied: no peer credentials (TCP connection)");
            Err(ApiError(Box::new(DaemonError::forbidden(
                "Mutating operations require a Unix socket connection with peer credentials",
            ))))
        }
    }
}

/// Require that a daemon API request comes from root or the daemon's own UID.
pub(super) fn require_socket_identity(creds: &Option<PeerCredentials>) -> Result<(), ApiError> {
    let daemon_uid = nix::unistd::geteuid().as_raw();

    match creds {
        Some(creds) if !creds.matches_current_process_identity() => {
            tracing::warn!(
                uid = creds.uid,
                gid = creds.gid,
                pid = creds.pid,
                daemon_uid,
                "Daemon API request denied: peer credentials no longer match live process identity"
            );
            Err(ApiError(Box::new(DaemonError::forbidden(
                "Daemon API requires live peer credentials from the current process",
            ))))
        }
        Some(creds) if creds.matches_daemon_identity(daemon_uid) => Ok(()),
        Some(creds) => {
            tracing::warn!(
                uid = creds.uid,
                gid = creds.gid,
                pid = creds.pid,
                daemon_uid,
                "Daemon API request denied: peer does not match daemon identity"
            );
            Err(ApiError(Box::new(DaemonError::forbidden(&format!(
                "Daemon API requires root or daemon uid {}; got uid={}",
                daemon_uid, creds.uid
            )))))
        }
        None => {
            tracing::warn!("Daemon API request denied: no peer credentials");
            Err(ApiError(Box::new(DaemonError::forbidden(
                "Daemon API requires a Unix socket connection with peer credentials",
            ))))
        }
    }
}

/// Auth gate middleware for defense-in-depth
///
/// Rejects all `/v1` daemon API requests unless the Unix socket peer is root or
/// the daemon's own service UID. Individual handlers still check their specific
/// action permissions.
pub(super) async fn auth_gate_middleware(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    request: Request,
    next: middleware::Next,
) -> Result<Response, ApiError> {
    let _ = state;
    tracing::trace!(
        method = %request.method(),
        path = %request.uri().path(),
        "Checking daemon auth gate"
    );
    require_socket_identity(&creds)?;
    Ok(next.run(request).await)
}

pub(super) fn job_visible_to_requester(
    creds: &Option<PeerCredentials>,
    requested_by_uid: Option<u32>,
) -> bool {
    match creds {
        Some(creds) if creds.is_root() => true,
        Some(creds) => requested_by_uid == Some(creds.uid),
        None => false,
    }
}

pub(super) fn ensure_job_visible(
    creds: &Option<PeerCredentials>,
    job: &DaemonJob,
    requested_id: &str,
) -> Result<(), ApiError> {
    if job_visible_to_requester(creds, job.requested_by_uid) {
        Ok(())
    } else {
        tracing::warn!(
            requested_job_id = requested_id,
            requested_by_uid = job.requested_by_uid,
            caller_uid = creds.as_ref().map(|creds| creds.uid),
            "Daemon transaction access denied: job owned by a different user"
        );
        Err(not_found_error("transaction", requested_id))
    }
}

pub(super) fn event_visible_to_requester(
    state: &SharedState,
    creds: &Option<PeerCredentials>,
    cache: &mut HashMap<String, bool>,
    event: &DaemonEvent,
) -> bool {
    if creds.as_ref().is_some_and(PeerCredentials::is_root) {
        return true;
    }

    let Some(job_id) = event.job_id() else {
        return false;
    };

    if let Some(visible) = cache.get(job_id) {
        return *visible;
    }

    let visible = state
        .open_db()
        .ok()
        .and_then(|conn| DaemonJob::find_by_id(&conn, job_id).ok().flatten())
        .is_some_and(|job| job_visible_to_requester(creds, job.requested_by_uid));
    cache.insert(job_id.to_string(), visible);
    visible
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        body_json, create_test_state, current_process_creds, test_router,
    };
    use super::*;
    use crate::daemon::auth::{Action, AuthChecker, PeerCredentials};
    use crate::daemon::{DaemonEvent, DaemonJob};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use tower::ServiceExt;

    #[test]
    fn test_require_auth_current_process_allowed() {
        let checker = AuthChecker::new();
        let creds = current_process_creds();
        assert!(require_auth(&checker, &creds, Action::Install).is_ok());
        assert!(require_auth(&checker, &creds, Action::Remove).is_ok());
        assert!(require_auth(&checker, &creds, Action::Update).is_ok());
        assert!(require_auth(&checker, &creds, Action::Rollback).is_ok());
        assert!(require_auth(&checker, &creds, Action::GarbageCollect).is_ok());
        assert!(require_auth(&checker, &creds, Action::CancelJob).is_ok());
    }

    #[test]
    fn test_require_auth_regular_user_denied() {
        let checker = AuthChecker::new();
        let creds = Some(PeerCredentials {
            pid: 1000,
            uid: 1000,
            gid: 1000,
        });
        assert!(require_auth(&checker, &creds, Action::Install).is_err());
        assert!(require_auth(&checker, &creds, Action::Remove).is_err());
        assert!(require_auth(&checker, &creds, Action::Update).is_err());
        assert!(require_auth(&checker, &creds, Action::Rollback).is_err());
    }

    #[test]
    fn test_require_auth_no_creds_denied() {
        let checker = AuthChecker::new();
        // TCP connection with no peer credentials
        let creds: Option<PeerCredentials> = None;
        assert!(require_auth(&checker, &creds, Action::Install).is_err());
        assert!(require_auth(&checker, &creds, Action::Remove).is_err());
    }

    #[tokio::test]
    async fn test_auth_gate_blocks_put_without_credentials() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = Request::builder()
            .method("PUT")
            .uri("/v1/transactions/some-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "PUT without credentials should be blocked by auth gate middleware"
        );

        let json = body_json(response).await;
        assert_eq!(json["status"], 403);
    }

    #[tokio::test]
    async fn test_auth_gate_blocks_delete_without_credentials() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = Request::builder()
            .method("DELETE")
            .uri("/v1/transactions/some-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "DELETE without credentials should be blocked by auth gate middleware"
        );

        let json = body_json(response).await;
        assert_eq!(json["status"], 403);
    }

    #[tokio::test]
    async fn test_auth_gate_blocks_get_without_credentials() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, None);

        let request = Request::builder()
            .method("GET")
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "GET without credentials should be blocked by auth gate middleware"
        );
    }

    #[tokio::test]
    async fn test_auth_gate_blocks_get_for_non_daemon_user() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let unauthorized_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };
        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: 2000,
                uid: unauthorized_uid,
                gid: unauthorized_uid,
            }),
        );

        let request = Request::builder()
            .method("GET")
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "GET from a non-root, non-daemon uid should be blocked"
        );
    }

    #[tokio::test]
    async fn test_auth_gate_revalidates_live_peer_identity() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: u32::MAX,
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = Request::builder()
            .method("GET")
            .uri("/v1/packages")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "GET with stale peer credentials should be blocked"
        );
    }

    #[test]
    fn test_event_visibility_filters_by_requesting_uid() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let visible_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 5}),
        )
        .with_uid(daemon_uid);
        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);

        {
            let conn = state.open_db().unwrap();
            visible_job.insert(&conn).unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let creds = Some(PeerCredentials {
            pid: std::process::id(),
            uid: daemon_uid,
            gid: daemon_uid,
        });
        let mut cache = HashMap::new();

        assert!(event_visible_to_requester(
            &state,
            &creds,
            &mut cache,
            &DaemonEvent::JobStarted {
                job_id: visible_job.id.clone(),
            }
        ));
        assert!(!event_visible_to_requester(
            &state,
            &creds,
            &mut cache,
            &DaemonEvent::JobStarted {
                job_id: hidden_job.id.clone(),
            }
        ));
        assert!(!event_visible_to_requester(
            &state,
            &creds,
            &mut cache,
            &DaemonEvent::StateCreated { state_number: 99 }
        ));
    }
}
