// apps/remi/src/server/rate_limit.rs
//! Rate limiting middleware for the external admin API.
//!
//! Three separate token buckets per source IP:
//! - Read (GET): default 60/min
//! - Write (POST/PUT/DELETE): default 10/min
//! - Auth failure: default 5/min (applied in auth middleware on 401)

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::Clock;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

/// Rate limiter set for the admin API.
pub struct AdminRateLimiters {
    /// Rate limiter for read operations (GET/HEAD)
    pub read: Arc<DefaultKeyedRateLimiter<IpAddr>>,
    /// Rate limiter for write operations (POST/PUT/DELETE)
    pub write: Arc<DefaultKeyedRateLimiter<IpAddr>>,
    /// Rate limiter for auth failures (applied separately)
    pub auth_fail: Arc<DefaultKeyedRateLimiter<IpAddr>>,
    /// Trusted proxy header name (snapshot from config at startup).
    /// Used by the rate-limit middleware to extract the real client IP
    /// via the same trusted-proxy policy as the public router.
    pub trusted_proxy_header: Option<String>,
}

impl AdminRateLimiters {
    /// Create a new set of rate limiters with the given per-minute limits.
    ///
    /// `trusted_proxy_header` is a snapshot of the server config at startup,
    /// used for proxy-aware client IP extraction in the rate-limit middleware.
    pub fn new(
        read_rpm: u32,
        write_rpm: u32,
        auth_fail_rpm: u32,
        trusted_proxy_header: Option<String>,
    ) -> Self {
        Self {
            read: Arc::new(Self::make_limiter(read_rpm)),
            write: Arc::new(Self::make_limiter(write_rpm)),
            auth_fail: Arc::new(Self::make_limiter(auth_fail_rpm)),
            trusted_proxy_header,
        }
    }

    fn make_limiter(rpm: u32) -> DefaultKeyedRateLimiter<IpAddr> {
        let quota = Quota::per_minute(NonZeroU32::new(rpm).unwrap_or(NonZeroU32::new(1).unwrap()));
        RateLimiter::keyed(quota)
    }
}

/// Extract client IP from the request with trusted-proxy awareness.
///
/// Reads `ConnectInfo` for the raw connection IP, then delegates to
/// [`crate::server::routes::extract_client_ip`] to apply Cloudflare and
/// trusted-proxy header extraction. The `trusted_proxy_header` should
/// come from `ServerState`; pass `None` if unavailable.
pub(crate) fn extract_ip_with_proxy(
    request: &Request<Body>,
    trusted_proxy_header: Option<&str>,
) -> IpAddr {
    let conn_ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or_else(|| IpAddr::from([127, 0, 0, 1]));
    crate::server::routes::extract_client_ip(request.headers(), &conn_ip, trusted_proxy_header)
}

/// Rate limiting middleware for the external admin API.
///
/// Checks the read or write bucket depending on the HTTP method.
/// Returns 429 with Retry-After header if the limit is exceeded.
///
/// Extracts `AdminRateLimiters` from an axum `Extension` layer, avoiding
/// the need to acquire the `ServerState` RwLock on every request.
pub async fn rate_limit_middleware(
    limiters: Option<Extension<Arc<AdminRateLimiters>>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let Some(Extension(limiters)) = limiters else {
        return next.run(request).await;
    };

    let ip = extract_ip_with_proxy(&request, limiters.trusted_proxy_header.as_deref());
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

/// Run periodic cleanup of rate limiter state to prevent unbounded memory growth.
///
/// Governor's `DefaultKeyedRateLimiter` uses a `DashMap` internally. Entries for
/// IPs that have replenished back to full quota are indistinguishable from fresh
/// entries and can be safely removed via `retain_recent()`. This task runs every
/// 5 minutes and also calls `shrink_to_fit()` to release excess DashMap capacity.
pub async fn run_limiter_cleanup(limiters: Arc<AdminRateLimiters>) {
    let interval = std::time::Duration::from_secs(300);
    loop {
        tokio::time::sleep(interval).await;
        limiters.read.retain_recent();
        limiters.read.shrink_to_fit();
        limiters.write.retain_recent();
        limiters.write.shrink_to_fit();
        limiters.auth_fail.retain_recent();
        limiters.auth_fail.shrink_to_fit();
        tracing::debug!(
            "Rate limiter cleanup: read={}, write={}, auth_fail={}",
            limiters.read.len(),
            limiters.write.len(),
            limiters.auth_fail.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_limiter_creation() {
        let limiters = AdminRateLimiters::new(60, 10, 5, None);
        let ip = IpAddr::from([1, 2, 3, 4]);

        // First request should pass
        assert!(limiters.read.check_key(&ip).is_ok());
        assert!(limiters.write.check_key(&ip).is_ok());
    }

    #[test]
    fn test_write_limiter_exhaustion() {
        // 2 per minute -- should exhaust quickly
        let limiters = AdminRateLimiters::new(60, 2, 5, None);
        let ip = IpAddr::from([10, 0, 0, 1]);

        // First 2 should pass
        assert!(limiters.write.check_key(&ip).is_ok());
        assert!(limiters.write.check_key(&ip).is_ok());

        // Third should be rate limited
        assert!(limiters.write.check_key(&ip).is_err());
    }

    #[test]
    fn test_different_ips_independent() {
        let limiters = AdminRateLimiters::new(60, 1, 5, None);
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
        let limiters = AdminRateLimiters::new(60, 10, 2, None);
        let ip = IpAddr::from([192, 168, 1, 1]);

        // First 2 auth failures OK
        assert!(!check_auth_failure(&limiters, ip));
        assert!(!check_auth_failure(&limiters, ip));

        // Third should trigger rate limit
        assert!(check_auth_failure(&limiters, ip));
    }
}
