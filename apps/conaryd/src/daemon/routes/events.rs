// apps/conaryd/src/daemon/routes/events.rs
//! Daemon event stream routes.

use super::*;

pub(super) fn router() -> Router<SharedState> {
    Router::new().route("/events", get(events_handler))
}

async fn events_handler(
    State(state): State<SharedState>,
    Extension(creds): Extension<Option<PeerCredentials>>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let guard = acquire_sse_connection(&state)?;
    let rx = state.subscribe();

    let mut visibility_cache = HashMap::new();
    let event_stream = BroadcastStream::new(rx).filter_map(move |result| match result {
        Ok(event) => {
            if !event_visible_to_requester(&state, &creds, &mut visibility_cache, &event) {
                return None;
            }

            match serde_json::to_string(&event) {
                Ok(json) => Some(Ok(Event::default()
                    .event(event.event_type_name())
                    .data(json))),
                Err(_) => None,
            }
        }
        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
            tracing::warn!("SSE client lagged {} events", n);
            Some(Ok(Event::default()
                .event("warning")
                .data(format!(r#"{{"lagged": {}}}"#, n))))
        }
    });

    let connected_event = stream::once(async {
        Ok(Event::default()
            .event("connected")
            .data(r#"{"status": "connected"}"#))
    });

    let guard_stream = futures::stream::once(async move {
        let _guard = guard;
        futures::future::pending::<Result<Event, Infallible>>().await
    });

    let stream = connected_event.chain(event_stream).chain(guard_stream);

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    ))
}
