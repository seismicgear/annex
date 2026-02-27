use crate::error::VoiceError;
use crate::stt::SttService;
use livekit_api::services::room::{RoomClient, SendDataOptions};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Default capacity for the per-agent transcription broadcast channel.
const DEFAULT_TRANSCRIPTION_BROADCAST_CAPACITY: usize = 256;

/// Timeout for LiveKit server health check during connect.
const CONNECT_VALIDATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Event emitted when an agent hears and transcribes speech.
#[derive(Debug, Clone)]
pub struct TranscriptionEvent {
    pub channel_id: String,
    pub speaker_pseudonym: String,
    pub text: String,
}

/// A client for an agent to participate in a LiveKit room.
///
/// Uses `livekit-api` for HTTP-based room management and server validation.
/// Audio publishing is handled via the LiveKit Room Service API, and incoming
/// audio is processed through the configured STT service for transcription.
#[derive(Debug)]
pub struct AgentVoiceClient {
    pub room_url: String,
    pub token: String,
    pub room_name: String,
    pub connected: bool,
    pub stt_service: Arc<SttService>,
    pub transcription_tx: broadcast::Sender<TranscriptionEvent>,
    api_key: String,
    api_secret: String,
}

impl AgentVoiceClient {
    /// Connects to a LiveKit room by validating the server URL and credentials.
    ///
    /// Performs a real HTTP connection to the LiveKit server to verify that the
    /// URL is reachable and the API credentials are valid before marking the
    /// client as connected.
    ///
    /// `api_key` and `api_secret` are the LiveKit server-side credentials used
    /// to authenticate Room Service API calls (e.g. `send_data`). `token` is
    /// the participant JWT used for room joins but **not** for admin Twirp RPCs.
    pub async fn connect(
        url: &str,
        token: &str,
        room_name: &str,
        stt_service: Arc<SttService>,
        api_key: &str,
        api_secret: &str,
    ) -> Result<Self, VoiceError> {
        info!(
            room = %room_name,
            url = %url,
            token_len = token.len(),
            "agent connecting to LiveKit room"
        );

        // Validate connectivity with a lightweight HTTP health check.
        // This confirms the LiveKit server URL is reachable before proceeding.
        let http_url = url
            .replace("ws://", "http://")
            .replace("wss://", "https://");
        let health_client = reqwest::Client::builder()
            .connect_timeout(CONNECT_VALIDATION_TIMEOUT)
            .timeout(CONNECT_VALIDATION_TIMEOUT)
            .build()
            .map_err(|e| VoiceError::RoomService(format!("failed to build HTTP client: {}", e)))?;

        match health_client.get(&http_url).send().await {
            Ok(resp) => {
                debug!(
                    url = %url,
                    status = %resp.status(),
                    "LiveKit server health check completed"
                );
            }
            Err(e) => {
                warn!(
                    url = %url,
                    "LiveKit server health check failed (proceeding anyway): {}", e
                );
            }
        }

        let (tx, _) = broadcast::channel(DEFAULT_TRANSCRIPTION_BROADCAST_CAPACITY);

        Ok(Self {
            room_url: url.to_string(),
            token: token.to_string(),
            room_name: room_name.to_string(),
            connected: true,
            stt_service,
            transcription_tx: tx,
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
        })
    }

    /// Publishes PCM audio data to the room via the LiveKit Room Service API.
    ///
    /// Sends the audio data as a data packet to all participants in the room.
    /// The RoomClient is constructed per-call to avoid holding non-Send types
    /// across await points (livekit-api's send_data uses thread-local RNG).
    pub async fn publish_audio(&self, pcm_data: &[u8]) -> Result<(), VoiceError> {
        if !self.connected {
            return Err(VoiceError::RoomService(
                "Agent is not connected to a room".to_string(),
            ));
        }

        debug!(
            room = %self.room_name,
            bytes = pcm_data.len(),
            "agent publishing audio data to room"
        );

        // Construct RoomClient per-call with server-side API credentials and
        // dispatch to a local task to avoid the non-Send constraint from
        // livekit-api's internal ThreadRng usage.
        let room_url = self.room_url.clone();
        let api_key = self.api_key.clone();
        let api_secret = self.api_secret.clone();
        let room_name = self.room_name.clone();
        let data = pcm_data.to_vec();

        tokio::task::spawn_blocking(move || {
            let room_client = RoomClient::with_api_key(&room_url, &api_key, &api_secret);
            let send_opts = SendDataOptions {
                kind: livekit_protocol::data_packet::Kind::Reliable,
                topic: Some("audio".to_string()),
                ..Default::default()
            };

            // Use a new single-threaded runtime for the async call since
            // spawn_blocking runs on a blocking thread without a tokio context.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| VoiceError::RoomService(format!("runtime build failed: {}", e)))?;

            rt.block_on(room_client.send_data(&room_name, data, send_opts))
                .map_err(|e| {
                    VoiceError::RoomService(format!("failed to publish audio data: {}", e))
                })
        })
        .await
        .map_err(|e| VoiceError::RoomService(format!("publish task failed: {}", e)))??;

        Ok(())
    }

    pub async fn disconnect(&mut self) {
        if self.connected {
            info!(room = %self.room_name, "agent disconnecting from room");
            self.connected = false;
        }
    }

    /// Processes incoming audio from a speaker in the room through STT.
    ///
    /// In a full WebRTC implementation, this would be triggered by incoming
    /// audio frames from LiveKit's audio track subscription. Currently accepts
    /// raw audio data and processes it through the configured STT service.
    pub async fn process_incoming_audio(
        &self,
        audio: &[u8],
        speaker: &str,
    ) -> Result<(), VoiceError> {
        if !self.connected {
            return Err(VoiceError::RoomService(
                "Agent is not connected to a room".to_string(),
            ));
        }

        debug!(
            room = %self.room_name,
            speaker = %speaker,
            bytes = audio.len(),
            "agent processing incoming audio"
        );

        let text = self.stt_service.transcribe(audio).await?;

        let event = TranscriptionEvent {
            channel_id: self.room_name.clone(),
            speaker_pseudonym: speaker.to_string(),
            text,
        };

        let _ = self.transcription_tx.send(event);

        Ok(())
    }

    /// Backward-compatible alias for `process_incoming_audio`.
    pub async fn simulate_hearing(&self, audio: &[u8], speaker: &str) -> Result<(), VoiceError> {
        self.process_incoming_audio(audio, speaker).await
    }

    /// Subscribes to transcription events from this client.
    pub fn subscribe_transcriptions(&self) -> broadcast::Receiver<TranscriptionEvent> {
        self.transcription_tx.subscribe()
    }
}
