use crate::config::{IceServer, LiveKitConfig};
use crate::error::VoiceError;
use livekit_api::access_token::{AccessToken, VideoGrants};
use livekit_api::services::room::{CreateRoomOptions, RoomClient};
use livekit_protocol::Room;
use std::sync::RwLock;
use std::time::Duration;

#[derive(Debug)]
pub struct VoiceService {
    config: LiveKitConfig,
    room_client: RoomClient,
    /// Runtime-updatable public URL override. When set (non-empty), takes
    /// precedence over `config.public_url` for browser-facing URLs.
    runtime_public_url: RwLock<String>,
    /// Explicitly disabled at runtime (e.g. desktop startup detected LiveKit
    /// failure). When true, `is_enabled()` returns false regardless of config.
    runtime_disabled: RwLock<bool>,
}

impl VoiceService {
    pub fn new(config: LiveKitConfig) -> Self {
        let room_client =
            RoomClient::with_api_key(&config.url, &config.api_key, &config.api_secret);
        Self {
            config,
            room_client,
            runtime_public_url: RwLock::new(String::new()),
            runtime_disabled: RwLock::new(false),
        }
    }

    /// Returns true if voice is considered available.
    ///
    /// A non-empty URL is necessary but not sufficient: if the service was
    /// explicitly disabled at runtime (e.g. desktop startup detected LiveKit
    /// was unreachable), this returns false even when the fallback dev URL
    /// is present in the config.
    pub fn is_enabled(&self) -> bool {
        let disabled = *self
            .runtime_disabled
            .read()
            .unwrap_or_else(|p| p.into_inner());
        !disabled && !self.config.url.is_empty()
    }

    /// Mark voice as disabled at runtime (e.g. LiveKit failed to start on desktop).
    pub fn set_runtime_disabled(&self, disabled: bool) {
        let mut w = self
            .runtime_disabled
            .write()
            .unwrap_or_else(|p| p.into_inner());
        *w = disabled;
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
    ///
    /// Returns an empty string if the resolved URL is loopback-only
    /// (127.0.0.1 / localhost / [::1]), because remote clients cannot reach it.
    /// Local clients (Tauri host mode connecting to its own server) still receive
    /// the loopback URL through the internal `get_url()` path.
    ///
    /// Checks the runtime-updatable override first, then falls back to config.
    pub fn get_public_url(&self) -> String {
        let runtime = self
            .runtime_public_url
            .read()
            .unwrap_or_else(|p| p.into_inner());
        let url = if !runtime.is_empty() {
            runtime.clone()
        } else if !self.config.public_url.is_empty() {
            self.config.public_url.clone()
        } else {
            self.config.url.clone()
        };
        if Self::is_loopback_url(&url) {
            String::new()
        } else {
            url
        }
    }

    /// Returns the public URL without the loopback guard, for local-only use
    /// (e.g. Tauri host mode where the client is on the same machine).
    pub fn get_url_for_local_client(&self) -> String {
        let runtime = self
            .runtime_public_url
            .read()
            .unwrap_or_else(|p| p.into_inner());
        if !runtime.is_empty() {
            runtime.clone()
        } else if self.config.public_url.is_empty() {
            self.config.url.clone()
        } else {
            self.config.public_url.clone()
        }
    }

    /// Set the public URL at runtime (e.g. from Tauri after acquiring a public endpoint).
    pub fn set_public_url(&self, url: String) {
        let mut w = self
            .runtime_public_url
            .write()
            .unwrap_or_else(|p| p.into_inner());
        *w = url;
    }

    /// Check whether a URL points to a loopback/private-only address.
    fn is_loopback_url(url: &str) -> bool {
        // Strip ws:// / wss:// / http:// / https:// prefix to get the host
        let stripped = url
            .trim_start_matches("ws://")
            .trim_start_matches("wss://")
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let host = stripped.split(':').next().unwrap_or("");
        host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]"
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
