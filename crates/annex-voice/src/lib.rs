//! Native WebRTC SFU voice infrastructure for Annex.

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
pub use service::{RoomInfo, SttTapFrame, VoiceService};
pub use stt::SttService;
pub use tts::TtsService;
