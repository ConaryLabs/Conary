// apps/conaryd/src/daemon/routes/sse.rs
//! SSE connection-limit guarding for daemon routes.

use super::errors::ApiError;
use super::types::SharedState;
use crate::daemon::{DaemonError, DaemonState};
use std::sync::Arc;
use std::sync::atomic::Ordering;

const MAX_DAEMON_SSE_CONNECTIONS: u64 = 64;

/// RAII guard that decrements the SSE connection counter on drop
pub(super) struct SseConnectionGuard {
    metrics: Arc<DaemonState>,
}

impl Drop for SseConnectionGuard {
    fn drop(&mut self) {
        self.metrics
            .metrics
            .sse_connections
            .fetch_sub(1, Ordering::Relaxed);
    }
}

pub(super) fn acquire_sse_connection(state: &SharedState) -> Result<SseConnectionGuard, ApiError> {
    let result = state.metrics.sse_connections.fetch_update(
        Ordering::AcqRel,
        Ordering::Relaxed,
        |current| {
            if current < MAX_DAEMON_SSE_CONNECTIONS {
                Some(current + 1)
            } else {
                None
            }
        },
    );

    if result.is_err() {
        return Err(ApiError(Box::new(DaemonError::new(
            "too_many_connections",
            "Too Many Connections",
            503,
            "Too many concurrent SSE connections",
        ))));
    }

    Ok(SseConnectionGuard {
        metrics: state.clone(),
    })
}
