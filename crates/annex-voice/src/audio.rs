//! Audio conversion for the speech-to-text path.
//!
//! The SFU decodes every inbound Opus track at 48 kHz mono and hands the
//! result to the STT tap as raw little-endian signed 16-bit PCM
//! ([`crate::SttTapFrame::pcm_s16le`]). whisper.cpp cannot read that.
//! Its loader — `read_audio_data` in `examples/common.cpp` — does three
//! things to whatever arrives on stdin, in this order:
//!
//! 1. `drwav_init_memory(...)`, which fails outright on headerless PCM
//!    (`error: failed to open WAV file from stdin`);
//! 2. rejects anything whose `sampleRate != COMMON_SAMPLE_RATE`, and
//!    `COMMON_SAMPLE_RATE` is 16000 (`must be 16 kHz`);
//! 3. rejects anything whose `bitsPerSample != 16`.
//!
//! So the tap's bytes failed on the first check and would have failed on
//! the second. This module is the adapter: band-limit, decimate 48 kHz →
//! 16 kHz, and wrap the result in a canonical 44-byte RIFF/WAVE header.
//!
//! The decimation is not a plain "keep every third sample". Dropping two
//! of every three samples without first removing everything above the new
//! Nyquist (8 kHz) folds 8–24 kHz content back down into the speech band
//! as aliasing, and the artefacts land exactly where a small whisper model
//! is least robust. A linear-phase windowed-sinc low-pass runs first.

use std::sync::OnceLock;

/// Sample rate the SFU's Opus decoder produces.
pub const SFU_SAMPLE_RATE: u32 = 48_000;

/// Sample rate whisper.cpp requires (`COMMON_SAMPLE_RATE`).
pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// 48000 / 16000. Both rates are fixed, so this is exact.
const DECIMATION: usize = (SFU_SAMPLE_RATE / WHISPER_SAMPLE_RATE) as usize;

/// Half-length of the anti-aliasing FIR. 24 gives 49 taps — enough
/// stopband attenuation for speech (>50 dB with a Hamming window) at a
/// cost of ~800k multiply-accumulates per second of audio, which is
/// nothing beside the whisper inference that follows.
const FIR_HALF: usize = 24;
const FIR_LEN: usize = 2 * FIR_HALF + 1;

/// Cutoff, in cycles per sample at 48 kHz. 7.2 kHz — 800 Hz of
/// transition band below the 8 kHz Nyquist of the 16 kHz output, so the
/// passband stays flat across the whole range speech actually occupies
/// and the fold-back is already deep in the stopband.
const FIR_CUTOFF_NORM: f32 = 7_200.0 / SFU_SAMPLE_RATE as f32;

fn fir() -> &'static [f32; FIR_LEN] {
    static FIR: OnceLock<[f32; FIR_LEN]> = OnceLock::new();
    FIR.get_or_init(|| {
        let mut h = [0f32; FIR_LEN];
        let mut sum = 0f32;
        for (i, tap) in h.iter_mut().enumerate() {
            let n = i as f32 - FIR_HALF as f32;
            // sinc(2*fc*n), with the removable singularity at n == 0.
            let x = 2.0 * FIR_CUTOFF_NORM * n;
            let sinc = if n == 0.0 {
                1.0
            } else {
                (std::f32::consts::PI * x).sin() / (std::f32::consts::PI * x)
            };
            // Hamming window.
            let w =
                0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (FIR_LEN - 1) as f32).cos();
            *tap = 2.0 * FIR_CUTOFF_NORM * sinc * w;
            sum += *tap;
        }
        // Normalise to unit DC gain. Without this the window scales the
        // passband by a few percent and every transcription is quietly
        // attenuated or boosted.
        for tap in h.iter_mut() {
            *tap /= sum;
        }
        h
    })
}

/// Decode little-endian signed 16-bit PCM bytes into samples.
///
/// A trailing odd byte is a truncated sample and is dropped: it cannot be
/// completed, and carrying it forward would shift every subsequent sample
/// by one byte and turn the rest of the buffer into noise.
pub fn s16le_to_samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// Band-limit and decimate 48 kHz mono samples to 16 kHz mono.
///
/// Input outside the FIR's reach is edge-extended rather than
/// zero-padded: a window of speech does not begin and end in silence, and
/// zero-padding puts a step discontinuity at both ends of every window,
/// which the model hears as a click.
pub fn downsample_48k_to_16k(input: &[i16]) -> Vec<i16> {
    if input.is_empty() {
        return Vec::new();
    }
    let h = fir();
    let out_len = input.len().div_ceil(DECIMATION);
    let mut out = Vec::with_capacity(out_len);
    let last = input.len() - 1;
    for m in 0..out_len {
        let center = m * DECIMATION;
        let mut acc = 0f32;
        for (k, tap) in h.iter().enumerate() {
            let idx = center as isize + k as isize - FIR_HALF as isize;
            let idx = idx.clamp(0, last as isize) as usize;
            acc += *tap * input[idx] as f32;
        }
        out.push(acc.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16);
    }
    out
}

