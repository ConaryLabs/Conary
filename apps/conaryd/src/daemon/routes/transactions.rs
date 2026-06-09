// apps/conaryd/src/daemon/routes/transactions.rs
//! Daemon job creation, transaction, and per-job streaming routes.

use super::auth::{ensure_job_visible, job_visible_to_requester, require_auth};
use super::db::run_db_query;
use super::errors::{
    ApiError, ApiResult, bad_request_error, internal_api_error, internal_error,
    internal_error_with, not_found_error,
};
use super::sse::{SseConnectionGuard, acquire_sse_connection};
use super::types::{
    CreateTransactionRequest, CreateTransactionResponse, DryRunResponse, DryRunSummary,
    PackageOperationRequest, SharedState, TransactionDetails, TransactionListQuery,
    TransactionOperation, TransactionSummary,
};
use crate::daemon::auth::{Action, PeerCredentials};
use crate::daemon::{DaemonError, DaemonEvent, DaemonJob, JobStatus};
use axum::{
    Router,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{
        Json,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
};
use futures::stream::{self, Stream};
use std::{convert::Infallible, sync::atomic::Ordering, time::Duration};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

type TransactionResult = ApiResult<(
    StatusCode,
    [(header::HeaderName, String); 1],
    Json<CreateTransactionResponse>,
)>;

pub(super) fn router() -> Router<SharedState> {
    Router::new()
        .route("/transactions", get(list_transactions_handler))
        .route("/transactions", post(create_transaction_handler))
        .route("/transactions/dry-run", post(dry_run_handler))
        .route("/transactions/{id}", get(get_transaction_handler))
        .route("/transactions/{id}", delete(cancel_transaction_handler))
        .route("/transactions/{id}/stream", get(transaction_stream_handler))
        .route("/packages/install", post(install_packages_handler))
        .route("/packages/remove", post(remove_packages_handler))
        .route("/packages/update", post(update_packages_handler))
        .route("/enhance", post(enhance_handler))
}

async fn list_transactions_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    Query(params): Query<TransactionListQuery>,
) -> ApiResult<Json<Vec<TransactionSummary>>> {
    let limit = params.limit.map(|n| n.min(1000));
    let jobs = run_db_query(&state, move |conn| {
        let status_filter = params.status.as_deref().and_then(|s| match s {
            "queued" => Some(JobStatus::Queued),
            "running" => Some(JobStatus::Running),
            "completed" => Some(JobStatus::Completed),
            "failed" => Some(JobStatus::Failed),
            "cancelled" => Some(JobStatus::Cancelled),
            _ => None,
        });

        match status_filter {
            Some(status) => DaemonJob::list_by_status(conn, status, limit),
            None => DaemonJob::list_all(conn, limit),
        }
    })
    .await?;

    let summaries: Vec<TransactionSummary> = jobs
        .iter()
        .filter(|job| job_visible_to_requester(&creds, job.requested_by_uid))
        .map(TransactionSummary::from)
        .collect();
    Ok(Json(summaries))
}

async fn create_transaction_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    headers: HeaderMap,
    Json(request): Json<CreateTransactionRequest>,
) -> TransactionResult {
    enqueue_transaction_request(state, creds, headers, request).await
}

