use thiserror::Error;

#[derive(Error, Debug)]
pub enum VoiceError {
    #[error("LiveKit API error: {0}")]
    LiveKit(#[from] livekit_api::access_token::AccessTokenError),

    #[error("Room service error: {0}")]
    RoomService(String),

    #[error("Invalid configuration: {0}")]
    Config(String),

    #[error("TTS error: {0}")]
    Tts(String),

    #[error("STT error: {0}")]
    Stt(String),

    #[error("Voice profile not found: {0}")]
    ProfileNotFound(String),
}
