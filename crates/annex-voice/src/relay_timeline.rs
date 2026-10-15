//! RTP timestamp continuity across publisher generations sharing one outgoing SSRC.
use std::time::Instant;

#[derive(Default)]
pub(crate) struct RelayTimeline {
    source: Option<(u64, u32)>,
    offset: u32,
    last_timestamp: Option<u32>,
    last_sent_at: Option<Instant>,
}
impl RelayTimeline {
    pub(crate) fn translate(
        &mut self,
        generation: u64,
        source_ssrc: u32,
        timestamp: u32,
        clock_rate: u32,
        now: Instant,
    ) -> u32 {
        let source = (generation, source_ssrc);
        if self.source != Some(source) {
            let next = self
                .last_timestamp
                .map(|last| {
                    // Preserve the real pause duration on this receiver's clock.
                    // A fresh capture's random timestamp must never schedule decoded
                    // frames hours into the future (or before the existing playout).
                    let elapsed = self
                        .last_sent_at
                        .map(|at| now.saturating_duration_since(at))
                        .unwrap_or_default();
                    let ticks = (elapsed.as_nanos() * u128::from(clock_rate) / 1_000_000_000)
                        .min(u128::from(u32::MAX)) as u32;
                    last.wrapping_add(ticks.max(1))
                })
                .unwrap_or(timestamp);
            self.offset = next.wrapping_sub(timestamp);
            self.source = Some(source);
        }
        let translated = timestamp.wrapping_add(self.offset);
        if self
            .last_timestamp
            .is_none_or(|last| translated.wrapping_sub(last) < 0x8000_0000)
        {
            self.last_timestamp = Some(translated);
            self.last_sent_at = Some(now);
        }
        translated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn restarted_camera_keeps_receiver_clock_instead_of_random_source_clock() {
        let now = Instant::now();
        let mut timeline = RelayTimeline::default();
        assert_eq!(
            timeline.translate(1, 123, 4_000_000, 90_000, now),
            4_000_000
        );
        assert_eq!(
            timeline.translate(1, 123, 4_003_000, 90_000, now + Duration::from_millis(33)),
            4_003_000
        );
        assert_eq!(
            timeline.translate(
                2,
                456,
                2_700_000_000,
                90_000,
                now + Duration::from_millis(3033)
            ),
            4_273_000
        );
        assert_eq!(
            timeline.translate(
                2,
                456,
                2_700_003_000,
                90_000,
                now + Duration::from_millis(3066)
            ),
            4_276_000
        );
        // Late packets retain their timestamp but cannot move the reset anchor back.
        assert_eq!(
            timeline.translate(
                2,
                456,
                2_700_001_000,
                90_000,
                now + Duration::from_millis(3070)
            ),
            4_274_000
        );
        assert_eq!(
            timeline.translate(3, 789, 7, 90_000, now + Duration::from_millis(4066)),
            4_366_000
        );
    }
    #[test]
    fn timestamp_wrap_and_reused_incoming_ssrc_preserve_continuity() {
        let now = Instant::now();
        let mut timeline = RelayTimeline::default();
        assert_eq!(
            timeline.translate(1, 123, u32::MAX - 999, 48_000, now),
            u32::MAX - 999
        );
        assert_eq!(
            timeline.translate(1, 123, 0, 48_000, now + Duration::from_millis(21)),
            0
        );
        assert_eq!(
            timeline.translate(2, 123, 123_456, 48_000, now + Duration::from_millis(1021)),
            48_000
        );
        assert_eq!(
            timeline.translate(2, 123, 124_416, 48_000, now + Duration::from_millis(1041)),
            48_960
        );
    }
}
