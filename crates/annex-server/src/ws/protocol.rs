//! Wire protocol types for the `/ws` upgrade and the messages that flow
//! through it.
//!
//! Serde tags and casing are load-bearing: every `#[serde(rename = …)]` and
//! `rename_all = "camelCase"` is part of the public WebSocket protocol the
//! frontend is built against. They are not changed here.

use annex_channels::Message;
use serde::{Deserialize, Serialize};

/// Query parameters for the WebSocket connection.
///
/// Accepts either a signed `token` (preferred) or a raw `pseudonym`
/// (legacy/backwards-compatible). When both are present, `token` takes
/// precedence.
#[derive(Debug, Deserialize)]
pub struct WsConnectParams {
    pub pseudonym: Option<String>,
    pub token: Option<String>,
}

/// Incoming WebSocket message types.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    #[serde(rename = "subscribe")]
    Subscribe {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    #[serde(rename = "unsubscribe")]
    Unsubscribe {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    #[serde(rename = "message")]
    Message {
        #[serde(rename = "channelId")]
        channel_id: String,
        content: String,
        #[serde(rename = "replyTo")]
        reply_to: Option<String>,
        #[serde(rename = "clientRequestId")]
        client_request_id: Option<String>,
    },
    #[serde(rename = "edit_message")]
    EditMessage {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
        content: String,
        /// Correlates an error frame back to this operation.
        ///
        /// `Message` has carried one from the start; these two did not, so a
        /// rejected edit or delete came back as an error the client could not
        /// attribute to anything. It could only show a generic message and
        /// leave the optimistic change it had already painted on screen.
        /// Optional, and defaulted: a client that omits it still edits and
        /// deletes exactly as before.
        #[serde(rename = "clientRequestId", default)]
        client_request_id: Option<String>,
    },
    #[serde(rename = "delete_message")]
    DeleteMessage {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
        /// Correlates an error frame back to this operation.
        ///
        /// `Message` has carried one from the start; these two did not, so a
        /// rejected edit or delete came back as an error the client could not
        /// attribute to anything. It could only show a generic message and
        /// leave the optimistic change it had already painted on screen.
        /// Optional, and defaulted: a client that omits it still edits and
        /// deletes exactly as before.
        #[serde(rename = "clientRequestId", default)]
        client_request_id: Option<String>,
    },
    #[serde(rename = "voice_intent")]
    VoiceIntent {
        #[serde(rename = "channelId")]
        channel_id: String,
        text: String,
    },
    /// Typing indicator — broadcast to channel subscribers.
    #[serde(rename = "typing")]
    Typing {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    /// Resume protocol — replay missed messages since the given message ID.
    #[serde(rename = "resume")]
    Resume {
        #[serde(rename = "channelId")]
        channel_id: String,
        /// The last message ID the client successfully received.
        #[serde(rename = "lastMessageId")]
        last_message_id: String,
    },
    #[serde(rename = "webrtc_offer")]
    WebRtcOffer {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "sdp")]
        sdp: String,
    },
    /// The client's answer to an offer the SERVER initiated.
    ///
    /// Normal call setup is client-offers / server-answers. This is the other
    /// direction: when someone joins or leaves a call, every other peer's
    /// track set changes, and adding a track to an established connection
    /// requires a fresh offer/answer. Without it a call cannot grow past the
    /// participants it started with.
    #[serde(rename = "webrtc_answer")]
    WebRtcAnswer {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "sdp")]
        sdp: String,
    },
    #[serde(rename = "webrtc_ice_candidate")]
    WebRtcIceCandidate {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "candidate")]
        candidate: String,
        #[serde(rename = "sdpMid")]
        sdp_mid: Option<String>,
        #[serde(rename = "sdpMLineIndex")]
        sdp_m_line_index: Option<u16>,
        #[serde(rename = "usernameFragment")]
        username_fragment: Option<String>,
    },
}

