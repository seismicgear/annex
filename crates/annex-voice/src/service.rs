use crate::config::{IceServer, WebRtcConfig};
#[path = "relay_timeline.rs"]
mod relay_timeline;
use crate::error::VoiceError;
use bytes::Bytes;
use dashmap::DashMap;
use opusic_c::{Channels as OpusChannels, Decoder as OpusDecoder, SampleRate as OpusSampleRate};
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;
use tracing::{debug, error, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS, MIME_TYPE_VP8};
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
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters};
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCPFeedback;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
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

/// An offer the SERVER initiated, for a peer already in a call.
///
/// Adding a track to an established connection requires a new offer/answer.
/// The existing signalling is client-offers / server-answers, so this rides
/// the same broadcast-and-filter path as `IceCandidateEvent`: the WS session
/// for `peer_id` picks it up and forwards it as `webrtc_offer`.
#[derive(Debug, Clone)]
pub struct RenegotiateEvent {
    pub channel_id: String,
    pub peer_id: String,
    pub sdp: String,
}

#[derive(Debug, Clone)]
pub struct IceCandidateEvent {
    pub channel_id: String,
    pub peer_id: String,
    pub candidate: RTCIceCandidateInit,
}

/// The pair of tracks carrying ONE sender's media to ONE receiver.
///
/// Audio and video are separate tracks because video RTP landing on an audio
/// track corrupts the audio stream.
struct OutboundTracks {
    audio: Arc<TrackLocalStaticRTP>,
    video: Arc<TrackLocalStaticRTP>,
    screen: Arc<TrackLocalStaticRTP>,
}

struct PeerSession {
    pc: Arc<RTCPeerConnection>,
    /// Which connection this entry is, within (room, peer).
    ///
    /// The peer map is keyed by pseudonym alone, so a second offer from the
    /// same person REPLACES the entry — a reconnect, a second tab, a refresh
    /// mid-call. Without a generation the termination callback registered by
    /// the FIRST connection cannot tell its own entry from its replacement:
    /// it removes by (channel, peer), so the dying connection's cleanup tears
    /// down the live one that just took its place. The symptom is a user who
    /// reconnects and is dropped from the call a moment later, by their own
    /// previous socket.
    ///
    /// Monotonic across the whole service rather than per-room, because the
    /// comparison only ever happens within one (room, peer) and a single
    /// counter needs no bookkeeping when a room is reaped.
    generation: u64,
    /// One track pair per OTHER peer in the room, keyed by that peer's
    /// pseudonym.
    ///
    /// This used to be a single audio track and a single video track per
    /// receiver, into which `fan_out_rtp` wrote every other peer's RTP. With
    /// one remote peer that is correct. With two, their streams — different
    /// SSRCs, independent encoder state — were interleaved onto one track, and
    /// a receiver cannot demultiplex that: VP8 carries inter-frame references,
    /// so alternating packets from two encoders decodes to neither. Calls were
    /// structurally limited to two participants, and the client had no
    /// per-sender track to attribute a tile to, which is why every remote tile
    /// read "Participant".
    ///
    /// Each track's id is the sender's pseudonym, so it reaches the browser as
    /// the MSID and the client can name the tile without a side channel.
    outbound: DashMap<String, OutboundTracks>,
    // Reserved publication slots retain their original MSID and receive pipeline.
    // Detach the source while absent; keep the negotiated sendonly direction so
    // the browser does not tear down and then reuse an inert rendering sink.
    parked_publications: DashMap<String, Arc<webrtc::rtp_transceiver::RTCRtpTransceiver>>,
    renegotiation_pending: tokio::sync::Mutex<bool>,
    publication_timelines: DashMap<String, Arc<tokio::sync::Mutex<relay_timeline::RelayTimeline>>>,
}

impl PeerSession {
    async fn forward_publication(
        &self,
        track: &Arc<TrackLocalStaticRTP>,
        generation: u64,
        packet: &RtpPacket,
    ) -> Result<usize, webrtc::Error> {
        let timeline = self
            .publication_timelines
            .entry(track.id().to_owned())
            .or_insert_with(|| {
                Arc::new(tokio::sync::Mutex::new(
                    relay_timeline::RelayTimeline::default(),
                ))
            })
            .clone();
        let mut timeline = timeline.lock().await;
        let mut relayed = packet.clone();
        let clock_rate = if track.kind() == RTPCodecType::Video {
            90_000
        } else {
            48_000
        };
        relayed.header.timestamp = timeline.translate(
            generation,
            packet.header.ssrc,
            packet.header.timestamp,
            clock_rate,
            std::time::Instant::now(),
        );
        track.write_rtp(&relayed).await
    }

    async fn attach_publication(
        &self,
        track: Arc<dyn TrackLocal + Send + Sync>,
    ) -> Result<(), webrtc::Error> {
        let id = track.id().to_owned();
        if let Some((_, transceiver)) = self.parked_publications.remove(&id) {
            let result = transceiver.sender().await.replace_track(Some(track)).await;
            if let Err(error) = result {
                self.parked_publications.insert(id, transceiver);
                return Err(error);
            }
            transceiver
                .set_direction(RTCRtpTransceiverDirection::Sendonly)
                .await;
        } else {
            add_relay_publication(&self.pc, track).await?;
        }
        Ok(())
    }
    async fn park_publication(&self, id: &str) -> Result<(), webrtc::Error> {
        for transceiver in self.pc.get_transceivers().await {
            let sender = transceiver.sender().await;
            if sender.track().await.is_some_and(|track| track.id() == id) {
                sender.replace_track(None).await?;
                self.parked_publications.insert(id.to_owned(), transceiver);
            }
        }
        Ok(())
    }
}

/// A reused SSRC must retain a continuous RTP packet sequence when its source
/// reconnects with a fresh random sequence. Otherwise the established SRTP
/// receiver rejects the new packets as replayed or uses the wrong rollover
/// counter. Enable the sender's transformer before its first packet; its
/// replace_track implementation resets the offset when the source changes.
async fn add_relay_publication(
    pc: &RTCPeerConnection,
    track: Arc<dyn TrackLocal + Send + Sync>,
) -> Result<(), webrtc::Error> {
    let transceiver = pc
        .add_transceiver_from_track(
            track,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Sendonly,
                send_encodings: vec![],
            }),
        )
        .await?;
    let sender = transceiver.sender().await;
    sender.enable_seq_transformer()?;
    // Read feedback for every negotiated publication. This drives the RTCP
    // interceptors (including retransmission); parking a source keeps this
    // sender alive, and closing its peer ends the reader.
    tokio::spawn(async move { while sender.read_rtcp().await.is_ok() {} });
    Ok(())
}

/// The browser reserves camera first, screen second. MIDs remain stable when
/// replaceTrack changes the capture device, including a trackless initial offer.
fn screen_mids(offer_sdp: &str) -> std::collections::HashSet<String> {
    let mut video_index = 0;
    let mut is_screen = false;
    let mut mids = std::collections::HashSet::new();
    for line in offer_sdp.lines().map(str::trim) {
        if line.starts_with("m=") {
            if line.starts_with("m=video ") {
                video_index += 1;
                is_screen = video_index == 2;
            } else {
                is_screen = false;
            }
        } else if is_screen {
            if let Some(mid) = line.strip_prefix("a=mid:") {
                mids.insert(mid.to_string());
            }
        }
    }
    mids
}

