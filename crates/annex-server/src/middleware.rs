use annex_identity::{get_platform_identity, PlatformIdentity};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::AppState;

/// Wrapper for `PlatformIdentity` to be stored in request extensions.
#[derive(Clone, Debug)]
pub struct IdentityContext(pub PlatformIdentity);

/// Middleware to authenticate requests via `X-Annex-Pseudonym` or `Authorization: Bearer`.
///
/// # Security Note
///
/// In this phase (Phase 2), authentication relies on the pseudonym acting as a bearer token.
/// There is currently no cryptographic signature verification for individual requests.
/// This is a known limitation of the current roadmap state. Future phases (Client/Hardening)
/// will likely introduce signed requests or session tokens.
///
/// For now, the "Bearer" token IS the pseudonym.
pub async fn auth_middleware(mut req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    // 1. Extract pseudonym from header
    let pseudonym = if let Some(val) = req.headers().get("X-Annex-Pseudonym") {
        val.to_str()
            .map_err(|_| StatusCode::UNAUTHORIZED)?
            .to_string()
    } else if let Some(val) = req.headers().get("Authorization") {
        let val_str = val.to_str().map_err(|_| StatusCode::UNAUTHORIZED)?;
        if let Some(token) = val_str.strip_prefix("Bearer ") {
            token.to_string()
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    // 2. Get AppState
    let state = req
        .extensions()
        .get::<Arc<AppState>>()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();

    let server_id = state.server_id;

    // 3. Verify Identity (blocking DB operation)
    let identity = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // Get Identity
        // We treat any error (including "not found") as Unauthorized for security
        get_platform_identity(&conn, server_id, &pseudonym).map_err(|_| StatusCode::UNAUTHORIZED)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    // 4. Check if active
    if !identity.active {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 5. Insert into extensions
    req.extensions_mut().insert(IdentityContext(identity));

    Ok(next.run(req).await)
}

/// Rate limiting key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RateLimitKey {
    /// Rate limit by IP address.
    Ip(IpAddr),
    /// Rate limit by pseudonym.
    Pseudonym(String),
}

/// Per-key sliding window state.
///
/// Stores the previous window count, the current window count, and the start
/// instant of the current window. The effective rate at any point is:
///   `prev_count * (1 - elapsed_ratio) + current_count`
/// where `elapsed_ratio = elapsed / WINDOW`. This prevents the 2x burst
/// that occurs at fixed-window boundaries.
#[derive(Debug, Clone)]
struct SlidingWindowEntry {
    prev_count: u32,
    curr_count: u32,
    window_start: Instant,
}

/// In-memory rate limiter state.
///
/// Uses a sliding window counter to prevent the 2x burst that occurs with
/// fixed-window rate limiters at window boundaries. See H-14 in the
/// production gap report.
#[derive(Clone, Debug)]
pub struct RateLimiter {
    state: Arc<Mutex<HashMap<RateLimitKey, SlidingWindowEntry>>>,
}

/// Rate limit window duration.
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if the request is allowed.
    ///
    /// Returns `true` if allowed, `false` if limit exceeded.
    /// Uses a sliding window to smooth burst behavior at window boundaries.
    pub fn check(&self, key: RateLimitKey, limit: u32) -> bool {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // Lock poisoned by a panicked thread. Recover by accepting the
                // poisoned guard — the worst that happens is a stale counter.
                // Refusing all requests because of a poisoned rate-limiter
                // would be a self-inflicted denial of service.
                tracing::error!("rate limiter lock poisoned, recovering with stale state");
                poisoned.into_inner()
            }
        };
        let now = Instant::now();

        // Periodic cleanup to prevent memory leak.
        // Evict entries whose previous AND current windows have both expired
        // (2 full windows ago). This preserves active rate limits while reclaiming memory.
        if state.len() > 10000 {
            let two_windows = RATE_LIMIT_WINDOW * 2;
            state.retain(|_, entry| now.duration_since(entry.window_start) <= two_windows);
        }

        let entry = state.entry(key).or_insert(SlidingWindowEntry {
            prev_count: 0,
            curr_count: 0,
            window_start: now,
        });

        let elapsed = now.duration_since(entry.window_start);

        if elapsed > RATE_LIMIT_WINDOW {
            // Rotate: current becomes previous, start a new window
            entry.prev_count = entry.curr_count;
            entry.curr_count = 0;
            entry.window_start = now;
        }

        // Calculate the sliding window estimate:
        // Weight the previous window's count by how much of it still overlaps.
        let elapsed_secs = now.duration_since(entry.window_start).as_secs_f64();
        let window_secs = RATE_LIMIT_WINDOW.as_secs_f64();
        let overlap_ratio = 1.0 - (elapsed_secs / window_secs).min(1.0);
        let estimated = (f64::from(entry.prev_count) * overlap_ratio)
            + f64::from(entry.curr_count);

        if estimated >= f64::from(limit) {
            false
        } else {
            entry.curr_count += 1;
            true
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Rate limiting middleware.
pub async fn rate_limit_middleware(req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    // 1. Get AppState
    let state = req
        .extensions()
        .get::<Arc<AppState>>()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();

    // 2. Identify Key
    // Check IdentityContext first (Pseudonym), then ConnectInfo (IP)
    // Note: IdentityContext is only available if auth_middleware runs *before* this middleware.
    // Currently, auth_middleware is not applied globally, so this usually falls back to IP.
    let key = if let Some(identity) = req.extensions().get::<IdentityContext>() {
        RateLimitKey::Pseudonym(identity.0.pseudonym_id.clone())
    } else if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        RateLimitKey::Ip(addr.ip())
    } else {
        // In test environments, ConnectInfo might be missing if not injected manually.
        // We log a warning (if we could) and fail safe or allow?
        // Safe default: Fail. Misconfiguration should be fixed.
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    };

    // 3. Get Policy Limit
    let limit = {
        let policy = match state.policy.read() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::error!("server policy lock poisoned");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        let path = req.uri().path();
        if path == "/api/registry/register" {
            policy.rate_limit.registration_limit
        } else if path == "/api/zk/verify-membership" {
            policy.rate_limit.verification_limit
        } else {
            policy.rate_limit.default_limit
        }
    };

    // 4. Check Limit
    if !state.rate_limiter.check(key, limit) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            axum::http::HeaderValue::from_static("60"),
        );
        return Ok(response);
    }

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_allows_within_limit() {
        let limiter = RateLimiter::new();
        let key = RateLimitKey::Ip("127.0.0.1".parse().unwrap());
        for _ in 0..5 {
            assert!(limiter.check(key.clone(), 5));
        }
        // 6th request should be denied
        assert!(!limiter.check(key, 5));
    }

    #[test]
    fn rate_limiter_different_keys_independent() {
        let limiter = RateLimiter::new();
        let key_a = RateLimitKey::Ip("10.0.0.1".parse().unwrap());
        let key_b = RateLimitKey::Ip("10.0.0.2".parse().unwrap());

        // Fill up key_a
        for _ in 0..3 {
            assert!(limiter.check(key_a.clone(), 3));
        }
        assert!(!limiter.check(key_a, 3));

        // key_b should still be allowed
        assert!(limiter.check(key_b, 3));
    }

    #[test]
    fn rate_limiter_eviction_preserves_active_limits() {
        let limiter = RateLimiter::new();

        // Fill with 10001 distinct IPs to trigger eviction
        for i in 0..10001u32 {
            let ip: IpAddr = std::net::Ipv4Addr::from(i.to_be_bytes()).into();
            limiter.check(RateLimitKey::Ip(ip), 100);
        }

        // Now check that the eviction happened without blanket clear.
        // The 10001st IP was just used (within window), so it should still be
        // rate-limited if we check again.
        let recent_ip: IpAddr = std::net::Ipv4Addr::from(10000u32.to_be_bytes()).into();
        let key = RateLimitKey::Ip(recent_ip);
        // The counter should still be 1 (not reset to 0 by blanket clear)
        // since the entry is within its 60-second window.
        // We can verify the limiter still tracks it by checking we can send
        // limit-1 more requests.
        for _ in 0..99 {
            assert!(limiter.check(key.clone(), 100));
        }
        // Now at 101 total, should be denied
        assert!(!limiter.check(key, 100));
    }

    #[test]
    fn sliding_window_prevents_boundary_burst() {
        // Verify that the sliding window prevents sending 2x limit across a
        // window boundary. With a fixed-window limiter, a client could send
        // `limit` requests at the end of window N and `limit` more at the
        // start of window N+1, effectively doubling throughput.
        //
        // With the sliding window, the previous window's count is weighted
        // by the overlap ratio, so only a small number of additional requests
        // are allowed in the new window (not the full limit again).
        let limiter = RateLimiter::new();
        let key = RateLimitKey::Ip("192.168.1.1".parse().unwrap());
        let limit = 100u32;

        // Use all 100 requests in the current window
        for _ in 0..limit {
            assert!(limiter.check(key.clone(), limit));
        }
        // 101st should be denied
        assert!(!limiter.check(key.clone(), limit));

        // Simulate window rotation: current count becomes previous,
        // current resets, new window starts
        {
            let mut state = limiter.state.lock().unwrap();
            let entry = state.get_mut(&key).unwrap();
            entry.prev_count = entry.curr_count;
            entry.curr_count = 0;
            entry.window_start = Instant::now();
        }

        // Count how many additional requests are allowed in the new window.
        // A fixed-window limiter would allow `limit` (100) more requests.
        // The sliding window should allow far fewer (close to 0 at the start).
        let mut allowed_after_rotation = 0u32;
        for _ in 0..limit {
            if limiter.check(key.clone(), limit) {
                allowed_after_rotation += 1;
            } else {
                break;
            }
        }

        // With a fixed window, we'd get `limit` (100) more.
        // With sliding window at t≈0, overlap_ratio ≈ 1.0, so we should get
        // very few additional requests (at most a handful due to float math).
        assert!(
            allowed_after_rotation < limit / 2,
            "sliding window should prevent 2x burst: got {} additional requests out of {} limit",
            allowed_after_rotation,
            limit
        );
    }
}