/// Outgoing WebSocket message payload with camelCase field names.
///
/// The inner `Message` struct uses snake_case for HTTP API responses.
/// WebSocket messages use camelCase to match the frontend `WsReceiveFrame` type.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WsMessagePayload {
    pub channel_id: String,
    pub message_id: String,
    pub sender_pseudonym: String,
    pub content: String,
    pub reply_to_message_id: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
    /// Echoed client request ID for correlating the server response with the
    /// original send. Only present on the direct reply to the sender, not on
    /// broadcast copies to other subscribers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

impl From<Message> for WsMessagePayload {
    fn from(m: Message) -> Self {
        Self {
            channel_id: m.channel_id,
            message_id: m.message_id,
            sender_pseudonym: m.sender_pseudonym,
            content: m.content,
            reply_to_message_id: m.reply_to_message_id,
            created_at: m.created_at,
            edited_at: m.edited_at,
            deleted_at: m.deleted_at,
            client_request_id: None,
        }
    }
}

/// Outgoing WebSocket message wrapper (for broadcast).
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    #[serde(rename = "message")]
    Message(WsMessagePayload),
    #[serde(rename = "message_edited")]
    MessageEdited(WsMessagePayload),
    #[serde(rename = "message_deleted")]
    MessageDeleted(WsMessagePayload),
    #[serde(rename = "transcription")]
    Transcription {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "speakerPseudonym")]
        speaker_pseudonym: String,
        text: String,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
        #[serde(rename = "clientRequestId", skip_serializing_if = "Option::is_none")]
        client_request_id: Option<String>,
    },
    /// Typing indicator — broadcast to channel subscribers (except the typer).
    #[serde(rename = "typing")]
    Typing {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "pseudonymId")]
        pseudonym_id: String,
    },
    /// Channel lifecycle events — broadcast to all connected users.
    #[serde(rename = "channel_created")]
    ChannelCreated { channel: serde_json::Value },
    #[serde(rename = "channel_deleted")]
    ChannelDeleted {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    /// End-to-end encryption was toggled on a channel. Broadcast so clients that
    /// already have the channel open switch to (or from) the E2E send path
    /// immediately, instead of sending plaintext until they reload.
    #[serde(rename = "channel_e2e_changed")]
    ChannelE2eChanged {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "e2eEnabled")]
        e2e_enabled: bool,
    },
    /// Resume acknowledgement — tells the client how many messages were replayed.
    ///
    /// `cursor_lost` distinguishes "you missed nothing" from "I could not work
    /// out what you missed". They are not the same answer and used to be sent
    /// as the same frame. The client's `lastMessageId` stops resolving as soon
    /// as retention deletes the row it names — a routine event on any channel
    /// with `retention_days` — and a `missedCount: 0` in that case tells a
    /// client that has been offline across a purge that its timeline is
    /// complete, when in fact the whole backlog is missing and nothing will
    /// ever fetch it. When this is set the count is meaningless and the client
    /// must reload the channel rather than trust its cursor.
    #[serde(rename = "resumed")]
    Resumed {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "missedCount")]
        missed_count: usize,
        #[serde(rename = "cursorLost")]
        cursor_lost: bool,
    },
    #[serde(rename = "webrtc_answer")]
    WebRtcAnswer {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "sdp")]
        sdp: String,
    },
    /// An offer the SERVER initiated, because this peer's track set changed —
    /// somebody joined or left the call. The client answers with
    /// `IncomingMessage::WebRtcAnswer`.
    #[serde(rename = "webrtc_offer")]
    WebRtcOffer {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "sdp")]
        sdp: String,
    },
    #[serde(rename = "webrtc_ice_candidate")]
    WebRtcIceCandidate {
        #[serde(rename = "channelId")]
        channel_id: String,
        candidate: String,
        #[serde(rename = "sdpMid", skip_serializing_if = "Option::is_none")]
        sdp_mid: Option<String>,
        #[serde(rename = "sdpMLineIndex", skip_serializing_if = "Option::is_none")]
        sdp_m_line_index: Option<u16>,
        #[serde(rename = "usernameFragment", skip_serializing_if = "Option::is_none")]
        username_fragment: Option<String>,
    },
}
