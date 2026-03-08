// conary-server/src/server/rate_limit.rs
//! Rate limiting middleware for the external admin API.
//!
//! Three separate token buckets per source IP:
//! - Read (GET): default 60/min
//! - Write (POST/PUT/DELETE): default 10/min
//! - Auth failure: default 5/min (applied in auth middleware on 401)

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::Clock;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::server::ServerState;

/// Rate limiter set for the admin API.
pub struct AdminRateLimiters {
    /// Rate limiter for read operations (GET/HEAD)
    pub read: Arc<DefaultKeyedRateLimiter<IpAddr>>,
    /// Rate limiter for write operations (POST/PUT/DELETE)
    pub write: Arc<DefaultKeyedRateLimiter<IpAddr>>,
    /// Rate limiter for auth failures (applied separately)
    pub auth_fail: Arc<DefaultKeyedRateLimiter<IpAddr>>,
}

impl AdminRateLimiters {
    /// Create a new set of rate limiters with the given per-minute limits.
    pub fn new(read_rpm: u32, write_rpm: u32, auth_fail_rpm: u32) -> Self {
        Self {
            read: Arc::new(Self::make_limiter(read_rpm)),
            write: Arc::new(Self::make_limiter(write_rpm)),
            auth_fail: Arc::new(Self::make_limiter(auth_fail_rpm)),
        }
    }

    fn make_limiter(rpm: u32) -> DefaultKeyedRateLimiter<IpAddr> {
        let quota = Quota::per_minute(NonZeroU32::new(rpm).unwrap_or(NonZeroU32::new(1).unwrap()));
        RateLimiter::keyed(quota)
    }
}

/// Extract client IP from the request.
///
/// Tries `ConnectInfo` first, falls back to 127.0.0.1.
pub(crate) fn extract_ip(request: &Request<Body>) -> IpAddr {
    request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or_else(|| IpAddr::from([127, 0, 0, 1]))
}

/// Rate limiting middleware for the external admin API.
///
/// Checks the read or write bucket depending on the HTTP method.
/// Returns 429 with Retry-After header if the limit is exceeded.
///
/// Extracts `AdminRateLimiters` from `ServerState` so the middleware can
/// share the router's state type (`Arc<RwLock<ServerState>>`).
pub async fn rate_limit_middleware(
    State(state): State<Arc<RwLock<ServerState>>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let limiters = {
        let s = state.read().await;
        s.rate_limiters.clone()
    };

    let Some(limiters) = limiters else {
        return next.run(request).await;
    };

    let ip = extract_ip(&request);
    let method = request.method().clone();

    let limiter = if method == Method::GET || method == Method::HEAD {
        &limiters.read
    } else {
        &limiters.write
    };

    if let Err(not_until) = limiter.check_key(&ip) {
        let wait_secs = not_until
            .wait_time_from(governor::clock::DefaultClock::default().now())
            .as_secs()
            .max(1);
        let retry_after = wait_secs.to_string();
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [
                ("content-type", "application/json"),
                ("retry-after", retry_after.as_str()),
            ],
            r#"{"error":"Rate limit exceeded","code":"RATE_LIMITED"}"#,
        )
            .into_response();
    }

    next.run(request).await
}

/// Record an auth failure for rate limiting purposes.
///
/// Called from the auth middleware when a 401 is returned.
/// Returns true if the auth failure rate limit is also exceeded
/// (in which case the caller should return 429 instead of 401).
pub fn check_auth_failure(limiters: &AdminRateLimiters, ip: IpAddr) -> bool {
    limiters.auth_fail.check_key(&ip).is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limiter_creation() {
        let limiters = AdminRateLimiters::new(60, 10, 5);
        let ip = IpAddr::from([1, 2, 3, 4]);

        // First request should pass
        assert!(limiters.read.check_key(&ip).is_ok());
        assert!(limiters.write.check_key(&ip).is_ok());
    }

    #[test]
    fn test_write_limiter_exhaustion() {
        // 2 per minute -- should exhaust quickly
        let limiters = AdminRateLimiters::new(60, 2, 5);
        let ip = IpAddr::from([10, 0, 0, 1]);

        // First 2 should pass
        assert!(limiters.write.check_key(&ip).is_ok());
        assert!(limiters.write.check_key(&ip).is_ok());

        // Third should be rate limited
        assert!(limiters.write.check_key(&ip).is_err());
    }

    #[test]
    fn test_different_ips_independent() {
        let limiters = AdminRateLimiters::new(60, 1, 5);
        let ip1 = IpAddr::from([10, 0, 0, 1]);
        let ip2 = IpAddr::from([10, 0, 0, 2]);

        // Exhaust ip1's write limit
        assert!(limiters.write.check_key(&ip1).is_ok());
        assert!(limiters.write.check_key(&ip1).is_err());

        // ip2 should still be fine
        assert!(limiters.write.check_key(&ip2).is_ok());
    }

    #[test]
    fn test_auth_failure_check() {
        let limiters = AdminRateLimiters::new(60, 10, 2);
        let ip = IpAddr::from([192, 168, 1, 1]);

        // First 2 auth failures OK
        assert!(!check_auth_failure(&limiters, ip));
        assert!(!check_auth_failure(&limiters, ip));

        // Third should trigger rate limit
        assert!(check_auth_failure(&limiters, ip));
    }
}
