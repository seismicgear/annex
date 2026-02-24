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
                "Model file not found: {:?}",
                model_path
            )));
        }

        if profile.speed < 0.1 || profile.speed > 10.0 {
            return Err(VoiceError::Config(
                "Speed must be between 0.1 and 10.0".to_string(),
            ));
        }

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
            .stderr(Stdio::piped());

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
            .map_err(|e| VoiceError::Tts(format!("Failed to spawn piper: {}", e)))?;

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
            .map_err(|e| VoiceError::Tts(format!("Failed to wait for piper: {}", e)))?;

        // Ensure writing finished successfully (or propagate error)
        match write_task.await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                return Err(VoiceError::Tts(format!(
                    "Failed to write to piper stdin: {}",
                    e
                )))
            }
            Err(e) => return Err(VoiceError::Tts(format!("Stdin task failed: {}", e))),
        }

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VoiceError::Tts(format!("Piper failed: {}", stderr)));
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

        let mut command = Command::new(&self.bark_binary);
        command
            .arg("--text")
            .arg(text)
            .arg("--output_raw")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = command
            .spawn()
            .map_err(|e| VoiceError::Tts(format!("Failed to spawn bark: {}", e)))?;

        let output = tokio::time::timeout(TTS_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                VoiceError::Tts(format!(
                    "Bark TTS process timed out after {} seconds",
                    TTS_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|e| VoiceError::Tts(format!("Failed to wait for bark: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VoiceError::Tts(format!("Bark failed: {}", stderr)));
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
        let mut command = Command::new("espeak-ng");
        command
            .arg("--stdout")
            .arg(text)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = command
            .spawn()
            .map_err(|e| VoiceError::Tts(format!("Failed to spawn espeak-ng: {}", e)))?;

        let output = tokio::time::timeout(TTS_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                VoiceError::Tts(format!(
                    "System TTS process timed out after {} seconds",
                    TTS_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|e| VoiceError::Tts(format!("Failed to wait for espeak-ng: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VoiceError::Tts(format!("espeak-ng failed: {}", stderr)));
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