/// Wrap 16-bit mono samples in a canonical 44-byte RIFF/WAVE header.
///
/// `drwav_init_memory` needs the container; the fields below are the
/// exact set it reads back out (`channels`, `sampleRate`,
/// `bitsPerSample`) plus the sizes that make the chunk walk terminate.
pub fn wav_pcm16_mono(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    // Everything after this field: 4 ("WAVE") + 24 (fmt chunk) + 8 (data
    // header) + payload.
    out.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // WAVE_FORMAT_PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// The whole adapter: 48 kHz mono s16-le tap bytes in, a 16 kHz mono WAV
/// whisper.cpp will accept out.
pub fn sfu_pcm_to_whisper_wav(pcm_s16le_48k: &[u8]) -> Vec<u8> {
    let samples = s16le_to_samples(pcm_s16le_48k);
    let resampled = downsample_48k_to_16k(&samples);
    wav_pcm16_mono(&resampled, WHISPER_SAMPLE_RATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, secs: f32, rate: f32) -> Vec<i16> {
        let n = (secs * rate) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate;
                ((2.0 * std::f32::consts::PI * freq * t).sin() * 12000.0) as i16
            })
            .collect()
    }

    /// Rough RMS of a band, by correlating against a reference tone.
    fn amplitude_at(samples: &[i16], freq: f32, rate: f32) -> f32 {
        let mut re = 0f64;
        let mut im = 0f64;
        for (i, s) in samples.iter().enumerate() {
            let t = i as f64 / rate as f64;
            let w = 2.0 * std::f64::consts::PI * freq as f64 * t;
            re += *s as f64 * w.cos();
            im += *s as f64 * w.sin();
        }
        (2.0 * (re * re + im * im).sqrt() / samples.len() as f64) as f32
    }

    #[test]
    fn the_header_is_what_drwav_reads_back() {
        // Field-by-field against `read_audio_data` in whisper.cpp's
        // examples/common.cpp, which rejects on channels, sampleRate and
        // bitsPerSample in that order.
        let wav = wav_pcm16_mono(&[1, -1, 2, -2], WHISPER_SAMPLE_RATE);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 1);
        assert_eq!(
            u16::from_le_bytes(wav[22..24].try_into().unwrap()),
            1,
            "mono"
        );
        assert_eq!(
            u32::from_le_bytes(wav[24..28].try_into().unwrap()),
            16_000,
            "whisper.cpp rejects anything that is not 16 kHz",
        );
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 8);
        // The RIFF size counts everything after itself.
        assert_eq!(
            u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
            wav.len() - 8,
        );
        assert_eq!(wav.len(), 44 + 8);
    }

    #[test]
    fn an_empty_buffer_still_produces_a_readable_container() {
        // A speaker who says nothing must not produce a malformed file.
        let wav = wav_pcm16_mono(&[], WHISPER_SAMPLE_RATE);
        assert_eq!(wav.len(), 44);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
    }

    #[test]
    fn speech_band_content_survives_the_downsample() {
        // 1 kHz is squarely inside the passband; it must come through at
        // essentially full amplitude, not merely "present".
        let input = tone(1_000.0, 0.25, 48_000.0);
        let out = downsample_48k_to_16k(&input);
        assert_eq!(out.len(), input.len() / 3);
        let before = amplitude_at(&input, 1_000.0, 48_000.0);
        let after = amplitude_at(&out, 1_000.0, 16_000.0);
        let ratio = after / before;
        assert!(
            (0.93..=1.07).contains(&ratio),
            "1 kHz passband gain {ratio:.3} is outside +/-7%",
        );
    }

    #[test]
    fn content_above_the_new_nyquist_does_not_fold_into_the_speech_band() {
        // This is the whole reason the filter exists. A 12 kHz tone
        // decimated 3:1 without band-limiting aliases to |12000 - 16000|
        // = 4 kHz, right on top of speech. Assert it does not.
        let input = tone(12_000.0, 0.25, 48_000.0);
        let out = downsample_48k_to_16k(&input);
        let source = amplitude_at(&input, 12_000.0, 48_000.0);
        let alias = amplitude_at(&out, 4_000.0, 16_000.0);
        assert!(
            alias < source * 0.01,
            "12 kHz folded back to 4 kHz at {:.1}% of its original amplitude",
            100.0 * alias / source,
        );

        // And the naive version really does alias, so the test above is
        // measuring the filter rather than an artefact of the harness.
        let naive: Vec<i16> = input.iter().step_by(3).copied().collect();
        let naive_alias = amplitude_at(&naive, 4_000.0, 16_000.0);
        assert!(
            naive_alias > source * 0.5,
            "plain 3:1 decimation should alias badly; measured {:.1}%",
            100.0 * naive_alias / source,
        );
    }

    #[test]
    fn a_trailing_odd_byte_is_dropped_rather_than_shifting_the_stream() {
        let samples = s16le_to_samples(&[0x01, 0x02, 0x03]);
        assert_eq!(samples, vec![i16::from_le_bytes([0x01, 0x02])]);
    }

    #[test]
    fn the_full_adapter_produces_exactly_what_whisper_accepts() {
        // 480 ms at 48 kHz -> 160 ms of samples at 16 kHz.
        let pcm: Vec<u8> = tone(440.0, 0.48, 48_000.0)
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let wav = sfu_pcm_to_whisper_wav(&pcm);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        let frames = u32::from_le_bytes(wav[40..44].try_into().unwrap()) / 2;
        assert_eq!(frames, (0.48 * 16_000.0) as u32);
    }
}
