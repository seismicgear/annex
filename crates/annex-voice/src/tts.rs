use crate::error::VoiceError;
use annex_types::voice::{VoiceModel, VoiceProfile};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::RwLock;

/// Maximum text input size for TTS (64 KiB). Prevents resource exhaustion from
/// oversized synthesis requests.
const MAX_TTS_INPUT_BYTES: usize = 64 * 1024;

/// Timeout for TTS process execution.
const TTS_TIMEOUT: Duration = Duration::from_secs(60);

/// Service for generating speech from text.
#[derive(Debug, Clone)]
pub struct TtsService {
    profiles: Arc<RwLock<HashMap<String, VoiceProfile>>>,
    voices_dir: PathBuf,
    piper_binary: PathBuf,
    bark_binary: PathBuf,
}

impl TtsService {
    /// Creates a new `TtsService` with the given voices directory, piper binary path,
    /// and bark binary path.
    pub fn new(
        voices_dir: impl AsRef<Path>,
        piper_binary: impl AsRef<Path>,
        bark_binary: impl AsRef<Path>,
    ) -> Self {
        Self {
            profiles: Arc::new(RwLock::new(HashMap::new())),
            voices_dir: voices_dir.as_ref().to_path_buf(),
            piper_binary: piper_binary.as_ref().to_path_buf(),
            bark_binary: bark_binary.as_ref().to_path_buf(),
        }
    }

    /// Adds a voice profile to the service.
    pub async fn add_profile(&self, profile: VoiceProfile) {
        self.profiles
            .write()
            .await
            .insert(profile.id.clone(), profile);
    }

    /// Retrieves a voice profile by ID.
    pub async fn get_profile(&self, id: &str) -> Option<VoiceProfile> {
        self.profiles.read().await.get(id).cloned()
    }

    /// Number of registered profiles (observability / tests).
    pub async fn profile_count(&self) -> usize {
        self.profiles.read().await.len()
    }

    /// Returns the first `.onnx` voice model in `voices_dir`, if any. Used to
    /// pick a concrete model for the Piper default profile.
    fn first_piper_model(&self) -> Option<(String, Option<String>)> {
        let entries = std::fs::read_dir(&self.voices_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("onnx") {
                let model = path.file_name()?.to_string_lossy().into_owned();
                let cfg = format!("{model}.json");
                let cfg_opt = if self.voices_dir.join(&cfg).exists() {
                    Some(cfg)
                } else {
                    None
                };
                return Some((model, cfg_opt));
            }
        }
        None
    }

    /// Whether a usable Piper backend is present (binary + at least one model).
    pub fn piper_available(&self) -> bool {
        self.piper_binary.exists() && self.first_piper_model().is_some()
    }

