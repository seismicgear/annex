use crate::error::VoiceError;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Maximum audio input size for STT (10 MiB). Prevents OOM from oversized payloads.
const MAX_STT_INPUT_BYTES: usize = 10 * 1024 * 1024;

/// Timeout for STT process execution.
const STT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct SttService {
    model_path: PathBuf,
    binary_path: PathBuf,
}

impl SttService {
    pub fn new(model_path: impl Into<PathBuf>, binary_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            binary_path: binary_path.into(),
        }
    }

    /// Path to the configured GGML model. Used by health/status endpoints
    /// that need to distinguish "STT not configured" from "STT configured
    /// but model missing on disk".
    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    /// Path to the configured whisper.cpp binary.
    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    /// Returns `true` when both the model file and the whisper binary are
    /// present on disk. Reported on `/api/voice/config-status` so the
    /// client (and operators) can tell the difference between
    /// "STT configured and ready" and "STT path set but the file is
    /// missing" — the latter used to silently pretend to be ready and
    /// 500 at request time.
    pub fn is_ready(&self) -> bool {
        self.model_path.is_file() && self.binary_path.is_file()
    }

    pub async fn transcribe(&self, audio_data: &[u8]) -> Result<String, VoiceError> {
        if audio_data.len() > MAX_STT_INPUT_BYTES {
            return Err(VoiceError::Stt(format!(
                "audio data exceeds maximum size: {} bytes (limit: {} bytes)",
                audio_data.len(),
                MAX_STT_INPUT_BYTES
            )));
        }

        let mut command = Command::new(&self.binary_path);

        // Standard whisper.cpp arguments:
        // -m <model_path>: path to GGML model
        // -f -: read from stdin
        // -otxt: output text format (implied if capturing stdout, but some versions output metadata)
        // We assume the binary outputs pure text to stdout or we parse it.
        // For simplicity, we assume stdout contains the transcription.
        //
        // `kill_on_drop(true)` ensures the child process is reaped if the
        // tokio future is cancelled (e.g. on STT_TIMEOUT). Without it,
        // every timeout leaks an orphaned whisper.cpp process — under a
        // malicious workload that pushes many oversized payloads, the
        // server eventually exhausts process slots / RAM.
        command
            .arg("-m")
            .arg(&self.model_path)
            .arg("-f")
            .arg("-") // read from stdin
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| VoiceError::Stt(format!("Failed to spawn STT binary: {e}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| VoiceError::Stt("Failed to open stdin".to_string()))?;

        // Write audio data to stdin
        stdin
            .write_all(audio_data)
            .await
            .map_err(|e| VoiceError::Stt(format!("Failed to write to stdin: {e}")))?;
        drop(stdin); // Close stdin to signal EOF

        let output = tokio::time::timeout(STT_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                VoiceError::Stt(format!(
                    "STT process timed out after {} seconds",
                    STT_TIMEOUT.as_secs()
                ))
            })?
            .map_err(|e| VoiceError::Stt(format!("Failed to read stdout: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(VoiceError::Stt(format!("STT binary failed: {stderr}")));
        }

        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(text)
    }
}
