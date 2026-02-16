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
    - Requires the `whisper` binary to be installed/available on the host.
  - **Mitigation**:
    - Future iterations can integrate `candle-whisper` directly into the binary for better performance and deployment simplicity.
