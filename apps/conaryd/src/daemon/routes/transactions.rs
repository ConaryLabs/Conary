// apps/conaryd/src/daemon/routes/transactions.rs
//! Daemon job creation, transaction, and per-job streaming routes.

use super::*;

type TransactionResult = ApiResult<(
    StatusCode,
    [(axum::http::header::HeaderName, String); 1],
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
    headers: axum::http::HeaderMap,
    Json(request): Json<CreateTransactionRequest>,
) -> TransactionResult {
    require_auth(
        &state.auth_checker,
        &creds,
        action_for_job_kind(determine_job_kind(&request.operations)),
    )?;

    if request.operations.is_empty() {
        return Err(bad_request_error("At least one operation is required"));
    }

    let idempotency_key = get_idempotency_key(&headers);

    if let Some(ref key) = idempotency_key {
        let key_clone = key.clone();
        let existing = run_db_query(&state, move |conn| {
            DaemonJob::find_by_idempotency_key(conn, &key_clone)
        })
        .await?;

        if let Some(existing_job) = existing {
            let location = format!("/v1/transactions/{}", existing_job.id);
            let queue_position = state.queue.position(&existing_job.id).await.unwrap_or(0);

            let response = CreateTransactionResponse {
                job_id: existing_job.id,
                status: existing_job.status.as_str().to_string(),
                queue_position,
                location: location.clone(),
            };

            return Ok((
                StatusCode::OK,
                [(axum::http::header::LOCATION, location)],
                Json(response),
            ));
        }
    }

    let job_kind = determine_job_kind(&request.operations);

    if !matches!(job_kind, crate::daemon::JobKind::Enhance) {
        return Err(ApiError(Box::new(DaemonError::bad_request(&format!(
            "Job kind '{}' is not yet supported by the daemon. \
             Use the CLI directly for install/remove/update operations.",
            job_kind.as_str()
        )))));
    }

    let spec = serde_json::to_value(&request.operations)
        .map_err(|e| internal_api_error("Failed to serialize daemon transaction request", e))?;

    let mut job = DaemonJob::new(job_kind, spec);
    if let Some(key) = idempotency_key {
        job.idempotency_key = Some(key);
    }

    let job_id = job.id.clone();

    if let Some(existing_job) = insert_or_dedup(&state, job.clone()).await? {
        let location = format!("/v1/transactions/{}", existing_job.id);
        let queue_position = state.queue.position(&existing_job.id).await.unwrap_or(0);
        let response = CreateTransactionResponse {
            job_id: existing_job.id,
            status: existing_job.status.as_str().to_string(),
            queue_position,
            location: location.clone(),
        };
        return Ok((
            StatusCode::OK,
            [(axum::http::header::LOCATION, location)],
            Json(response),
        ));
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
        [(axum::http::header::LOCATION, location)],
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
    require_auth(
        &state.auth_checker,
        &creds,
        action_for_job_kind(determine_job_kind(&request.operations)),
    )?;

    if request.operations.is_empty() {
        return Err(bad_request_error("At least one operation is required"));
    }

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

async fn forward_package_operation(
    state: State<SharedState>,
    creds: Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    request: PackageOperationRequest,
    require_packages: bool,
    to_operation: impl FnOnce(PackageOperationRequest) -> TransactionOperation,
) -> TransactionResult {
    if require_packages && request.packages.is_empty() {
        return Err(bad_request_error("At least one package name is required"));
    }

    let tx_request = CreateTransactionRequest {
        operations: vec![to_operation(request)],
    };

    create_transaction_handler(state, creds, headers, Json(tx_request)).await
}

async fn install_packages_handler(
    state: State<SharedState>,
    creds: Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    forward_package_operation(state, creds, headers, request, true, |r| {
        TransactionOperation::Install {
            packages: r.packages,
            allow_downgrade: r.options.allow_downgrade,
            skip_deps: r.options.skip_deps,
        }
    })
    .await
}

async fn remove_packages_handler(
    state: State<SharedState>,
    creds: Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    forward_package_operation(state, creds, headers, request, true, |r| {
        TransactionOperation::Remove {
            packages: r.packages,
            cascade: r.options.cascade,
            remove_orphans: r.options.remove_orphans,
        }
    })
    .await
}

async fn update_packages_handler(
    state: State<SharedState>,
    creds: Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<PackageOperationRequest>,
) -> TransactionResult {
    forward_package_operation(state, creds, headers, request, false, |r| {
        TransactionOperation::Update {
            packages: r.packages,
            security_only: r.options.security_only,
        }
    })
    .await
}

async fn enhance_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
    headers: axum::http::HeaderMap,
    Json(spec): Json<crate::daemon::EnhanceJobSpec>,
) -> TransactionResult {
    require_auth(&state.auth_checker, &creds, Action::Enhance)?;

    let idempotency_key = get_idempotency_key(&headers);
    if let Some(ref key) = idempotency_key {
        let key_clone = key.clone();
        let existing = run_db_query(&state, move |conn| {
            DaemonJob::find_by_idempotency_key(conn, &key_clone)
        })
        .await?;

        if let Some(existing_job) = existing {
            let location = format!("/v1/transactions/{}", existing_job.id);
            let queue_position = state.queue.position(&existing_job.id).await.unwrap_or(0);
            let response = CreateTransactionResponse {
                job_id: existing_job.id,
                status: existing_job.status.as_str().to_string(),
                queue_position,
                location: location.clone(),
            };
            return Ok((
                StatusCode::OK,
                [(axum::http::header::LOCATION, location)],
                Json(response),
            ));
        }
    }

    let job_spec = serde_json::to_value(&spec)
        .map_err(|e| internal_api_error("Failed to serialize daemon enhancement request", e))?;

    let mut job = DaemonJob::new(crate::daemon::JobKind::Enhance, job_spec);
    if let Some(key) = idempotency_key {
        job.idempotency_key = Some(key);
    }
    let job_id = job.id.clone();

    if let Some(existing_job) = insert_or_dedup(&state, job.clone()).await? {
        let location = format!("/v1/transactions/{}", existing_job.id);
        let queue_position = state.queue.position(&existing_job.id).await.unwrap_or(0);
        let response = CreateTransactionResponse {
            job_id: existing_job.id,
            status: existing_job.status.as_str().to_string(),
            queue_position,
            location: location.clone(),
        };
        return Ok((
            StatusCode::OK,
            [(axum::http::header::LOCATION, location)],
            Json(response),
        ));
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
        [(axum::http::header::LOCATION, location)],
        Json(response),
    ))
}

fn get_idempotency_key(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
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
