use crate::{config::LiveKitConfig, error::VoiceError};
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
                ..Default::default()
            })
            .with_ttl(Duration::from_secs(60 * 60)); // 1 hour TTL

        token.to_jwt().map_err(VoiceError::LiveKit)
    }
}
