use crate::error::VoiceError;
use crate::stt::SttService;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

/// Default capacity for the per-agent transcription broadcast channel.
const DEFAULT_TRANSCRIPTION_BROADCAST_CAPACITY: usize = 256;

/// Event emitted when an agent hears and transcribes speech.
#[derive(Debug, Clone)]
pub struct TranscriptionEvent {
    pub channel_id: String,
    pub speaker_pseudonym: String,
    pub text: String,
}

/// A client for an agent to participate in a LiveKit room.
///
/// In a production environment with `livekit` crate available, this would wrap
/// a `livekit::Room` and `livekit::LocalAudioTrack`.
///
/// Due to compilation constraints in the current environment, this is a simulation.
#[derive(Debug)]
pub struct AgentVoiceClient {
    pub room_url: String,
    pub token: String,
    pub room_name: String,
    pub connected: bool,
    pub stt_service: Arc<SttService>,
    pub transcription_tx: broadcast::Sender<TranscriptionEvent>,
}

impl AgentVoiceClient {
    /// Connects to a LiveKit room.
    pub async fn connect(
        url: &str,
        token: &str,
        room_name: &str,
        stt_service: Arc<SttService>,
    ) -> Result<Self, VoiceError> {
        info!(
            "Agent connecting to LiveKit room '{}' at '{}' with token length {}",
            room_name,
            url,
            token.len()
        );

        // Simulate connection delay
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let (tx, _) = broadcast::channel(DEFAULT_TRANSCRIPTION_BROADCAST_CAPACITY);

        Ok(Self {
            room_url: url.to_string(),
            token: token.to_string(),
            room_name: room_name.to_string(),
            connected: true,
            stt_service,
            transcription_tx: tx,
        })
    }

    /// Publishes PCM audio data to the room.
    pub async fn publish_audio(&self, pcm_data: &[u8]) -> Result<(), VoiceError> {
        if !self.connected {
            return Err(VoiceError::RoomService(
                "Agent is not connected to a room".to_string(),
            ));
        }

        info!(
            "Agent publishing {} bytes of audio to room '{}'",
            pcm_data.len(),
            self.room_name
        );

        // Simulate playback time roughly (assuming 22050Hz 16-bit mono)
        // 2 bytes per sample, 22050 samples per second = 44100 bytes/sec
        // duration = len / 44100
        // tokio::time::sleep(std::time::Duration::from_secs_f32(pcm_data.len() as f32 / 44100.0)).await;
        // Don't sleep here in the stub as it blocks the handler unless spawned.
        // In real impl, publishing is async and buffered.

        Ok(())
    }

    pub async fn disconnect(&mut self) {
        if self.connected {
            info!("Agent disconnecting from room '{}'", self.room_name);
            self.connected = false;
        }
    }

    /// Simulates the agent hearing audio from a speaker in the room.
    /// In a real implementation, this would be triggered by incoming audio frames from LiveKit.
    pub async fn simulate_hearing(&self, audio: &[u8], speaker: &str) -> Result<(), VoiceError> {
        if !self.connected {
            return Err(VoiceError::RoomService(
                "Agent is not connected to a room".to_string(),
            ));
        }

        info!(
            "Agent hearing {} bytes from '{}' in room '{}'",
            audio.len(),
            speaker,
            self.room_name
        );

        let text = self.stt_service.transcribe(audio).await?;

        let event = TranscriptionEvent {
            channel_id: self.room_name.clone(),
            speaker_pseudonym: speaker.to_string(),
            text,
        };

        // Broadcast event
        let _ = self.transcription_tx.send(event);

        Ok(())
    }

    /// Subscribes to transcription events from this client.
    pub fn subscribe_transcriptions(&self) -> broadcast::Receiver<TranscriptionEvent> {
        self.transcription_tx.subscribe()
    }
}
