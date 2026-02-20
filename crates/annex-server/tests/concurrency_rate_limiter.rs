//! Concurrency tests for the sliding-window rate limiter.
//!
//! These tests verify the rate limiter is correct under concurrent access:
//! - Multiple threads checking the same key simultaneously
//! - Many distinct keys under concurrent load
//! - No panics, deadlocks, or poisoned mutex recovery failures
//! - Limits are enforced even under contention

use annex_server::middleware::{RateLimitCategory, RateLimitKey, RateLimiter};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn test_rate_limiter_concurrent_same_key() {
    let limiter = Arc::new(RateLimiter::new());
    let allowed_count = Arc::new(AtomicU32::new(0));
    let denied_count = Arc::new(AtomicU32::new(0));
    let limit = 100u32;

    let mut handles = Vec::new();

    // Spawn 200 concurrent tasks all checking the same key
    for _ in 0..200 {
        let limiter = limiter.clone();
        let allowed = allowed_count.clone();
        let denied = denied_count.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            let key = RateLimitKey::Ip("10.0.0.1".parse().unwrap(), RateLimitCategory::Default);
            if limiter.check(key, limit) {
                allowed.fetch_add(1, Ordering::Relaxed);
            } else {
                denied.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await.expect("task should not panic");
    }

    let total_allowed = allowed_count.load(Ordering::Relaxed);
    let total_denied = denied_count.load(Ordering::Relaxed);

    assert_eq!(total_allowed + total_denied, 200, "all requests should be counted");
    // Due to the sliding window estimate, the exact cutoff point varies based on
    // timing. We verify that *some* requests were denied (we sent 200 with limit 100).
    assert!(
        total_denied > 0,
        "at least some requests should be denied (allowed={}, denied={})",
        total_allowed,
        total_denied
    );
    // And that the limiter allowed at most limit + 1 requests (accounting for
    // the current request being counted before the check).
    assert!(
        total_allowed <= limit + 1,
        "should not allow significantly more than the limit (allowed={}, limit={})",
        total_allowed,
        limit
    );
}

#[tokio::test]
async fn test_rate_limiter_concurrent_distinct_keys() {
    let limiter = Arc::new(RateLimiter::new());
    let limit = 5u32;

    let mut handles = Vec::new();

    // 50 distinct IPs, each sends exactly `limit` requests concurrently
    for ip_idx in 0..50u32 {
        for _ in 0..limit {
            let limiter = limiter.clone();
            handles.push(tokio::task::spawn_blocking(move || {
                let ip: IpAddr = std::net::Ipv4Addr::from(ip_idx.to_be_bytes()).into();
                let key = RateLimitKey::Ip(ip, RateLimitCategory::Default);
                limiter.check(key, limit)
            }));
        }
    }

    let mut results: Vec<bool> = Vec::new();
    for handle in handles {
        results.push(handle.await.expect("task should not panic"));
    }

    // All 250 requests (50 IPs * 5 each) should be within their respective limits
    let allowed = results.iter().filter(|&&r| r).count();
    assert_eq!(
        allowed,
        250,
        "each IP sends exactly `limit` requests, all should be allowed"
    );
}

#[tokio::test]
async fn test_rate_limiter_concurrent_high_volume_no_panic() {
    // Stress test: 1000 concurrent requests from 100 distinct IPs.
    // The primary assertion is that no panics or deadlocks occur.
    let limiter = Arc::new(RateLimiter::new());

    let mut handles = Vec::new();
    for i in 0..1000u32 {
        let limiter = limiter.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            let ip_idx = i % 100;
            let ip: IpAddr = std::net::Ipv4Addr::from(ip_idx.to_be_bytes()).into();
            let key = RateLimitKey::Ip(ip, RateLimitCategory::Default);
            let _ = limiter.check(key, 10);
        }));
    }

    for handle in handles {
        handle.await.expect("rate limiter should not panic under high concurrency");
    }
}

#[tokio::test]
async fn test_rate_limiter_pseudonym_key_concurrent() {
    let limiter = Arc::new(RateLimiter::new());
    let allowed = Arc::new(AtomicU32::new(0));
    let limit = 10u32;

    let mut handles = Vec::new();

    // Same pseudonym, 20 concurrent requests
    for _ in 0..20 {
        let limiter = limiter.clone();
        let allowed = allowed.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            let key = RateLimitKey::Pseudonym("agent_alpha".to_string(), RateLimitCategory::Default);
            if limiter.check(key, limit) {
                allowed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await.expect("task should not panic");
    }

    let total_allowed = allowed.load(Ordering::Relaxed);
    assert!(
        total_allowed <= limit + 1,
        "pseudonym rate limit should be enforced (allowed={}, limit={})",
        total_allowed,
        limit
    );
}

#[tokio::test]
async fn test_rate_limiter_eviction_under_concurrent_load() {
    // Test that the eviction logic (triggered at > 10_000 entries) doesn't
    // cause panics or incorrect behavior under concurrent access.
    let limiter = Arc::new(RateLimiter::new());

    let mut handles = Vec::new();

    // Create 10_100 distinct keys to trigger eviction
    for i in 0..10_100u32 {
        let limiter = limiter.clone();
        handles.push(tokio::task::spawn_blocking(move || {
            let ip: IpAddr = std::net::Ipv4Addr::from(i.to_be_bytes()).into();
            let key = RateLimitKey::Ip(ip, RateLimitCategory::Default);
            let _ = limiter.check(key, 100);
        }));
    }

    for handle in handles {
        handle.await.expect("eviction under load should not panic");
    }

    // Verify a recently-used key still works
    let key = RateLimitKey::Ip("0.0.39.115".parse::<IpAddr>().unwrap(), RateLimitCategory::Default); // IP from index 10099
    assert!(
        limiter.check(key, 100),
        "recently-used key should still be tracked after eviction"
    );
}
