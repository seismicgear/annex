//! Native WebRTC SFU voice infrastructure for Annex.

pub mod agent;
pub mod audio;
pub mod config;
pub mod error;
pub mod service;
mod speech_dsp;
pub mod stt;
#[cfg(test)]
mod test_support;
pub mod token;
pub mod tts;
mod voice_profile;

pub use agent::{AgentVoiceClient, TranscriptionEvent};
pub use config::{
    IceServer, WebRtcConfig, DEV_WEBRTC_API_KEY, DEV_WEBRTC_API_SECRET, DEV_WEBRTC_URL,
};
pub use error::VoiceError;
pub use service::{IceCandidateEvent, RoomInfo, SttTapFrame, VoiceService};
pub use stt::{SttReadiness, SttService};
pub use token::{
    derive_voice_token_secret, generate_join_token, verify_join_token, VoiceClaims,
    VoiceTokenError, VOICE_TOKEN_DEFAULT_TTL_SECS,
};
pub use tts::TtsService;
pub use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
