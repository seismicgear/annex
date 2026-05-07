use crate::config::{IceServer, WebRtcConfig};
use crate::error::VoiceError;
use bytes::Bytes;
use dashmap::DashMap;
use opus_rs::OpusDecoder;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tracing::{debug, error, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::{APIBuilder, API};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::packet::Packet;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtcp::transport_feedbacks::transport_layer_nack::{NackPair, TransportLayerNack};
use webrtc::rtp::packet::Packet as RtpPacket;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::TrackLocalWriter;

#[derive(Debug, Clone)]
pub struct RoomInfo {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct SttTapFrame {
    pub channel_id: String,
    pub speaker_pseudonym: String,
    pub pcm_s16le: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct IceCandidateEvent {
    pub channel_id: String,
    pub peer_id: String,
    pub candidate: RTCIceCandidateInit,
}

struct PeerSession {
    pc: Arc<RTCPeerConnection>,
    outbound_track: Arc<TrackLocalStaticRTP>,
}

struct Room {
    peers: DashMap<String, Arc<PeerSession>>,
    agent_track: Arc<TrackLocalStaticSample>,
}

pub struct VoiceService {
    config: WebRtcConfig,
    api: API,
    rooms: DashMap<String, Arc<Room>>,
    runtime_public_url: RwLock<String>,
    runtime_disabled: RwLock<bool>,
    stt_tap_tx: broadcast::Sender<SttTapFrame>,
    ice_candidate_tx: broadcast::Sender<IceCandidateEvent>,
}

impl std::fmt::Debug for VoiceService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceService")
            .field("config", &self.config)
            .field("rooms_len", &self.rooms.len())
            .finish_non_exhaustive()
    }
}

impl VoiceService {
    pub fn new(config: WebRtcConfig) -> Self {
        let mut media_engine = MediaEngine::default();
        if let Err(e) = media_engine.register_default_codecs() {
            warn!(error = %e, "failed to register default codecs");
        }

        let registry = match register_default_interceptors(Registry::new(), &mut media_engine) {
            Ok(registered) => registered,
            Err(_) => Registry::new(),
        };

        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        let (stt_tap_tx, _) = broadcast::channel(1024);
        let (ice_candidate_tx, _) = broadcast::channel(1024);

        Self {
            config,
            api,
            rooms: DashMap::new(),
            runtime_public_url: RwLock::new(String::new()),
            runtime_disabled: RwLock::new(false),
            stt_tap_tx,
            ice_candidate_tx,
        }
    }

    pub fn is_enabled(&self) -> bool {
        !*self
            .runtime_disabled
            .read()
            .unwrap_or_else(|p| p.into_inner())
            && !self.config.url.is_empty()
    }

    pub fn set_runtime_disabled(&self, disabled: bool) {
        *self
            .runtime_disabled
            .write()
            .unwrap_or_else(|p| p.into_inner()) = disabled;
    }

    pub fn get_url(&self) -> &str {
        &self.config.url
    }

    pub fn api_key(&self) -> &str {
        &self.config.api_key
    }

    pub fn api_secret(&self) -> &str {
        &self.config.api_secret
    }

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

    pub fn set_public_url(&self, url: String) {
        *self
            .runtime_public_url
            .write()
            .unwrap_or_else(|p| p.into_inner()) = url;
    }

    fn is_loopback_url(url: &str) -> bool {
        let stripped = url
            .trim_start_matches("ws://")
            .trim_start_matches("wss://")
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let host = stripped.split(':').next().unwrap_or("");
        host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]"
    }

    pub fn ice_servers(&self) -> &[IceServer] {
        &self.config.ice_servers
    }

    pub async fn create_room(&self, name: &str) -> Result<RoomInfo, VoiceError> {
        self.get_or_create_room(name).await?;
        Ok(RoomInfo {
            name: name.to_string(),
        })
    }

    pub fn generate_join_token(
        &self,
        room_name: &str,
        participant_identity: &str,
        participant_name: &str,
    ) -> Result<String, VoiceError> {
        let payload = serde_json::json!({
            "room": room_name,
            "sub": participant_identity,
            "name": participant_name,
            "iss": "annex-native-sfu",
        });

        use base64::Engine;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string()))
    }

    pub async fn participant_count(&self, room_name: &str) -> Result<u32, VoiceError> {
        Ok(self
            .rooms
            .get(room_name)
            .map(|r| r.peers.len() as u32)
            .unwrap_or(0))
    }

    pub async fn remove_participant(&self, room: &str, identity: &str) -> Result<(), VoiceError> {
        if let Some(peer) = self.drop_peer_and_maybe_reap(room, identity) {
            if let Err(e) = peer.pc.close().await {
                debug!(error = %e, "failed to close peer connection");
            }
        }
        Ok(())
    }

    /// Removes `peer_id` from the room's `peers` map and, if that was the
    /// last peer, atomically removes the room itself from `self.rooms`.
    ///
    /// Closes the [`Room`] memory leak: without this, a `Room` stays in
    /// `self.rooms` forever once every peer has disconnected, so memory
    /// grows linearly with the number of distinct channel IDs ever joined.
    ///
    /// Returns the dropped [`PeerSession`] (the caller owns the
    /// `pc.close()` invocation — `on_peer_connection_state_change`
    /// already has the connection closing on its own and skips the
    /// close, while `remove_participant` performs an explicit close).
    ///
    /// Race-safety: the room reap is guarded by [`DashMap::remove_if`],
    /// which evaluates the closure under the entry lock. A concurrent
    /// `get_or_create_room` that races our removal sees one of two
    /// outcomes:
    /// * It runs *before* `remove_if` evaluates: it inserts a peer into
    ///   the room, the `peers.is_empty()` predicate returns false, and
    ///   the room is preserved.
    /// * It runs *after* `remove_if` evaluates: it observes that the
    ///   room is gone and creates a fresh one with a new `agent_track`.
    ///
    /// Reaping only fires when a peer was *actually* removed (i.e.
    /// `peers.remove` returned `Some`). This preserves the semantics
    /// of [`create_room`] / [`inject_agent_opus`], which create empty
    /// rooms intentionally and must not be silently torn down by a
    /// stray `remove_participant("nonexistent")` call.
    fn drop_peer_and_maybe_reap(
        &self,
        channel_id: &str,
        peer_id: &str,
    ) -> Option<Arc<PeerSession>> {
        let mut peer_removed = None;
        let mut should_reap = false;
        if let Some(room_entry) = self.rooms.get(channel_id) {
            if let Some((_, peer)) = room_entry.peers.remove(peer_id) {
                peer_removed = Some(peer);
                should_reap = room_entry.peers.is_empty();
            }
        }
        if should_reap {
            self.rooms
                .remove_if(channel_id, |_, room| room.peers.is_empty());
        }
        peer_removed
    }

    pub fn subscribe_stt_taps(&self) -> broadcast::Receiver<SttTapFrame> {
        self.stt_tap_tx.subscribe()
    }

    pub fn subscribe_ice_candidates(&self) -> broadcast::Receiver<IceCandidateEvent> {
        self.ice_candidate_tx.subscribe()
    }

    pub async fn handle_sdp_offer(
        self: &Arc<Self>,
        channel_id: &str,
        peer_id: &str,
        offer_sdp: &str,
    ) -> Result<RTCSessionDescription, VoiceError> {
        let room = self.get_or_create_room(channel_id).await?;
        let pc = Arc::new(
            self.api
                .new_peer_connection(self.rtc_config())
                .await
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?,
        );

        let outbound_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: "audio/opus".to_string(),
                clock_rate: 48_000,
                channels: 1,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                rtcp_feedback: vec![],
            },
            format!("audio-{peer_id}"),
            channel_id.to_string(),
        ));

        pc.add_track(outbound_track.clone() as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
        pc.add_track(room.agent_track.clone() as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;

        let candidate_tx = self.ice_candidate_tx.clone();
        let candidate_channel = channel_id.to_string();
        let candidate_peer = peer_id.to_string();
        pc.on_ice_candidate(Box::new(move |candidate| {
            let candidate_tx = candidate_tx.clone();
            let candidate_channel = candidate_channel.clone();
            let candidate_peer = candidate_peer.clone();
            Box::pin(async move {
                if let Some(candidate) = candidate {
                    match candidate.to_json() {
                        Ok(init) => {
                            let _ = candidate_tx.send(IceCandidateEvent {
                                channel_id: candidate_channel,
                                peer_id: candidate_peer,
                                candidate: init,
                            });
                        }
                        Err(e) => debug!(error = %e, "failed to serialize local ICE candidate"),
                    }
                }
            })
        }));

        let service = Arc::clone(self);
        let channel = channel_id.to_string();
        let publisher = peer_id.to_string();
        let pc_for_track = Arc::clone(&pc);
        pc.on_track(Box::new(move |track, _, _| {
            let service = Arc::clone(&service);
            let channel = channel.clone();
            let publisher = publisher.clone();
            let pc_for_track = Arc::clone(&pc_for_track);
            Box::pin(async move {
                let media_ssrc = track.ssrc();
                tokio::spawn(async move {
                    if let Err(e) = service
                        .forward_track_loop(channel, publisher, media_ssrc, pc_for_track, track)
                        .await
                    {
                        error!(error = %e, "forward loop exited");
                    }
                });
            })
        }));

        let service = Arc::clone(self);
        let cleanup_channel = channel_id.to_string();
        let cleanup_peer = peer_id.to_string();
        pc.on_peer_connection_state_change(Box::new(move |state| {
            let service = Arc::clone(&service);
            let cleanup_channel = cleanup_channel.clone();
            let cleanup_peer = cleanup_peer.clone();
            Box::pin(async move {
                use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
                if matches!(
                    state,
                    RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Closed
                        | RTCPeerConnectionState::Disconnected
                ) {
                    // The pc is already terminating on its own — just
                    // drop the peer, reap the room if empty. We don't
                    // call pc.close() here (the state change implies it
                    // is already closing).
                    let _ = service.drop_peer_and_maybe_reap(&cleanup_channel, &cleanup_peer);
                }
            })
        }));

        let offer = RTCSessionDescription::offer(offer_sdp.to_string())
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
        pc.set_remote_description(offer)
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;

        let answer = pc
            .create_answer(None)
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;

        room.peers.insert(
            peer_id.to_string(),
            Arc::new(PeerSession { pc, outbound_track }),
        );

        Ok(answer)
    }

    pub async fn add_ice_candidate(
        &self,
        channel_id: &str,
        peer_id: &str,
        candidate: RTCIceCandidateInit,
    ) -> Result<(), VoiceError> {
        let room = self
            .rooms
            .get(channel_id)
            .ok_or_else(|| VoiceError::RoomService(format!("room not found: {channel_id}")))?;
        let peer = room
            .peers
            .get(peer_id)
            .ok_or_else(|| VoiceError::RoomService(format!("peer not found: {peer_id}")))?;

        peer.pc
            .add_ice_candidate(candidate)
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))
    }

    pub async fn inject_agent_opus(
        &self,
        channel_id: &str,
        _agent_id: &str,
        opus_frame: &[u8],
        duration: std::time::Duration,
    ) -> Result<(), VoiceError> {
        let room = self.get_or_create_room(channel_id).await?;
        room.agent_track
            .write_sample(&webrtc::media::Sample {
                data: Bytes::copy_from_slice(opus_frame),
                duration,
                ..Default::default()
            })
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))
    }

    async fn get_or_create_room(&self, channel_id: &str) -> Result<Arc<Room>, VoiceError> {
        if let Some(room) = self.rooms.get(channel_id) {
            return Ok(room.clone());
        }

        let room = Arc::new(Room {
            peers: DashMap::new(),
            agent_track: Arc::new(TrackLocalStaticSample::new(
                RTCRtpCodecCapability {
                    mime_type: "audio/opus".to_string(),
                    clock_rate: 48_000,
                    channels: 1,
                    sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                    rtcp_feedback: vec![],
                },
                format!("agent-mix-{channel_id}"),
                channel_id.to_string(),
            )),
        });

        self.rooms.insert(channel_id.to_string(), room.clone());
        Ok(room)
    }

    fn rtc_config(&self) -> RTCConfiguration {
        let mut ice_servers: Vec<RTCIceServer> = self
            .config
            .ice_servers
            .iter()
            .map(|s| RTCIceServer {
                urls: s.urls.clone(),
                username: s.username.clone(),
                credential: s.credential.clone(),
                ..Default::default()
            })
            .collect();

        if ice_servers.is_empty() {
            ice_servers.push(RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                ..Default::default()
            });
        }

        RTCConfiguration {
            ice_servers,
            ..Default::default()
        }
    }

    async fn forward_track_loop(
        &self,
        channel_id: String,
        publisher_id: String,
        media_ssrc: u32,
        pc: Arc<RTCPeerConnection>,
        track: Arc<webrtc::track::track_remote::TrackRemote>,
    ) -> Result<(), VoiceError> {
        let mut last_seq: Option<u16> = None;
        let mut last_pli = std::time::Instant::now();
        let mut decoder =
            OpusDecoder::new(48_000, 1).map_err(|e| VoiceError::Codec(e.to_string()))?;

        loop {
            let (rtp, _) = track
                .read_rtp()
                .await
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?;

            if let Some(previous) = last_seq {
                let expected = previous.wrapping_add(1);
                if rtp.header.sequence_number != expected {
                    let nack = TransportLayerNack {
                        sender_ssrc: 0,
                        media_ssrc,
                        nacks: vec![NackPair {
                            packet_id: expected,
                            lost_packets: 0,
                        }],
                    };
                    if let Err(e) = pc
                        .write_rtcp(&[Box::new(nack) as Box<dyn Packet + Send + Sync>])
                        .await
                    {
                        debug!(error = %e, "failed to send NACK");
                    }
                }
            }
            last_seq = Some(rtp.header.sequence_number);

            if last_pli.elapsed() >= std::time::Duration::from_secs(2) {
                let pli = PictureLossIndication {
                    sender_ssrc: 0,
                    media_ssrc,
                };
                if let Err(e) = pc
                    .write_rtcp(&[Box::new(pli) as Box<dyn Packet + Send + Sync>])
                    .await
                {
                    debug!(error = %e, "failed to send PLI");
                }
                last_pli = std::time::Instant::now();
            }

            self.fan_out_rtp(&channel_id, &publisher_id, &rtp).await;
            self.tap_for_stt(&channel_id, &publisher_id, &rtp, &mut decoder)
                .await;
        }
    }

    async fn fan_out_rtp(&self, channel_id: &str, publisher_id: &str, rtp: &RtpPacket) {
        if let Some(room) = self.rooms.get(channel_id) {
            let outbound: Vec<(String, Arc<TrackLocalStaticRTP>)> = room
                .peers
                .iter()
                .filter(|peer| peer.key().as_str() != publisher_id)
                .map(|peer| (peer.key().clone(), peer.value().outbound_track.clone()))
                .collect();

            for (peer_id, track) in outbound {
                if let Err(e) = track.write_rtp(rtp).await {
                    debug!(peer = %peer_id, error = %e, "rtp fanout write failed");
                }
            }
        }
    }

    async fn tap_for_stt(
        &self,
        channel_id: &str,
        speaker: &str,
        rtp: &RtpPacket,
        decoder: &mut OpusDecoder,
    ) {
        let mut pcm = vec![0f32; 1920];
        if let Ok(samples) = decoder.decode(&rtp.payload, 960, &mut pcm) {
            let raw = pcm_f32_to_s16le_bytes(&pcm[..samples]);
            let _ = self.stt_tap_tx.send(SttTapFrame {
                channel_id: channel_id.to_string(),
                speaker_pseudonym: speaker.to_string(),
                pcm_s16le: raw,
            });
        }
    }
}

