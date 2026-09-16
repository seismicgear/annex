use crate::audio;
use crate::error::VoiceError;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Maximum audio input size for STT (10 MiB). Prevents OOM from oversized payloads.
const MAX_STT_INPUT_BYTES: usize = 10 * 1024 * 1024;

/// Timeout for STT process execution.
const STT_TIMEOUT: Duration = Duration::from_secs(120);

/// Why STT is or is not usable, in the operator's terms.
///
/// `is_ready()` returned a bare `bool`, which was reported on
/// `/api/voice/config-status` as `stt_ready` and rendered by the client as
/// a single sentence covering four different situations. An operator who
/// saw it could not tell whether the model was missing, the binary was
/// missing, or the binary was there but not executable — and the last of
/// those is the one a manual install produces most often.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttReadiness {
    Ready,
    ModelMissing(PathBuf),
    BinaryMissing(PathBuf),
    BinaryNotExecutable(PathBuf),
}

impl SttReadiness {
    pub fn is_ready(&self) -> bool {
        matches!(self, SttReadiness::Ready)
    }

    /// One sentence naming the specific file and what to do about it.
    ///
    /// `scripts/setup-stt.sh` is named explicitly: a message that says
    /// only "the model is missing" leaves the person who can fix it with
    /// nowhere to go.
    pub fn detail(&self) -> String {
        match self {
            SttReadiness::Ready => "Transcription is ready.".to_string(),
            SttReadiness::ModelMissing(p) => format!(
                "The GGML model file is missing: {}. Run scripts/setup-stt.sh to download it, or set ANNEX_STT_MODEL_PATH to an existing model.",
                p.display()
            ),
            SttReadiness::BinaryMissing(p) => format!(
                "The whisper.cpp binary is missing: {}. Run scripts/setup-stt.sh, or set ANNEX_STT_BINARY_PATH to an existing whisper-cli build.",
                p.display()
            ),
            SttReadiness::BinaryNotExecutable(p) => format!(
                "The whisper.cpp binary at {} exists but is not executable. Run `chmod +x` on it, or re-run scripts/setup-stt.sh.",
                p.display()
            ),
        }
    }
}

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

    /// What is actually wrong, if anything.
    ///
    /// The executable-bit check is not decoration. A binary extracted from
    /// an archive without the mode bit, or copied by a Dockerfile `COPY`
    /// from a context that lost permissions, passes `is_file()` and then
    /// fails at spawn time with `Permission denied` — a per-frame error on
    /// the transcription path, which is exactly where nobody is looking.
    pub fn readiness(&self) -> SttReadiness {
        if !self.model_path.is_file() {
            return SttReadiness::ModelMissing(self.model_path.clone());
        }
        if !self.binary_path.is_file() {
            return SttReadiness::BinaryMissing(self.binary_path.clone());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = std::fs::metadata(&self.binary_path)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                // An unreadable metadata call is not proof of a bad bit;
                // let the spawn report it rather than inventing a cause.
                .unwrap_or(true);
            if !executable {
                return SttReadiness::BinaryNotExecutable(self.binary_path.clone());
            }
        }
        SttReadiness::Ready
    }

    /// Returns `true` when transcription can actually be attempted.
    ///
    /// Reported on `/api/voice/config-status` as `stt_ready` so the client
    /// (and operators) can tell the difference between "STT configured and
    /// ready" and "STT path set but the file is missing" — the latter used
    /// to silently pretend to be ready and fail at request time. Use
    /// [`Self::readiness`] when you need to say *which* file.
    pub fn is_ready(&self) -> bool {
        self.readiness().is_ready()
    }

    /// Transcribe one window of call audio.
    ///
    /// `pcm_s16le_48k` is raw little-endian signed 16-bit mono PCM at
    /// 48 kHz — the format the SFU's Opus decoder produces and the STT tap
    /// carries ([`crate::SttTapFrame::pcm_s16le`]).
    ///
    /// These bytes used to be written to whisper.cpp's stdin unchanged,
    /// and whisper.cpp cannot read them: `read_audio_data` in
    /// `examples/common.cpp` calls `drwav_init_memory` on whatever arrives
    /// and then rejects anything that is not 16 kHz 16-bit mono/stereo.
    /// Headerless 48 kHz PCM failed at the first check, every time, in
    /// every deployment. See [`crate::audio`] for the conversion.
    pub async fn transcribe(&self, pcm_s16le_48k: &[u8]) -> Result<String, VoiceError> {
        if pcm_s16le_48k.len() > MAX_STT_INPUT_BYTES {
            return Err(VoiceError::Stt(format!(
                "audio data exceeds maximum size: {} bytes (limit: {} bytes)",
                pcm_s16le_48k.len(),
                MAX_STT_INPUT_BYTES
            )));
        }

        let wav = audio::sfu_pcm_to_whisper_wav(pcm_s16le_48k);
        // A container with no frames is not worth a process. whisper.cpp
        // would load it, transcribe silence and return nothing, at the
        // cost of a fork and a model load.
        if wav.len() <= 44 {
            return Ok(String::new());
        }

        let mut command = Command::new(&self.binary_path);

        // whisper.cpp CLI arguments:
        //   -m <model_path>  path to the GGML model
        //   -f -             read the input file from stdin
        //   -nt              no timestamps, so stdout is the transcript
        //                    and nothing else. Without it every line
        //                    arrives as `[00:00:00.000 --> ...]   text`
        //                    and the caption strip shows the brackets.
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
            .arg("-nt")
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

        // A child that exits before reading all of stdin closes the read
        // end, and the next write returns `BrokenPipe`. That is the
        // child's business, not a transcription failure: whisper.cpp
        // bails out early on a header it dislikes, and the useful
        // diagnosis is on its stderr and in its exit status, both of
        // which we are about to read. Returning here instead threw away
        // the real message and reported "Failed to write to stdin" —
        // and, because the exit is a race against our own write, it did
        // so only sometimes.
        if let Err(e) = stdin.write_all(&wav).await {
            if e.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(VoiceError::Stt(format!("Failed to write to stdin: {e}")));
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_script(dir: &std::path::Path, name: &str, body: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    fn tone_pcm(secs: f32) -> Vec<u8> {
        let n = (secs * 48_000.0) as usize;
        (0..n)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let s = ((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 9000.0) as i16;
                s.to_le_bytes()
            })
            .collect()
    }

    #[test]
    fn readiness_names_the_missing_model() {
        let dir = tempfile::tempdir().unwrap();
        let bin = write_script(dir.path(), "whisper", "#!/bin/sh\n", 0o755);
        let svc = SttService::new(dir.path().join("absent.bin"), &bin);
        assert_eq!(
            svc.readiness(),
            SttReadiness::ModelMissing(dir.path().join("absent.bin")),
        );
        assert!(svc.readiness().detail().contains("setup-stt.sh"));
        assert!(!svc.is_ready());
    }

    #[test]
    fn readiness_names_the_missing_binary() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let svc = SttService::new(&model, dir.path().join("absent-whisper"));
        assert_eq!(
            svc.readiness(),
            SttReadiness::BinaryMissing(dir.path().join("absent-whisper")),
        );
    }

    #[test]
    fn a_binary_without_the_executable_bit_is_not_ready() {
        // The case `is_file()` could never see. An archive extracted
        // without modes, or a COPY from a context that lost them, leaves a
        // file that spawns with `Permission denied` on every frame.
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let bin = write_script(dir.path(), "whisper", "#!/bin/sh\n", 0o644);
        let svc = SttService::new(&model, &bin);
        assert_eq!(svc.readiness(), SttReadiness::BinaryNotExecutable(bin));
        assert!(svc.readiness().detail().contains("chmod"));
    }

    #[test]
    fn readiness_is_ready_when_both_files_are_usable() {
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let bin = write_script(dir.path(), "whisper", "#!/bin/sh\n", 0o755);
        let svc = SttService::new(&model, &bin);
        assert_eq!(svc.readiness(), SttReadiness::Ready);
        assert!(svc.is_ready());
    }

    #[tokio::test]
    async fn whisper_receives_a_16khz_wav_not_the_raw_tap_bytes() {
        // The defect this file exists to close. The stand-in asserts the
        // three things whisper.cpp's own loader asserts — RIFF/WAVE magic,
        // a 16 kHz sample rate, 16-bit — and fails loudly otherwise.
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let bin = write_script(
            dir.path(),
            "whisper",
            r#"#!/usr/bin/env python3
import sys, struct
data = sys.stdin.buffer.read()
if data[0:4] != b"RIFF" or data[8:12] != b"WAVE":
    sys.stderr.write("not a RIFF/WAVE container: %r\n" % data[:12]); sys.exit(1)
rate = struct.unpack("<I", data[24:28])[0]
bits = struct.unpack("<H", data[34:36])[0]
chans = struct.unpack("<H", data[22:24])[0]
if rate != 16000: sys.stderr.write("sample rate %d, want 16000\n" % rate); sys.exit(1)
if bits != 16: sys.stderr.write("bits %d\n" % bits); sys.exit(1)
if chans != 1: sys.stderr.write("channels %d\n" % chans); sys.exit(1)
frames = struct.unpack("<I", data[40:44])[0] // 2
sys.stdout.write("ok %d frames" % frames)
"#,
            0o755,
        );
        let svc = SttService::new(&model, &bin);
        // 600 ms at 48 kHz -> 9600 frames at 16 kHz.
        let text = svc.transcribe(&tone_pcm(0.6)).await.expect("transcribe");
        assert_eq!(text, "ok 9600 frames");
    }

    #[tokio::test]
    async fn a_child_that_exits_without_reading_stdin_is_not_a_transcription_failure() {
        // `write_all` to a child that has already closed its read end
        // returns BrokenPipe. Treating that as the error discarded the
        // child's own stderr and exit status, and — because it is a race
        // against process exit — did so intermittently. A payload larger
        // than the pipe buffer makes the race deterministic.
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let bin = write_script(
            dir.path(),
            "whisper",
            "#!/bin/sh\nprintf 'early exit transcript'\nexit 0\n",
            0o755,
        );
        let svc = SttService::new(&model, &bin);
        // ~4 s at 48 kHz = 384 KiB in, 128 KiB of WAV out — comfortably
        // past the 64 KiB default pipe buffer, so the write cannot
        // complete before the child is gone.
        let text = svc.transcribe(&tone_pcm(4.0)).await.expect("transcribe");
        assert_eq!(text, "early exit transcript");
    }

    #[tokio::test]
    async fn a_failing_child_still_reports_its_own_stderr() {
        // The other half of the broken-pipe change: a genuine failure must
        // not be swallowed by it.
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let bin = write_script(
            dir.path(),
            "whisper",
            "#!/bin/sh\necho 'error: failed to load model' >&2\nexit 1\n",
            0o755,
        );
        let svc = SttService::new(&model, &bin);
        let err = svc.transcribe(&tone_pcm(4.0)).await.expect_err("must fail");
        assert!(
            format!("{err}").contains("failed to load model"),
            "the child's own message must survive: {err}",
        );
    }

    #[tokio::test]
    async fn an_empty_window_costs_no_process() {
        // A missing binary would make any spawn fail, so reaching Ok here
        // proves nothing was spawned.
        let svc = SttService::new("/nonexistent/model", "/nonexistent/whisper");
        assert_eq!(svc.transcribe(&[]).await.unwrap(), "");
    }

    #[tokio::test]
    async fn oversized_input_is_refused_before_spawning() {
        let svc = SttService::new("/nonexistent/model", "/nonexistent/whisper");
        let err = svc
            .transcribe(&vec![0u8; MAX_STT_INPUT_BYTES + 2])
            .await
            .expect_err("must refuse");
        assert!(format!("{err}").contains("exceeds maximum size"));
    }
}
