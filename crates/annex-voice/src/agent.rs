use crate::error::VoiceError;
use tracing::info;

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
}

impl AgentVoiceClient {
    /// Connects to a LiveKit room.
    pub async fn connect(url: &str, token: &str, room_name: &str) -> Result<Self, VoiceError> {
        info!(
            "Agent connecting to LiveKit room '{}' at '{}' with token length {}",
            room_name,
            url,
            token.len()
        );

        // Simulate connection delay
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Ok(Self {
            room_url: url.to_string(),
            token: token.to_string(),
            room_name: room_name.to_string(),
            connected: true,
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
}
