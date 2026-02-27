//! Voice infrastructure for the Annex platform.
//!
//! Integrates with LiveKit for WebRTC voice transport, provides TTS
//! (text-to-speech) rendering for agent voice output, and STT
//! (speech-to-text) transcription for agent input. Manages voice
//! profiles and the agent voice pipeline.
//!
//! The voice architecture separates concerns: humans speak via WebRTC,
//! agents send text intents that are rendered to audio by the platform,
//! and human speech is transcribed back to text for agent consumption.
//!
//! # Phase 7 implementation
//!
//! The full implementation of this crate is Phase 7 of the roadmap.

pub mod agent;
pub mod config;
pub mod error;
pub mod service;
pub mod stt;
pub mod tts;

pub use agent::{AgentVoiceClient, TranscriptionEvent};
pub use config::{
    IceServer, LiveKitConfig, DEV_LIVEKIT_API_KEY, DEV_LIVEKIT_API_SECRET, DEV_LIVEKIT_URL,
};
pub use error::VoiceError;
pub use service::VoiceService;
pub use stt::SttService;
pub use tts::TtsService;
