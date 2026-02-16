# 0003. Voice TTS Model Selection

**Date**: 2026-02-20
**Status**: Accepted

## Context

Annex needs to render agent text intents into audible speech. This must be done:
1.  **Locally**: To maintain sovereignty, the system should not rely on cloud TTS APIs.
2.  **Fast**: Latency should be low enough for near-conversational interaction (< 2s).
3.  **Low Resource**: The system must run on consumer hardware (CPU inference preferred).
4.  **Flexible**: Must support multiple distinct voices/profiles.

The options considered were:
1.  **Piper**: Fast, lightweight, runs on CPU, supports multiple voices (ONNX-based).
2.  **Bark**: High quality, supports non-speech sounds, but heavy (needs GPU for reasonable speed).
3.  **Parler-TTS**: Good quality, but potentially slower and heavier than Piper.
4.  **System TTS**: Depend on OS TTS (e.g., `espeak`, `say`). Low quality, inconsistent across platforms.

## Decision

We will use **Piper** as the primary TTS engine.

**Implementation Strategy**:
- The `annex-voice` crate will invoke the `piper` binary as a subprocess.
- Voice profiles will map to specific Piper `.onnx` model files and configuration JSONs.
- The system will stream PCM output from Piper directly into LiveKit audio tracks (or buffer and send).

## Consequences

**Positive**:
- Extremely fast inference on CPU (often > 10x realtime).
- Small model sizes (< 100MB).
- Simple deployment (single binary + model files).
- Python-free runtime dependency (just the binary).

**Negative**:
- Voice quality is "good enough" but less expressive than Bark/Parler.
- Requires managing binary distribution or user installation of `piper`.

## Compliance

This decision aligns with the sovereign node architecture by ensuring all inference happens on the local machine without external dependencies beyond the model files.
