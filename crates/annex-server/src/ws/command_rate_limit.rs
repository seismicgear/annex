//! Per-WebSocket-session token bucket for state-mutating commands.
//!
//! The HTTP rate limiter does not apply to WebSocket frames, and
//! [`crate::ws::typing_throttle`] only covers `IncomingMessage::Typing`.
//! The remaining expensive commands — `Message`, `EditMessage`,
//! `DeleteMessage`, `VoiceIntent`, and `Resume` — each hit the database
//! and/or fan out to every subscriber of a channel, yet had no
//! per-connection cap. An authenticated member could therefore drive a
//! single socket to flood `message` frames at the rate the OS delivers
//! them, turning one connection into sustained DB-write + N-subscriber
//! broadcast load.
//!
//! This is a classic token bucket: the session starts with [`BUCKET_CAPACITY`]
//! tokens, every admitted command spends one, and tokens refill at
//! [`REFILL_TOKENS_PER_SEC`]. Legitimate chat — bursty but low average
//! rate — never notices it: a user can fire a full burst of
//! [`BUCKET_CAPACITY`] commands instantly, then sustain
//! [`REFILL_TOKENS_PER_SEC`]/s indefinitely. A flooder is clamped to the
//! sustained refill rate.
//!
//! The bucket is per-session (not per-channel like the typing throttle):
//! spamming writes is equally abusive regardless of how many channels the
//! flood is spread across, so a single shared budget is the right shape.

use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Maximum burst: the number of commands a freshly-connected (or idle)
/// session may fire back-to-back before refill governs the rate. Sized
/// well above any plausible human burst (paste-splitting a few messages,
/// rapid edits) so legitimate use never hits the cap.
pub(crate) const BUCKET_CAPACITY: f64 = 30.0;

/// Sustained admit rate once the burst budget is spent. 10/s is far above
/// human chat cadence (~1–2/s at peak) but orders of magnitude below the
/// thousands/s an automated flood would attempt.
pub(crate) const REFILL_TOKENS_PER_SEC: f64 = 10.0;

#[derive(Debug)]
struct BucketState {
    tokens: f64,
    last_refill: Instant,
}

/// Per-session command token bucket. `new()` starts full so the first
/// burst is admitted immediately.
#[derive(Debug)]
pub struct CommandRateLimiter {
    state: Mutex<BucketState>,
}

impl CommandRateLimiter {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(BucketState {
                tokens: BUCKET_CAPACITY,
                last_refill: Instant::now(),
            }),
        }
    }

    /// Attempt to spend one token. Returns `true` iff the command is
    /// admitted. Generic over the clock so tests can drive it
    /// deterministically; production calls [`try_admit`].
    pub(crate) async fn try_admit_at(&self, now: Instant) -> bool {
        let mut guard = self.state.lock().await;

        // Refill proportional to elapsed time, capped at capacity.
        let elapsed = now.saturating_duration_since(guard.last_refill);
        if elapsed > Duration::ZERO {
            guard.tokens =
                (guard.tokens + elapsed.as_secs_f64() * REFILL_TOKENS_PER_SEC).min(BUCKET_CAPACITY);
            guard.last_refill = now;
        }

        if guard.tokens >= 1.0 {
            guard.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Wall-clock variant of [`try_admit_at`].
    pub(crate) async fn try_admit(&self) -> bool {
        self.try_admit_at(Instant::now()).await
    }
}

impl Default for CommandRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn admits_a_full_burst_up_to_capacity() {
        let limiter = CommandRateLimiter::new();
        let t0 = Instant::now();
        // Capacity back-to-back admits with no time passing.
        for _ in 0..(BUCKET_CAPACITY as usize) {
            assert!(limiter.try_admit_at(t0).await);
        }
        // The very next one (still t0) is rejected — bucket empty.
        assert!(!limiter.try_admit_at(t0).await);
    }

    #[tokio::test]
    async fn refills_at_the_sustained_rate() {
        let limiter = CommandRateLimiter::new();
        let t0 = Instant::now();
        // Drain the bucket.
        for _ in 0..(BUCKET_CAPACITY as usize) {
            assert!(limiter.try_admit_at(t0).await);
        }
        assert!(!limiter.try_admit_at(t0).await);

        // 1 second later: ~REFILL_TOKENS_PER_SEC tokens available.
        let t1 = t0 + Duration::from_secs(1);
        let mut admitted = 0usize;
        for _ in 0..100 {
            if limiter.try_admit_at(t1).await {
                admitted += 1;
            }
        }
        assert_eq!(
            admitted, REFILL_TOKENS_PER_SEC as usize,
            "after 1s the bucket should admit exactly the refill quota"
        );
    }

    #[tokio::test]
    async fn high_frequency_flood_is_clamped_to_refill_rate() {
        // The DoS shape: 10_000 commands attempted across 1 second.
        // Admits = initial burst (capacity) + refill over the window.
        let limiter = CommandRateLimiter::new();
        let t0 = Instant::now();
        let mut admitted = 0usize;
        for i in 0..10_000u64 {
            // 0.1ms apart → 10_000 attempts span exactly 1 second.
            let now = t0 + Duration::from_micros(i * 100);
            if limiter.try_admit_at(now).await {
                admitted += 1;
            }
        }
        // Upper bound: full burst + one window of refill, plus a small
        // slack for fractional-token accounting at the boundary.
        let ceiling = (BUCKET_CAPACITY + REFILL_TOKENS_PER_SEC) as usize + 1;
        assert!(
            admitted <= ceiling,
            "flood admitted {admitted}; expected <= {ceiling}"
        );
        // Sanity: the legitimate initial burst still got through.
        assert!(admitted >= BUCKET_CAPACITY as usize);
    }

    #[tokio::test]
    async fn refill_never_exceeds_capacity() {
        let limiter = CommandRateLimiter::new();
        let t0 = Instant::now();
        // Idle for a long time — tokens must cap at BUCKET_CAPACITY,
        // not accumulate unboundedly.
        let later = t0 + Duration::from_secs(3600);
        let mut admitted = 0usize;
        for _ in 0..1000 {
            if limiter.try_admit_at(later).await {
                admitted += 1;
            }
        }
        assert_eq!(
            admitted, BUCKET_CAPACITY as usize,
            "an hour of idle must not bank more than one full bucket"
        );
    }
}
