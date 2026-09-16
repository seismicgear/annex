# 0004. Voice STT Model

- **Status**: Accepted
- **Context**:
  - Agents need to "hear" human participants in voice channels to respond intelligently.
  - The system must remain sovereign and local-first (no cloud STT APIs).
  - The solution must be compatible with Rust and run on CPU if necessary (though GPU is preferred).
  - We need a balance between latency and accuracy.

- **Decision**:
  - We will use **Whisper** (specifically `whisper.cpp` or a Rust binding like `candle-whisper`) as the STT engine.
  - The `annex-voice` crate will wrap the Whisper execution.
  - For the initial implementation, we invoke the `whisper` binary via `tokio::process::Command` to avoid complex build dependencies in the workspace (similar to the decision for Piper in ADR-0003).
  - The model used will be `ggml-base.en.bin` (or configurable) to ensure reasonable latency on CPU.
  - Transcriptions are delivered to agents via the existing WebSocket connection using a `transcription` event type.

- **Consequences**:
  - **Pros**:
    - High accuracy with Whisper.
    - No external API dependencies.
    - Decoupled process model (server doesn't crash if STT crashes).
  - **Cons**:
    - Invoking a binary per audio chunk or session might have overhead (session-based long-running process is preferred for production, but binary invocation is simpler for MVP).
      **This landed as a defect, not a cost.** The shipped tap loop called
      `transcribe()` once per 20 ms RTP packet — a fork, a model load and an
      inference fifty times a second per speaker — over a window shorter than
      whisper returns anything for. Corrected 2026-09-15: audio is buffered
      per `(channel, speaker)` and flushed at 2 s or after 700 ms of quiet,
      with at most two transcriptions in flight per room. The per-invocation
      model is kept; it is the per-*packet* invocation that was wrong.
    - Requires the `whisper` binary to be installed/available on the host.
  - **Mitigation**:
    - Future iterations can integrate `candle-whisper` directly into the binary for better performance and deployment simplicity.

- **Interface note (added 2026-09-15)**: whisper.cpp's `read_audio_data`
  (`examples/common.cpp`) calls `drwav_init_memory` on whatever arrives on
  stdin and then rejects anything that is not mono/stereo, 16 kHz
  (`COMMON_SAMPLE_RATE`), 16-bit. The SFU decodes Opus at 48 kHz and the tap
  carries headerless PCM, so the audio failed the first check on every frame
  and would have failed the second. The conversion lives in
  `crates/annex-voice/src/audio.rs`: a 49-tap linear-phase windowed-sinc
  low-pass, 3:1 decimation, and a canonical 44-byte RIFF/WAVE header. Plain
  3:1 decimation without the filter folds 8-24 kHz content into the speech
  band; the test `content_above_the_new_nyquist_does_not_fold_into_the_speech_band`
  measures both the filtered and the naive version so the number means
  something.