    /// Registers a built-in `"default"` voice profile if one is not already
    /// present, so agent speech always has a profile to synthesize with.
    ///
    /// This closes the P4-VOICE-3 gap: the WS voice handler falls back to the
    /// profile id `"default"` when an agent has no per-agent profile, but
    /// nothing ever registered that id, so synthesis failed with
    /// `ProfileNotFound` (the "TTS failed" path) before any backend was tried.
    ///
    /// Backend selection: prefer **Piper** when its binary and a voice model
    /// are present (the production default); otherwise fall back to **System**
    /// (espeak-ng), which needs no model file and works on a fresh install.
    /// Returns the [`VoiceModel`] chosen.
    pub async fn provision_default_profile(&self) -> VoiceModel {
        if let Some(existing) = self.get_profile("default").await {
            return existing.model;
        }
        let (model, model_path, config_path) = if self.piper_available() {
            let (m, c) = self
                .first_piper_model()
                .expect("piper_available checked a model exists");
            (VoiceModel::Piper, m, c)
        } else {
            // System/espeak-ng ignores model_path; leave it empty.
            (VoiceModel::System, String::new(), None)
        };
        self.add_profile(VoiceProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            model,
            model_path,
            config_path,
            speed: 1.0,
            pitch: 1.0,
            speaker_id: None,
        })
        .await;
        model
    }

    /// Synthesizes speech from the given text using the specified profile.
    ///
    /// Returns raw PCM audio data (s16le, usually 22050Hz depending on model).
    pub async fn synthesize(&self, text: &str, profile_id: &str) -> Result<Vec<u8>, VoiceError> {
        let profile = self
            .get_profile(profile_id)
            .await
            .ok_or_else(|| VoiceError::ProfileNotFound(profile_id.to_string()))?;

        match profile.model {
            VoiceModel::Piper => self.synthesize_piper(text, &profile).await,
            VoiceModel::Bark => self.synthesize_bark(text, &profile).await,
            VoiceModel::System => self.synthesize_system(text, &profile).await,
        }
    }

    async fn synthesize_piper(
        &self,
        text: &str,
        profile: &VoiceProfile,
    ) -> Result<Vec<u8>, VoiceError> {
        if text.len() > MAX_TTS_INPUT_BYTES {
            return Err(VoiceError::Tts(format!(
                "text exceeds maximum size: {} bytes (limit: {} bytes)",
                text.len(),
                MAX_TTS_INPUT_BYTES
            )));
        }
        let model_path = if Path::new(&profile.model_path).is_absolute() {
            PathBuf::from(&profile.model_path)
        } else {
            self.voices_dir.join(&profile.model_path)
        };

        if !model_path.exists() {
            return Err(VoiceError::Tts(format!(
                "Model file not found: {model_path:?}"
            )));
        }

        if profile.speed < 0.1 || profile.speed > 10.0 {
            return Err(VoiceError::Config(
                "Speed must be between 0.1 and 10.0".to_string(),
            ));
        }

        // `kill_on_drop(true)` ensures the piper child process is reaped
        // if the tokio future is cancelled (e.g. on TTS_TIMEOUT). Without
        // it, every timeout leaks an orphaned piper process; with the
        // 64 KiB text cap and the 60s timeout that's still a real
        // resource leak under sustained malicious inputs.
        let mut command = Command::new(&self.piper_binary);
        command
            .arg("--model")
            .arg(model_path)
            .arg("--output_raw")
            // Length scale is inverse of speed (roughly).
            // If speed is 2.0 (faster), length_scale should be 0.5 (shorter).
            .arg("--length_scale")
            .arg((1.0 / profile.speed).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // If config path is explicit, maybe pass it? Piper usually infers it as .json
        if let Some(config) = &profile.config_path {
            let config_path = if Path::new(config).is_absolute() {
                PathBuf::from(config)
            } else {
                self.voices_dir.join(config)
            };
            command.arg("--config").arg(config_path);
        }

        if let Some(speaker) = profile.speaker_id {
            command.arg("--speaker").arg(speaker.to_string());
        }

        let mut child = command
            .spawn()
            .map_err(|e| VoiceError::Tts(format!("Failed to spawn piper: {e}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| VoiceError::Tts("Failed to open stdin".to_string()))?;
        let text_owned = text.to_string();

        // Spawn a task to write to stdin to avoid deadlock if output buffer fills up
        let write_task = tokio::spawn(async move { stdin.write_all(text_owned.as_bytes()).await });

        let output = tokio::time::timeout(TTS_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                VoiceError::Tts(format!(
                    "TTS process timed out after {} seconds",
                    TTS_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|e| VoiceError::Tts(format!("Failed to wait for piper: {e}")))?;

        // Ensure writing finished successfully (or propagate error)
        match write_task.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                return Err(VoiceError::Tts(format!(
                    "Failed to write to piper stdin: {e}"
                )))
            }
            Err(e) => return Err(VoiceError::Tts(format!("Stdin task failed: {e}"))),
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VoiceError::Tts(format!("Piper failed: {stderr}")));
        }

        Ok(output.stdout)
    }

    /// Synthesizes speech using Bark (Python-based neural TTS).
    ///
    /// Expects `bark_binary` to point to a Python wrapper script that accepts
    /// `--text <text> --output_raw` and writes raw PCM (s16le) to stdout.
    async fn synthesize_bark(
        &self,
        text: &str,
        _profile: &VoiceProfile,
    ) -> Result<Vec<u8>, VoiceError> {
        if text.len() > MAX_TTS_INPUT_BYTES {
            return Err(VoiceError::Tts(format!(
                "text exceeds maximum size: {} bytes (limit: {} bytes)",
                text.len(),
                MAX_TTS_INPUT_BYTES
            )));
        }

        if self.bark_binary.as_os_str().is_empty() {
            return Err(VoiceError::Tts(
                "Bark TTS binary path is not configured. Set bark_binary_path in config \
                 or ANNEX_BARK_BINARY_PATH environment variable."
                    .to_string(),
            ));
        }

        if !self.bark_binary.exists() {
            return Err(VoiceError::Tts(format!(
                "Bark TTS binary not found: {:?}",
                self.bark_binary
            )));
        }

        // Reap on cancellation so a TTS_TIMEOUT doesn't leak an
        // orphaned Python process. Same reasoning as in
        // `synthesize_piper`.
        let mut command = Command::new(&self.bark_binary);
        command
            .arg("--text")
            .arg(text)
            .arg("--output_raw")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = command
            .spawn()
            .map_err(|e| VoiceError::Tts(format!("Failed to spawn bark: {e}")))?;

        let output = tokio::time::timeout(TTS_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                VoiceError::Tts(format!(
                    "Bark TTS process timed out after {} seconds",
                    TTS_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|e| VoiceError::Tts(format!("Failed to wait for bark: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VoiceError::Tts(format!("Bark failed: {stderr}")));
        }

        Ok(output.stdout)
    }

    /// Synthesizes speech using the system's native TTS engine.
    ///
    /// Uses `espeak-ng` as the cross-platform fallback. On Linux, `espeak-ng`
    /// outputs WAV to stdout via `--stdout`; the 44-byte WAV header is stripped
    /// to return raw PCM data.
    async fn synthesize_system(
        &self,
        text: &str,
        _profile: &VoiceProfile,
    ) -> Result<Vec<u8>, VoiceError> {
        if text.len() > MAX_TTS_INPUT_BYTES {
            return Err(VoiceError::Tts(format!(
                "text exceeds maximum size: {} bytes (limit: {} bytes)",
                text.len(),
                MAX_TTS_INPUT_BYTES
            )));
        }

        // Use espeak-ng as the cross-platform fallback. It outputs WAV to stdout
        // via --stdout; we strip the 44-byte WAV header to get raw PCM.
        //
        // `kill_on_drop(true)` reaps the espeak-ng child if the tokio
        // future is cancelled (e.g. on TTS_TIMEOUT) — same reasoning as
        // in `synthesize_piper` and `synthesize_bark`.
        let mut command = Command::new("espeak-ng");
        command
            .arg("--stdout")
            .arg(text)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = command
            .spawn()
            .map_err(|e| VoiceError::Tts(format!("Failed to spawn espeak-ng: {e}")))?;

        let output = tokio::time::timeout(TTS_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                VoiceError::Tts(format!(
                    "System TTS process timed out after {} seconds",
                    TTS_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|e| VoiceError::Tts(format!("Failed to wait for espeak-ng: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VoiceError::Tts(format!("espeak-ng failed: {stderr}")));
        }

        // Strip the 44-byte WAV header to return raw PCM data.
        let wav_data = output.stdout;
        if wav_data.len() > 44 {
            Ok(wav_data[44..].to_vec())
        } else {
            Ok(wav_data)
        }
    }
}

/// Encodes little-endian signed PCM into Opus frames (20ms packets).
pub fn encode_pcm_to_opus_frames(
    pcm_s16le: &[u8],
    input_sample_rate: usize,
    input_channels: usize,
) -> Result<Vec<Vec<u8>>, VoiceError> {
    use opus_rs::{Application, OpusEncoder};

    if input_channels == 0 {
        return Err(VoiceError::Codec(
            "input channels cannot be zero".to_string(),
        ));
    }

    if pcm_s16le.len() % 2 != 0 {
        return Err(VoiceError::Codec(
            "PCM payload must be 16-bit aligned".to_string(),
        ));
    }

    let source_samples: Vec<i16> = pcm_s16le
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();

    let mono_48k = if input_sample_rate == 48_000 && input_channels == 1 {
        source_samples
    } else {
        // Lightweight linear resampler + downmix to mono for server-side agent synthesis.
        let frames = source_samples.len() / input_channels;
        if frames == 0 {
            return Ok(Vec::new());
        }

        let mut mono: Vec<f32> = Vec::with_capacity(frames);
        for frame_idx in 0..frames {
            let mut sum = 0f32;
            for ch in 0..input_channels {
                let idx = frame_idx * input_channels + ch;
                sum += source_samples[idx] as f32;
            }
            mono.push(sum / input_channels as f32);
        }

        if input_sample_rate == 48_000 {
            mono.into_iter().map(|s| s as i16).collect()
        } else {
            let ratio = input_sample_rate as f32 / 48_000f32;
            let out_len = ((mono.len() as f32) / ratio).max(1.0) as usize;
            let mut out = Vec::with_capacity(out_len);
            for i in 0..out_len {
                let src_pos = i as f32 * ratio;
                let lo = src_pos.floor() as usize;
                let hi = (lo + 1).min(mono.len() - 1);
                let frac = src_pos - lo as f32;
                let sample = mono[lo] * (1.0 - frac) + mono[hi] * frac;
                out.push(sample.clamp(i16::MIN as f32, i16::MAX as f32) as i16);
            }
            out
        }
    };

    let mut encoder = OpusEncoder::new(48_000, 1, Application::Audio)
        .map_err(|e| VoiceError::Codec(format!("failed to create opus encoder: {e}")))?;
    encoder.use_cbr = false;

    let frame_size = 960; // 20ms @ 48kHz
    let mut packets = Vec::new();
    let mut cursor = 0;
    while cursor < mono_48k.len() {
        let remaining = mono_48k.len() - cursor;
        let take = remaining.min(frame_size);
        let mut frame = vec![0i16; frame_size];
        frame[..take].copy_from_slice(&mono_48k[cursor..cursor + take]);

        // opus-rs::OpusEncoder::encode expects normalized [-1.0, 1.0] f32
        // (it scales by 32768 internally and clamps to i16 — see
        // opus-rs-0.1.12/src/lib.rs:366). Passing raw i16-range floats
        // here drove every non-trivial sample to ±32767 inside the
        // encoder, producing maximally-clipped agent audio.
        let frame_f32: Vec<f32> = frame.into_iter().map(|s| s as f32 / 32768.0).collect();
        let mut encoded = vec![0u8; 4000];
        let written = encoder
            .encode(&frame_f32, frame_size, &mut encoded)
            .map_err(|e| VoiceError::Codec(format!("opus encode failed: {e}")))?;
        encoded.truncate(written);
        packets.push(encoded);
        cursor += take;
    }

    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a silent 16kHz mono s16le buffer of the given duration.
    fn silent_pcm_16k(duration_ms: usize) -> Vec<u8> {
        let samples = 16 * duration_ms; // 16 samples / ms
        vec![0u8; samples * 2]
    }

    /// Build a 16kHz mono s16le buffer that contains a 440Hz sine wave
    /// at half-scale amplitude (16384). This simulates real TTS output.
    fn sine_pcm_16k(duration_ms: usize) -> Vec<u8> {
        let samples = 16 * duration_ms;
        let mut out = Vec::with_capacity(samples * 2);
        for i in 0..samples {
            let phase = (i as f32) * 2.0 * std::f32::consts::PI * 440.0 / 16_000.0;
            let amp = (phase.sin() * 16_384.0) as i16;
            out.extend_from_slice(&amp.to_le_bytes());
        }
        out
    }

    fn espeak_available() -> bool {
        std::process::Command::new("espeak-ng")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn provision_default_profile_registers_default() {
        // No piper binary/model present → System default. The "default" id MUST
        // exist afterwards (the P4-VOICE-3 fix: the WS handler's "default"
        // fallback now resolves instead of ProfileNotFound).
        let tts = TtsService::new("nonexistent_voices_dir", "nonexistent_piper", "bark");
        assert!(tts.get_profile("default").await.is_none());
        let model = tts.provision_default_profile().await;
        assert_eq!(model, VoiceModel::System);
        let profile = tts
            .get_profile("default")
            .await
            .expect("default profile must exist after provisioning");
        assert_eq!(profile.id, "default");
        // Idempotent.
        tts.provision_default_profile().await;
        assert_eq!(tts.profile_count().await, 1);
    }

    #[tokio::test]
    async fn agent_voice_synthesizes_via_default_profile() {
        // End-to-end agent voice: provision the default profile, then
        // synthesize — proving an agent's speech actually produces audio
        // (closes P4-VOICE-3, whose old integration test asserted the failure).
        if !espeak_available() {
            eprintln!("[tts] skipping: espeak-ng not installed");
            return;
        }
        let tts = TtsService::new("nonexistent_voices_dir", "nonexistent_piper", "bark");
        tts.provision_default_profile().await;
        let pcm = tts
            .synthesize("Hello from an Annex agent.", "default")
            .await
            .expect("agent voice synthesis via the default profile must succeed");
        assert!(
            !pcm.is_empty(),
            "synthesized agent audio must be non-empty PCM"
        );
        // And it must be encodable into the opus frames the SFU mixer consumes
        // (espeak-ng emits 22.05kHz mono s16le).
        let frames = encode_pcm_to_opus_frames(&pcm, 22_050, 1).expect("encode agent audio");
        assert!(!frames.is_empty(), "agent audio must yield ≥1 opus frame");
    }

    #[test]
    fn encode_silent_pcm_succeeds() {
        // 60ms of silence → 3 frames of 20ms each at 48kHz mono.
        let pcm = silent_pcm_16k(60);
        let packets = encode_pcm_to_opus_frames(&pcm, 16_000, 1).expect("encode");
        assert_eq!(packets.len(), 3);
        for p in &packets {
            assert!(!p.is_empty(), "every opus frame should have ≥1 byte");
        }
    }

    #[test]
    fn encode_sine_pcm_succeeds() {
        // Real TTS-like signal — half-amplitude 440Hz tone. This used to
        // pass through the encoder as raw [-16384, 16384] floats, which
        // got internally scaled to ±32767 (full clip). Now we normalise
        // first, so the encoded packets should be well-formed.
        let pcm = sine_pcm_16k(40);
        let packets = encode_pcm_to_opus_frames(&pcm, 16_000, 1).expect("encode");
        assert_eq!(packets.len(), 2);
        for p in &packets {
            assert!(!p.is_empty(), "every opus frame should have ≥1 byte");
        }
    }

    /// End-to-end regression: the half-amplitude 440Hz tone must
    /// round-trip through encode → decode without exploding to clipped
    /// noise. The previous implementation passed raw i16-range floats
    /// to the encoder, which the encoder internally scaled by 32768
    /// before clamping — driving every non-zero sample to ±32767 and
    /// destroying the signal. We don't try to assert exact PCM equality
    /// (Opus is lossy), but the decoded peak amplitude should be in the
    /// same ballpark as the input amplitude (~16384). A maximally
    /// clipped encoder produces full-scale noise (peaks pushed to
    /// ±32767), which the round-trip below would surface as a peak
    /// noticeably above the input's 16384.
    #[test]
    fn encode_then_decode_preserves_amplitude_envelope() {
        use opus_rs::OpusDecoder;

        let pcm = sine_pcm_16k(40);
        let packets = encode_pcm_to_opus_frames(&pcm, 16_000, 1).expect("encode");

        let mut decoder = OpusDecoder::new(48_000, 1).expect("decoder");
        let mut decoded_peak: f32 = 0.0;
        let mut decoded_total_samples = 0usize;
        for pkt in &packets {
            let mut out = vec![0f32; 1920];
            if let Ok(n) = decoder.decode(pkt, 960, &mut out) {
                for s in &out[..n] {
                    decoded_peak = decoded_peak.max(s.abs());
                }
                decoded_total_samples += n;
            }
        }
        assert!(
            decoded_total_samples > 0,
            "decoder produced no samples — encoder output is unusable"
        );
        // Decoded floats are normalised [-1.0, 1.0]. Half-amplitude
        // input is ~0.5; after Opus's lossy compression the peak should
        // still be below ~0.85 (well under full-scale clip). The pre-
        // fix behaviour clipped to ±32767 / 32768 ≈ 1.0.
        assert!(
            decoded_peak < 0.85,
            "decoded peak amplitude {decoded_peak} suggests encoder is clipping the input"
        );
        assert!(
            decoded_peak > 0.2,
            "decoded peak amplitude {decoded_peak} suggests encoder is dropping the signal"
        );
    }

    #[test]
    fn encode_rejects_unaligned_pcm() {
        // 1 byte → not 16-bit aligned.
        let err = encode_pcm_to_opus_frames(&[0u8], 16_000, 1).expect_err("must reject odd-length");
        match err {
            VoiceError::Codec(msg) => assert!(msg.contains("16-bit aligned")),
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn encode_rejects_zero_channels() {
        let err =
            encode_pcm_to_opus_frames(&[0u8; 4], 16_000, 0).expect_err("must reject zero channels");
        match err {
            VoiceError::Codec(msg) => assert!(msg.contains("channels cannot be zero")),
            other => panic!("wrong error: {other:?}"),
        }
    }
}
