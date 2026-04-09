//! Native WebRTC SFU voice infrastructure for Annex.

pub mod agent;
pub mod config;
pub mod error;
pub mod service;
pub mod stt;
pub mod tts;

pub use agent::{AgentVoiceClient, TranscriptionEvent};
pub use config::{
    IceServer, WebRtcConfig, DEV_WEBRTC_API_KEY, DEV_WEBRTC_API_SECRET, DEV_WEBRTC_URL,
};
pub use error::VoiceError;
pub use service::{IceCandidateEvent, RoomInfo, SttTapFrame, VoiceService};
pub use stt::SttService;
pub use tts::TtsService;
pub use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