async fn enqueue_transaction_request(
    state: SharedState,
    creds: Option<PeerCredentials>,
    headers: HeaderMap,
    request: CreateTransactionRequest,
) -> TransactionResult {
    if request.operations.is_empty() {
        return Err(bad_request_error("At least one operation is required"));
    }

    validate_transaction_operations(&request.operations, false)?;

    let job_kind = determine_job_kind(&request.operations);
    require_auth_for_operations(&state, &creds, &request.operations)?;

    let spec = serde_json::to_value(&request.operations)
        .map_err(|e| internal_api_error("Failed to serialize daemon transaction request", e))?;

    let idempotency_key = get_idempotency_key(&headers);

    if let Some(ref key) = idempotency_key {
        let key_clone = key.clone();
        let existing = run_db_query(&state, move |conn| {
            DaemonJob::find_by_idempotency_key(conn, &key_clone)
        })
        .await?;

        if let Some(existing_job) = existing {
            return idempotent_job_response(&state, existing_job, job_kind, &spec).await;
        }
    }

    let mut job = DaemonJob::new(job_kind, spec);
    if let Some(key) = idempotency_key {
        job.idempotency_key = Some(key);
    }
    if let Some(creds) = &creds {
        job = job.with_uid(creds.uid);
    }

    let job_id = job.id.clone();

    if let Some(existing_job) = insert_or_dedup(&state, job.clone()).await? {
        return idempotent_job_response(&state, existing_job, job_kind, &job.spec).await;
    }

    let _cancel_token = state
        .queue
        .enqueue(job, crate::daemon::JobPriority::Normal)
        .await;

    let queue_position = state.queue.position(&job_id).await.unwrap_or(0);

    state.emit(DaemonEvent::JobQueued {
        job_id: job_id.clone(),
        position: queue_position,
    });

    state.metrics.jobs_total.fetch_add(1, Ordering::Relaxed);

    let location = format!("/v1/transactions/{}", job_id);
    let response = CreateTransactionResponse {
        job_id,
        status: "queued".to_string(),
        queue_position,
        location: location.clone(),
    };

    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, location)],
        Json(response),
    ))
}

fn determine_job_kind(operations: &[TransactionOperation]) -> crate::daemon::JobKind {
    use crate::daemon::JobKind;

    let mut has_install = false;
    let mut has_remove = false;
    let mut has_update = false;

    for op in operations {
        match op {
            TransactionOperation::Install { .. } => has_install = true,
            TransactionOperation::Remove { .. } => has_remove = true,
            TransactionOperation::Update { .. } => has_update = true,
        }
    }

    match (has_install, has_remove, has_update) {
        (true, false, false) => JobKind::Install,
        (false, true, false) => JobKind::Remove,
        (false, false, true) => JobKind::Update,
        _ => JobKind::Install,
    }
}

fn require_auth_for_operations(
    state: &SharedState,
    creds: &Option<PeerCredentials>,
    operations: &[TransactionOperation],
) -> Result<(), ApiError> {
    let mut required_actions = Vec::new();

    for op in operations {
        let action = match op {
            TransactionOperation::Install { .. } => Action::Install,
            TransactionOperation::Remove { .. } => Action::Remove,
            TransactionOperation::Update { .. } => Action::Update,
        };

        if !required_actions.contains(&action) {
            required_actions.push(action);
        }
    }

    for action in required_actions {
        require_auth(&state.auth_checker, creds, action)?;
    }

    Ok(())
}

fn validate_transaction_operations(
    operations: &[TransactionOperation],
    allow_mixed_kinds: bool,
) -> Result<(), ApiError> {
    let mut first_kind = None;

    for op in operations {
        let current_kind = match op {
            TransactionOperation::Install { .. } => crate::daemon::JobKind::Install,
            TransactionOperation::Remove { .. } => crate::daemon::JobKind::Remove,
            TransactionOperation::Update { .. } => crate::daemon::JobKind::Update,
        };

        if let Some(kind) = first_kind {
            if !allow_mixed_kinds && kind != current_kind {
                return Err(bad_request_error(
                    "Mutating daemon transactions must contain one package operation kind",
                ));
            }
        } else {
            first_kind = Some(current_kind);
        }

        match op {
            TransactionOperation::Install { packages, .. }
            | TransactionOperation::Remove { packages, .. }
                if packages.is_empty() =>
            {
                return Err(bad_request_error(
                    "Install and remove operations require at least one package",
                ));
            }
            TransactionOperation::Install { .. }
            | TransactionOperation::Remove { .. }
            | TransactionOperation::Update { .. } => {}
        }
    }

    Ok(())
}

async fn get_transaction_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    Path(id): Path<String>,
) -> ApiResult<Json<TransactionDetails>> {
    let queue_position = state.queue.position(&id).await;
    let job_id = id.clone();
    let job = run_db_query(&state, move |conn| DaemonJob::find_by_id(conn, &job_id)).await?;

    let job = job.ok_or_else(|| not_found_error("transaction", &id))?;
    ensure_job_visible(&creds, &job, &id)?;
    Ok(Json(TransactionDetails::from_job(&job, queue_position)))
}

