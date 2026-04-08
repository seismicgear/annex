use crate::config::{IceServer, LiveKitConfig};
use crate::error::VoiceError;
use audiopus::coder::Decoder as OpusDecoder;
use audiopus::{Channels, SampleRate};
use bytes::Bytes;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
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

#[derive(Debug)]
pub struct VoiceService {
    config: LiveKitConfig,
    api: API,
    rooms: DashMap<String, Arc<Room>>,
    runtime_public_url: RwLock<String>,
    runtime_disabled: RwLock<bool>,
    stt_tap_tx: broadcast::Sender<SttTapFrame>,
    ice_candidate_tx: broadcast::Sender<IceCandidateEvent>,
}

impl VoiceService {
    pub fn new(config: LiveKitConfig) -> Self {
        let mut media_engine = MediaEngine::default();
        if let Err(e) = media_engine.register_default_codecs() {
            warn!(error = %e, "failed to register default codecs");
        }

        let mut registry = Registry::new();
        if let Ok(registered) = register_default_interceptors(registry, &mut media_engine) {
            registry = registered;
        }

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
        !*self.runtime_disabled.blocking_read() && !self.config.url.is_empty()
    }

    pub fn set_runtime_disabled(&self, disabled: bool) {
        *self.runtime_disabled.blocking_write() = disabled;
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
        let runtime = self.runtime_public_url.blocking_read();
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
        let runtime = self.runtime_public_url.blocking_read();
        if !runtime.is_empty() {
            runtime.clone()
        } else if self.config.public_url.is_empty() {
            self.config.url.clone()
        } else {
            self.config.public_url.clone()
        }
    }

    pub fn set_public_url(&self, url: String) {
        *self.runtime_public_url.blocking_write() = url;
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
        if let Some(room_entry) = self.rooms.get(room) {
            if let Some((_, peer)) = room_entry.peers.remove(identity) {
                if let Err(e) = peer.pc.close().await {
                    debug!(error = %e, "failed to close peer connection");
                }
            }
        }
        Ok(())
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
                    if let Some(room) = service.rooms.get(&cleanup_channel) {
                        room.peers.remove(&cleanup_peer);
                    }
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
        let mut decoder = OpusDecoder::new(SampleRate::Hz48000, Channels::Mono)
            .map_err(|e| VoiceError::Codec(e.to_string()))?;

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
        let mut pcm = vec![0i16; 1920];
        if let Ok(samples) = decoder.decode(&rtp.payload, &mut pcm, false) {
            let mut raw = Vec::with_capacity(samples * 2);
            for s in pcm.into_iter().take(samples) {
                raw.extend_from_slice(&s.to_le_bytes());
            }
            let _ = self.stt_tap_tx.send(SttTapFrame {
                channel_id: channel_id.to_string(),
                speaker_pseudonym: speaker.to_string(),
                pcm_s16le: raw,
            });
        }
    }
}
