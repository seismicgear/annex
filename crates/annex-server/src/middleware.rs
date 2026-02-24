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

/// Rate limit endpoint category — each category has its own counter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RateLimitCategory {
    Registration,
    Verification,
    Default,
}

/// Rate limiting key — combines identity (IP or pseudonym) with endpoint category
/// so that static file requests don't consume the registration budget.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RateLimitKey {
    /// Rate limit by IP address and category.
    Ip(IpAddr, RateLimitCategory),
    /// Rate limit by pseudonym and category.
    Pseudonym(String, RateLimitCategory),
}

/// Per-key state for the sliding window rate limiter.
#[derive(Debug, Clone)]
struct WindowState {
    /// Count in the previous (completed) window.
    prev_count: u32,
    /// Count in the current window so far.
    curr_count: u32,
    /// Start of the current window.
    window_start: Instant,
}

/// Rate limit window duration.
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// In-memory rate limiter state.
///
/// Uses a sliding window counter to prevent the 2x burst that fixed-window
/// counters allow at window boundaries. The effective count is:
/// `prev_count * (1 - elapsed_fraction) + curr_count`, which smoothly
/// transitions between windows.
#[derive(Clone, Debug)]
pub struct RateLimiter {
    state: Arc<Mutex<HashMap<RateLimitKey, WindowState>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if the request is allowed.
    ///
    /// Returns `true` if allowed, `false` if limit exceeded.
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
        // Evict only entries whose window has fully expired (previous + current).
        if state.len() > 10000 {
            state.retain(|_, ws| now.duration_since(ws.window_start) <= RATE_LIMIT_WINDOW * 2);
        }

        let ws = state.entry(key).or_insert(WindowState {
            prev_count: 0,
            curr_count: 0,
            window_start: now,
        });

        let elapsed = now.duration_since(ws.window_start);

        if elapsed > RATE_LIMIT_WINDOW {
            // Rotate: current becomes previous, start a new current window.
            ws.prev_count = ws.curr_count;
            ws.curr_count = 0;
            ws.window_start = now;
        }

        ws.curr_count += 1;

        // Sliding window estimate: weight the previous window's count by the
        // fraction of the window that has NOT yet elapsed.
        let elapsed_frac = now
            .duration_since(ws.window_start)
            .as_secs_f64()
            / RATE_LIMIT_WINDOW.as_secs_f64();
        let prev_weight = 1.0 - elapsed_frac.min(1.0);
        let effective = (ws.prev_count as f64 * prev_weight) + ws.curr_count as f64;

        effective <= limit as f64
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Rate limiting middleware.
///
/// Also performs one-time auto-detection of the server's public URL when no
/// explicit `ANNEX_PUBLIC_URL` is configured. Uses `X-Forwarded-Host` /
/// `X-Forwarded-Proto` headers (for reverse-proxy deployments) with a
/// fallback to the standard `Host` header.
pub async fn rate_limit_middleware(req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    // 1. Get AppState
    let state = req
        .extensions()
        .get::<Arc<AppState>>()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();

    // Auto-detect public URL from request headers if not yet configured.
    // Skips localhost/loopback addresses so the first real public request
    // sets the URL correctly (useful when the admin hits the server locally
    // before any external traffic arrives).
    {
        let needs_detection = state
            .public_url
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty();

        if needs_detection {
            let proto = req
                .headers()
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("http");

            let host = req
                .headers()
                .get("x-forwarded-host")
                .or_else(|| req.headers().get("host"))
                .and_then(|v| v.to_str().ok());

            if let Some(host) = host {
                // Extract hostname without port for the loopback check
                let hostname = host.split(':').next().unwrap_or(host);
                let is_loopback = matches!(
                    hostname,
                    "localhost" | "127.0.0.1" | "0.0.0.0" | "::1" | "[::1]"
                );

                if !is_loopback {
                    let detected = format!("{proto}://{host}");
                    let mut url = state
                        .public_url
                        .write()
                        .unwrap_or_else(|p| p.into_inner());
                    if url.is_empty() {
                        tracing::info!(public_url = %detected, "auto-detected server public URL from request headers");
                        *url = detected;
                    }
                }
            }
        }
    }

    // 2. Classify endpoint and get limit
    let (category, limit) = {
        let policy = match state.policy.read() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::error!("server policy lock poisoned");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };
        let path = req.uri().path();
        if path == "/api/registry/register" {
            (RateLimitCategory::Registration, policy.rate_limit.registration_limit)
        } else if path == "/api/zk/verify-membership" {
            (RateLimitCategory::Verification, policy.rate_limit.verification_limit)
        } else {
            (RateLimitCategory::Default, policy.rate_limit.default_limit)
        }
    };

    // 3. Identify Key (IP or pseudonym) combined with endpoint category
    // so that e.g. static file requests don't consume the registration budget.
    let key = if let Some(identity) = req.extensions().get::<IdentityContext>() {
        RateLimitKey::Pseudonym(identity.0.pseudonym_id.clone(), category)
    } else if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        RateLimitKey::Ip(addr.ip(), category)
    } else {
        tracing::error!("rate_limit_middleware: request has neither IdentityContext nor ConnectInfo<SocketAddr>");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
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
        let key = RateLimitKey::Ip("127.0.0.1".parse().unwrap(), RateLimitCategory::Default);
        for _ in 0..5 {
            assert!(limiter.check(key.clone(), 5));
        }
        // 6th request should be denied
        assert!(!limiter.check(key, 5));
    }

    #[test]
    fn rate_limiter_different_keys_independent() {
        let limiter = RateLimiter::new();
        let key_a = RateLimitKey::Ip("10.0.0.1".parse().unwrap(), RateLimitCategory::Default);
        let key_b = RateLimitKey::Ip("10.0.0.2".parse().unwrap(), RateLimitCategory::Default);

        // Fill up key_a
        for _ in 0..3 {
            assert!(limiter.check(key_a.clone(), 3));
        }
        assert!(!limiter.check(key_a, 3));

        // key_b should still be allowed
        assert!(limiter.check(key_b, 3));
    }

    #[test]
    fn rate_limiter_categories_independent() {
        let limiter = RateLimiter::new();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let default_key = RateLimitKey::Ip(ip, RateLimitCategory::Default);
        let reg_key = RateLimitKey::Ip(ip, RateLimitCategory::Registration);

        // Exhaust default limit
        for _ in 0..60 {
            assert!(limiter.check(default_key.clone(), 60));
        }
        assert!(!limiter.check(default_key, 60));

        // Registration counter should be unaffected
        assert!(limiter.check(reg_key, 20));
    }

    #[test]
    fn rate_limiter_eviction_preserves_active_limits() {
        let limiter = RateLimiter::new();

        // Fill with 10001 distinct IPs to trigger eviction
        for i in 0..10001u32 {
            let ip: IpAddr = std::net::Ipv4Addr::from(i.to_be_bytes()).into();
            limiter.check(RateLimitKey::Ip(ip, RateLimitCategory::Default), 100);
        }

        // Now check that the eviction happened without blanket clear.
        // The 10001st IP was just used (within window), so it should still be
        // rate-limited if we check again.
        let recent_ip: IpAddr = std::net::Ipv4Addr::from(10000u32.to_be_bytes()).into();
        let key = RateLimitKey::Ip(recent_ip, RateLimitCategory::Default);
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
}