async fn cancel_transaction_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    require_auth(&state.auth_checker, &creds, Action::CancelJob)?;

    let job_id = id.clone();
    let lookup_id = job_id.clone();
    let job = run_db_query(&state, move |conn| DaemonJob::find_by_id(conn, &lookup_id)).await?;
    let job = match job {
        Some(job) => job,
        None => return Err(not_found_error("transaction", &id)),
    };
    ensure_job_visible(&creds, &job, &id)?;

    let queue_cancelled = state.cancel_job(&job_id).await;
    let update_id = job_id.clone();
    let db_cancelled =
        run_db_query(&state, move |conn| DaemonJob::cancel(conn, &update_id)).await?;

    if db_cancelled || queue_cancelled {
        state.emit(DaemonEvent::JobCancelled { job_id });
        Ok(StatusCode::NO_CONTENT)
    } else {
        let find_id = id.clone();
        let job = run_db_query(&state, move |conn| DaemonJob::find_by_id(conn, &find_id)).await?;

        match job {
            Some(j) => Err(ApiError(Box::new(DaemonError::conflict(&format!(
                "Transaction '{}' is already {}",
                id,
                j.status.as_str()
            ))))),
            None => Err(not_found_error("transaction", &id)),
        }
    }
}

async fn transaction_stream_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let job_id = id.clone();
    let rx = state.subscribe();

    let check_id = job_id.clone();
    let job = run_db_query(&state, move |conn| DaemonJob::find_by_id(conn, &check_id)).await?;

    let job = match job {
        Some(j) => j,
        None => return Err(not_found_error("transaction", &id)),
    };
    ensure_job_visible(&creds, &job, &id)?;

    let guard = acquire_sse_connection(&state)?;

    fn daemon_event_to_sse(event: &DaemonEvent) -> Option<Result<Event, Infallible>> {
        serde_json::to_string(event)
            .ok()
            .map(|json| Ok(Event::default().event(event.event_type_name()).data(json)))
    }

    let connected_data = serde_json::json!({
        "status": "connected",
        "job_id": &job.id,
        "current_status": job.status.as_str()
    });
    let connected_event = stream::once(async move {
        Ok(Event::default()
            .event("connected")
            .data(connected_data.to_string()))
    });

    let is_already_terminal = matches!(
        job.status,
        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled
    );

    let terminal_stream = if is_already_terminal {
        let terminal_event = match job.status {
            JobStatus::Completed => DaemonEvent::JobCompleted {
                job_id: job.id.clone(),
                duration_ms: 0,
            },
            JobStatus::Failed => DaemonEvent::JobFailed {
                job_id: job.id.clone(),
                error: job
                    .error
                    .unwrap_or_else(|| DaemonError::internal("Job failed (details unavailable)")),
            },
            JobStatus::Cancelled => DaemonEvent::JobCancelled {
                job_id: job.id.clone(),
            },
            JobStatus::Queued | JobStatus::Running => DaemonEvent::JobFailed {
                job_id: job.id.clone(),
                error: DaemonError::internal(
                    "Non-terminal job status reached terminal event synthesis",
                ),
            },
        };

        futures::future::Either::Left(stream::once(async move {
            daemon_event_to_sse(&terminal_event).unwrap_or_else(|| {
                Ok(Event::default()
                    .event("error")
                    .data(r#"{"error":"failed to serialize terminal event"}"#))
            })
        }))
    } else {
        futures::future::Either::Right(stream::empty())
    };

    let live_stream = if is_already_terminal {
        drop(guard);
        futures::future::Either::Left(stream::empty())
    } else {
        futures::future::Either::Right(JobSseStream {
            inner: BroadcastStream::new(rx),
            job_id: job_id.clone(),
            terminated: false,
            _guard: guard,
        })
    };

    let final_stream = connected_event.chain(terminal_stream).chain(live_stream);

    Ok(Sse::new(final_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    ))
}

struct JobSseStream {
    inner: BroadcastStream<DaemonEvent>,
    job_id: String,
    terminated: bool,
    _guard: SseConnectionGuard,
}

impl Stream for JobSseStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;

        let this = self.get_mut();

        if this.terminated {
            return Poll::Ready(None);
        }

        loop {
            match std::pin::Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => {
                    if event.job_id() != Some(this.job_id.as_str()) {
                        continue;
                    }
                    let is_term = matches!(
                        event,
                        DaemonEvent::JobCompleted { .. }
                            | DaemonEvent::JobFailed { .. }
                            | DaemonEvent::JobCancelled { .. }
                    );
                    if is_term {
                        this.terminated = true;
                    }
                    if let Ok(json) = serde_json::to_string(&event) {
                        return Poll::Ready(Some(Ok(Event::default()
                            .event(event.event_type_name())
                            .data(json))));
                    }
                    continue;
                }
                Poll::Ready(Some(Err(
                    tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n),
                ))) => {
                    tracing::warn!("SSE client (job {}) lagged {} events", this.job_id, n);
                    return Poll::Ready(Some(Ok(Event::default()
                        .event("warning")
                        .data(format!(r#"{{"lagged": {}}}"#, n)))));
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

async fn dry_run_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    Json(request): Json<CreateTransactionRequest>,
) -> ApiResult<Json<DryRunResponse>> {
    if request.operations.is_empty() {
        return Err(bad_request_error("At least one operation is required"));
    }

    validate_transaction_operations(&request.operations, true)?;
    require_auth_for_operations(&state, &creds, &request.operations)?;

    let mut install = Vec::new();
    let mut remove = Vec::new();
    let mut update = Vec::new();

    for op in &request.operations {
        match op {
            TransactionOperation::Install { packages, .. } => {
                install.extend(packages.iter().cloned());
            }
            TransactionOperation::Remove { packages, .. } => {
                remove.extend(packages.iter().cloned());
            }
            TransactionOperation::Update { packages, .. } => {
                update.extend(packages.iter().cloned());
            }
        }
    }

    let total_affected = install.len() + remove.len() + update.len();

    let response = DryRunResponse {
        operations: request.operations,
        summary: DryRunSummary {
            install,
            remove,
            update,
            total_affected,
        },
    };

    Ok(Json(response))
}

async fn install_packages_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    headers: HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    enqueue_transaction_request(
        state,
        creds,
        headers,
        CreateTransactionRequest {
            operations: vec![TransactionOperation::Install {
                packages: request.packages,
                allow_downgrade: request.options.allow_downgrade,
                skip_deps: request.options.skip_deps,
                dry_run: request.options.dry_run,
                no_scripts: request.options.no_scripts,
                yes: request.options.yes,
                apply_intent: request.options.apply_intent,
                allow_live_system_mutation: request.options.allow_live_system_mutation,
            }],
        },
    )
    .await
}

async fn remove_packages_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    headers: HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    enqueue_transaction_request(
        state,
        creds,
        headers,
        CreateTransactionRequest {
            operations: vec![TransactionOperation::Remove {
                packages: request.packages,
                cascade: request.options.cascade,
                remove_orphans: request.options.remove_orphans,
                no_scripts: request.options.no_scripts,
                purge_files: request.options.purge_files,
                apply_intent: request.options.apply_intent,
                allow_live_system_mutation: request.options.allow_live_system_mutation,
            }],
        },
    )
    .await
}

async fn update_packages_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    headers: HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    enqueue_transaction_request(
        state,
        creds,
        headers,
        CreateTransactionRequest {
            operations: vec![TransactionOperation::Update {
                packages: request.packages,
                security_only: request.options.security_only,
                dry_run: request.options.dry_run,
                yes: request.options.yes,
                apply_intent: request.options.apply_intent,
                allow_live_system_mutation: request.options.allow_live_system_mutation,
            }],
        },
    )
    .await
}

async fn enhance_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    headers: HeaderMap,
    Json(spec): Json<crate::daemon::EnhanceJobSpec>,
) -> TransactionResult {
    require_auth(&state.auth_checker, &creds, Action::Enhance)?;

    let job_spec = serde_json::to_value(&spec)
        .map_err(|e| internal_api_error("Failed to serialize daemon enhancement request", e))?;

    let idempotency_key = get_idempotency_key(&headers);
    if let Some(ref key) = idempotency_key {
        let key_clone = key.clone();
        let existing = run_db_query(&state, move |conn| {
            DaemonJob::find_by_idempotency_key(conn, &key_clone)
        })
        .await?;

        if let Some(existing_job) = existing {
            return idempotent_job_response(
                &state,
                existing_job,
                crate::daemon::JobKind::Enhance,
                &job_spec,
            )
            .await;
        }
    }

    let mut job = DaemonJob::new(crate::daemon::JobKind::Enhance, job_spec);
    if let Some(key) = idempotency_key {
        job.idempotency_key = Some(key);
    }
    let job_id = job.id.clone();

    if let Some(existing_job) = insert_or_dedup(&state, job.clone()).await? {
        return idempotent_job_response(
            &state,
            existing_job,
            crate::daemon::JobKind::Enhance,
            &job.spec,
        )
        .await;
    }

    let _cancel_token = state
        .queue
        .enqueue(job, crate::daemon::JobPriority::Normal)
        .await;

    let queue_position = state.queue.position(&job_id).await.unwrap_or(0);

    state.emit(DaemonEvent::JobQueued {
        job_id: job_id.clone(),
        position: queue_position,
    });

    state.metrics.jobs_total.fetch_add(1, Ordering::Relaxed);

    let location = format!("/v1/transactions/{}", job_id);
    let response = CreateTransactionResponse {
        job_id,
        status: "queued".to_string(),
        queue_position,
        location: location.clone(),
    };

    Ok((
        StatusCode::ACCEPTED,
        [(header::LOCATION, location)],
        Json(response),
    ))
}

fn get_idempotency_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

async fn idempotent_job_response(
    state: &SharedState,
    existing_job: DaemonJob,
    expected_kind: crate::daemon::JobKind,
    expected_spec: &serde_json::Value,
) -> TransactionResult {
    if existing_job.kind != expected_kind || &existing_job.spec != expected_spec {
        return Err(ApiError(Box::new(DaemonError::conflict(
            "Idempotency key is already associated with a different daemon job",
        ))));
    }

    let location = format!("/v1/transactions/{}", existing_job.id);
    let queue_position = state.queue.position(&existing_job.id).await.unwrap_or(0);
    let response = CreateTransactionResponse {
        job_id: existing_job.id.clone(),
        status: existing_job.status.as_str().to_string(),
        queue_position,
        location: location.clone(),
    };

    Ok((
        StatusCode::OK,
        [(header::LOCATION, location)],
        Json(response),
    ))
}

async fn insert_or_dedup(
    state: &SharedState,
    job: DaemonJob,
) -> Result<Option<DaemonJob>, ApiError> {
    let state = state.clone();
    let job_clone = job.clone();
    tokio::task::spawn_blocking(move || {
        let conn = state
            .open_db()
            .map_err(|e| Box::new(internal_error_with("Failed to open daemon database", e)))?;
        match job_clone.insert(&conn) {
            Ok(()) => Ok(None),
            Err(conary_core::Error::Database(ref db_err))
                if db_err.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
            {
                if let Some(ref key) = job.idempotency_key {
                    match DaemonJob::find_by_idempotency_key(&conn, key) {
                        Ok(Some(existing)) => Ok(Some(existing)),
                        _ => Err(Box::new(internal_error(
                            "Idempotency conflict but existing job not found",
                        ))),
                    }
                } else {
                    Err(Box::new(internal_error_with(
                        "Daemon job insert failed without idempotency key",
                        db_err,
                    )))
                }
            }
            Err(e) => Err(Box::new(internal_error_with("Daemon job insert failed", e))),
        }
    })
    .await
    .map_err(|e| internal_api_error("Daemon job insert task join failed", e))?
    .map_err(ApiError)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        body_json, create_test_state, current_process_creds, test_router,
    };
    use crate::daemon::auth::PeerCredentials;
    use crate::daemon::{DaemonJob, JobStatus};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_handler_list_transactions_empty() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/transactions")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_handler_get_transaction_not_found() {
        let (state, _dir) = create_test_state();
        let app = test_router(state, current_process_creds());

        let request = Request::builder()
            .uri("/v1/transactions/nonexistent-job-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let json = body_json(response).await;
        assert_eq!(json["status"], 404);
        assert!(
            json["detail"]
                .as_str()
                .unwrap()
                .contains("nonexistent-job-id")
        );
    }

    #[tokio::test]
    async fn test_handler_create_transaction_queues_package_jobs() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state.clone(), root_creds);

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx"]
                }
            ]
        });

        let request = Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let json = body_json(response).await;
        assert_eq!(json["status"], "queued");
        assert_eq!(json["queue_position"], 0);
        let job_id = json["job_id"].as_str().unwrap();
        assert_eq!(json["location"], format!("/v1/transactions/{job_id}"));

        let job = {
            let conn = state.open_db().unwrap();
            DaemonJob::find_by_id(&conn, job_id).unwrap().unwrap()
        };
        assert_eq!(job.kind, crate::daemon::JobKind::Install);
        assert_eq!(job.status, JobStatus::Queued);
        assert_eq!(
            job.spec,
            serde_json::json!([
                {
                    "type": "install",
                    "packages": ["nginx"],
                    "allow_downgrade": false,
                    "skip_deps": false
                }
            ])
        );
        assert_eq!(
            job.requested_by_uid,
            current_process_creds().map(|creds| creds.uid)
        );
    }

    #[tokio::test]
    async fn test_handler_create_transaction_empty_operations() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state.clone(), root_creds);

        let body = serde_json::json!({
            "operations": []
        });

        let request = Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let json = body_json(response).await;
        assert_eq!(json["status"], 400);
        assert!(json["detail"].as_str().unwrap().contains("operation"));
    }

    #[tokio::test]
    async fn test_handler_create_transaction_rejects_mixed_package_kinds() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx"]
                },
                {
                    "type": "remove",
                    "packages": ["vim"]
                }
            ]
        });

        let request = Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let json = body_json(response).await;
        assert!(
            json["detail"]
                .as_str()
                .unwrap()
                .contains("one package operation kind")
        );
    }

    #[tokio::test]
    async fn test_handler_create_transaction_invalid_json() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from("not valid json"))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // Axum returns 400 Bad Request for JSON deserialization failures
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_handler_create_transaction_forbidden() {
        let (state, _dir) = create_test_state();
        // No credentials (simulates TCP connection)
        let app = test_router(state, None);

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx"]
                }
            ]
        });

        let request = Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let json = body_json(response).await;
        assert_eq!(json["status"], 403);
    }

    #[tokio::test]
    async fn test_handler_create_transaction_idempotency() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["curl"]
                }
            ]
        });
        let body_str = serde_json::to_string(&body).unwrap();

        let app1 = test_router(state.clone(), root_creds);
        let request1 = Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .header("x-idempotency-key", "idem-key-42")
            .body(Body::from(body_str.clone()))
            .unwrap();

        let response1 = app1.oneshot(request1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);
        let json1 = body_json(response1).await;
        assert_eq!(json1["status"], "queued");

        let app2 = test_router(state, root_creds);
        let request2 = Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .header("x-idempotency-key", "idem-key-42")
            .body(Body::from(body_str))
            .unwrap();

        let response2 = app2.oneshot(request2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
        let json2 = body_json(response2).await;
        assert_eq!(json2["status"], "queued");
        assert_eq!(json2["job_id"], json1["job_id"]);
        assert_eq!(json2["location"], json1["location"]);
    }

    #[tokio::test]
    async fn test_handler_enhance_idempotency() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let idempotency_key = "enhance-key-42";

        let body = serde_json::json!({
            "batch_size": 1,
            "trove_ids": [],
            "types": [],
            "force": false
        });
        let body_str = serde_json::to_string(&body).unwrap();

        let request1 = Request::builder()
            .method("POST")
            .uri("/v1/enhance")
            .header("content-type", "application/json")
            .header("x-idempotency-key", idempotency_key)
            .body(Body::from(body_str.clone()))
            .unwrap();

        let response1 = test_router(state.clone(), root_creds)
            .oneshot(request1)
            .await
            .unwrap();
        assert_eq!(response1.status(), StatusCode::ACCEPTED);
        let json1 = body_json(response1).await;

        let request2 = Request::builder()
            .method("POST")
            .uri("/v1/enhance")
            .header("content-type", "application/json")
            .header("x-idempotency-key", idempotency_key)
            .body(Body::from(body_str))
            .unwrap();

        let response2 = test_router(state, root_creds)
            .oneshot(request2)
            .await
            .unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
        let json2 = body_json(response2).await;
        assert_eq!(json2["status"], "queued");
        assert_eq!(json2["job_id"], json1["job_id"]);
        assert_eq!(json2["location"], json1["location"]);
    }

    #[tokio::test]
    async fn test_handler_create_transaction_rejects_existing_enhance_idempotency_key() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let idempotency_key = "shared-enhance-key";

        let enhance_body = serde_json::json!({
            "batch_size": 1,
            "trove_ids": [],
            "types": [],
            "force": false
        });
        let enhance_request = Request::builder()
            .method("POST")
            .uri("/v1/enhance")
            .header("content-type", "application/json")
            .header("x-idempotency-key", idempotency_key)
            .body(Body::from(serde_json::to_string(&enhance_body).unwrap()))
            .unwrap();

        let enhance_response = test_router(state.clone(), root_creds)
            .oneshot(enhance_request)
            .await
            .unwrap();
        assert_eq!(enhance_response.status(), StatusCode::ACCEPTED);

        let package_body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["curl"]
                }
            ]
        });
        let package_request = Request::builder()
            .method("POST")
            .uri("/v1/transactions")
            .header("content-type", "application/json")
            .header("x-idempotency-key", idempotency_key)
            .body(Body::from(serde_json::to_string(&package_body).unwrap()))
            .unwrap();

        let package_response = test_router(state, root_creds)
            .oneshot(package_request)
            .await
            .unwrap();

        assert_eq!(package_response.status(), StatusCode::CONFLICT);
        let json = body_json(package_response).await;
        assert_eq!(json["status"], 409);
        assert!(json["detail"].as_str().unwrap().contains("Idempotency key"));
    }

    #[tokio::test]
    async fn test_handler_get_transaction_after_creation() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();

        // Insert a job directly (transaction API rejects unsupported kinds)
        let job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 5}),
        )
        .with_uid(nix::unistd::geteuid().as_raw());
        let job_id = job.id.clone();
        {
            let conn = state.open_db().unwrap();
            job.insert(&conn).unwrap();
        }

        let app = test_router(state, root_creds);
        let get_req = Request::builder()
            .uri(format!("/v1/transactions/{}", job_id))
            .body(Body::empty())
            .unwrap();

        let get_resp = app.oneshot(get_req).await.unwrap();
        assert_eq!(get_resp.status(), StatusCode::OK);

        let details = body_json(get_resp).await;
        assert_eq!(details["id"].as_str().unwrap(), job_id);
        assert_eq!(details["kind"], "enhance");
        assert_eq!(details["status"], "queued");
    }

    #[tokio::test]
    async fn test_handler_list_transactions_with_status_filter() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();

        // Insert a job directly (transaction API rejects unsupported kinds)
        let job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 5}),
        )
        .with_uid(nix::unistd::geteuid().as_raw());
        {
            let conn = state.open_db().unwrap();
            job.insert(&conn).unwrap();
        }

        // List queued transactions
        let app2 = test_router(state.clone(), root_creds);
        let list_req = Request::builder()
            .uri("/v1/transactions?status=queued")
            .body(Body::empty())
            .unwrap();

        let list_resp = app2.oneshot(list_req).await.unwrap();
        assert_eq!(list_resp.status(), StatusCode::OK);

        let json = body_json(list_resp).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["status"], "queued");

        // List completed (should be empty)
        let app3 = test_router(state, root_creds);
        let list_req2 = Request::builder()
            .uri("/v1/transactions?status=completed")
            .body(Body::empty())
            .unwrap();

        let list_resp2 = app3.oneshot(list_req2).await.unwrap();
        assert_eq!(list_resp2.status(), StatusCode::OK);

        let json2 = body_json(list_resp2).await;
        assert!(json2.is_array());
        assert_eq!(json2.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_handler_list_transactions_filters_by_requesting_uid() {
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

        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: std::process::id(),
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = Request::builder()
            .uri("/v1/transactions")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 1);
        assert_eq!(json[0]["id"], visible_job.id);
    }

    #[tokio::test]
    async fn test_handler_get_transaction_hides_foreign_job() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);
        let hidden_job_id = hidden_job.id.clone();

        {
            let conn = state.open_db().unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: std::process::id(),
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = Request::builder()
            .uri(format!("/v1/transactions/{}", hidden_job_id))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handler_transaction_stream_hides_foreign_job() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);
        let hidden_job_id = hidden_job.id.clone();

        {
            let conn = state.open_db().unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: std::process::id(),
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = Request::builder()
            .uri(format!("/v1/transactions/{}/stream", hidden_job_id))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handler_cancel_transaction_not_found() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let request = Request::builder()
            .method("DELETE")
            .uri("/v1/transactions/nonexistent-id")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_handler_cancel_transaction_hides_foreign_job() {
        let (state, _dir) = create_test_state();
        let daemon_uid = nix::unistd::geteuid().as_raw();
        let other_uid = if daemon_uid == 42_424 { 42_425 } else { 42_424 };

        let hidden_job = DaemonJob::new(
            crate::daemon::JobKind::Enhance,
            serde_json::json!({"batch_size": 7}),
        )
        .with_uid(other_uid);
        let hidden_job_id = hidden_job.id.clone();

        {
            let conn = state.open_db().unwrap();
            hidden_job.insert(&conn).unwrap();
        }

        let app = test_router(
            state,
            Some(PeerCredentials {
                pid: std::process::id(),
                uid: daemon_uid,
                gid: daemon_uid,
            }),
        );

        let request = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/transactions/{}", hidden_job_id))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_package_routes_queue_package_jobs() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state.clone(), root_creds);

        for (expected_position, (path, operation, expected_kind)) in [
            (
                "/v1/packages/install",
                "install",
                crate::daemon::JobKind::Install,
            ),
            (
                "/v1/packages/remove",
                "remove",
                crate::daemon::JobKind::Remove,
            ),
            (
                "/v1/packages/update",
                "update",
                crate::daemon::JobKind::Update,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let body = serde_json::json!({
                "packages": ["demo"],
                "options": {}
            });
            let request = Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();

            let response = app.clone().oneshot(request).await.unwrap();

            assert_eq!(response.status(), StatusCode::ACCEPTED);

            let json = body_json(response).await;
            assert_eq!(json["status"], "queued");
            assert_eq!(json["queue_position"], expected_position);
            let job_id = json["job_id"].as_str().unwrap();

            let job = {
                let conn = state.open_db().unwrap();
                DaemonJob::find_by_id(&conn, job_id).unwrap().unwrap()
            };
            assert_eq!(
                job.kind, expected_kind,
                "{operation} route queued wrong kind"
            );
            assert_eq!(job.spec[0]["type"], operation);
            assert_eq!(job.spec[0]["packages"], serde_json::json!(["demo"]));
        }
    }

    #[tokio::test]
    async fn test_handler_dry_run_returns_package_summary() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "operations": [
                {
                    "type": "install",
                    "packages": ["nginx", "curl"]
                },
                {
                    "type": "remove",
                    "packages": ["vim"]
                }
            ]
        });

        let request = Request::builder()
            .method("POST")
            .uri("/v1/transactions/dry-run")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let json = body_json(response).await;
        assert_eq!(
            json["operations"],
            serde_json::json!([
                {
                    "type": "install",
                    "packages": ["nginx", "curl"],
                    "allow_downgrade": false,
                    "skip_deps": false
                },
                {
                    "type": "remove",
                    "packages": ["vim"],
                    "cascade": false,
                    "remove_orphans": false
                }
            ])
        );
        assert_eq!(
            json["summary"]["install"],
            serde_json::json!(["nginx", "curl"])
        );
        assert_eq!(json["summary"]["remove"], serde_json::json!(["vim"]));
        assert_eq!(json["summary"]["update"], serde_json::json!([]));
        assert_eq!(json["summary"]["total_affected"], 3);
    }

    #[tokio::test]
    async fn test_handler_dry_run_empty_operations() {
        let (state, _dir) = create_test_state();
        let root_creds = current_process_creds();
        let app = test_router(state, root_creds);

        let body = serde_json::json!({
            "operations": []
        });

        let request = Request::builder()
            .method("POST")
            .uri("/v1/transactions/dry-run")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