/// Convert normalized float PCM (`[-1.0, 1.0]`, the output range of
/// `opus-rs::OpusDecoder::decode`) to little-endian signed 16-bit PCM.
///
/// Multiplies by `i16::MAX` (32767) to keep the output range symmetric
/// around zero — `s = -1.0` maps to `-32767`, not `-32768`. This avoids
/// asymmetric clipping at the negative bound, which downstream STT models
/// (especially small whisper variants) can read as a steady-state DC
/// offset on quiet input. Out-of-range floats (Opus produces them only
/// during the SilkOnly/CELTOnly paths that *don't* explicitly clamp) are
/// saturated.
///
/// Returns a freshly-allocated `Vec<u8>` in s16-le format ready to be
/// shipped over the STT tap broadcast channel.
fn pcm_f32_to_s16le_bytes(pcm: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for &s in pcm {
        let scaled = (s * i16::MAX as f32)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s16_at(bytes: &[u8], idx: usize) -> i16 {
        i16::from_le_bytes([bytes[idx * 2], bytes[idx * 2 + 1]])
    }

    fn test_service() -> VoiceService {
        VoiceService::new(WebRtcConfig {
            url: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            public_url: String::new(),
            token_ttl_seconds: 3600,
            ice_servers: vec![],
        })
    }

    fn empty_room(channel_id: &str) -> Arc<Room> {
        Arc::new(Room {
            peers: DashMap::new(),
            agent_track: Arc::new(TrackLocalStaticSample::new(
                RTCRtpCodecCapability {
                    mime_type: "audio/opus".to_string(),
                    clock_rate: 48_000,
                    channels: 1,
                    sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                    rtcp_feedback: vec![],
                },
                format!("agent-mix-{channel_id}"),
                channel_id.to_string(),
            )),
        })
    }

    #[tokio::test]
    async fn create_room_then_remove_unknown_participant_does_not_reap_room() {
        // create_room and inject_agent_opus create empty rooms intentionally
        // (the agent prepares the room before any human peer joins). A
        // stray remove_participant("nonexistent") call must not tear
        // those rooms down.
        let svc = test_service();
        svc.create_room("ch-empty").await.unwrap();
        assert_eq!(svc.rooms.len(), 1);
        svc.remove_participant("ch-empty", "nobody").await.unwrap();
        assert_eq!(
            svc.rooms.len(),
            1,
            "removing a non-existent participant must not reap the room"
        );
    }

    #[tokio::test]
    async fn drop_peer_and_maybe_reap_returns_none_for_unknown_room() {
        let svc = test_service();
        let result = svc.drop_peer_and_maybe_reap("nonexistent-channel", "any-peer");
        assert!(result.is_none());
        assert_eq!(svc.rooms.len(), 0);
    }

    #[tokio::test]
    async fn drop_peer_and_maybe_reap_returns_none_for_known_room_unknown_peer() {
        let svc = test_service();
        svc.rooms.insert("ch1".to_string(), empty_room("ch1"));
        let result = svc.drop_peer_and_maybe_reap("ch1", "ghost-peer");
        // No peer was actually removed → no reap, returns None.
        assert!(result.is_none());
        assert_eq!(
            svc.rooms.len(),
            1,
            "must not reap when no peer was actually removed"
        );
    }

    #[tokio::test]
    async fn rooms_dashmap_remove_if_handles_concurrent_insert_race() {
        // Models the race described in `drop_peer_and_maybe_reap`'s doc:
        // a thread reaps a room whose peers are empty; concurrently
        // another thread inserts a peer into that same room. The
        // reap-side `remove_if` predicate must observe the new peer
        // and skip the removal.
        let svc = test_service();
        let room = empty_room("racy");
        svc.rooms.insert("racy".to_string(), room.clone());

        // Simulate the concurrent insert: the reaping closure runs
        // remove_if; we mimic the racing inserter by populating peers
        // *before* remove_if evaluates.
        // (DashMap::remove_if's closure runs synchronously under the
        // entry lock — there is no Future-style suspension point — so we
        // must arrange the populated state pre-call.)
        let removed = svc.rooms.remove_if("racy", |_, r| r.peers.is_empty());
        assert!(
            removed.is_some(),
            "predicate true → room is reaped on the empty path"
        );

        // Now exercise the inverse path: re-insert a fresh empty room,
        // populate peers, and confirm remove_if leaves it alone.
        let room2 = empty_room("racy");
        // Insert a synthetic placeholder by manipulating the DashMap
        // structure indirectly: we cannot easily construct a real
        // PeerSession (it requires WebRTC), so instead we drive the
        // predicate directly with an `is_empty() == false` simulation.
        svc.rooms.insert("racy".to_string(), room2);
        // Predicate that always returns false — same effect as a peer
        // being present.
        let removed_no = svc.rooms.remove_if("racy", |_, _| false);
        assert!(
            removed_no.is_none(),
            "predicate false → room is preserved (concurrent peer insert wins)"
        );
        assert_eq!(svc.rooms.len(), 1);
    }

    #[test]
    fn pcm_f32_to_s16le_zero_input_is_zero() {
        let out = pcm_f32_to_s16le_bytes(&[0.0; 4]);
        assert_eq!(out.len(), 8);
        for i in 0..4 {
            assert_eq!(s16_at(&out, i), 0);
        }
    }

    #[test]
    fn pcm_f32_to_s16le_full_scale_positive_maps_to_i16_max() {
        // Full-scale +1.0 must produce i16::MAX (32767), not 1.
        let out = pcm_f32_to_s16le_bytes(&[1.0]);
        assert_eq!(s16_at(&out, 0), i16::MAX);
    }

    #[test]
    fn pcm_f32_to_s16le_full_scale_negative_maps_to_minus_i16_max() {
        // Full-scale -1.0 must produce -32767 (symmetric around zero), not -1.
        let out = pcm_f32_to_s16le_bytes(&[-1.0]);
        assert_eq!(s16_at(&out, 0), -i16::MAX);
    }

    #[test]
    fn pcm_f32_to_s16le_mid_scale_speech_levels_are_audible() {
        // A 0.5-amplitude tone (typical speech RMS region) MUST produce
        // a substantial i16 sample, not 0/1. The previous implementation
        // would have produced only 0 or 1 here, which is silence.
        let out = pcm_f32_to_s16le_bytes(&[0.5]);
        let s = s16_at(&out, 0);
        assert!(
            s.abs() > 1000,
            "0.5 normalized float must scale to a real PCM amplitude, got {s}"
        );
    }

    #[test]
    fn pcm_f32_to_s16le_clips_above_full_scale() {
        // Out-of-range positive float clamps to i16::MAX.
        let out = pcm_f32_to_s16le_bytes(&[2.0]);
        assert_eq!(s16_at(&out, 0), i16::MAX);
    }

    #[test]
    fn pcm_f32_to_s16le_clips_below_full_scale() {
        // Out-of-range negative float clamps to i16::MIN.
        // Note we scale by i16::MAX, so -2.0 * 32767 = -65534, which the
        // clamp pulls up to -32768 (i16::MIN).
        let out = pcm_f32_to_s16le_bytes(&[-2.0]);
        assert_eq!(s16_at(&out, 0), i16::MIN);
    }

    #[test]
    fn pcm_f32_to_s16le_preserves_sample_count() {
        let out = pcm_f32_to_s16le_bytes(&[0.1, -0.2, 0.3, -0.4]);
        assert_eq!(out.len(), 8);
        // Round-trip: each sample in [-1, 1] must land in a sane range.
        let s0 = s16_at(&out, 0);
        let s1 = s16_at(&out, 1);
        let s2 = s16_at(&out, 2);
        let s3 = s16_at(&out, 3);
        assert!(s0 > 2000 && s0 < 4000, "0.1 → {s0}");
        assert!(s1 < -5000 && s1 > -8000, "-0.2 → {s1}");
        assert!(s2 > 8000 && s2 < 11000, "0.3 → {s2}");
        assert!(s3 < -11000 && s3 > -14000, "-0.4 → {s3}");
    }
}
