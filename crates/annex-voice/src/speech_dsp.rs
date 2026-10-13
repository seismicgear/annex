//! Independent, fixed-format time and pitch processing of synthesized speech.
//! Signalsmith Stretch 0.1.3 is pinned in Cargo; no external DSP executable is used.
use crate::{error::VoiceError, tts::SynthesizedAudio};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) fn validate_factors(speed: f32, pitch: f32) -> Result<(), VoiceError> {
    if !speed.is_finite() || !(0.1..=10.0).contains(&speed) {
        return Err(VoiceError::Config(
            "Speed must be finite and between 0.1 and 10.0".into(),
        ));
    }
    if !pitch.is_finite() || !(0.25..=4.0).contains(&pitch) {
        return Err(VoiceError::Config(
            "Pitch must be finite and between 0.25 and 4.0".into(),
        ));
    }
    Ok(())
}

pub(crate) fn transform(
    mut audio: SynthesizedAudio,
    speed: f32,
    pitch: f32,
    cancelled: &AtomicBool,
) -> Result<SynthesizedAudio, VoiceError> {
    validate_factors(speed, pitch)?;
    let channels = audio.channels;
    let rate = audio.sample_rate;
    if !(1..=2).contains(&channels)
        || !(8_000..=192_000).contains(&rate)
        || audio.pcm_s16le.is_empty()
        || !audio.pcm_s16le.len().is_multiple_of(2 * channels)
    {
        return Err(VoiceError::Tts(
            "Speech engine returned invalid PCM format or length".into(),
        ));
    }
    let frames = audio.pcm_s16le.len() / (2 * channels);
    let output_frames = (frames as f64 / f64::from(speed)).round().max(1.0) as usize;
    if frames > rate * 120 || output_frames > rate * 600 {
        return Err(VoiceError::Tts(
            "Speech audio exceeds the bounded synthesis duration".into(),
        ));
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(VoiceError::Tts("Speech processing was cancelled".into()));
    }
    if speed == 1.0 && pitch == 1.0 {
        return Ok(audio);
    }
    let input: Vec<f32> = audio
        .pcm_s16le
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect();
    let mut stretch = signalsmith_stretch::Stretch::preset_default(channels as u32, rate as u32);
    stretch.set_transpose_factor(pitch, None);
    let input_latency = stretch.input_latency();
    let output_latency = stretch.output_latency();
    let mut preroll = vec![0.0; input_latency * channels];
    let available = preroll.len().min(input.len());
    preroll[..available].copy_from_slice(&input[..available]);
    stretch.seek(&preroll, f64::from(speed));
    // Supply the documented input-latency tail, then flush output latency.
    // Small calls preserve a bounded cancellation interval even at 0.1x speed.
    let mut output = Vec::with_capacity((output_frames + output_latency) * channels);
    let mut consumed = 0usize;
    let mut produced = 0usize;
    while consumed < frames {
        if cancelled.load(Ordering::Acquire) {
            return Err(VoiceError::Tts("Speech processing was cancelled".into()));
        }
        let take = (frames - consumed).min(1024);
        let mut chunk = vec![0.0; take * channels];
        let start = (input_latency + consumed) * channels;
        if start < input.len() {
            let count = chunk.len().min(input.len() - start);
            chunk[..count].copy_from_slice(&input[start..start + count]);
        }
        let next_output =
            ((consumed + take) as u128 * output_frames as u128 / frames as u128) as usize;
        let mut rendered = vec![0.0; (next_output - produced) * channels];
        stretch.process(&chunk, &mut rendered);
        output.extend_from_slice(&rendered);
        consumed += take;
        produced = next_output;
    }
    let mut tail = vec![0.0; output_latency * channels];
    stretch.flush(&mut tail);
    output.extend_from_slice(&tail);
    let start = output_latency * channels;
    let trimmed = &output[start..start + output_frames * channels];
    if trimmed.iter().any(|sample| !sample.is_finite()) {
        return Err(VoiceError::Tts(
            "Speech processor returned non-finite audio".into(),
        ));
    }
    audio.pcm_s16le = trimmed
        .iter()
        .flat_map(|sample| {
            (sample
                .mul_add(32768.0, 0.0)
                .round()
                .clamp(-32768.0, 32767.0) as i16)
                .to_le_bytes()
        })
        .collect();
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(rate: usize, frequency: f64, seconds: f64) -> SynthesizedAudio {
        let pcm_s16le = (0..(rate as f64 * seconds) as usize)
            .flat_map(|n| {
                let sample = (0.35
                    * (std::f64::consts::TAU * frequency * n as f64 / rate as f64).sin()
                    * 32767.0) as i16;
                sample.to_le_bytes()
            })
            .collect();
        SynthesizedAudio {
            pcm_s16le,
            sample_rate: rate,
            channels: 1,
        }
    }
    // Independent zero-crossing measurement of steady-state physical frequency.
    fn frequency(audio: &SynthesizedAudio) -> f64 {
        let samples: Vec<i16> = audio
            .pcm_s16le
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let middle = &samples[samples.len() / 4..samples.len() * 3 / 4];
        let crossings: Vec<usize> = middle
            .windows(2)
            .enumerate()
            .filter_map(|(n, s)| (s[0] <= 0 && s[1] > 0).then_some(n))
            .collect();
        assert!(
            crossings.len() > 20,
            "output must contain sustained audible energy"
        );
        (crossings.len() - 1) as f64 * audio.sample_rate as f64
            / (crossings.last().unwrap() - crossings[0]) as f64
    }
    #[test]
    fn pitch_changes_frequency_without_changing_duration_or_sample_rate() {
        for rate in [16_000, 22_050, 24_000, 48_000] {
            for factor in [0.5, 1.5, 2.0] {
                let source = tone(rate, 440.0, 2.0);
                let bytes = source.pcm_s16le.len();
                let audio = transform(source, 1.0, factor, &AtomicBool::new(false)).unwrap();
                assert_eq!(audio.pcm_s16le.len(), bytes);
                assert_eq!(audio.sample_rate, rate);
                assert!((frequency(&audio) - 440.0 * f64::from(factor)).abs() < 4.0);
            }
        }
    }
    #[test]
    fn speed_changes_duration_without_changing_pitch_and_combines_independently() {
        for (speed, pitch) in [(0.5, 1.0), (2.0, 1.0), (1.5, 1.5), (0.1, 0.25), (10.0, 4.0)] {
            let source = tone(24_000, 220.0, 4.0);
            let expected = (96_000.0 / f64::from(speed)).round() as usize;
            let audio = transform(source, speed, pitch, &AtomicBool::new(false)).unwrap();
            assert_eq!(audio.pcm_s16le.len(), expected * 2);
            assert!((frequency(&audio) - 220.0 * f64::from(pitch)).abs() < 5.0);
        }
    }
    #[test]
    fn neutral_is_bit_exact_and_invalid_or_cancelled_input_is_rejected() {
        let source = tone(24_000, 440.0, 0.02);
        let original = source.pcm_s16le.clone();
        assert_eq!(
            transform(source, 1.0, 1.0, &AtomicBool::new(false))
                .unwrap()
                .pcm_s16le,
            original
        );
        for factor in [f32::NAN, f32::INFINITY, 0.0, -1.0] {
            assert!(validate_factors(factor, 1.0).is_err());
            assert!(validate_factors(1.0, factor).is_err());
        }
        assert!(transform(tone(24_000, 440.0, 1.0), 1.0, 2.0, &AtomicBool::new(true)).is_err());
    }
    #[test]
    fn short_transformed_sounds_are_bounded_and_finite() {
        let audio =
            transform(tone(24_000, 440.0, 0.01), 2.0, 1.5, &AtomicBool::new(false)).unwrap();
        assert_eq!(audio.pcm_s16le.len(), 240);
    }
}
