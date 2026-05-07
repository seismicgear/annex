//! Per-WebSocket-session token bucket for `IncomingMessage::Typing`.
//!
//! The HTTP rate limiter does not apply to WebSocket frames, so without
//! this an authenticated client can fire `Typing` events at an unbounded
//! rate. Each `Typing` event triggers a `connection_manager.broadcast`
//! to every subscriber of the channel; the fan-out makes a 10k-events/s
//! attacker into a 10k × N-subscribers/s broadcast load.
//!
//! `MAX_WS_MESSAGE_BYTES` ([F24]) does not help here — typing frames
//! are tiny and well under the cap.
//!
//! Design: per-session, per-channel sliding floor. A typing event for
//! `channel_id` is admitted iff the previous admitted event for the
//! same `channel_id` was at least [`TYPING_DEBOUNCE`] ago. The map is
//! pruned opportunistically on every call so it cannot grow without
//! bound (its size is bounded by the number of distinct channels the
//! peer has typed in within the debounce window).
//!
//! Why per-channel and not a global bucket: the legitimate UI use case
//! is "I'm typing in channel X" — clients re-send roughly once per
//! second to maintain the "typing" state. A global bucket would either
//! be too tight (drops legitimate events when typing in two channels)
//! or too loose (still admits sustained spam from one channel). The
//! per-channel debounce mirrors the natural cadence of the client.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// Minimum time between admitted typing events for the same channel
/// from a single connection. Chosen as 800ms because legitimate
/// clients re-send typing pings at ~1Hz; 800ms allows for clock jitter
/// without admitting a faster cadence.
pub(crate) const TYPING_DEBOUNCE: Duration = Duration::from_millis(800);

/// Garbage-collect entries whose last admission was longer ago than
/// this. Keeps the map size bounded even for a peer that bounces
/// across many channels.
const TYPING_GC_HORIZON: Duration = Duration::from_secs(60);

/// Per-session typing debouncer. `Default::default()` is the empty
/// state — no channels seen yet.
///
/// The struct is `pub` (rather than `pub(crate)`) only because it
/// appears in [`crate::ws::context::CommandContext`], which is itself
/// `pub` to keep the `dispatch::dispatch` signature crate-public.
/// External consumers have no reason to construct a `TypingThrottle`
/// directly — and there are no constructors or methods reachable
/// outside the crate beyond the `Default` impl.
#[derive(Debug, Default)]
pub struct TypingThrottle {
    last_seen: Mutex<HashMap<String, Instant>>,
}

impl TypingThrottle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Admit a typing event for `channel_id`. Returns `true` iff the
    /// caller should proceed with the broadcast.
    ///
    /// The check is generic over the clock so tests can drive it with
    /// a deterministic time source. Production code uses
    /// [`try_admit`] which substitutes `Instant::now()`.
    pub(crate) async fn try_admit_at(&self, channel_id: &str, now: Instant) -> bool {
        let mut guard = self.last_seen.lock().await;

        // Opportunistic prune: anything older than TYPING_GC_HORIZON
        // is rate-limited at TYPING_DEBOUNCE anyway, so dropping it
        // does not change the answer for any future call. Bounds the
        // map size to "channels typed in within the last 60s."
        guard.retain(|_, last| now.duration_since(*last) < TYPING_GC_HORIZON);

        match guard.get(channel_id) {
            Some(prev) if now.duration_since(*prev) < TYPING_DEBOUNCE => false,
            _ => {
                guard.insert(channel_id.to_string(), now);
                true
            }
        }
    }

    /// Wall-clock variant of [`try_admit_at`].
    pub(crate) async fn try_admit(&self, channel_id: &str) -> bool {
        self.try_admit_at(channel_id, Instant::now()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_typing_event_is_admitted() {
        let throttle = TypingThrottle::new();
        assert!(throttle.try_admit("ch1").await);
    }

    #[tokio::test]
    async fn second_typing_event_within_debounce_is_dropped() {
        let throttle = TypingThrottle::new();
        let t0 = Instant::now();
        assert!(throttle.try_admit_at("ch1", t0).await);
        // 100ms later: still inside the debounce.
        assert!(
            !throttle
                .try_admit_at("ch1", t0 + Duration::from_millis(100))
                .await
        );
        // 700ms later: still inside the 800ms debounce.
        assert!(
            !throttle
                .try_admit_at("ch1", t0 + Duration::from_millis(700))
                .await
        );
    }

    #[tokio::test]
    async fn typing_event_after_debounce_is_admitted() {
        let throttle = TypingThrottle::new();
        let t0 = Instant::now();
        assert!(throttle.try_admit_at("ch1", t0).await);
        assert!(
            throttle
                .try_admit_at("ch1", t0 + TYPING_DEBOUNCE + Duration::from_millis(1))
                .await
        );
    }

    #[tokio::test]
    async fn debounce_is_per_channel() {
        let throttle = TypingThrottle::new();
        let t0 = Instant::now();
        assert!(throttle.try_admit_at("ch1", t0).await);
        // Different channel, even within the debounce window for ch1,
        // is admitted independently.
        assert!(
            throttle
                .try_admit_at("ch2", t0 + Duration::from_millis(50))
                .await
        );
        // ch1 itself within the window is still rejected.
        assert!(
            !throttle
                .try_admit_at("ch1", t0 + Duration::from_millis(100))
                .await
        );
    }

    #[tokio::test]
    async fn flood_at_high_frequency_admits_at_most_one_per_debounce() {
        // The real-world DoS shape: a malicious client sends 1000 typing
        // events in rapid succession. Without the throttle, every one
        // would fan out to every channel subscriber. With the throttle,
        // at most one is admitted per debounce window.
        let throttle = TypingThrottle::new();
        let t0 = Instant::now();
        let mut admitted = 0usize;
        for i in 0..1000u64 {
            // 1ms between attempts → 1000 attempts span 1 second.
            let now = t0 + Duration::from_millis(i);
            if throttle.try_admit_at("ch1", now).await {
                admitted += 1;
            }
        }
        // 1 second / 800ms debounce = 1 admit at t=0, 1 admit at t≈800ms.
        // The exact count depends on integer division but must be small.
        assert!(
            admitted <= 2,
            "high-frequency flood admitted {admitted} events in 1s; expected ≤ 2"
        );
        assert!(
            admitted >= 1,
            "expected at least the initial event to be admitted"
        );
    }

    #[tokio::test]
    async fn old_entries_are_garbage_collected() {
        let throttle = TypingThrottle::new();
        let t0 = Instant::now();
        // Type in 100 distinct channels.
        for i in 0..100 {
            assert!(throttle.try_admit_at(&format!("ch{i}"), t0).await);
        }
        assert_eq!(throttle.last_seen.lock().await.len(), 100);

        // Two minutes later (well past the 60s GC horizon), one new
        // typing event prunes everything.
        let later = t0 + Duration::from_secs(120);
        assert!(throttle.try_admit_at("new", later).await);
        let map = throttle.last_seen.lock().await;
        assert_eq!(
            map.len(),
            1,
            "old entries should be pruned when GC horizon is exceeded"
        );
        assert!(map.contains_key("new"));
    }
}
