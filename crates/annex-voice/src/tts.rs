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
/// The real local Bark model may take over a minute on supported CPU hosts.
/// Keep a finite cancellation/reaping deadline without truncating normal output.
const BARK_TTS_TIMEOUT: Duration = Duration::from_secs(180);

/// PCM together with the format produced by the selected speech model.
pub struct SynthesizedAudio {
    pub pcm_s16le: Vec<u8>,
    pub sample_rate: usize,
    pub channels: usize,
}

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
    pub async fn provision_default_profile(&self) -> Result<VoiceModel, VoiceError> {
        if let Some(existing) = self.get_profile("default").await {
            self.validate_profile(&existing).await?;
            return Ok(existing.model);
        }
        let (model, model_path, config_path) = if self.piper_available() {
            let (m, c) = self
                .first_piper_model()
                .expect("piper_available checked a model exists");
            (VoiceModel::Piper, m, c)
        } else {
            // Empty System model_path selects the checked native en-us voice.
            (VoiceModel::System, String::new(), None)
        };
        let profile = VoiceProfile {
            id: "default".into(),
            name: "Default".into(),
            model,
            model_path,
            config_path,
            speed: 1.0,
            pitch: 1.0,
            speaker_id: None,
        };
        self.validate_profile(&profile).await?;
        self.add_profile(profile).await;
        Ok(model)
    }

    /// Validate the exact profile the caller intends to load or assign.
    pub async fn validate_profile(&self, profile: &VoiceProfile) -> Result<(), VoiceError> {
        let prepared = crate::voice_profile::prepare(profile, &self.voices_dir).await?;
        self.validate_backend(&prepared).await
    }

    async fn validate_backend(
        &self,
        prepared: &crate::voice_profile::PreparedProfile,
    ) -> Result<(), VoiceError> {
        use crate::voice_profile::PreparedProfile;
        match prepared {
            PreparedProfile::Piper { .. } => {
                if !self.piper_binary.is_file() {
                    return Err(VoiceError::Config(
                        "Piper executable is not installed".into(),
                    ));
                }
            }
            PreparedProfile::Bark {
                model,
                revision,
                speaker,
                ..
            } => {
                if !self.bark_binary.is_file() {
                    return Err(VoiceError::Config(
                        "Bark executable is not installed".into(),
                    ));
                }
                let mut command = Command::new(&self.bark_binary);
                command
                    .arg("--check")
                    .arg("--model")
                    .arg(model)
                    .arg("--revision")
                    .arg(revision);
                if let Some(id) = speaker {
                    command.arg("--speaker").arg(id.to_string());
                }
                let receipt =
                    run_provider(command, None, Duration::from_secs(20), "Bark readiness").await?;
                let receipt: serde_json::Value =
                    serde_json::from_slice(&receipt).map_err(|_| {
                        VoiceError::Config(
                            "Bark wrapper does not support the required offline --check contract"
                                .into(),
                        )
                    })?;
                if receipt["model"] != *model
                    || receipt["revision"] != *revision
                    || receipt["speaker"] != serde_json::json!(speaker)
                    || receipt["sample_rate"] != 24_000
                    || receipt["weights_present"] != true
                {
                    return Err(VoiceError::Config(
                        "Bark readiness did not confirm the selected pinned model and speaker"
                            .into(),
                    ));
                }
            }
            PreparedProfile::System { voice, .. } => {
                let mut command = Command::new("espeak-ng");
                command.arg("--voices");
                let voices = run_provider(
                    command,
                    None,
                    Duration::from_secs(10),
                    "System voice catalog",
                )
                .await?;
                let mut command = Command::new("espeak-ng");
                command.arg("--voices=variant");
                let variants = run_provider(
                    command,
                    None,
                    Duration::from_secs(10),
                    "System variant catalog",
                )
                .await?;
                if !crate::voice_profile::system_voice_exists(
                    voice,
                    &String::from_utf8_lossy(&voices),
                    &String::from_utf8_lossy(&variants),
                ) {
                    return Err(VoiceError::Config(format!(
                        "System voice is not installed: {voice}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Returns native-format PCM using one immutable profile/config snapshot.
    pub async fn synthesize(&self, text: &str, profile_id: &str) -> Result<Vec<u8>, VoiceError> {
        Ok(self.synthesize_audio(text, profile_id).await?.pcm_s16le)
    }

    pub async fn synthesize_audio(
        &self,
        text: &str,
        profile_id: &str,
    ) -> Result<SynthesizedAudio, VoiceError> {
        use crate::voice_profile::PreparedProfile;
        if text.trim().is_empty() || text.len() > MAX_TTS_INPUT_BYTES {
            return Err(VoiceError::Tts(format!(
                "Text requires 1 to {MAX_TTS_INPUT_BYTES} nonblank UTF-8 bytes"
            )));
        }
        let profile = self
            .get_profile(profile_id)
            .await
            .ok_or_else(|| VoiceError::ProfileNotFound(profile_id.into()))?;
        let prepared = crate::voice_profile::prepare(&profile, &self.voices_dir).await?;
        self.validate_backend(&prepared).await?;
        let (audio, speed, pitch) = match prepared {
            PreparedProfile::Piper {
                model,
                config,
                sample_rate,
                speaker,
                speed,
                pitch,
            } => {
                // The same bytes determine format and synthesis, even when an
                // operator edits the original JSON while the child is running.
                let private = tempfile::tempdir().map_err(|e| VoiceError::Tts(e.to_string()))?;
                let config_path = private.path().join("voice.json");
                tokio::fs::write(&config_path, &config)
                    .await
                    .map_err(|e| VoiceError::Tts(e.to_string()))?;
                let mut command = Command::new(&self.piper_binary);
                command
                    .arg("--model")
                    .arg(model)
                    .arg("--config")
                    .arg(config_path)
                    .arg("--output_raw")
                    .arg("--length_scale")
                    // Native length_scale affects phonemes but leaves fixed
                    // timing outside the multiplier. Apply the shared physical
                    // speed/pitch factors to the complete neutral utterance.
                    .arg("1");
                if let Some(speaker) = speaker {
                    command.arg("--speaker").arg(speaker.to_string());
                }
                let pcm_s16le =
                    run_provider(command, Some(text.as_bytes()), TTS_TIMEOUT, "Piper").await?;
                (
                    SynthesizedAudio {
                        pcm_s16le,
                        sample_rate,
                        channels: 1,
                    },
                    speed,
                    pitch,
                )
            }
            PreparedProfile::Bark {
                model,
                revision,
                speaker,
                speed,
                pitch,
            } => {
                let mut command = Command::new(&self.bark_binary);
                command
                    .arg("--model")
                    .arg(model)
                    .arg("--revision")
                    .arg(revision)
                    .arg("--text")
                    .arg(text)
                    .arg("--output_raw");
                if let Some(speaker) = speaker {
                    command.arg("--speaker").arg(speaker.to_string());
                }
                let pcm_s16le = run_provider(command, None, BARK_TTS_TIMEOUT, "Bark").await?;
                (
                    SynthesizedAudio {
                        pcm_s16le,
                        sample_rate: 24_000,
                        channels: 1,
                    },
                    speed,
                    pitch,
                )
            }
            PreparedProfile::System {
                voice,
                speed,
                pitch,
            } => {
                let mut command = Command::new("espeak-ng");
                // espeak's -p is an arbitrary 0..99 control, not a frequency
                // multiplier. Use an explicit native baseline and independent
                // DSP factors instead of pretending -p(50*pitch) is equivalent.
                command
                    .arg("--stdout")
                    .arg("-v")
                    .arg(voice)
                    .arg("-s")
                    .arg("175")
                    .arg("-p")
                    .arg("50")
                    .arg("--stdin");
                let wav =
                    run_provider(command, Some(text.as_bytes()), TTS_TIMEOUT, "System").await?;
                (decode_pcm_wav(&wav)?, speed, pitch)
            }
        };
        use std::sync::atomic::{AtomicBool, Ordering};
        struct CancelOnDrop(Arc<AtomicBool>);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let guard = CancelOnDrop(cancelled.clone());
        let result = tokio::task::spawn_blocking(move || {
            crate::speech_dsp::transform(audio, speed, pitch, &cancelled)
        })
        .await
        .map_err(|e| VoiceError::Tts(format!("Speech processing task failed: {e}")))?;
        drop(guard);
        result
    }
}

async fn run_provider(
    mut command: Command,
    input: Option<&[u8]>,
    timeout: Duration,
    label: &str,
) -> Result<Vec<u8>, VoiceError> {
    use tokio::io::AsyncReadExt;
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| VoiceError::Tts(format!("Failed to spawn {label}: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| VoiceError::Tts("Provider stdout missing".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| VoiceError::Tts("Provider stderr missing".into()))?;
    let mut stdin = child.stdin.take();
    // Bound memory before collection. If a child exceeds either limit, drop
    // its pipe, kill it, and await reaping instead of waiting for a deadlock.
    let operation = async {
        let write = async {
            if let (Some(mut pipe), Some(input)) = (stdin.take(), input) {
                pipe.write_all(input).await?;
                pipe.shutdown().await?;
            }
            Ok::<(), std::io::Error>(())
        };
        let read = async {
            let mut bytes = Vec::new();
            stdout
                .take(96 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .await?;
            if bytes.len() > 96 * 1024 * 1024 {
                return Err(std::io::Error::other(
                    "Provider audio exceeds the output limit",
                ));
            }
            Ok::<_, std::io::Error>(bytes)
        };
        let errors = async {
            let mut bytes = Vec::new();
            stderr.take(1024 * 1024 + 1).read_to_end(&mut bytes).await?;
            if bytes.len() > 1024 * 1024 {
                return Err(std::io::Error::other(
                    "Provider diagnostic output exceeds the limit",
                ));
            }
            Ok::<_, std::io::Error>(bytes)
        };
        let ((), bytes, errors) = tokio::try_join!(write, read, errors)?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, bytes, errors))
    };
    match tokio::time::timeout(timeout, operation).await {
        Ok(Ok((status, bytes, _errors))) if status.success() => Ok(bytes),
        Ok(Ok((_, _, errors))) => Err(VoiceError::Tts(format!(
            "{label} failed: {}",
            String::from_utf8_lossy(&errors)
        ))),
        failure => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(VoiceError::Tts(match failure {
                Ok(Err(error)) => format!("{label} process failed: {error}"),
                _ => format!(
                    "{label} process timed out after {} seconds",
                    timeout.as_secs()
                ),
            }))
        }
    }
}

fn decode_pcm_wav(wav: &[u8]) -> Result<SynthesizedAudio, VoiceError> {
    let invalid = || VoiceError::Tts("Speech engine returned an invalid 16-bit PCM WAV".into());
    if wav.len() < 12 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err(invalid());
    }
    let mut offset = 12usize;
    let mut format = None;
    while offset.checked_add(8).is_some_and(|end| end <= wav.len()) {
        let kind = &wav[offset..offset + 4];
        let size = u32::from_le_bytes(wav[offset + 4..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        // espeak streams stdout with an unknown data length in its header.
        let available = size.min(wav.len() - offset);
        if kind == b"fmt " && available >= 16 {
            let encoding = u16::from_le_bytes(wav[offset..offset + 2].try_into().unwrap());
            let channels =
                u16::from_le_bytes(wav[offset + 2..offset + 4].try_into().unwrap()) as usize;
            let rate = u32::from_le_bytes(wav[offset + 4..offset + 8].try_into().unwrap()) as usize;
            let bits = u16::from_le_bytes(wav[offset + 14..offset + 16].try_into().unwrap());
            if encoding != 1 || bits != 16 || channels == 0 || !(8_000..=192_000).contains(&rate) {
                return Err(invalid());
            }
            format = Some((rate, channels));
        } else if kind == b"data" {
            let (sample_rate, channels) = format.ok_or_else(invalid)?;
            if !available.is_multiple_of(2 * channels) {
                return Err(invalid());
            }
            return Ok(SynthesizedAudio {
                pcm_s16le: wav[offset..offset + available].to_vec(),
                sample_rate,
                channels,
            });
        }
        offset = offset
            .checked_add(size)
            .and_then(|n| n.checked_add(size % 2))
            .ok_or_else(invalid)?;
    }
    Err(invalid())
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

    if !pcm_s16le.len().is_multiple_of(2) {
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

    #[tokio::test]
    async fn bark_provider_receives_pinned_model_revision_and_speaker() {
        let directory = tempfile::tempdir().unwrap();
        let receipt = directory.path().join("arguments.jsonl");
        let script=format!("import json,sys\na=sys.argv\nwith open({},'a') as f: f.write(json.dumps(a[1:])+'\\n')\nif '--check' in a: print(json.dumps({{'model':a[a.index('--model')+1],'revision':a[a.index('--revision')+1],'speaker':int(a[a.index('--speaker')+1]),'sample_rate':24000,'weights_present':True}}))\nelse: sys.stdout.buffer.write(bytes(4800))\n",serde_json::to_string(&receipt.to_string_lossy()).unwrap());
        let binary = crate::test_support::python_process(directory.path(), "bark", &script);
        let tts = TtsService::new(directory.path(), "piper", &binary);
        let profile = VoiceProfile {
            id: "bark".into(),
            name: "Bark".into(),
            model: VoiceModel::Bark,
            model_path: String::new(),
            config_path: None,
            speed: 1.0,
            pitch: 1.0,
            speaker_id: Some(4),
        };
        tts.add_profile(profile).await;
        let audio = tts
            .synthesize_audio("A provider contract test.", "bark")
            .await
            .unwrap();
        assert_eq!(audio.sample_rate, 24_000);
        assert_eq!(audio.pcm_s16le.len(), 4800);
        let rows: Vec<Vec<String>> = std::fs::read_to_string(receipt)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains(&"--check".into()));
        for args in rows {
            let value =
                |flag: &str| args[args.iter().position(|arg| arg == flag).unwrap() + 1].as_str();
            assert_eq!(value("--model"), crate::voice_profile::BARK_MODEL);
            assert_eq!(value("--revision"), crate::voice_profile::BARK_REVISION);
            assert_eq!(value("--speaker"), "4");
        }
    }

    #[tokio::test]
    async fn concurrent_profile_and_config_changes_do_not_change_inflight_audio_format() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        let release = directory.path().join("release");
        let receipt = directory.path().join("actual-config.json");
        let quoted = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
        let script=format!("import json,sys,time,pathlib\nsys.stdin.buffer.read()\npathlib.Path({}).write_text('ready')\nwhile not pathlib.Path({}).exists(): time.sleep(0.01)\na=sys.argv\nc=json.loads(pathlib.Path(a[a.index('--config')+1]).read_text())\npathlib.Path({}).write_text(json.dumps({{'config':c,'args':a[1:]}}))\nsys.stdout.buffer.write(bytes(4800))\n",quoted(&ready),quoted(&release),quoted(&receipt));
        let binary =
            crate::test_support::python_process(directory.path(), "piper-snapshot", &script);
        std::fs::write(directory.path().join("voice.onnx"), b"model fixture").unwrap();
        let config = directory.path().join("voice.onnx.json");
        std::fs::write(
            &config,
            br#"{"audio":{"sample_rate":24000},"num_speakers":2}"#,
        )
        .unwrap();
        let tts = TtsService::new(directory.path(), binary, "bark");
        let profile = VoiceProfile {
            id: "snapshot".into(),
            name: "Snapshot".into(),
            model: VoiceModel::Piper,
            model_path: "voice.onnx".into(),
            config_path: None,
            speed: 2.0,
            pitch: 1.0,
            speaker_id: Some(1),
        };
        tts.add_profile(profile.clone()).await;
        let task = {
            let tts = tts.clone();
            tokio::spawn(async move {
                tts.synthesize_audio("A bounded snapshot.", "snapshot")
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        std::fs::write(
            &config,
            br#"{"audio":{"sample_rate":48000},"num_speakers":1}"#,
        )
        .unwrap();
        tts.add_profile(VoiceProfile {
            speed: 0.5,
            speaker_id: None,
            ..profile
        })
        .await;
        std::fs::write(release, b"continue").unwrap();
        let audio = task.await.unwrap().unwrap();
        assert_eq!(audio.sample_rate, 24_000);
        assert_eq!(
            audio.pcm_s16le.len(),
            2400,
            "the captured speed of 2 scales the whole utterance"
        );
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(receipt).unwrap()).unwrap();
        assert_eq!(value["config"]["audio"]["sample_rate"], 24_000);
        let args = value["args"].as_array().unwrap();
        assert_eq!(
            args[args.iter().position(|x| x == "--length_scale").unwrap() + 1],
            "1"
        );
        assert_eq!(
            args[args.iter().position(|x| x == "--speaker").unwrap() + 1],
            "1"
        );
    }

    #[tokio::test]
    async fn speech_format_comes_from_the_piper_model() {
        let dir = tempfile::tempdir().unwrap();
        let binary = crate::test_support::python_process(
            dir.path(),
            "piper",
            "import sys\nsys.stdin.buffer.read()\nsys.stdout.buffer.write(bytes(4410))\n",
        );
        std::fs::write(dir.path().join("voice.onnx"), b"fixture").unwrap();
        std::fs::write(
            dir.path().join("voice.onnx.json"),
            br#"{"audio":{"sample_rate":22050}}"#,
        )
        .unwrap();
        let service = TtsService::new(dir.path(), &binary, "bark");
        service
            .add_profile(VoiceProfile {
                id: "format".into(),
                name: "format".into(),
                model: VoiceModel::Piper,
                model_path: "voice.onnx".into(),
                config_path: None,
                speed: 1.0,
                pitch: 1.0,
                speaker_id: None,
            })
            .await;
        let audio = service.synthesize_audio("test", "format").await.unwrap();
        assert_eq!(audio.sample_rate, 22_050);
        assert_eq!(audio.channels, 1);
        assert_eq!(
            encode_pcm_to_opus_frames(&audio.pcm_s16le, audio.sample_rate, audio.channels)
                .unwrap()
                .len(),
            5,
            "100 ms must remain 100 ms"
        );
    }

    #[test]
    fn system_speech_uses_wav_format_and_excludes_headers() {
        let wav = crate::audio::sfu_pcm_to_whisper_wav(&vec![0; 960 * 2]);
        let audio = decode_pcm_wav(&wav).unwrap();
        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.pcm_s16le.len(), 640);
    }

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
    async fn provision_default_profile_registers_checked_model() {
        let directory = tempfile::tempdir().unwrap();
        let binary = crate::test_support::python_process(
            directory.path(),
            "piper-default",
            "print('provider contract fixture')",
        );
        std::fs::write(directory.path().join("voice.onnx"), b"model fixture").unwrap();
        std::fs::write(
            directory.path().join("voice.onnx.json"),
            br#"{"audio":{"sample_rate":22050},"num_speakers":1}"#,
        )
        .unwrap();
        let tts = TtsService::new(directory.path(), binary, "bark");
        assert_eq!(
            tts.provision_default_profile().await.unwrap(),
            VoiceModel::Piper
        );
        assert_eq!(tts.profile_count().await, 1);
        tts.provision_default_profile().await.unwrap();
        assert_eq!(tts.profile_count().await, 1);
    }

    #[tokio::test]
    #[ignore = "requires real espeak-ng on PATH; run explicitly with --ignored"]
    async fn agent_voice_synthesizes_via_default_profile() {
        // End-to-end agent voice: provision the default profile, then
        // synthesize — proving an agent's speech actually produces audio
        // (closes P4-VOICE-3, whose old integration test asserted the failure).
        assert!(
            espeak_available(),
            "this real-provider test requires espeak-ng on PATH"
        );
        let tts = TtsService::new("nonexistent_voices_dir", "nonexistent_piper", "bark");
        tts.provision_default_profile().await.unwrap();
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