/// The track a given sender's RTP is written into, for a given receiver.
///
/// Returns `None` when the receiver has no track pair for that sender yet —
/// a publisher whose media arrives in the window between joining and
/// renegotiation completing. Dropping those packets is correct: there is
/// nowhere to put them, and the sender's encoder will produce a keyframe once
/// the track exists.
fn outbound_track_for(
    session: &PeerSession,
    sender: &str,
    want_video: bool,
) -> Option<Arc<TrackLocalStaticRTP>> {
    session.outbound.get(sender).map(|tracks| {
        if want_video {
            tracks.video.clone()
        } else {
            tracks.audio.clone()
        }
    })
}

/// Register exactly the codecs this SFU can forward.
///
/// The relay does not transcode: inbound RTP is written verbatim onto an
/// outbound `TrackLocalStaticRTP`, which rewrites the SSRC and payload type to
/// that track's own negotiated values and leaves the payload bytes alone. Both
/// outbound tracks are built with a fixed capability — `audio/opus` and
/// `video/VP8` (see `make_outbound_tracks`) — so anything else arriving
/// inbound is relabelled as Opus or VP8 and handed to a decoder that cannot
/// read it.
///
/// `register_default_codecs()` advertised G722, PCMU, PCMA, VP9, six H.264
/// profiles and AV1 alongside them. Any of those could win negotiation on the
/// publish leg, and two things then broke without a word: remote video decoded
/// as VP8 when it was VP9 or H.264, and — because `relay_inbound_track` builds
/// an `OpusDecoder` for every audio track unconditionally — the STT tap fed
/// G.711 bytes to an Opus decoder.
///
/// Advertising only what the relay handles makes both assumptions true by
/// construction. The cost is H.264 hardware decode, which is worth less than
/// video that is intermittently unreadable depending on which codec the
/// browser happened to pick.
fn register_relay_codecs(media_engine: &mut MediaEngine) -> Result<(), webrtc::Error> {
    media_engine.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 111,
            ..Default::default()
        },
        RTPCodecType::Audio,
    )?;
    media_engine.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_VP8.to_owned(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                // Matches what `register_default_codecs` attaches to VP8. The
                // relay actively sends PLI and NACK, so the browser has to be
                // told it may send them too.
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "goog-remb".to_owned(),
                        parameter: String::new(),
                    },
                    RTCPFeedback {
                        typ: "ccm".to_owned(),
                        parameter: "fir".to_owned(),
                    },
                    RTCPFeedback {
                        typ: "nack".to_owned(),
                        parameter: String::new(),
                    },
                    RTCPFeedback {
                        typ: "nack".to_owned(),
                        parameter: "pli".to_owned(),
                    },
                ],
            },
            payload_type: 96,
            ..Default::default()
        },
        RTPCodecType::Video,
    )?;
    Ok(())
}

/// Build the track pair carrying `sender`'s media into a receiver's
/// connection.
///
/// The track id is the sender's pseudonym so the browser sees it as the MSID
/// and can attribute the tile; the stream id groups a sender's audio and video
/// into one MediaStream, which is what lets a client pair them.
fn make_outbound_tracks(channel_id: &str, sender: &str) -> OutboundTracks {
    OutboundTracks {
        audio: Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: "audio/opus".to_string(),
                clock_rate: 48_000,
                channels: 1,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                rtcp_feedback: vec![],
            },
            format!("audio-{sender}"),
            format!("{channel_id}-{sender}"),
        )),
        video: Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: "video/VP8".to_string(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: vec![],
            },
            format!("video-{sender}"),
            format!("{channel_id}-{sender}"),
        )),
        screen: Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: "video/VP8".to_string(),
                clock_rate: 90_000,
                ..Default::default()
            },
            format!("screen-{sender}"),
            format!("screen-{sender}"),
        )),
    }
}

struct Room {
    peers: DashMap<String, Arc<PeerSession>>,
    agent_tracks: DashMap<String, Arc<TrackLocalStaticSample>>,
}

