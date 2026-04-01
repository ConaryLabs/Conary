// apps/remi/src/server/handlers/admin/events.rs
//! SSE event stream handler

use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::sync::{Arc, LazyLock};
use tokio::sync::{RwLock, Semaphore};

use crate::server::ServerState;
use crate::server::auth::{TokenScopes, json_error};

/// Maximum number of concurrent SSE connections.
///
/// Prevents resource exhaustion from too many long-lived connections.
/// Each SSE connection holds a broadcast receiver and a keep-alive timer.
static SSE_SEMAPHORE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(100));

#[derive(Deserialize)]
pub struct EventsQuery {
    pub filter: Option<String>,
}

pub async fn sse_events(
    State(state): State<Arc<RwLock<ServerState>>>,
    scopes: Option<axum::Extension<TokenScopes>>,
    Query(query): Query<EventsQuery>,
) -> Response {
    // Any valid token can subscribe
    if scopes.is_none() {
        return json_error(401, "Not authenticated", "UNAUTHORIZED");
    }

    // Limit concurrent SSE connections to prevent resource exhaustion
    let _permit = match SSE_SEMAPHORE.try_acquire() {
        Ok(permit) => permit,
        Err(_) => {
            return json_error(
                503,
                "Too many concurrent SSE connections",
                "SSE_LIMIT_REACHED",
            );
        }
    };

    let filters: Option<Vec<String>> = query
        .filter
        .map(|f| f.split(',').map(|s| s.trim().to_string()).collect());

    let rx = {
        let s = state.read().await;
        s.admin_events.subscribe()
    };

    let stream = async_stream::stream! {
        // Hold the semaphore permit for the lifetime of the stream.
        // When the client disconnects, the permit is dropped automatically.
        let _permit = _permit;
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(ref filters) = filters
                        && !filters.iter().any(|f| event.event_type.starts_with(f.as_str()))
                    {
                        continue;
                    }
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok::<_, std::convert::Infallible>(
                        axum::response::sse::Event::default()
                            .event(&event.event_type)
                            .data(data)
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("SSE client lagged by {} events", n);
                    yield Ok(
                        axum::response::sse::Event::default()
                            .event("error")
                            .data(format!(r#"{{"error":"Lagged by {n} events","code":"LAGGED"}}"#))
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    axum::response::sse::Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(std::time::Duration::from_secs(30))
                .text("ping"),
        )
        .into_response()
}
