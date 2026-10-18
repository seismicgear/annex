//! Publication intent is independent of RTP liveness. A reserved transceiver
//! can remain live after replaceTrack(None); receivers need an explicit stop.

use super::service::VoiceService;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaState {
    pub microphone: bool,
    pub camera: bool,
    pub screen_share: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct PublicationState {
    pub revision: u64,
    // Older clients do not report intent; preserve their RTP-based behavior.
    pub state: Option<MediaState>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaStateEvent {
    pub channel_id: String,
    pub peer_id: String,
    pub generation: u64,
    pub revision: u64,
    pub present: bool,
    pub state: Option<MediaState>,
}

impl VoiceService {
    pub fn subscribe_media_states(&self) -> tokio::sync::broadcast::Receiver<MediaStateEvent> {
        self.media_state_tx.subscribe()
    }

    /// Snapshot after admission, before the new client starts publishing.
    pub fn media_states(&self, channel_id: &str) -> Vec<MediaStateEvent> {
        let Some(room) = self.rooms.get(channel_id) else {
            return vec![];
        };
        room.peers
            .iter()
            .map(|peer| {
                let state = peer.media_state.read().unwrap_or_else(|p| p.into_inner());
                MediaStateEvent {
                    channel_id: channel_id.to_owned(),
                    peer_id: peer.key().clone(),
                    generation: peer.generation,
                    revision: state.revision,
                    present: true,
                    state: state.state,
                }
            })
            .collect()
    }

    /// The authenticated signaling handler supplies peer_id; callers cannot
    /// change another participant or a connection replaced since this update.
    pub fn update_media_state(
        &self,
        channel_id: &str,
        peer_id: &str,
        generation: u64,
        state: MediaState,
    ) -> bool {
        let Some(room) = self.rooms.get(channel_id) else {
            return false;
        };
        let Some(peer) = room.peers.get(peer_id) else {
            return false;
        };
        if peer.generation != generation {
            return false;
        }
        let mut current = peer.media_state.write().unwrap_or_else(|p| p.into_inner());
        if current.state == Some(state) {
            return true;
        }
        current.state = Some(state);
        current.revision += 1;
        let _ = self.media_state_tx.send(MediaStateEvent {
            channel_id: channel_id.to_owned(),
            peer_id: peer_id.to_owned(),
            generation,
            revision: current.revision,
            present: true,
            state: Some(state),
        });
        true
    }
}