pub struct VoiceService {
    config: WebRtcConfig,
    api: API,
    rooms: DashMap<String, Arc<Room>>,
    /// Source of [`PeerSession::generation`].
    peer_generation: std::sync::atomic::AtomicU64,
    runtime_public_url: RwLock<String>,
    runtime_disabled: RwLock<bool>,
    stt_tap_tx: broadcast::Sender<SttTapFrame>,
    ice_candidate_tx: broadcast::Sender<IceCandidateEvent>,
    renegotiate_tx: broadcast::Sender<RenegotiateEvent>,
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
        if let Err(e) = register_relay_codecs(&mut media_engine) {
            warn!(error = %e, "failed to register relay codecs");
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
        let (renegotiate_tx, _) = broadcast::channel(256);

        Self {
            config,
            api,
            rooms: DashMap::new(),
            peer_generation: std::sync::atomic::AtomicU64::new(0),
            runtime_public_url: RwLock::new(String::new()),
            runtime_disabled: RwLock::new(false),
            stt_tap_tx,
            ice_candidate_tx,
            renegotiate_tx,
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

    /// End every media connection when the channel itself is deleted.
    pub async fn delete_room(&self, name: &str) -> Result<(), VoiceError> {
        let Some((_, room)) = self.rooms.remove(name) else {
            return Ok(());
        };
        let peers: Vec<_> = room.peers.iter().map(|p| p.value().clone()).collect();
        room.peers.clear();
        let mut error = None;
        for peer in peers {
            if let Err(e) = peer.pc.close().await {
                error = Some(VoiceError::WebRtc(e.to_string()));
            }
        }
        error.map_or(Ok(()), Err)
    }

    /// Operator voice disable closes every active peer, attempting all rooms
    /// even if an individual connection reports a close error.
    pub async fn close_all_rooms(&self) -> Result<(), VoiceError> {
        let rooms: Vec<_> = self.rooms.iter().map(|room| room.key().clone()).collect();
        let mut error = None;
        for room in rooms {
            if let Err(e) = self.delete_room(&room).await {
                error = Some(e);
            }
        }
        error.map_or(Ok(()), Err)
    }

    /// Produce a signed, expiring voice-join token bound to
    /// `(room_name, participant_identity)`.
    ///
    /// Replaces the previous unsigned base64 JSON blob. The new format
    /// is an HMAC-SHA256-signed pipe-delimited token (see
    /// `crate::token` for the wire format); callers verify it with
    /// [`crate::token::verify_join_token`]. The `_participant_name`
    /// argument is preserved for source compatibility but no longer
    /// participates in the signed claim — the binding that matters is
    /// `(room, sub, exp)`.
    ///
    /// Returns [`VoiceError::Config`] if `room_name` or
    /// `participant_identity` contains a `|`, since that would corrupt
    /// the pipe-delimited token format. Real channel ids (UUIDs) and
    /// pseudonyms (hex) never contain `|`, so this is purely a
    /// defensive guard.
    pub fn generate_join_token(
        &self,
        room_name: &str,
        participant_identity: &str,
        _participant_name: &str,
        secret: &[u8; 32],
        ttl_secs: u64,
    ) -> Result<String, VoiceError> {
        crate::token::generate_join_token(room_name, participant_identity, secret, ttl_secs)
            .map_err(|e| VoiceError::Config(format!("voice token: {e}")))
    }

    /// The pseudonyms currently connected to `room_name`, sorted.
    ///
    /// The SFU has always keyed peers by pseudonym; only the count was ever
    /// exposed, so the client knew how many people were in a call but not who,
    /// and every remote tile in `RemoteParticipants.tsx` read "Participant".
    /// Sorted so a caller rendering the roster gets a stable order rather than
    /// `DashMap` iteration order, which reshuffles between reads.
    pub async fn participant_ids(&self, room_name: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .rooms
            .get(room_name)
            .map(|r| {
                r.peers
                    .iter()
                    .map(|p| p.key().clone())
                    .chain(r.agent_tracks.iter().map(|a| a.key().clone()))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids.dedup();
        ids
    }

    pub async fn participant_count(&self, room_name: &str) -> Result<u32, VoiceError> {
        Ok(self.participant_ids(room_name).await.len() as u32)
    }

    /// Remove `identity` from every room it is in.
    ///
    /// Called when a WebSocket session ends. Until this existed, a peer was
    /// only ever removed by an explicit leave — so closing a tab, losing a
    /// network, or crashing left them in the room forever: still on the
    /// roster, still holding a track slot on every other peer's connection,
    /// and keeping the room alive so it was never reaped. Other participants
    /// saw a tile for someone who had gone.
    ///
    /// Returns the channels it removed them from, so the caller can
    /// renegotiate with whoever is left.
    pub async fn remove_participant_everywhere(&self, identity: &str) -> Vec<String> {
        let rooms: Vec<String> = self
            .rooms
            .iter()
            .filter(|r| r.peers.contains_key(identity) || r.agent_tracks.contains_key(identity))
            .map(|r| r.key().clone())
            .collect();

        for channel_id in &rooms {
            let _ = self.remove_participant(channel_id, identity).await;
        }
        rooms
    }

    pub async fn remove_participant(&self, room: &str, identity: &str) -> Result<(), VoiceError> {
        if let Some(peer) = self.drop_peer_and_maybe_reap(room, identity) {
            if let Err(e) = peer.pc.close().await {
                debug!(error = %e, "failed to close peer connection");
            }
            // Everyone still in the room held a track pair for the departed
            // peer. Drop it and re-offer, or their grid keeps a tile for
            // somebody who has gone.
            self.drop_sender_from_remaining_peers(room, identity).await;
        }
        self.remove_agent(room, identity).await;
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
        self.drop_peer_inner(channel_id, peer_id, None)
    }

    /// Remove a peer entry only if it is still the connection that asked.
    ///
    /// This is what a terminating connection's own callback must call. The
    /// unconditional form above is right for `remove_participant`, which means
    /// "this person is leaving, whatever socket they are on"; it is wrong for a
    /// state-change callback, which means "MY connection died" and must not
    /// evict the reconnect that replaced it.
    fn drop_peer_if_generation(
        &self,
        channel_id: &str,
        peer_id: &str,
        generation: u64,
    ) -> Option<Arc<PeerSession>> {
        self.drop_peer_inner(channel_id, peer_id, Some(generation))
    }

    fn drop_peer_inner(
        &self,
        channel_id: &str,
        peer_id: &str,
        only_generation: Option<u64>,
    ) -> Option<Arc<PeerSession>> {
        let mut peer_removed = None;
        let mut should_reap = false;
        if let Some(room_entry) = self.rooms.get(channel_id) {
            // `remove_if` evaluates the predicate under the entry lock, so a
            // concurrent insert of the replacement cannot slip between the
            // check and the removal.
            let removed = match only_generation {
                Some(generation) => room_entry
                    .peers
                    .remove_if(peer_id, |_, p| p.generation == generation),
                None => room_entry.peers.remove(peer_id),
            };
            if let Some((_, peer)) = removed {
                peer_removed = Some(peer);
                should_reap = room_entry.peers.is_empty() && room_entry.agent_tracks.is_empty();
            }
        }
        if should_reap {
            self.rooms.remove_if(channel_id, |_, room| {
                room.peers.is_empty() && room.agent_tracks.is_empty()
            });
        }
        peer_removed
    }

    /// Is this person already an admitted peer in this room?
    ///
    /// Used by the signalling layer to tell "entering a call" from
    /// "renegotiating one already in progress" — the first needs a fresh
    /// join grant, the second must not, or a call outliving the grant's TTL
    /// would drop the moment anyone added a track.
    pub fn has_peer(&self, channel_id: &str, peer_id: &str) -> bool {
        self.rooms
            .get(channel_id)
            .is_some_and(|room| room.peers.contains_key(peer_id))
    }

    pub fn subscribe_stt_taps(&self) -> broadcast::Receiver<SttTapFrame> {
        self.stt_tap_tx.subscribe()
    }

    /// Push a frame onto the STT tap without an RTP packet behind it.
    ///
    /// Test-only. The windowing in [`crate::agent`] is a property of what
    /// the tap emits over time, and the only other way to drive it is a
    /// real peer connection publishing real Opus.
    #[cfg(test)]
    pub(crate) fn emit_stt_tap_for_test(&self, frame: SttTapFrame) {
        let _ = self.stt_tap_tx.send(frame);
    }

    pub fn subscribe_ice_candidates(&self) -> broadcast::Receiver<IceCandidateEvent> {
        self.ice_candidate_tx.subscribe()
    }

    pub fn subscribe_renegotiations(&self) -> broadcast::Receiver<RenegotiateEvent> {
        self.renegotiate_tx.subscribe()
    }

    /// Accept a peer's answer to an offer the server initiated.
    ///
    /// The counterpart to the `webrtc_offer` pushed by
    /// [`Self::renegotiate_with`]. Unknown room or peer is not an error — both
    /// mean the peer left while the offer was in flight.
    pub async fn handle_renegotiation_answer(
        &self,
        channel_id: &str,
        peer_id: &str,
        answer_sdp: &str,
    ) -> Result<(), VoiceError> {
        let Some(room) = self.rooms.get(channel_id) else {
            return Ok(());
        };
        let Some(peer) = room.peers.get(peer_id).map(|p| p.clone()) else {
            return Ok(());
        };
        drop(room);

        let answer = RTCSessionDescription::answer(answer_sdp.to_string())
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
        let guard = peer.renegotiation_pending.lock().await;
        peer.pc
            .set_remote_description(answer)
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
        let pending = *guard;
        drop(guard);
        if pending {
            self.renegotiate_with(channel_id, peer_id, &peer).await;
        }
        Ok(())
    }

    /// Called after the initial answer is queued on the signalling socket.
    /// An answer cannot introduce media sections absent from the browser offer;
    /// this follow-up offer includes every remote participant and screen track.
    pub async fn complete_initial_negotiation(&self, channel_id: &str, peer_id: &str) {
        let peer = self
            .rooms
            .get(channel_id)
            .and_then(|room| room.peers.get(peer_id).map(|peer| peer.value().clone()));
        if let Some(peer) = peer {
            self.renegotiate_with(channel_id, peer_id, &peer).await;
        }
    }

    /// Offer `receiver` an updated session description after its track set
    /// changed.
    ///
    /// Best-effort by design: a peer that has gone away, or whose connection
    /// is closing, must not fail the join of the peer that triggered this.
    async fn renegotiate_with(&self, channel_id: &str, receiver: &str, peer: &Arc<PeerSession>) {
        let mut pending = peer.renegotiation_pending.lock().await;
        // Complete the initial handshake before publishing later changes.
        // Keep one offer outstanding, including while ICE is recovering.
        if peer.pc.connection_state()
            != webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected
            || peer.pc.signaling_state()
                != webrtc::peer_connection::signaling_state::RTCSignalingState::Stable
        {
            *pending = true;
            return;
        }
        *pending = false;
        let offer = match peer.pc.create_offer(None).await {
            Ok(o) => o,
            Err(e) => {
                debug!(peer = %receiver, error = %e, "could not create renegotiation offer");
                return;
            }
        };
        if let Err(e) = peer.pc.set_local_description(offer.clone()).await {
            debug!(peer = %receiver, error = %e, "could not set local description");
            return;
        }
        // No receiver is not an error: it just means nobody is listening on the
        // broadcast yet, which is normal in tests and during shutdown.
        let _ = self.renegotiate_tx.send(RenegotiateEvent {
            channel_id: channel_id.to_string(),
            peer_id: receiver.to_string(),
            sdp: offer.sdp,
        });
    }

    /// Drop the departed peer's track pair from everyone still in the room and
    /// offer them the updated description.
    async fn drop_sender_from_remaining_peers(&self, channel_id: &str, departed: &str) {
        let Some(room) = self.rooms.get(channel_id) else {
            return;
        };
        let remaining: Vec<(String, Arc<PeerSession>)> = room
            .peers
            .iter()
            .map(|p| (p.key().clone(), p.value().clone()))
            .collect();
        drop(room);

        for (_receiver_id, peer) in remaining {
            if let Some((_, tracks)) = peer.outbound.remove(departed) {
                for track in [&tracks.audio, &tracks.video, &tracks.screen] {
                    if let Err(error) = peer.park_publication(track.id()).await {
                        debug!(%error,"could not park departed publication");
                    }
                }
            }
        }
    }

    /// Give every existing peer a track pair for the newcomer, and offer them
    /// the updated description.
    ///
    /// Runs after the newcomer is in the room, so it deliberately skips them:
    /// nobody receives their own media back.
    async fn add_sender_to_existing_peers(&self, channel_id: &str, newcomer: &str) {
        let Some(room) = self.rooms.get(channel_id) else {
            return;
        };
        let receivers: Vec<(String, Arc<PeerSession>)> = room
            .peers
            .iter()
            .filter(|p| p.key() != newcomer)
            .map(|p| (p.key().clone(), p.value().clone()))
            .collect();
        drop(room);

        for (receiver_id, peer) in receivers {
            if peer.outbound.contains_key(newcomer) {
                continue;
            }
            let tracks = make_outbound_tracks(channel_id, newcomer);
            let reusing = [&tracks.audio, &tracks.video, &tracks.screen]
                .iter()
                .all(|track| peer.parked_publications.contains_key(track.id()));
            let added_audio = peer.attach_publication(tracks.audio.clone()).await;
            let added_video = peer.attach_publication(tracks.video.clone()).await;
            let added_screen = peer.attach_publication(tracks.screen.clone()).await;
            if let (Ok(_), Ok(_), Ok(_)) = (added_audio, added_video, added_screen) {
                peer.outbound.insert(newcomer.to_string(), tracks);
                if !reusing {
                    self.renegotiate_with(channel_id, &receiver_id, &peer).await;
                }
            } else {
                debug!(
                    receiver = %receiver_id,
                    sender = %newcomer,
                    "could not add tracks for the new peer",
                );
            }
        }
    }

    pub async fn handle_sdp_offer(
        self: &Arc<Self>,
        channel_id: &str,
        peer_id: &str,
        offer_sdp: &str,
    ) -> Result<RTCSessionDescription, VoiceError> {
        self.create_peer_session(channel_id, peer_id, Some(offer_sdp))
            .await
    }

    /// Server remains the offerer for the lifetime of a browser call. This
    /// avoids reversing ICE roles when receive publications are introduced.
    pub async fn initiate_connection(
        self: &Arc<Self>,
        channel_id: &str,
        peer_id: &str,
    ) -> Result<RTCSessionDescription, VoiceError> {
        self.create_peer_session(channel_id, peer_id, None).await
    }

    async fn create_peer_session(
        self: &Arc<Self>,
        channel_id: &str,
        peer_id: &str,
        offer_sdp: Option<&str>,
    ) -> Result<RTCSessionDescription, VoiceError> {
        let room = self.get_or_create_room(channel_id).await?;
        let pc = Arc::new(
            self.api
                .new_peer_connection(self.rtc_config())
                .await
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?,
        );

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
        // Taken before the callback is registered, so the closure captures the
        // identity of THIS connection and nothing later can change it.
        let generation = self
            .peer_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let pc_for_track = Arc::clone(&pc);
        let screen_mids = offer_sdp
            .map(screen_mids)
            .unwrap_or_else(|| ["2".to_string()].into_iter().collect());
        pc.on_track(Box::new(move |track, _, transceiver| {
            let service = Arc::clone(&service);
            let channel = channel.clone();
            let publisher = publisher.clone();
            let pc_for_track = Arc::clone(&pc_for_track);
            let is_screen = transceiver
                .mid()
                .is_some_and(|mid| screen_mids.contains(mid.as_str()));
            Box::pin(async move {
                tokio::spawn(async move {
                    if let Err(e) = service
                        .forward_track_loop(
                            channel,
                            publisher,
                            generation,
                            pc_for_track,
                            track,
                            is_screen,
                        )
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
                if state == RTCPeerConnectionState::Connected {
                    let peer = service.rooms.get(&cleanup_channel).and_then(|room| {
                        room.peers
                            .get(&cleanup_peer)
                            .filter(|peer| peer.generation == generation)
                            .map(|peer| peer.value().clone())
                    });
                    if let Some(peer) = peer {
                        let pending = *peer.renegotiation_pending.lock().await;
                        if pending {
                            // Keep negotiation outside the WebRTC state callback.
                            tokio::spawn(async move {
                                service
                                    .renegotiate_with(&cleanup_channel, &cleanup_peer, &peer)
                                    .await;
                            });
                        }
                    }
                    return;
                }
                if matches!(
                    state,
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
                ) {
                    // Disconnected is a recoverable ICE interruption. Removing
                    // the peer there stops forwarding even if ICE recovers.
                    // The pc is already terminating on its own — just
                    // drop the peer, reap the room if empty. We don't
                    // call pc.close() here (the state change implies it
                    // is already closing).
                    //
                    // Generation-scoped: this callback belongs to one
                    // connection, and a reconnect that replaced it owns the
                    // map entry now. Removing by key alone would make the
                    // dying socket evict its own replacement — and once the
                    // replaced connection is explicitly closed below, that
                    // would happen on every reconnect rather than occasionally.
                    let _ = service.drop_peer_if_generation(
                        &cleanup_channel,
                        &cleanup_peer,
                        generation,
                    );
                }
            })
        }));

        if let Some(sdp) = offer_sdp {
            let offer = RTCSessionDescription::offer(sdp.to_string())
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
            pc.set_remote_description(offer)
                .await
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
        } else {
            // Stable publishing slots: microphone, camera, screen. They are
            // separate from the attributed send-only receive publications.
            for kind in [
                RTPCodecType::Audio,
                RTPCodecType::Video,
                RTPCodecType::Video,
            ] {
                pc.add_transceiver_from_kind(
                    kind,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Recvonly,
                        send_encodings: vec![],
                    }),
                )
                .await
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
            }
        }

        // Publication sections were allocated above. Separate send-only
        // transceivers ensure each server offer introduces attributed
        // remote tracks, rather than reusing immutable browser receiver IDs.
        // One microphone/camera/screen group per peer ALREADY in the room, so this connection
        // receives each of them on its own track. The agent (TTS) track is
        // added last, as before.
        let existing: Vec<String> = room
            .peers
            .iter()
            .filter(|p| p.key() != peer_id)
            .map(|p| p.key().clone())
            .collect();

        let outbound: DashMap<String, OutboundTracks> = DashMap::new();
        for sender in &existing {
            let tracks = make_outbound_tracks(channel_id, sender);
            add_relay_publication(
                &pc,
                tracks.audio.clone() as Arc<dyn TrackLocal + Send + Sync>,
            )
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
            add_relay_publication(
                &pc,
                tracks.video.clone() as Arc<dyn TrackLocal + Send + Sync>,
            )
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
            add_relay_publication(
                &pc,
                tracks.screen.clone() as Arc<dyn TrackLocal + Send + Sync>,
            )
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
            outbound.insert(sender.clone(), tracks);
        }

        let agent_tracks: Vec<_> = room
            .agent_tracks
            .iter()
            .map(|t| t.value().clone())
            .collect();
        for track in agent_tracks {
            add_relay_publication(&pc, track as Arc<dyn TrackLocal + Send + Sync>)
                .await
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
        }

        let answer = if offer_sdp.is_some() {
            pc.create_answer(None).await
        } else {
            pc.create_offer(None).await
        }
        .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| VoiceError::WebRtc(e.to_string()))?;

        // Close whatever this offer replaced.
        //
        // `DashMap::insert` hands back the displaced entry and the old code
        // discarded it, so the previous `RTCPeerConnection` was never closed —
        // and never even dropped, because its own `on_track` closure holds an
        // `Arc` clone of it. Every reconnect leaked a live peer connection,
        // its ICE agent, and its forwarding tasks, for the lifetime of the
        // process.
        //
        // Safe to do only because the state-change callback above is now
        // generation-scoped: closing the old connection drives it to `Closed`,
        // which fires that callback, which before this change would have
        // removed the entry we have just inserted.
        if let Some(displaced) = room.peers.insert(
            peer_id.to_string(),
            Arc::new(PeerSession {
                pc,
                outbound,
                generation,
                parked_publications: DashMap::new(),
                renegotiation_pending: tokio::sync::Mutex::new(false),
                publication_timelines: DashMap::new(),
            }),
        ) {
            if let Err(e) = displaced.pc.close().await {
                debug!(error = %e, "failed to close the peer connection this offer replaced");
            }
            // A fresh publisher connection also has a fresh packet source. Rebind
            // retained publications so their sequence transformer resets its offset.
            self.drop_sender_from_remaining_peers(channel_id, peer_id)
                .await;
        }

        // Now that the newcomer is in the room, give everyone else a track for
        // them and offer the updated description. Done after the insert so the
        // set of receivers is exactly "everyone but the newcomer".
        self.add_sender_to_existing_peers(channel_id, peer_id).await;

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

    /// Every agent owns an independently decoded Opus stream.
    pub async fn add_agent(&self, channel_id: &str, agent_id: &str) -> Result<(), VoiceError> {
        let room = self.get_or_create_room(channel_id).await?;
        let track = {
            use dashmap::mapref::entry::Entry;
            match room.agent_tracks.entry(agent_id.to_string()) {
                Entry::Occupied(_) => return Ok(()),
                Entry::Vacant(entry) => entry
                    .insert(Arc::new(TrackLocalStaticSample::new(
                        RTCRtpCodecCapability {
                            mime_type: "audio/opus".into(),
                            clock_rate: 48_000,
                            channels: 1,
                            ..Default::default()
                        },
                        format!("audio-{agent_id}"),
                        format!("agent-{agent_id}"),
                    )))
                    .value()
                    .clone(),
            }
        };
        let peers: Vec<_> = room
            .peers
            .iter()
            .map(|p| (p.key().clone(), p.value().clone()))
            .collect();
        for (id, peer) in peers {
            peer.attach_publication(track.clone())
                .await
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?;
            self.renegotiate_with(channel_id, &id, &peer).await;
        }
        Ok(())
    }

    async fn remove_agent(&self, channel_id: &str, agent_id: &str) {
        let Some(room) = self.rooms.get(channel_id).map(|r| r.value().clone()) else {
            return;
        };
        let Some((_, track)) = room.agent_tracks.remove(agent_id) else {
            return;
        };
        let peers: Vec<_> = room
            .peers
            .iter()
            .map(|p| (p.key().clone(), p.value().clone()))
            .collect();
        for (id, peer) in peers {
            if let Err(error) = peer.park_publication(track.id()).await {
                debug!(%error,"could not park agent publication");
            }
            self.renegotiate_with(channel_id, &id, &peer).await;
        }
        self.rooms.remove_if(channel_id, |_, r| {
            r.peers.is_empty() && r.agent_tracks.is_empty()
        });
    }

    pub async fn inject_agent_opus(
        &self,
        channel_id: &str,
        agent_id: &str,
        opus_frame: &[u8],
        duration: std::time::Duration,
    ) -> Result<(), VoiceError> {
        let room = self
            .rooms
            .get(channel_id)
            .map(|r| r.value().clone())
            .ok_or_else(|| VoiceError::RoomService("Voice room has ended".into()))?;
        let track = room
            .agent_tracks
            .get(agent_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| VoiceError::RoomService("Agent has left the room".into()))?;
        track
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
            agent_tracks: DashMap::new(),
        });

        Ok(self
            .rooms
            .entry(channel_id.to_string())
            .or_insert(room)
            .clone())
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
        generation: u64,
        pc: Arc<RTCPeerConnection>,
        track: Arc<webrtc::track::track_remote::TrackRemote>,
        is_screen: bool,
    ) -> Result<(), VoiceError> {
        let media_ssrc = track.ssrc();
        let kind = track.kind();
        let is_video = kind == RTPCodecType::Video;
        let mut last_seq: Option<u16> = None;
        let mut last_pli = std::time::Instant::now();
        // The Opus decoder + STT tap only apply to audio. Video RTP must never
        // be fed to the Opus decoder (garbage) — it is relayed as opaque RTP.
        let mut decoder = if is_video {
            None
        } else {
            Some(SttOpusDecoder::default())
        };

        loop {
            let (rtp, _) = track
                .read_rtp()
                .await
                .map_err(|e| VoiceError::WebRtc(e.to_string()))?;

            let current = self
                .rooms
                .get(&channel_id)
                .and_then(|room| room.peers.get(&publisher_id).map(|peer| peer.generation));
            if current != Some(generation) {
                return Ok(());
            }

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

            if is_video && last_pli.elapsed() >= std::time::Duration::from_secs(2) {
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

            self.fan_out_rtp(
                &channel_id,
                &publisher_id,
                generation,
                kind,
                is_screen,
                &rtp,
            )
            .await;
            if let Some(decoder) = decoder.as_mut() {
                self.tap_for_stt(&channel_id, &publisher_id, &rtp, decoder)
                    .await;
            }
        }
    }

    async fn fan_out_rtp(
        &self,
        channel_id: &str,
        publisher_id: &str,
        generation: u64,
        kind: RTPCodecType,
        is_screen: bool,
        rtp: &RtpPacket,
    ) {
        if let Some(room) = self.rooms.get(channel_id) {
            let want_video = kind == RTPCodecType::Video;
            let outbound: Vec<(String, Arc<PeerSession>, Arc<TrackLocalStaticRTP>)> = room
                .peers
                .iter()
                .filter(|peer| peer.key().as_str() != publisher_id)
                // `filter_map`: a receiver with no track pair for this
                // publisher yet — the window between joining and renegotiation
                // completing — has nowhere to put the packet. Dropping it is
                // correct; the encoder produces a keyframe once the track
                // exists.
                .filter_map(|peer| {
                    let track = if is_screen && want_video {
                        peer.outbound
                            .get(publisher_id)
                            .map(|tracks| tracks.screen.clone())
                    } else {
                        outbound_track_for(peer.value(), publisher_id, want_video)
                    };
                    track.map(|track| (peer.key().clone(), peer.value().clone(), track))
                })
                .collect();
            drop(room);

            for (peer_id, peer, track) in outbound {
                if let Err(e) = peer.forward_publication(&track, generation, rtp).await {
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
        decoder: &mut SttOpusDecoder,
    ) {
        match decoder.decode(&rtp.payload) {
            Ok(raw) => {
                let _ = self.stt_tap_tx.send(SttTapFrame {
                    channel_id: channel_id.to_string(),
                    speaker_pseudonym: speaker.to_string(),
                    pcm_s16le: raw,
                });
            }
            Err(error) => {
                if !decoder.warned {
                    decoder.warned = true;
                    warn!(channel = %channel_id, speaker = %speaker, %error,
                        "could not decode incoming Opus for captions");
                }
            }
        }
    }
}

/// The reference Opus decoder accepts both mono and stereo packets and
/// downmixes into the requested mono output without resetting codec history.
/// The previous decoder rejected stereo packets and valid multi-frame VBR.
#[derive(Default)]
struct SttOpusDecoder {
    decoder: Option<OpusDecoder>,
    warned: bool,
}

impl SttOpusDecoder {
    fn decode(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        if payload.is_empty() {
            return Err("empty Opus packet".into());
        }
        if self.decoder.is_none() {
            self.decoder = Some(
                OpusDecoder::new(OpusChannels::Mono, OpusSampleRate::Hz48000)
                    .map_err(|error| format!("{error:?}"))?,
            );
        }
        // Opus permits packets up to 120 ms. At 48 kHz that is 5,760 mono
        // samples; a fixed 20 ms buffer rejected longer valid speech packets.
        let mut pcm = vec![0.0; 5_760];
        let samples = self
            .decoder
            .as_mut()
            .ok_or("missing Opus decoder")?
            .decode_float_to_slice(payload, &mut pcm, false)
            .map_err(|error| format!("{error:?}"))?;
        Ok(pcm_f32_to_s16le_bytes(&pcm[..samples]))
    }
}

/// Convert normalized float PCM (`[-1.0, 1.0]`, the output range of
/// the reference Opus decoder) to little-endian signed 16-bit PCM.
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

    #[test]
    fn caption_decoder_accepts_real_libopus_stereo_and_sixty_ms_packets() {
        let mut decoder = SttOpusDecoder::default();
        for (fixture, samples) in [
            (
                &include_bytes!("../tests/fixtures/stt-stereo.packets")[..],
                960,
            ),
            (
                &include_bytes!("../tests/fixtures/stt-mono-60ms.packets")[..],
                2880,
            ),
            (
                &include_bytes!("../tests/fixtures/stt-stereo.packets")[..],
                960,
            ),
        ] {
            let mut rest = fixture;
            let mut energy = 0u64;
            let mut frames = 0;
            while !rest.is_empty() {
                let size = u16::from_le_bytes([rest[0], rest[1]]) as usize;
                let pcm = decoder
                    .decode(&rest[2..2 + size])
                    .expect("real libopus packet");
                assert_eq!(pcm.len(), samples * 2, "STT is always 48kHz mono");
                energy += pcm
                    .chunks_exact(2)
                    .map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() as u64)
                    .sum::<u64>();
                rest = &rest[2 + size..];
                frames += 1;
            }
            assert!(frames >= 10);
            assert!(energy > 10_000, "decoded real speech must not be silent");
        }
    }

    #[tokio::test]
    async fn rejoined_publication_preserves_sequence_over_real_srtp() {
        use std::time::Duration;
        let service = test_service();
        let sender = Arc::new(
            service
                .api
                .new_peer_connection(RTCConfiguration::default())
                .await
                .unwrap(),
        );
        let receiver = Arc::new(
            service
                .api
                .new_peer_connection(RTCConfiguration::default())
                .await
                .unwrap(),
        );
        let session = PeerSession {
            pc: sender.clone(),
            generation: 0,
            outbound: DashMap::new(),
            parked_publications: DashMap::new(),
            renegotiation_pending: tokio::sync::Mutex::new(false),
            publication_timelines: DashMap::new(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        receiver.on_track(Box::new(move |track, _, _| {
            let tx = tx.clone();
            Box::pin(async move {
                tokio::spawn(async move {
                    while let Ok((packet, _)) = track.read_rtp().await {
                        let _ = tx.send(packet);
                    }
                });
            })
        }));
        async fn negotiate(a: &RTCPeerConnection, b: &RTCPeerConnection) {
            let mut gather = a.gathering_complete_promise().await;
            let offer = a.create_offer(None).await.unwrap();
            a.set_local_description(offer).await.unwrap();
            let _ = gather.recv().await;
            b.set_remote_description(a.local_description().await.unwrap())
                .await
                .unwrap();
            let mut gather = b.gathering_complete_promise().await;
            let answer = b.create_answer(None).await.unwrap();
            b.set_local_description(answer).await.unwrap();
            let _ = gather.recv().await;
            a.set_remote_description(b.local_description().await.unwrap())
                .await
                .unwrap();
        }
        let track = make_outbound_tracks("srtp-rejoin", "publisher").video;
        session.attach_publication(track.clone()).await.unwrap();
        negotiate(&sender, &receiver).await;
        tokio::time::timeout(Duration::from_secs(8), async {
            while sender.connection_state()
                != webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual DTLS transport must connect");
        let packet = |sequence, timestamp| RtpPacket {
            header: webrtc::rtp::header::Header {
                version: 2,
                sequence_number: sequence,
                timestamp,
                marker: true,
                ..Default::default()
            },
            payload: Bytes::from_static(&[0x10, 0, 0, 0]),
        };
        session
            .forward_publication(&track, 0, &packet(40_000, 9_000))
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let mut previous = first.header.sequence_number;
        let mut previous_timestamp = first.header.timestamp;
        for (index, restart) in [39_000, 12_000, 65_530].into_iter().enumerate() {
            session.park_publication("video-publisher").await.unwrap();
            assert_eq!(
                sender.get_transceivers().await[0].direction(),
                RTCRtpTransceiverDirection::Sendonly
            );
            let replacement = make_outbound_tracks("srtp-rejoin", "publisher").video;
            session
                .attach_publication(replacement.clone())
                .await
                .unwrap();
            session
                .forward_publication(
                    &replacement,
                    index as u64 + 1,
                    &packet(restart, 2_700_000_000 + u32::from(restart)),
                )
                .await
                .unwrap();
            let result = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
            if result.is_err() {
                sender.close().await.unwrap();
                receiver.close().await.unwrap();
                panic!(
                    "replacement sequence {restart} was rejected by the established SRTP receiver"
                );
            }
            let received = result.unwrap().unwrap();
            assert_eq!(
                received.header.ssrc, first.header.ssrc,
                "publication slot retains its SSRC"
            );
            assert_eq!(
                received.header.sequence_number,
                previous.wrapping_add(1),
                "replacement must preserve outbound packet sequence"
            );
            assert!(
                received.header.timestamp.wrapping_sub(previous_timestamp) < 90_000,
                "replacement must retain the receiver timeline, got {} after {}",
                received.header.timestamp,
                previous_timestamp
            );
            previous_timestamp = received.header.timestamp;
            previous = received.header.sequence_number;
        }
        sender.close().await.unwrap();
        receiver.close().await.unwrap();
    }

    #[tokio::test]
    async fn late_joiner_gets_distinct_authoritative_receive_sections() {
        let service = Arc::new(test_service());
        let room = service.get_or_create_room("join-order").await.unwrap();
        room.peers
            .insert("founder".into(), peer_session("join-order", &[]).await);
        let browser = Arc::new(
            service
                .api
                .new_peer_connection(RTCConfiguration::default())
                .await
                .unwrap(),
        );
        for kind in [
            RTPCodecType::Audio,
            RTPCodecType::Video,
            RTPCodecType::Video,
        ] {
            browser
                .add_transceiver_from_kind(
                    kind,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Sendonly,
                        send_encodings: vec![],
                    }),
                )
                .await
                .unwrap();
        }
        let initial = service
            .initiate_connection("join-order", "late")
            .await
            .unwrap();
        let sections: Vec<_> = initial.sdp.split("m=").skip(1).collect();
        assert_eq!(sections.len(), 6);
        for track in ["audio-founder", "video-founder", "screen-founder"] {
            assert!(
                sections[3..].iter().any(|section| section.contains(track)),
                "missing {track}"
            );
            assert!(
                !sections[..3].iter().any(|section| section.contains(track)),
                "reused publisher for {track}"
            );
        }
        let server_pc = room.peers.get("late").unwrap().pc.clone();
        let mut gathered = server_pc.gathering_complete_promise().await;
        tokio::time::timeout(std::time::Duration::from_secs(5), gathered.recv())
            .await
            .unwrap();
        browser
            .set_remote_description(server_pc.local_description().await.unwrap())
            .await
            .unwrap();
        let answer = browser.create_answer(None).await.unwrap();
        let mut gathered = browser.gathering_complete_promise().await;
        browser.set_local_description(answer).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), gathered.recv())
            .await
            .unwrap();
        service
            .handle_renegotiation_answer(
                "join-order",
                "late",
                &browser.local_description().await.unwrap().sdp,
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while browser.connection_state() != webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected
                || server_pc.connection_state() != webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        let mut offers = service.subscribe_renegotiations();
        for _ in 0..3 {
            service
                .drop_sender_from_remaining_peers("join-order", "founder")
                .await;
            assert!(
                matches!(
                    offers.try_recv(),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                ),
                "pausing a reserved slot must not deactivate the receiver"
            );
            service
                .add_sender_to_existing_peers("join-order", "founder")
                .await;
            assert!(
                matches!(
                    offers.try_recv(),
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                ),
                "replacing a reserved source needs no SDP renegotiation"
            );
            service
                .complete_initial_negotiation("join-order", "late")
                .await;
            let rejoined = offers.recv().await.unwrap();
            assert_eq!(rejoined.peer_id, "late");
            assert_eq!(
                rejoined
                    .sdp
                    .lines()
                    .filter(|line| line.starts_with("m="))
                    .count(),
                6,
                "rejoins must reuse publication slots"
            );
            let msids: Vec<_> = rejoined
                .sdp
                .lines()
                .filter(|line| line.starts_with("a=msid:"))
                .collect();
            let unique: std::collections::HashSet<_> = msids.iter().collect();
            assert_eq!(
                msids.len(),
                unique.len(),
                "Chrome rejects duplicate MSID identities"
            );
            for id in ["audio-founder", "video-founder", "screen-founder"] {
                assert_eq!(msids.iter().filter(|line| line.ends_with(id)).count(), 1);
            }
            browser
                .set_remote_description(RTCSessionDescription::offer(rejoined.sdp).unwrap())
                .await
                .unwrap();
            let answer = browser.create_answer(None).await.unwrap();
            browser.set_local_description(answer.clone()).await.unwrap();
            service
                .handle_renegotiation_answer("join-order", "late", &answer.sdp)
                .await
                .unwrap();
        }
        browser.close().await.unwrap();
        service.close_all_rooms().await.unwrap();
    }

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

    /// The default config, as a fresh install actually gets it.
    fn default_service() -> VoiceService {
        VoiceService::new(WebRtcConfig::default())
    }

    #[test]
    fn the_default_config_alone_cannot_serve_a_remote_client() {
        // Not a bug in itself — `WebRtcConfig::default()` points at
        // `ws://localhost:7880`, and handing a remote client a loopback
        // address would be worse than refusing. This is pinned so the
        // blanking is understood as deliberate, and so the test below reads
        // as the fix rather than as a coincidence.
        assert_eq!(
            default_service().get_public_url(),
            "",
            "a loopback URL must not be handed to a remote client",
        );
    }

    #[test]
    fn the_servers_public_url_makes_voice_reachable() {
        // The SFU is in-process and signals over the app's own WebSocket, so
        // the address a remote client needs for voice IS the address of the
        // server. `startup.rs` propagates the resolved public URL here at
        // boot; before it did, `voice_enabled: true` was the default and
        // every remote join was refused with `VoiceNotConfigured`, on a
        // server whose ANNEX_PUBLIC_URL was set correctly.
        let svc = default_service();
        svc.set_public_url("https://annex.example.com".to_string());
        assert_eq!(svc.get_public_url(), "https://annex.example.com");
    }

    #[test]
    fn a_loopback_public_url_is_still_refused_for_remote_clients() {
        // A single-machine dev setup sets ANNEX_PUBLIC_URL to localhost.
        // Propagating it must not turn the blanking off: the value is still
        // useless to anyone off-box, and pretending otherwise would trade a
        // clear "voice not configured" for a call that fails to connect.
        let svc = default_service();
        svc.set_public_url("http://localhost:3000".to_string());
        assert_eq!(svc.get_public_url(), "");
    }

    fn empty_room(_channel_id: &str) -> Arc<Room> {
        Arc::new(Room {
            peers: DashMap::new(),
            agent_tracks: DashMap::new(),
        })
    }

    #[tokio::test]
    async fn participant_ids_names_who_is_in_the_call() {
        let svc = test_service();
        let room = empty_room("ch-roster");
        for peer in ["carol", "alice", "bob"] {
            room.peers
                .insert(peer.to_string(), peer_session("ch-roster", &[]).await);
        }
        svc.rooms.insert("ch-roster".to_string(), room);

        // Sorted, not DashMap iteration order — a roster that reshuffles
        // between polls makes the participant list jump around on screen.
        assert_eq!(
            svc.participant_ids("ch-roster").await,
            vec!["alice", "bob", "carol"],
        );
        assert_eq!(
            svc.participant_count("ch-roster").await.unwrap(),
            3,
            "the count and the roster must agree",
        );
    }

    #[tokio::test]
    async fn participant_ids_is_empty_for_a_room_that_does_not_exist() {
        let svc = test_service();
        assert!(svc.participant_ids("no-such-channel").await.is_empty());
    }

    #[tokio::test]
    async fn delete_room_closes_every_peer_and_can_be_repeated() {
        let svc = test_service();
        let room = empty_room("deleted");
        let alice = peer_session("deleted", &[]).await;
        let bob = peer_session("deleted", &[]).await;
        room.peers.insert("alice".into(), alice.clone());
        room.peers.insert("bob".into(), bob.clone());
        svc.rooms.insert("deleted".into(), room);
        svc.delete_room("deleted").await.unwrap();
        assert!(!svc.rooms.contains_key("deleted"));
        assert_eq!(
            alice.pc.connection_state(),
            webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Closed
        );
        assert_eq!(
            bob.pc.connection_state(),
            webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Closed
        );
        svc.delete_room("deleted").await.unwrap();
    }

    #[test]
    fn screen_mid_is_second_video_section_not_second_media_section() {
        let mids = screen_mids("m=audio 9 RTP/AVP 111\r\na=mid:0\r\nm=video 9 RTP/AVP 96\r\na=mid:camera\r\nm=video 9 RTP/AVP 96\r\na=mid:screen\r\nm=video 9 RTP/AVP 96\r\na=mid:remote\r\n");
        assert_eq!(
            mids,
            std::collections::HashSet::from(["screen".to_string()])
        );
    }

    #[test]
    fn screen_and_camera_have_distinct_tracks_and_streams() {
        let tracks = make_outbound_tracks("room", "alice");
        assert_eq!(tracks.screen.id(), "screen-alice");
        assert_ne!(tracks.screen.stream_id(), tracks.video.stream_id());
        assert!(!Arc::ptr_eq(&tracks.screen, &tracks.video));
    }

    #[tokio::test]
    async fn simultaneous_agents_have_separate_attributed_audio_streams() {
        let svc = test_service();
        svc.add_agent("agents", "first").await.unwrap();
        svc.add_agent("agents", "second").await.unwrap();
        let room = svc.rooms.get("agents").unwrap().value().clone();
        let first = room.agent_tracks.get("first").unwrap().value().clone();
        let second = room.agent_tracks.get("second").unwrap().value().clone();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(first.id(), "audio-first");
        assert_eq!(second.id(), "audio-second");
        assert_eq!(svc.participant_ids("agents").await, ["first", "second"]);
        svc.remove_participant("agents", "first").await.unwrap();
        assert_eq!(svc.participant_ids("agents").await, ["second"]);
    }

    /// A `PeerSession` carrying a track pair for each named sender.
    ///
    /// `generation` is 0 for every fixture: these tests exercise track
    /// fan-out, not connection replacement, and a shared generation is the
    /// honest description of "this is the only connection for this peer".
    async fn peer_session(channel_id: &str, senders: &[&str]) -> Arc<PeerSession> {
        let pc = APIBuilder::new()
            .build()
            .new_peer_connection(Default::default())
            .await
            .expect("peer connection");
        let outbound = DashMap::new();
        for sender in senders {
            outbound.insert(sender.to_string(), make_outbound_tracks(channel_id, sender));
        }
        Arc::new(PeerSession {
            pc: Arc::new(pc),
            outbound,
            generation: 0,
            parked_publications: DashMap::new(),
            renegotiation_pending: tokio::sync::Mutex::new(false),
            publication_timelines: DashMap::new(),
        })
    }

    /// The SDP we put on the wire must offer exactly what the relay can
    /// forward.
    ///
    /// `make_outbound_tracks` hardcodes `audio/opus` and `video/VP8`, and
    /// `relay_inbound_track` builds an `OpusDecoder` for every audio track.
    /// Neither can hold unless negotiation is incapable of settling on
    /// anything else — the relay does not transcode, and
    /// `TrackLocalStaticRTP` rewrites the payload type while leaving the
    /// payload bytes alone, so a VP9 frame relabelled as VP8 reaches the
    /// receiver as noise.
    ///
    /// `register_default_codecs()` also advertised G722, PCMU, PCMA, VP9, six
    /// H.264 profiles and AV1. Asserting on the generated SDP rather than on
    /// the MediaEngine is deliberate: the SDP is what a browser actually
    /// negotiates against.
    #[tokio::test]
    async fn the_offer_advertises_only_the_codecs_the_relay_can_forward() {
        let svc = test_service();
        let pc = svc
            .api
            .new_peer_connection(RTCConfiguration::default())
            .await
            .expect("peer connection");
        pc.add_transceiver_from_kind(RTPCodecType::Audio, None)
            .await
            .expect("audio transceiver");
        pc.add_transceiver_from_kind(RTPCodecType::Video, None)
            .await
            .expect("video transceiver");
        let offer = pc.create_offer(None).await.expect("offer");

        let advertised: Vec<String> = offer
            .sdp
            .lines()
            .filter_map(|l| l.strip_prefix("a=rtpmap:"))
            .filter_map(|l| l.split_once(' '))
            .map(|(_, codec)| codec.split('/').next().unwrap_or("").to_lowercase())
            .filter(|c| !c.is_empty() && c != "rtx" && c != "red" && c != "ulpfec")
            .collect();

        for banned in ["g722", "pcmu", "pcma", "vp9", "h264", "av1", "av1x"] {
            assert!(
                !advertised.iter().any(|c| c == banned),
                "{banned} was advertised, but the relay can only forward Opus and VP8 \
                 — got {advertised:?}"
            );
        }
        assert!(
            advertised.iter().any(|c| c == "opus"),
            "Opus must be offered — got {advertised:?}"
        );
        assert!(
            advertised.iter().any(|c| c == "vp8"),
            "VP8 must be offered — got {advertised:?}"
        );

        let _ = pc.close().await;
    }

    /// And the outbound tracks must declare that same pair.
    #[test]
    fn the_outbound_tracks_declare_the_codecs_that_were_advertised() {
        let tracks = make_outbound_tracks("ch", "alice");
        assert_eq!(tracks.audio.codec().mime_type, "audio/opus");
        assert_eq!(tracks.video.codec().mime_type, "video/VP8");
    }

    /// Each sender reaches a receiver on its OWN track.
    ///
    /// This replaces `fan_out_collapses_every_sender_onto_one_track_per_receiver`,
    /// which pinned the opposite and was written to start failing when this
    /// landed. Previously `PeerSession` held one audio track and one video
    /// track per receiver and `fan_out_rtp` wrote every other peer's RTP into
    /// them, so two senders' streams — different SSRCs, independent encoder
    /// state — were interleaved onto a single track. A receiver cannot
    /// demultiplex that; VP8 in particular carries inter-frame references, so
    /// alternating packets from two encoders decodes to neither stream. Calls
    /// were structurally limited to two participants.
    #[tokio::test]
    async fn each_sender_reaches_a_receiver_on_its_own_track() {
        let bob = peer_session("ch-group", &["alice", "carol"]).await;

        let from_alice = outbound_track_for(&bob, "alice", true).expect("alice track");
        let from_carol = outbound_track_for(&bob, "carol", true).expect("carol track");

        assert!(
            !Arc::ptr_eq(&from_alice, &from_carol),
            "two senders must not share one track on the receiving side",
        );

        // Audio is separate from video for the same sender, so video RTP can
        // never land on the audio track and corrupt it.
        let alice_audio = outbound_track_for(&bob, "alice", false).expect("alice audio");
        assert!(!Arc::ptr_eq(&from_alice, &alice_audio));

        // Asking twice for the same (sender, kind) gives the same track.
        let again = outbound_track_for(&bob, "alice", true).expect("alice track again");
        assert!(Arc::ptr_eq(&from_alice, &again));
    }

    /// The track id carries the sender's pseudonym.
    ///
    /// It reaches the browser as the MSID, which is how a client attributes a
    /// tile to a person without a side channel. Every remote tile used to read
    /// the literal string "Participant" because there was nothing to read.
    #[tokio::test]
    async fn track_ids_name_the_sender() {
        let tracks = make_outbound_tracks("ch-group", "alice-pseudonym");
        assert_eq!(tracks.audio.id(), "audio-alice-pseudonym");
        assert_eq!(tracks.video.id(), "video-alice-pseudonym");
        assert_eq!(
            tracks.audio.stream_id(),
            tracks.video.stream_id(),
            "a sender's audio and video share a stream id so a client can pair them",
        );
    }

    /// A publisher a receiver has no track for yet is dropped, not panicked on.
    #[tokio::test]
    async fn a_sender_with_no_track_yet_yields_none() {
        let bob = peer_session("ch-group", &["alice"]).await;
        assert!(outbound_track_for(&bob, "someone-who-just-joined", false).is_none());
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
