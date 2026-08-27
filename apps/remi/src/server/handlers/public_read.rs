// apps/remi/src/server/handlers/public_read.rs
//! HTTP adaptation for the signed public-universe read authority.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tokio::sync::RwLock;

use crate::server::ServerState;
use crate::server::catalog_authority::{CatalogAuthority, ProfileRevisionSelection};
use crate::server::public_universe::PublicUniverseSnapshot;

use super::HandlerResult;

pub(crate) const UNIVERSE_REVISION_HEADER: HeaderName =
    HeaderName::from_static("x-conary-universe-revision");
pub(crate) const UNIVERSE_SEQUENCE_HEADER: HeaderName =
    HeaderName::from_static("x-conary-universe-sequence");
pub(crate) const PROFILE_REVISION_HEADER: HeaderName =
    HeaderName::from_static("x-conary-profile-revision");
const ERROR_HEADER: HeaderName = HeaderName::from_static("x-conary-error");

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PublicUniverseUnavailableReason {
    NoActiveUniverse,
    ProfileNotInUniverse,
    AuthorityUnavailable,
    SearchIndexUnavailable,
    SearchIndexRevisionMismatch,
}

#[derive(Serialize)]
struct PublicUniverseUnavailable<'a> {
    code: &'static str,
    reason: PublicUniverseUnavailableReason,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<&'a str>,
    retry_after_seconds: u32,
}

#[derive(Clone)]
pub(crate) struct PublicReadContext {
    pub(crate) db_path: PathBuf,
    pub(crate) catalog_authority: CatalogAuthority,
    pub(crate) universe: PublicUniverseSnapshot,
}

impl PublicReadContext {
    pub(crate) fn profile_for_route(&self, route: &str) -> HandlerResult<ProfileRevisionSelection> {
        let source_profile =
            conary_core::repository::supported_profiles::profile_for_remi_route(route)
                .expect("validated public route has one exact source profile");
        self.universe
            .profile(source_profile.id())
            .cloned()
            .ok_or_else(|| {
                unavailable_response(
                    PublicUniverseUnavailableReason::ProfileNotInUniverse,
                    Some(source_profile.id()),
                )
                .into()
            })
    }
}

pub(crate) async fn context(state: &Arc<RwLock<ServerState>>) -> HandlerResult<PublicReadContext> {
    let (db_path, catalog_authority) = {
        let guard = state.read().await;
        (
            guard.config.db_path.clone(),
            guard.catalog_authority.clone(),
        )
    };
    let lookup_path = db_path.clone();
    let loaded =
        tokio::task::spawn_blocking(move || PublicUniverseSnapshot::load(&lookup_path)).await;
    let universe = match loaded {
        Ok(Ok(Some(universe))) => universe,
        Ok(Ok(None)) => {
            return Err(unavailable_response(
                PublicUniverseUnavailableReason::NoActiveUniverse,
                None,
            )
            .into());
        }
        Ok(Err(error)) => {
            tracing::error!(%error, "public universe authority lookup failed");
            return Err(unavailable_response(
                PublicUniverseUnavailableReason::AuthorityUnavailable,
                None,
            )
            .into());
        }
        Err(error) => {
            tracing::error!(%error, "public universe authority task failed");
            return Err(unavailable_response(
                PublicUniverseUnavailableReason::AuthorityUnavailable,
                None,
            )
            .into());
        }
    };
    Ok(PublicReadContext {
        db_path,
        catalog_authority,
        universe,
    })
}

pub(crate) async fn run<T, F>(context: &'static str, task: F) -> HandlerResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            tracing::error!(%error, "public read failed in {context}");
            Err(
                unavailable_response(PublicUniverseUnavailableReason::AuthorityUnavailable, None)
                    .into(),
            )
        }
        Err(error) => {
            tracing::error!(%error, "public read task failed in {context}");
            Err(
                unavailable_response(PublicUniverseUnavailableReason::AuthorityUnavailable, None)
                    .into(),
            )
        }
    }
}

pub(crate) fn unavailable_response(
    reason: PublicUniverseUnavailableReason,
    profile: Option<&str>,
) -> Response {
    let message = match reason {
        PublicUniverseUnavailableReason::NoActiveUniverse => {
            "no signed public package universe is active"
        }
        PublicUniverseUnavailableReason::ProfileNotInUniverse => {
            "the requested profile is absent from the active public universe"
        }
        PublicUniverseUnavailableReason::AuthorityUnavailable => {
            "the signed public package authority could not be established"
        }
        PublicUniverseUnavailableReason::SearchIndexUnavailable => {
            "the public search projection is not available"
        }
        PublicUniverseUnavailableReason::SearchIndexRevisionMismatch => {
            "the public search projection does not match the active universe"
        }
    };
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(PublicUniverseUnavailable {
            code: "PUBLIC_UNIVERSE_UNAVAILABLE",
            reason,
            message,
            profile,
            retry_after_seconds: 30,
        }),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        ERROR_HEADER,
        HeaderValue::from_static("PUBLIC_UNIVERSE_UNAVAILABLE"),
    );
    response
}

pub(crate) fn stamp(
    mut response: Response,
    universe: &PublicUniverseSnapshot,
    profile: Option<&ProfileRevisionSelection>,
) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        UNIVERSE_REVISION_HEADER,
        HeaderValue::from_str(&universe.identity().manifest_sha256)
            .expect("validated universe digest is an HTTP header value"),
    );
    headers.insert(
        UNIVERSE_SEQUENCE_HEADER,
        HeaderValue::from_str(&universe.identity().sequence.to_string())
            .expect("universe sequence is an HTTP header value"),
    );
    if let Some(profile) = profile {
        headers.insert(
            PROFILE_REVISION_HEADER,
            HeaderValue::from_str(&profile.profile_revision_sha256)
                .expect("validated profile digest is an HTTP header value"),
        );
    }
    response
}

#[cfg(test)]
mod tests;
