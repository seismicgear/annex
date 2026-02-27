use crate::config::{IceServer, LiveKitConfig};
use crate::error::VoiceError;
use livekit_api::access_token::{AccessToken, VideoGrants};
use livekit_api::services::room::{CreateRoomOptions, RoomClient};
use livekit_protocol::Room;
use std::time::Duration;

#[derive(Debug)]
pub struct VoiceService {
    config: LiveKitConfig,
    room_client: RoomClient,
}

impl VoiceService {
    pub fn new(config: LiveKitConfig) -> Self {
        let room_client =
            RoomClient::with_api_key(&config.url, &config.api_key, &config.api_secret);
        Self {
            config,
            room_client,
        }
    }

    pub fn is_enabled(&self) -> bool {
        !self.config.url.is_empty()
    }

    pub fn get_url(&self) -> &str {
        &self.config.url
    }

    /// Returns the LiveKit API key for server-side Room Service calls.
    pub fn api_key(&self) -> &str {
        &self.config.api_key
    }

    /// Returns the LiveKit API secret for server-side Room Service calls.
    pub fn api_secret(&self) -> &str {
        &self.config.api_secret
    }

    /// Returns the browser-facing URL. Falls back to the internal URL if no
    /// public URL is configured.
    pub fn get_public_url(&self) -> &str {
        if self.config.public_url.is_empty() {
            &self.config.url
        } else {
            &self.config.public_url
        }
    }

    pub async fn create_room(&self, name: &str) -> Result<Room, VoiceError> {
        let options = CreateRoomOptions::default();

        self.room_client
            .create_room(name, options)
            .await
            .map_err(|e| VoiceError::RoomService(e.to_string()))
    }

    pub fn generate_join_token(
        &self,
        room_name: &str,
        participant_identity: &str,
        participant_name: &str,
    ) -> Result<String, VoiceError> {
        let token = AccessToken::with_api_key(&self.config.api_key, &self.config.api_secret)
            .with_identity(participant_identity)
            .with_name(participant_name)
            .with_grants(VideoGrants {
                room_join: true,
                room: room_name.to_string(),
                can_publish: true,
                can_subscribe: true,
                can_publish_data: true,
                ..Default::default()
            })
            .with_ttl(Duration::from_secs(self.config.token_ttl_seconds));

        token.to_jwt().map_err(VoiceError::LiveKit)
    }

    pub async fn remove_participant(&self, room: &str, identity: &str) -> Result<(), VoiceError> {
        self.room_client
            .remove_participant(room, identity)
            .await
            .map_err(|e| VoiceError::RoomService(e.to_string()))
    }

    /// Returns the configured ICE (STUN/TURN) servers for WebRTC NAT traversal.
    pub fn ice_servers(&self) -> &[IceServer] {
        &self.config.ice_servers
    }

    /// Returns the number of participants currently in a room.
    /// Returns 0 if the room does not exist.
    pub async fn participant_count(&self, room_name: &str) -> Result<u32, VoiceError> {
        match self.room_client.list_participants(room_name).await {
            Ok(participants) => Ok(participants.len() as u32),
            Err(_) => Ok(0), // Room doesn't exist yet
        }
    }
}
