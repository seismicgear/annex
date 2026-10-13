//! Orchestration for the channel + message HTTP and WebSocket surfaces.
//!
//! Each public method here is the orchestration the matching `api_channels`
//! handler (or the corresponding `IncomingMessage` arm in `api_ws`) used to
//! do inline:
//!
//!   * validate input (lengths, query length);
//!   * acquire a DB connection from the pool inside a blocking task;
//!   * run capability / alignment / membership / ZK-binding checks;
//!   * drive `annex_channels` (and, for joins, `annex_graph` and the voice
//!     service) under one logical operation;
//!   * return parsed DTOs ready for the handler to serialize.
//!
//! Handlers in `api_channels.rs` are reduced to: extract
//! `Extension<Arc<AppState>>` and `Extension<IdentityContext>`, deserialize
//! the request body / path / query, call into here, map
//! [`ChannelServiceError`] to the right `StatusCode` (see
//! [`ChannelServiceError::status_code`]), and serialize the response.
//!
//! The two `IncomingMessage::{Message, EditMessage, DeleteMessage}` arms of
//! `api_ws.rs` similarly delegate persistence (and the federation-flag
//! lookup, in the send-message case) to [`ChannelService::send_message`],
//! [`ChannelService::edit_message`], and [`ChannelService::delete_message`].
//! WebSocket-only concerns — `OutgoingMessage` framing, `connection_manager`
//! broadcasts, federated-message relay spawning — stay at the WS call site.
//!
//! The error type intentionally distinguishes [`ChannelServiceError::VoiceDisabled`]
//! and [`ChannelServiceError::VoiceNotConfigured`] from the generic
//! [`ChannelServiceError::Forbidden`] / [`ChannelServiceError::ServiceUnavailable`]
//! variants because `POST /api/channels/:id/voice/join` ships a structured
//! JSON error body (`{error, message, setup_hint}`) that the standard
//! `ApiError` body shape does not provide. Voice handlers match on those
//! two variants explicitly.

use std::sync::Arc;

use annex_channels::{
    add_member, create_channel, create_message, delete_channel, delete_message_in_transaction,
    edit_message_in_transaction, get_channel, get_edit_history, get_message, is_member,
    list_channels, list_messages, remove_member, Channel, ChannelError, CreateChannelParams,
    CreateMessageParams, Message, MessageEdit,
};

/// Outcome of `send_message`: tells the caller whether the persisted
/// message is brand-new or a return of a previously-accepted
/// `client_request_id`. Used by the WS arm to skip federated relay on
/// replay (the peer already received the envelope on the first send).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// A new row was inserted for this send.
    Inserted,
    /// The `client_request_id` matched a previous send; the returned
    /// `Message` is the original.
    Replayed,
}
use annex_graph::delete_edge;
use annex_identity::PlatformIdentity;
use annex_types::{AlignmentStatus, ChannelType, EdgeKind, FederationScope, RoleCode};
use axum::http::{HeaderMap, StatusCode};
use rusqlite::{params, OptionalExtension};
use thiserror::Error;

use crate::services::agent_policy::{self, ActionRefusal};

use crate::api_federation::find_commitment_for_pseudonym;
use crate::middleware::verify_zk_membership_header;
use crate::AppState;

/// Maximum length for a channel ID.
pub(crate) const MAX_CHANNEL_ID_LEN: usize = 128;
/// Maximum length for a channel name.
pub(crate) const MAX_CHANNEL_NAME_LEN: usize = 256;
/// Maximum length for a channel topic.
pub(crate) const MAX_TOPIC_LEN: usize = 1024;
/// Maximum length of the `q` query parameter for `/api/messages/search`.
pub(crate) const MAX_SEARCH_QUERY_LEN: usize = 200;
/// Cap applied to the per-channel page size in history / search responses.
pub(crate) const MAX_HISTORY_PAGE: u32 = 100;
/// Cap applied to a single search call (matches `annex_channels::search_messages`).
pub(crate) const MAX_SEARCH_PAGE: u32 = 50;
/// Per-channel scan window for encrypted-at-rest search. Bodies are stored
/// encrypted, so search decrypts a bounded recent window in memory and filters
/// there (a SQL `LIKE` cannot match ciphertext). Matches older than this window
/// are not returned — the trade-off for content-at-rest confidentiality without
/// a separate searchable index.
pub(crate) const SEARCH_SCAN_CAP: u32 = 1000;

/// Errors returned by [`ChannelService`].
///
/// Variants mirror the HTTP status families the previous inline handlers
/// produced — keeping the wire shape unchanged across the refactor.
#[derive(Debug, Error)]
pub enum ChannelServiceError {
    /// 400 — caller-induced format / value problem (empty / oversized
    /// fields, channel does not support voice, blank search query).
    #[error("{0}")]
    BadRequest(String),
    /// 403 — caller authenticated but policy or membership rejects the
    /// request (capability gate, alignment, non-member, ZK proof rejected).
    #[error("{0}")]
    Forbidden(String),
    /// The proof is valid but its published root has expired. No effect occurred.
    #[error("zk_root_retired")]
    ZkRootRetired,
    /// 404 — referenced channel or message does not exist.
    #[error("{0}")]
    NotFound(String),
    /// 409 — duplicate-state collision: channel UNIQUE constraint hit at
    /// insert time.
    #[error("{0}")]
    Conflict(String),
    /// 503 — the service is unavailable for reasons unrelated to the
    /// caller (e.g. WebRTC URL not configured for a voice request).
    #[error("{0}")]
    ServiceUnavailable(String),
    /// 500 — internal error. Always logged before being returned.
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Remote(annex_federation::transport::ApplicationError),
    /// 403 voice-specific — server policy `voice_enabled = false`.
    /// Carries no payload because the JSON body is fixed.
    #[error("voice disabled by server policy")]
    VoiceDisabled,
    /// 503 voice-specific — WebRTC URL / credentials are not configured.
    /// Carries no payload because the JSON body is fixed.
    #[error("voice not configured")]
    VoiceNotConfigured,
}

/// `agent_policy` answers to no protocol of its own, so each caller maps its
/// refusal. Two callers do it identically, which is what this `From` is for.
impl From<ActionRefusal> for ChannelServiceError {
    fn from(refusal: ActionRefusal) -> Self {
        match refusal {
            ActionRefusal::Forbidden(m) => ChannelServiceError::Forbidden(m),
            ActionRefusal::Internal(m) => ChannelServiceError::Internal(m),
        }
    }
}
impl From<super::channel_authority::AuthorityError> for ChannelServiceError {
    fn from(error: super::channel_authority::AuthorityError) -> Self {
        use super::channel_authority::AuthorityError as E;
        match error {
            E::BadRequest(s) => Self::BadRequest(s),
            E::Forbidden(s) => Self::Forbidden(s),
            E::NotFound(s) => Self::NotFound(s),
            E::Conflict(s) => Self::Conflict(s),
            E::Unavailable(s) => Self::ServiceUnavailable(s),
            E::Internal(s) => Self::Internal(s),
            E::Remote(error) => Self::Remote(error),
        }
    }
}

impl ChannelServiceError {
    /// HTTP status the handler should reply with for this variant. Voice
    /// handlers consult this only as a fallback — the structured-JSON
    /// variants are matched explicitly.
    pub fn status_code(&self) -> StatusCode {
        match self {
            ChannelServiceError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ChannelServiceError::Forbidden(_)
            | ChannelServiceError::VoiceDisabled
            | ChannelServiceError::ZkRootRetired => StatusCode::FORBIDDEN,
            ChannelServiceError::NotFound(_) => StatusCode::NOT_FOUND,
            ChannelServiceError::Conflict(_) => StatusCode::CONFLICT,
            ChannelServiceError::ServiceUnavailable(_)
            | ChannelServiceError::VoiceNotConfigured => StatusCode::SERVICE_UNAVAILABLE,
            ChannelServiceError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ChannelServiceError::Remote(error) => {
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY)
            }
        }
    }
}

fn storage_error(error: crate::at_rest::AtRestError, message_id: &str) -> ChannelServiceError {
    tracing::error!(message_id, %error, "message storage integrity failure");
    ChannelServiceError::Internal("stored message integrity check failed".into())
}

/// Called under the message writer transaction. Broadcast channels allow
/// members to read announcements, but only current moderators may publish or
/// revise them. A later capability grant/revocation takes effect immediately.
pub(crate) fn check_publish_permission(
    conn: &rusqlite::Connection,
    server_id: i64,
    pseudonym_id: &str,
    channel_id: &str,
    action: agent_policy::AgentAction,
) -> Result<(), ChannelServiceError> {
    let channel = get_channel(conn, channel_id).map_err(map_channel_err)?;
    if channel.server_id != server_id {
        return Err(ChannelServiceError::NotFound("channel not found".into()));
    }
    if !is_member(conn, server_id, channel_id, pseudonym_id).map_err(map_channel_err)? {
        return Err(ChannelServiceError::Forbidden(
            "not a channel member".into(),
        ));
    }
    let identity = annex_identity::get_platform_identity(conn, server_id, pseudonym_id)
        .map_err(|e| ChannelServiceError::Internal(format!("identity: {e}")))?;
    if !identity.active {
        return Err(ChannelServiceError::Forbidden(
            "identity is inactive".into(),
        ));
    }
    if channel.channel_type == ChannelType::Broadcast && !identity.can_moderate {
        return Err(ChannelServiceError::Forbidden(
            "only moderators can publish in a broadcast channel".into(),
        ));
    }
    agent_policy::check_agent_action(conn, server_id, pseudonym_id, action)
        .map_err(ChannelServiceError::from)
}

/// Final local membership writer boundary. The earlier routing/read checks
/// cannot authorize an unsigned mutation after reconciliation attaches a remote
/// authority, nor after the captured session or join policy changes.
fn write_local_membership(
    conn: &rusqlite::Connection,
    server_id: i64,
    captured: &PlatformIdentity,
    channel_id: &str,
    joining: bool,
) -> Result<(), ChannelServiceError> {
    if conn.is_autocommit() {
        return Err(ChannelServiceError::Internal(
            "membership authorization requires the writer transaction".into(),
        ));
    }
    let current = annex_identity::get_platform_identity(conn, server_id, &captured.pseudonym_id)
        .map_err(|_| ChannelServiceError::Forbidden("local identity is unavailable".into()))?;
    if !current.active || current.token_epoch != captured.token_epoch {
        return Err(ChannelServiceError::Forbidden(
            "local session was revoked".into(),
        ));
    }
    let remote:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_authorities WHERE server_id=?1 AND channel_id=?2 AND local_authority=0)",params![server_id,channel_id],|r|r.get(0)).map_err(|e|ChannelServiceError::Internal(e.to_string()))?;
    if remote {
        return Err(ChannelServiceError::Conflict(
            "channel authority changed; retry through the pinned authority".into(),
        ));
    }
    let channel = get_channel(conn, channel_id).map_err(map_channel_err)?;
    if channel.server_id != server_id {
        return Err(ChannelServiceError::NotFound("channel not found".into()));
    }
    if joining {
        super::channel_policy::check_join_policy(conn, server_id, &channel, &current).map_err(
            |error| match error {
                super::channel_policy::JoinRefusal::Forbidden(message) => {
                    ChannelServiceError::Forbidden(message)
                }
                super::channel_policy::JoinRefusal::Internal(message) => {
                    ChannelServiceError::Internal(message)
                }
            },
        )?;
    }
    if joining {
        add_member(conn, server_id, channel_id, &captured.pseudonym_id).map_err(map_channel_err)?;
    } else {
        remove_member(conn, server_id, channel_id, &captured.pseudonym_id)
            .map_err(map_channel_err)?;
    }
    Ok(())
}

/// Maps a [`ChannelError`] to a [`ChannelServiceError`], logging non-NotFound
/// errors so they don't disappear silently when the handler converts the
/// service error to an HTTP status.
fn map_channel_err(err: ChannelError) -> ChannelServiceError {
    match err {
        ChannelError::NotFound(msg) => ChannelServiceError::NotFound(msg),
        other => {
            tracing::error!(error = %other, "channel operation failed");
            ChannelServiceError::Internal(other.to_string())
        }
    }
}

/// Body of `POST /api/channels`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreateChannelRequest {
    pub channel_id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub topic: Option<String>,
    pub vrp_topic_binding: Option<String>,
    pub required_capabilities_json: Option<String>,
    pub agent_min_alignment: Option<AlignmentStatus>,
    pub retention_days: Option<u32>,
    pub federation_scope: FederationScope,
}

/// Channel + the requesting pseudonym's membership flag (response shape for
/// `GET /api/channels`).
#[derive(Debug, serde::Serialize)]
pub struct ChannelWithMembership {
    #[serde(flatten)]
    pub channel: Channel,
    pub is_member: bool,
}

/// One ICE server entry in [`JoinVoiceResponse`].
#[derive(Debug, serde::Serialize)]
pub struct IceServerResponse {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub credential: String,
}

/// Response body for `POST /api/channels/:id/voice/join`.
#[derive(Debug, serde::Serialize)]
pub struct JoinVoiceResponse {
    pub token: String,
    pub url: String,
    pub ice_servers: Vec<IceServerResponse>,
}

/// Response body for `GET /api/channels/:id/voice/status`.
#[derive(Debug, serde::Serialize)]
pub struct VoiceStatusResponse {
    pub participants: u32,
    /// Pseudonyms of everyone currently in the call, sorted.
    ///
    /// The SFU has always keyed peers by pseudonym, but only the count was
    /// exposed — so a client could say "3 people are here" and not who, and
    /// every remote tile rendered the literal string "Participant".
    pub participant_ids: Vec<String>,
    pub active: bool,
}

/// Response body for `GET /api/messages/search`.
///
/// This used to be a bare `Vec<Message>`, which could not express the one
/// thing a searcher most needs to know: whether the search actually looked
/// everywhere. Message bodies are encrypted at rest, so a SQL `LIKE` cannot
/// match and the server decrypts a bounded recent window per channel
/// ([`SEARCH_SCAN_CAP`]) and filters in memory. Anything older was never
/// examined — and the client, handed an empty array, said "No messages
/// found", which is a statement about the archive the server had not made.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SearchResponse {
    pub results: Vec<Message>,
    /// True when every channel in scope was searched to its end.
    ///
    /// False means at least one channel had more messages than the scan
    /// window, so a non-match is "not in the part we read", not "not here".
    pub complete: bool,
    /// How far back the scan reached in each channel, so the client can say
    /// so in the user's units without hardcoding a server constant.
    pub scanned_per_channel: u32,
}

/// Outcome of a successful `POST /api/channels` call.
///
/// The handler turns this into the usual `{"status": "created"}` response
/// after broadcasting the embedded `channel_payload` over the WebSocket.
pub struct CreateChannelOutcome {
    /// Fields used by the caller to build the `OutgoingMessage::ChannelCreated`
    /// broadcast. Pre-formatted as a `serde_json::Value` so the handler can
    /// `merge` into its `OutgoingMessage` without re-running `format!`.
    pub broadcast_payload: serde_json::Value,
    /// Set when the created channel needed a freshly-allocated voice room.
    /// `false` for text-only channels or when the voice service is disabled.
    pub voice_room_attempted: bool,
}

/// Channel + message orchestration, layered on top of `annex_channels`,
/// `annex_graph`, and the voice service. See the module docstring for the
/// handler-thinning contract.
pub struct ChannelService {
    state: Arc<AppState>,
}

impl ChannelService {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Channel CRUD
    // ─────────────────────────────────────────────────────────────────────

    /// `POST /api/channels` orchestration.
    ///
    /// Steps: capability gate (`can_moderate`), input validation, blocking
    /// insert (mapping UNIQUE-violation to 409), best-effort voice-room
    /// allocation when the channel type carries voice capacity, and a
    /// `serde_json::Value` payload that the handler then folds into a
    /// `ChannelCreated` broadcast. The voice-room create error is logged
    /// but not propagated, matching the previous handler behaviour: the DB
    /// row exists and the channel is usable as a text channel even if room
    /// allocation failed.
    pub async fn create_channel(
        &self,
        identity: &PlatformIdentity,
        req: CreateChannelRequest,
    ) -> Result<CreateChannelOutcome, ChannelServiceError> {
        // Creating a channel sets policy other participants live under, so it
        // is grouped with voice rather than with text for a partially-aligned
        // agent. See `AgentAction::allowed_when_partial`.
        {
            let pool = self.state.pool.clone();
            let server_id = self.state.server_id;
            let pid = identity.pseudonym_id.clone();
            tokio::task::spawn_blocking(move || -> Result<(), ChannelServiceError> {
                let conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                agent_policy::check_agent_action(
                    &conn,
                    server_id,
                    &pid,
                    agent_policy::AgentAction::CreateChannel,
                )
                .map_err(ChannelServiceError::from)
            })
            .await
            .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;
        }

        if !identity.can_moderate {
            return Err(ChannelServiceError::Forbidden(
                "insufficient capabilities".to_string(),
            ));
        }

        if req.channel_id.len() > MAX_CHANNEL_ID_LEN || req.channel_id.is_empty() {
            return Err(ChannelServiceError::BadRequest(
                "invalid channel_id".to_string(),
            ));
        }
        if req.name.len() > MAX_CHANNEL_NAME_LEN || req.name.trim().is_empty() {
            return Err(ChannelServiceError::BadRequest("invalid name".to_string()));
        }
        if let Some(ref t) = req.topic {
            if t.len() > MAX_TOPIC_LEN || t.trim().is_empty() {
                return Err(ChannelServiceError::BadRequest("invalid topic".to_string()));
            }
        }

        let broadcast_payload = serde_json::json!({
            "channel_id": req.channel_id,
            "name": req.name,
            "channel_type": format!("{:?}", req.channel_type),
            "topic": req.topic,
            "federation_scope": format!("{:?}", req.federation_scope),
        });

        let params = CreateChannelParams {
            server_id: self.state.server_id,
            channel_id: req.channel_id.clone(),
            name: req.name.clone(),
            channel_type: req.channel_type,
            topic: req.topic.clone(),
            vrp_topic_binding: req.vrp_topic_binding,
            required_capabilities_json: req.required_capabilities_json,
            agent_min_alignment: req.agent_min_alignment,
            retention_days: req.retention_days,
            federation_scope: req.federation_scope,
        };

        let pool = self.state.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
            create_channel(&conn, &params).map_err(|e| {
                if let ChannelError::Database(rusqlite::Error::SqliteFailure(error_code, _)) = &e {
                    if error_code.code == rusqlite::ffi::ErrorCode::ConstraintViolation {
                        return ChannelServiceError::Conflict("channel already exists".to_string());
                    }
                }
                map_channel_err(e)
            })
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;

        let voice_room_attempted = (req.channel_type == ChannelType::Voice
            || req.channel_type == ChannelType::Hybrid)
            && self.state.voice_service.is_enabled();
        if voice_room_attempted {
            if let Err(e) = self.state.voice_service.create_room(&req.channel_id).await {
                tracing::error!(
                    "failed to create WebRTC room for channel {}: {}",
                    req.channel_id,
                    e
                );
                // Non-fatal: the DB row exists. Match the prior handler
                // behaviour and continue.
            }
        }

        Ok(CreateChannelOutcome {
            broadcast_payload,
            voice_room_attempted,
        })
    }

    /// `GET /api/channels` orchestration: per-server channel list, annotated
    /// with the requesting pseudonym's membership flag for each row.
    pub async fn list_channels(
        &self,
        pseudonym_id: &str,
    ) -> Result<Vec<ChannelWithMembership>, ChannelServiceError> {
        let pool = self.state.pool.clone();
        let server_id = self.state.server_id;
        let pseudonym_id = pseudonym_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| {
                tracing::error!(error = %e, "failed to get db connection for list_channels");
                ChannelServiceError::Internal(format!("pool: {e}"))
            })?;
            let channels = list_channels(&conn, server_id).map_err(map_channel_err)?;

            let result: Vec<ChannelWithMembership> = channels
                .into_iter()
                .map(|ch| {
                    let member =
                        is_member(&conn, server_id, &ch.channel_id, &pseudonym_id).unwrap_or(false);
                    ChannelWithMembership {
                        channel: ch,
                        is_member: member,
                    }
                })
                .collect();
            Ok(result)
        })
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "list_channels task join error");
            ChannelServiceError::Internal(format!("join: {e}"))
        })?
    }

    /// `GET /api/channels/:id` orchestration. Moderators see any channel;
    /// regular users must be members, otherwise 403 (preventing metadata
    /// leakage on private channels).
    pub async fn get_channel(
        &self,
        identity: &PlatformIdentity,
        channel_id: &str,
    ) -> Result<Channel, ChannelServiceError> {
        super::channel_replication::refresh_for_request(
            self.state.clone(),
            identity,
            channel_id,
            None,
        )
        .await?;
        let pool = self.state.pool.clone();
        let server_id = self.state.server_id;
        let cid = channel_id.to_string();
        let pid = identity.pseudonym_id.clone();
        let can_moderate = identity.can_moderate;

        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| {
                tracing::error!(error = %e, "failed to get db connection for get_channel");
                ChannelServiceError::Internal(format!("pool: {e}"))
            })?;

            if !can_moderate {
                let member = is_member(&conn, server_id, &cid, &pid)
                    .map_err(|e| ChannelServiceError::Internal(format!("is_member: {e}")))?;
                if !member {
                    return Err(ChannelServiceError::Forbidden("not a member".to_string()));
                }
            }

            get_channel(&conn, &cid).map_err(map_channel_err)
        })
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "get_channel task join error");
            ChannelServiceError::Internal(format!("join: {e}"))
        })?
    }

    /// `DELETE /api/channels/:id` orchestration. Moderator-only. Returns
    /// after the row (and child messages / members) are deleted; the
    /// handler is responsible for unsubscribing live websocket subscribers
    /// and broadcasting the `ChannelDeleted` event.
    pub async fn delete_channel(
        &self,
        identity: &PlatformIdentity,
        channel_id: &str,
    ) -> Result<(), ChannelServiceError> {
        if !identity.can_moderate {
            return Err(ChannelServiceError::Forbidden(
                "insufficient capabilities".to_string(),
            ));
        }

        let pool = self.state.pool.clone();
        let cid = channel_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| {
                tracing::error!(error = %e, "failed to get db connection for delete_channel");
                ChannelServiceError::Internal(format!("pool: {e}"))
            })?;
            delete_channel(&conn, &cid).map_err(map_channel_err)
        })
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "delete_channel task join error");
            ChannelServiceError::Internal(format!("join: {e}"))
        })??;

        {
            let mut sessions = self
                .state
                .voice_sessions
                .write()
                .unwrap_or_else(|e| e.into_inner());
            sessions.retain(|(_, room), client| {
                if room == channel_id {
                    client.close();
                    false
                } else {
                    true
                }
            });
        }
        self.state
            .voice_service
            .delete_room(channel_id)
            .await
            .map_err(|e| ChannelServiceError::Internal(format!("delete voice room: {e}")))?;
        Ok(())
    }

    // ─────────────────────────────────────────────────────────────────────
    // Membership
    // ─────────────────────────────────────────────────────────────────────

    /// `POST /api/channels/:id/join` orchestration.
    ///
    /// Steps:
    ///   1. Bind the ZK membership proof header to the authenticated identity.
    ///   2. Load the channel.
    ///   3. Capability gate from `required_capabilities_json`.
    ///   4. Channel-type gate (Agent channels reject non-agents).
    ///   5. Agent alignment gate (Conflict / Partial / channel-min).
    ///   6. Insert into `channel_members` and, for agents, draft an
    ///      `AgentServing` graph edge.
    ///   7. For voice / hybrid channels and AI agents, connect (or reuse) a
    ///      voice-side `AgentVoiceClient` and subscribe its transcription
    ///      stream onto the requester's WebSocket.
    pub async fn join_channel(
        &self,
        identity: &PlatformIdentity,
        headers: &HeaderMap,
        channel_id: &str,
    ) -> Result<(), ChannelServiceError> {
        self.enforce_zk(identity, headers).await?;

        let channel = self.fetch_channel(channel_id.to_string()).await?;

        if super::channel_replication::maybe_forward(
            self.state.clone(),
            identity,
            channel_id,
            None,
            Some(&uuid::Uuid::new_v4().to_string()),
            serde_json::json!({"kind":"join"}),
        )
        .await?
        .is_some()
        {
            if identity.participant_type == RoleCode::AiAgent
                && matches!(
                    channel.channel_type,
                    ChannelType::Voice | ChannelType::Hybrid
                )
            {
                self.connect_agent_voice_client(identity, channel_id)
                    .await?;
            }
            return Ok(());
        }

        // The join gates, from the same function the federated path calls.
        //
        // These used to be written out here and nowhere else, which is how
        // `FederationService::join_federated_channel` came to insert members
        // with none of them applied — a channel's capability requirement, its
        // agent-only restriction and its minimum alignment governed local
        // members and no one else.
        tokio::task::spawn_blocking({
            let pool = self.state.pool.clone();
            let server_id = self.state.server_id;
            let channel = channel.clone();
            let identity = identity.clone();
            move || -> Result<(), ChannelServiceError> {
                let conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                crate::services::channel_policy::check_join_policy(
                    &conn, server_id, &channel, &identity,
                )
                .map_err(|e| match e {
                    crate::services::channel_policy::JoinRefusal::Forbidden(m) => {
                        ChannelServiceError::Forbidden(m)
                    }
                    crate::services::channel_policy::JoinRefusal::Internal(m) => {
                        ChannelServiceError::Internal(m)
                    }
                })
            }
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;

        if identity.participant_type == RoleCode::AiAgent
            && matches!(
                channel.channel_type,
                ChannelType::Voice | ChannelType::Hybrid
            )
        {
            self.ensure_voice_allowed(identity, channel_id).await?;
        }

        // Insert membership row and, for agents, an AgentServing edge.
        tokio::task::spawn_blocking({
            let pool = self.state.pool.clone();
            let server_id = self.state.server_id;
            let cid = channel_id.to_string();
            let pid = identity.pseudonym_id.clone();
            let is_agent = identity.participant_type == RoleCode::AiAgent;
            let admitted_identity=identity.clone();
            move || -> Result<(), ChannelServiceError> {
                let conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| ChannelServiceError::Internal(format!("join transaction: {e}")))?;
                write_local_membership(&tx,server_id,&admitted_identity,&cid,true)?;
                if is_agent {
                    tx.execute("INSERT INTO graph_edges (server_id,from_node,to_node,kind,weight) VALUES (?1,?2,?3,'AGENT_SERVING',1.0) ON CONFLICT(server_id,from_node,to_node,kind) DO NOTHING", params![server_id, pid, annex_graph::channel_vertex(server_id, &cid)])
                        .map_err(|e| ChannelServiceError::Internal(format!("serving edge: {e}")))?;
                }
                tx.commit().map_err(|e| ChannelServiceError::Internal(format!("join commit: {e}")))?;
                Ok(())
            }
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;

        if identity.participant_type == RoleCode::AiAgent
            && (channel.channel_type == ChannelType::Voice
                || channel.channel_type == ChannelType::Hybrid)
        {
            self.connect_agent_voice_client(identity, channel_id)
                .await?;
        }

        Ok(())
    }

    /// `POST /api/channels/:id/leave` orchestration.
    ///
    /// Returns the `Channel` so the caller can drive the post-leave
    /// side-effects (websocket unsubscribe, voice-room participant
    /// removal). The voice cleanup happens here too — moving it into the
    /// handler would split a single concern across two modules.
    pub async fn leave_channel(
        &self,
        identity: &PlatformIdentity,
        channel_id: &str,
    ) -> Result<Channel, ChannelServiceError> {
        let channel = self.fetch_channel(channel_id.to_string()).await?;

        let remote = {
            let conn = self
                .state
                .pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(e.to_string()))?;
            super::channel_replication::remote_binding(&conn, &self.state, channel_id)?.is_some()
        };
        if remote {
            // A preceding signed self-leave already removed ordinary membership
            // and saved its accepted receipt. The conventional HTTP leave stays
            // idempotent without issuing another authority mutation.
            let member = {
                let conn = self
                    .state
                    .pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(e.to_string()))?;
                is_member(
                    &conn,
                    self.state.server_id,
                    channel_id,
                    &identity.pseudonym_id,
                )
                .map_err(map_channel_err)?
            };
            if member {
                super::channel_replication::maybe_forward(
                    self.state.clone(),
                    identity,
                    channel_id,
                    None,
                    Some(&uuid::Uuid::new_v4().to_string()),
                    serde_json::json!({"kind":"leave"}),
                )
                .await?;
            }
            self.end_channel_participation(&identity.pseudonym_id, channel_id)
                .await;
            return Ok(channel);
        }

        tokio::task::spawn_blocking({
            let state = self.state.clone();
            let pool = self.state.pool.clone();
            let server_id = self.state.server_id;
            let cid = channel_id.to_string();
            let pid = identity.pseudonym_id.clone();
            let is_agent = identity.participant_type == RoleCode::AiAgent;
            let admitted_identity = identity.clone();
            move || -> Result<(), ChannelServiceError> {
                let conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                let tx = rusqlite::Transaction::new_unchecked(
                    &conn,
                    rusqlite::TransactionBehavior::Immediate,
                )
                .map_err(|e| ChannelServiceError::Internal(e.to_string()))?;
                super::channel_manifest::require_signed_departure(&tx, &state, &cid, &pid)?;
                write_local_membership(&tx, server_id, &admitted_identity, &cid, false)?;

                if is_agent {
                    delete_edge(
                        &tx,
                        server_id,
                        &pid,
                        &annex_graph::channel_vertex(server_id, &cid),
                        EdgeKind::AgentServing,
                    )
                    .map_err(|e| ChannelServiceError::Internal(format!("delete_edge: {e}")))?;
                }
                tx.commit()
                    .map_err(|e| ChannelServiceError::Internal(e.to_string()))?;
                Ok(())
            }
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;

        self.disconnect_agent_voice_client(&identity.pseudonym_id, channel_id);
        if channel.channel_type == ChannelType::Voice || channel.channel_type == ChannelType::Hybrid
        {
            if let Err(e) = self
                .state
                .voice_service
                .remove_participant(channel_id, &identity.pseudonym_id)
                .await
            {
                tracing::warn!(
                    "failed to remove participant {} from voice room {}: {}",
                    identity.pseudonym_id,
                    channel_id,
                    e
                );
            }
        }

        self.cleanup_room_transcription(channel_id).await;
        Ok(channel)
    }

    // ─────────────────────────────────────────────────────────────────────
    // Messages — REST surface
    // ─────────────────────────────────────────────────────────────────────

    /// `GET /api/channels/:id/messages` orchestration: ZK proof binding,
    /// membership check, paginated history fetch (capped at
    /// [`MAX_HISTORY_PAGE`]).
    pub async fn get_history(
        &self,
        identity: &PlatformIdentity,
        headers: &HeaderMap,
        channel_id: &str,
        before: Option<String>,
        limit: Option<u32>,
    ) -> Result<Vec<Message>, ChannelServiceError> {
        self.enforce_zk(identity, headers).await?;
        super::channel_replication::refresh_for_request(
            self.state.clone(),
            identity,
            channel_id,
            None,
        )
        .await?;
        self.require_membership(&identity.pseudonym_id, channel_id)
            .await?;

        let pool = self.state.pool.clone();
        let server_id = self.state.server_id;
        let cid = channel_id.to_string();
        let limit = limit.map(|l| l.min(MAX_HISTORY_PAGE));
        let cipher = self.state.message_cipher();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
            let mut messages =
                list_messages(&conn, server_id, &cid, before, limit).map_err(map_channel_err)?;
            // Decrypt at-rest content for display (no-op for legacy plaintext
            // or for E2E client ciphertext, which is not ours to open).
            for m in &mut messages {
                cipher
                    .decrypt_in_place(&mut m.content)
                    .map_err(|error| storage_error(error, &m.message_id))?;
            }
            Ok(messages)
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    /// `GET /api/messages/search` orchestration. Validates `q`, enforces
    /// per-channel membership when a `channel_id` is provided, otherwise
    /// scopes the substring sweep to the channels the requester is a
    /// member of (preventing cross-channel data leakage).
    pub async fn search_messages(
        &self,
        identity: &PlatformIdentity,
        headers: &HeaderMap,
        query: String,
        channel_id: Option<String>,
        limit: Option<u32>,
    ) -> Result<SearchResponse, ChannelServiceError> {
        // Search returns decrypted message content, so it needs the same gate
        // as reading that content directly.
        //
        // `get_history` has called `enforce_zk` since the gate was introduced;
        // this route never did. Under `enforce_zk_proofs = true` — the shipped
        // default, and the posture nothing in the suite exercised until now —
        // `GET /api/channels/{id}/messages` answered 403 without a membership
        // proof while `GET /api/messages/search?channel_id={id}` returned the
        // same messages from the same channel to the same caller with a
        // session token alone. A session token outlives its holder's access to
        // the ZK secret that minted it, so the gate was bypassable by asking
        // for the content through the other door.
        self.enforce_zk(identity, headers).await?;

        if query.trim().is_empty() {
            return Err(ChannelServiceError::BadRequest(
                "query must not be empty".to_string(),
            ));
        }
        if query.len() > MAX_SEARCH_QUERY_LEN {
            return Err(ChannelServiceError::BadRequest(
                "query too long".to_string(),
            ));
        }

        if let Some(cid) = channel_id.as_deref() {
            super::channel_replication::refresh_for_request(
                self.state.clone(),
                identity,
                cid,
                None,
            )
            .await?;
            self.require_membership(&identity.pseudonym_id, cid).await?;
        } else {
            let channels = {
                let conn = self
                    .state
                    .pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(e.to_string()))?;
                let mut stmt=conn.prepare("SELECT a.channel_id FROM channel_authorities a JOIN channel_members m ON m.server_id=a.server_id AND m.channel_id=a.channel_id WHERE a.server_id=?1 AND a.local_authority=0 AND m.pseudonym_id=?2 ORDER BY a.channel_id").map_err(|e|ChannelServiceError::Internal(e.to_string()))?;
                let rows = stmt
                    .query_map(params![self.state.server_id, identity.pseudonym_id], |r| {
                        r.get::<_, String>(0)
                    })
                    .map_err(|e| ChannelServiceError::Internal(e.to_string()))?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| ChannelServiceError::Internal(e.to_string()))?;
                rows
            };
            for cid in channels {
                super::channel_replication::refresh_for_request(
                    self.state.clone(),
                    identity,
                    &cid,
                    None,
                )
                .await?;
            }
        }

        let pool = self.state.pool.clone();
        let server_id = self.state.server_id;
        let pseudonym_id = identity.pseudonym_id.clone();
        let limit = limit.unwrap_or(20).min(MAX_SEARCH_PAGE);
        let cipher = self.state.message_cipher();

        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;

            // Content is stored encrypted at rest, so a SQL LIKE cannot match.
            // Instead scan a bounded recent window per channel, decrypt in
            // memory, and substring-filter here. `query` is matched
            // case-insensitively to mirror SQLite's default LIKE semantics.
            let needle = query.to_lowercase();
            // Returns the hits and whether this channel was read to its end.
            // A short candidate list means the channel ran out before the
            // window did; a full one means there is older history nobody
            // looked at, which is the difference between "no match" and "no
            // match in the part we read".
            let scan = |cid: &str| -> Result<(Vec<Message>, bool), ChannelServiceError> {
                let mut hits = Vec::new();
                let candidates = annex_channels::scan_messages(&conn, server_id, cid, SEARCH_SCAN_CAP)
                    .map_err(map_channel_err)?;
                let exhausted = (candidates.len() as u32) < SEARCH_SCAN_CAP;
                for mut m in candidates {
                    cipher.decrypt_in_place(&mut m.content).map_err(|error| storage_error(error, &m.message_id))?;
                    if m.content.to_lowercase().contains(&needle) {
                        hits.push(m);
                        if hits.len() >= limit as usize {
                            break;
                        }
                    }
                }
                Ok((hits, exhausted))
            };

            // Without a target channel, restrict the sweep to channels the
            // user is in. Searching every server channel would leak content
            // from private channels the user has not joined.
            if channel_id.is_none() {
                let mut stmt = conn
                    .prepare(
                        "SELECT channel_id FROM channel_members WHERE server_id = ?1 AND pseudonym_id = ?2",
                    )
                    .map_err(|e| ChannelServiceError::Internal(format!("prepare: {e}")))?;
                let member_channels: Vec<String> = stmt
                    .query_map(params![server_id, pseudonym_id], |row| row.get(0))
                    .map_err(|e| ChannelServiceError::Internal(format!("query: {e}")))?
                    .filter_map(|r| r.ok())
                    .collect();

                if member_channels.is_empty() {
                    return Ok(SearchResponse {
                        results: vec![],
                        // Nothing was skipped: there was nothing in scope.
                        complete: true,
                        scanned_per_channel: SEARCH_SCAN_CAP,
                    });
                }

                let mut all_results = Vec::new();
                let mut complete = true;
                for cid in &member_channels {
                    let (mut hits, exhausted) = scan(cid)?;
                    complete &= exhausted;
                    all_results.append(&mut hits);
                }
                // `id` breaks the tie because `created_at` is `datetime('now')`
                // at one-second resolution, so messages sent in the same second
                // compare equal and their order across channels would otherwise
                // depend on which channel happened to be walked first.
                all_results.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
                all_results.truncate(limit as usize);
                Ok(SearchResponse {
                    results: all_results,
                    complete,
                    scanned_per_channel: SEARCH_SCAN_CAP,
                })
            } else {
                // The `if` arm above covers the all-channels search, so this
                // branch only runs when a channel id was supplied — but the
                // compiler does not know that, and `.unwrap()` here would
                // panic inside a `spawn_blocking`, which reaches the client as
                // an unexplained 500. Name the condition instead.
                let Some(id) = channel_id.as_deref() else {
                    return Err(ChannelServiceError::Internal(
                        "search reached the single-channel branch with no channel id".to_string(),
                    ));
                };
                let (results, complete) = scan(id)?;
                Ok(SearchResponse {
                    results,
                    complete,
                    scanned_per_channel: SEARCH_SCAN_CAP,
                })
            }
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    /// `GET /api/channels/:id/messages/:mid/edits` orchestration: enforce
    /// channel membership before exposing the edit history to anyone.
    pub async fn get_message_edits(
        &self,
        identity: &PlatformIdentity,
        channel_id: &str,
        message_id: &str,
    ) -> Result<Vec<MessageEdit>, ChannelServiceError> {
        super::channel_replication::refresh_for_request(
            self.state.clone(),
            identity,
            channel_id,
            None,
        )
        .await?;
        self.require_membership(&identity.pseudonym_id, channel_id)
            .await?;

        let pool = self.state.pool.clone();
        let server_id = self.state.server_id;
        let cid = channel_id.to_string();
        let mid = message_id.to_string();
        let cipher = self.state.message_cipher();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
            // Both identifiers go into the query. `require_membership` above
            // can only vouch for the channel; the message has to be tied to
            // that same channel or the two path segments are independently
            // attacker-chosen.
            let mut edits =
                get_edit_history(&conn, server_id, &cid, &mid).map_err(map_channel_err)?;
            for e in &mut edits {
                cipher
                    .decrypt_in_place(&mut e.old_content)
                    .map_err(|error| storage_error(error, &mid))?;
            }
            Ok(edits)
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    // ─────────────────────────────────────────────────────────────────────
    // Messages — WebSocket surface (used by `api_ws.rs` arms)
    // ─────────────────────────────────────────────────────────────────────

    /// `IncomingMessage::Message` orchestration.
    ///
    /// Membership check + persistence + load of the federation flag for the
    /// channel. The caller (the `api_ws` arm) handles the websocket
    /// broadcast and the federated-relay spawn.
    ///
    /// The third tuple element (`bool`) is the federation flag. The fourth
    /// (`SendOutcome`) tells the caller whether the persisted message is a
    /// fresh insert or an idempotent replay of a previously-accepted
    /// `client_request_id`. The caller still broadcasts in both cases (so
    /// the original sender observes its own send even on retry) but skips
    /// the federation relay on `Replayed` — the peer already received the
    /// envelope the first time.
    pub async fn send_message(
        &self,
        sender_pseudonym: &str,
        channel_id: &str,
        content: String,
        reply_to: Option<String>,
        client_request_id: Option<String>,
    ) -> Result<(Message, bool, SendOutcome), ChannelServiceError> {
        self.send_message_with_attachments(
            sender_pseudonym,
            channel_id,
            content,
            reply_to,
            client_request_id,
            Vec::new(),
        )
        .await
    }

    pub async fn send_message_with_attachments(
        &self,
        sender_pseudonym: &str,
        channel_id: &str,
        content: String,
        reply_to: Option<String>,
        client_request_id: Option<String>,
        attachment_ids: Vec<String>,
    ) -> Result<(Message, bool, SendOutcome), ChannelServiceError> {
        self.require_membership_for(
            sender_pseudonym,
            channel_id,
            Some(agent_policy::AgentAction::SendText),
        )
        .await?;

        let server_id = self.state.server_id;
        let pool = self.state.pool.clone();
        // The whole state, because the federation enqueue below runs inside
        // this transaction and needs the signing key, the public URL, the
        // federation config and the at-rest cipher.
        let state = self.state.clone();
        let cid = channel_id.to_string();
        let sender = sender_pseudonym.to_string();
        let request_id = client_request_id;
        let cipher = self.state.message_cipher();
        tokio::task::spawn_blocking(
            move || -> Result<(Message, bool, SendOutcome), ChannelServiceError> {
                let mut conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;

                // ONE transaction, opened before the idempotency lookup.
                //
                // The lookup used to run on the bare pooled connection, 46
                // lines before the transaction began. Two concurrent sends
                // sharing (sender, client_request_id) both reached that SELECT
                // before either had written, both saw nothing, and both
                // committed a message — two rows against one idempotency
                // record, because the ledger INSERT is `OR IGNORE` and its
                // rowcount was discarded. Reproduced against SQLite; invisible
                // on the standard in-memory harness, which has a single
                // connection and therefore serialises the two closures.
                //
                // `IMMEDIATE` takes the RESERVED lock at BEGIN, so the second
                // racer now blocks here and, when it proceeds, reads a snapshot
                // that already contains the winner's ledger row. It takes the
                // replay branch, which is what the caller expects and what the
                // UNIQUE constraint in migration 035 was added to guarantee.
                //
                // Deliberately NOT a fast-path read outside the transaction
                // with a re-check inside it: the fast path is the bug, and a
                // version that keeps it "for performance" is the same defect
                // with a narrower window.
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| ChannelServiceError::Internal(format!("tx begin: {e}")))?;

                  // Publishing authority can change while this send waits for
                  // the writer lock. Check the current membership/capability
                  // in the transaction that will persist and enqueue it.
                  check_publish_permission(
                      &tx, server_id, &sender, &cid, agent_policy::AgentAction::SendText,
                  )?;

                // Idempotency lookup: if the same sender repeats the same
                // client_request_id, return the original message instead
                // of inserting a duplicate. Scope is
                // (server_id, sender_pseudonym, client_request_id) — see
                // migration 035 for the rationale.
                if let Some(ref rid) = request_id {
                    let existing_message_id: Option<String> = tx
                        .query_row(
                            "SELECT message_id FROM message_request_ids \
                             WHERE server_id = ?1 AND sender_pseudonym = ?2 \
                               AND client_request_id = ?3",
                            rusqlite::params![server_id, &sender, rid],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|e| ChannelServiceError::Internal(format!("idem lookup: {e}")))?;

                    if let Some(mid) = existing_message_id {
                        // Hydrate the original message and short-circuit.
                        let mut msg = get_message(&tx, &mid).map_err(map_channel_err)?;
                        if msg.channel_id != cid {
                            return Err(ChannelServiceError::Conflict("client request ID was already used in another channel".into()));
                        }
                        cipher.decrypt_in_place(&mut msg.content).map_err(|error| storage_error(error, &msg.message_id))?;
                        let channel = get_channel(&tx, &cid).map_err(map_channel_err)?;
                        let is_federated =
                            matches!(channel.federation_scope, FederationScope::Federated);
                        // Release the write lock immediately: a pure replay
                        // wrote nothing and must not hold RESERVED across the
                        // return.
                        tx.rollback()
                            .map_err(|e| ChannelServiceError::Internal(format!("tx rollback: {e}")))?;
                        return Ok((msg, is_federated, SendOutcome::Replayed));
                    }
                }

                // Fresh send. The message, its idempotency row and its
                // federation outbox rows are all written here and commit
                // together.
                // BEGIN IMMEDIATE, not DEFERRED.
                //
                // `create_message` reads before it writes — it resolves the
                // channel's retention days first. Under a DEFERRED
                // transaction that read takes a WAL snapshot, and the INSERT
                // that follows has to upgrade to a writer. If any other
                // connection has committed in between, SQLite returns
                // SQLITE_BUSY_SNAPSHOT *immediately*: the busy handler is
                // never invoked, because waiting cannot resolve a snapshot
                // conflict, so `busy_timeout` does nothing. The send failed
                // with "database is locked" and the user was told
                // "Failed to send message: internal error".
                //
                // `edit_message` and `delete_message` were fixed for exactly
                // this (see the [F31] regression test in annex-channels);
                // sending was missed. IMMEDIATE takes the RESERVED lock at
                // BEGIN, so contention becomes a wait bounded by
                // `busy_timeout` instead of an instant failure.

                let encrypted_epoch = super::e2e_service::validate_message_body(&tx,&state,&cid,&sender,&content)?;
                let message_id = uuid::Uuid::new_v4().to_string();
                // Encrypt the body at rest; keep the plaintext to return so the
                // broadcast + federation relay see cleartext.
                let plaintext = content;
                let params = CreateMessageParams {
                    channel_id: cid.clone(),
                    message_id: message_id.clone(),
                    sender_pseudonym: sender.clone(),
                    content: cipher.encrypt(&plaintext).map_err(|error| storage_error(error, &message_id))?,
                    reply_to_message_id: reply_to,
                };
                let mut msg = create_message(&tx, &params).map_err(map_channel_err)?;
                super::upload_service::associate_message_uploads(&tx,&state,&cid,&sender,&message_id,&attachment_ids,encrypted_epoch.as_ref().map(|(epoch,key)|(*epoch,key.as_str())))?;
                msg.content = plaintext;

                if let Some(ref rid) = request_id {
                    // The rowcount is checked, not discarded.
                    //
                    // `let _ =` was the second half of the duplicate-message
                    // defect: when the OR IGNORE hit the UNIQUE constraint it
                    // meant a racer had already claimed this request id, and
                    // the code committed its own message anyway. The old
                    // comment called that "both are valid messages, only the
                    // second mapping is dropped" — but the caller asked for
                    // one message and got two, which is exactly what an
                    // idempotency key exists to prevent.
                    //
                    // With the transaction opened above this should now be
                    // unreachable; it is kept because it is the only signal
                    // that it has become reachable again.
                    let inserted = tx
                        .execute(
                            "INSERT OR IGNORE INTO message_request_ids \
                             (server_id, channel_id, sender_pseudonym, client_request_id, message_id) \
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            rusqlite::params![server_id, &cid, &sender, rid, &message_id],
                        )
                        .map_err(|e| ChannelServiceError::Internal(format!("idem insert: {e}")))?;
                    if inserted == 0 {
                        tracing::warn!(
                            sender = %sender,
                            channel_id = %cid,
                            "a concurrent send claimed this client_request_id despite the \
                             IMMEDIATE transaction; returning the winner's message rather than \
                             committing a duplicate"
                        );
                        let winner: String = tx
                            .query_row(
                                "SELECT message_id FROM message_request_ids \
                                 WHERE server_id = ?1 AND sender_pseudonym = ?2 \
                                   AND client_request_id = ?3",
                                rusqlite::params![server_id, &sender, rid],
                                |row| row.get(0),
                            )
                            .map_err(|e| {
                                ChannelServiceError::Internal(format!("idem winner lookup: {e}"))
                            })?;
                        let mut won = get_message(&tx, &winner).map_err(map_channel_err)?;
                        cipher.decrypt_in_place(&mut won.content).map_err(|error| storage_error(error, &won.message_id))?;
                        let channel = get_channel(&tx, &cid).map_err(map_channel_err)?;
                        let is_federated =
                            matches!(channel.federation_scope, FederationScope::Federated);
                        tx.rollback().map_err(|e| {
                            ChannelServiceError::Internal(format!("tx rollback: {e}"))
                        })?;
                        return Ok((won, is_federated, SendOutcome::Replayed));
                    }
                }

                let channel = get_channel(&tx, &cid).map_err(map_channel_err)?;
                let is_federated = matches!(channel.federation_scope, FederationScope::Federated);

                // The federation obligation commits WITH the message.
                //
                // It used to be a detached `tokio::spawn` fired after this
                // commit, two task hops and a separate pooled connection later.
                // Everything in that gap — a crash, a pool failure, the storage
                // gate tripping — left a message that had been persisted and
                // broadcast locally, with no outbox row and nothing recording
                // that it owed one. The sender was told it had sent. A retry did
                // not heal it either: a retry returns `Replayed`, and the caller
                // skipped the relay on `Replayed` by design.
                //
                // Failing the send when the enqueue fails is deliberate. A
                // federated message that cannot be queued for delivery has not
                // been sent, and reporting success is how the old path lost
                // messages quietly.
                let ordered=super::channel_authority::record_local_message(&tx,&state,&cid,&msg,super::channel_authority::EventKind::MessageAccepted,Some(&attachment_ids))?;
                if is_federated && !ordered {
                    crate::services::federation_service::enqueue_message_envelope(
                        &tx, &state, &cid, &msg,
                    )
                    .map_err(|e| {
                        ChannelServiceError::Internal(format!(
                            "could not queue this message for federation: {e}"
                        ))
                    })?;
                }

                tx.commit()
                    .map_err(|e| ChannelServiceError::Internal(format!("tx commit: {e}")))?;

                Ok((msg, is_federated, SendOutcome::Inserted))
            },
        )
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    /// `IncomingMessage::EditMessage` orchestration. Membership check +
    /// `annex_channels::edit_message` (which enforces ownership and the
    /// edit window). Edit/history/federation outbox commit together; the caller
    /// owns only the post-commit broadcast.
    ///
    /// Returns the updated message plus whether the channel is
    /// `FEDERATED`-scoped. Federation is already durably queued when this
    /// method returns; callers must not enqueue the edit a second time.
    pub async fn edit_message(
        &self,
        sender_pseudonym: &str,
        channel_id: &str,
        message_id: &str,
        new_content: &str,
    ) -> Result<(Message, bool), ChannelServiceError> {
        self.edit_message_with_attachments(
            sender_pseudonym,
            channel_id,
            message_id,
            new_content,
            None,
        )
        .await
    }

    pub async fn edit_message_with_attachments(
        &self,
        sender_pseudonym: &str,
        channel_id: &str,
        message_id: &str,
        new_content: &str,
        attachment_ids: Option<Vec<String>>,
    ) -> Result<(Message, bool), ChannelServiceError> {
        self.require_membership_for(
            sender_pseudonym,
            channel_id,
            Some(agent_policy::AgentAction::EditText),
        )
        .await?;

        let pool = self.state.pool.clone();
        let mid = message_id.to_string();
        let cid = channel_id.to_string();
        let pseudo = sender_pseudonym.to_string();
        let content = new_content.to_string();
        let cipher = self.state.message_cipher();
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || -> Result<(Message, bool), ChannelServiceError> {
            let mut conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| ChannelServiceError::Internal(format!("edit tx begin: {e}")))?;
            check_publish_permission(
                &tx,
                state.server_id,
                &pseudo,
                &cid,
                agent_policy::AgentAction::EditText,
            )?;
            // Store the new body encrypted; the prior (encrypted) body is moved
            // into message_edits by edit_message as-is. Return plaintext for the
            // broadcast/relay.
            // The message must belong to the channel the caller named.
            //
            // `require_membership` above checks the CLIENT-SUPPLIED
            // channel_id; the mutation below is keyed on message_id alone.
            // Without tying them together, a member of channel B could edit
            // or delete their own messages in channel A — one they had left
            // or been removed from — by naming B in the frame. Ownership is
            // still enforced by `edit_message`/`delete_message`, so this is
            // narrower than reading another channel's content, but it
            // defeats the rule that you cannot change anything in a channel
            // you are not in.
            //
            // It also decides federation. `is_federated` is read from the
            // channel the caller NAMED, so an edit to a message in a
            // federated channel, submitted under a local channel id, is
            // never relayed — peers keep the old text and the servers
            // diverge with nothing logged. Same defect class as the edit
            // history keyed on message_id alone.
            let existing = get_message(&tx, &mid).map_err(map_channel_err)?;
            if existing.channel_id != cid {
                return Err(ChannelServiceError::NotFound(format!(
                    "message {mid} is not in channel {cid}"
                )));
            }
            // Validate the original before moving it into immutable edit history.
            // Corruption must not be hidden by replacing the current body.
            cipher
                .decrypt(&existing.content)
                .map_err(|error| storage_error(error, &mid))?;
            let encrypted_epoch =
                super::e2e_service::validate_message_body(&tx, &state, &cid, &pseudo, &content)?;
            let stored = cipher
                .encrypt(&content)
                .map_err(|error| storage_error(error, &mid))?;
            let mut msg = edit_message_in_transaction(&tx, &mid, &pseudo, &stored)
                .map_err(map_channel_err)?;
            msg.content = content;
            if let Some(ref ids) = attachment_ids {
                super::upload_service::associate_message_uploads(
                    &tx,
                    &state,
                    &cid,
                    &pseudo,
                    &mid,
                    ids,
                    encrypted_epoch
                        .as_ref()
                        .map(|(epoch, key)| (*epoch, key.as_str())),
                )?;
            }
            let channel = get_channel(&tx, &cid).map_err(map_channel_err)?;
            let is_federated = matches!(channel.federation_scope, FederationScope::Federated);
            let ordered = super::channel_authority::record_local_message(
                &tx,
                &state,
                &cid,
                &msg,
                super::channel_authority::EventKind::MessageEdited,
                attachment_ids.as_deref(),
            )?;
            if is_federated && !ordered {
                crate::services::federation_service::enqueue_edit_envelope(
                    &tx,
                    &state,
                    &cid,
                    &mid,
                    &pseudo,
                    &msg.content,
                )
                .map_err(|e| {
                    ChannelServiceError::Internal(format!(
                        "could not queue this edit for federation: {e}"
                    ))
                })?;
            }
            tx.commit()
                .map_err(|e| ChannelServiceError::Internal(format!("edit tx commit: {e}")))?;
            Ok((msg, is_federated))
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    /// Delete the message, its revision history, and enqueue every remote
    /// tombstone in one writer transaction. The caller broadcasts only after
    /// the complete deletion obligation is durable.
    pub async fn delete_message(
        &self,
        sender_pseudonym: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(Message, bool), ChannelServiceError> {
        let state = self.state.clone();
        let mid = message_id.to_string();
        let cid = channel_id.to_string();
        let pseudo = sender_pseudonym.to_string();
        tokio::task::spawn_blocking(move || -> Result<(Message, bool), ChannelServiceError> {
            let mut conn = state
                .pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| ChannelServiceError::Internal(format!("delete tx: {e}")))?;
            // Deleting one's prior announcement remains permitted after losing
            // moderator rights, but active membership and agent policy still apply.
            super::e2e_service::require_member(&tx, &state, &cid, &pseudo)?;
            agent_policy::check_agent_action(
                &tx,
                state.server_id,
                &pseudo,
                agent_policy::AgentAction::DeleteText,
            )?;
            let existing = get_message(&tx, &mid).map_err(map_channel_err)?;
            if existing.channel_id != cid {
                return Err(ChannelServiceError::NotFound(format!(
                    "message {mid} is not in channel {cid}"
                )));
            }
            let msg = delete_message_in_transaction(&tx, &mid, &pseudo).map_err(map_channel_err)?;
            let channel = get_channel(&tx, &cid).map_err(map_channel_err)?;
            let is_federated = matches!(channel.federation_scope, FederationScope::Federated);
            let ordered = super::channel_authority::record_local_message(
                &tx,
                &state,
                &cid,
                &msg,
                super::channel_authority::EventKind::MessageDeleted,
                None,
            )?;
            if is_federated && !ordered {
                super::federation_service::enqueue_redaction_envelope(
                    &tx, &state, &cid, &mid, &pseudo, "deleted",
                )
                .map_err(|e| {
                    ChannelServiceError::Internal(format!(
                        "could not queue this deletion for federation: {e}"
                    ))
                })?;
            }
            tx.commit()
                .map_err(|e| ChannelServiceError::Internal(format!("delete tx commit: {e}")))?;
            Ok((msg, is_federated))
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    // Voice

    /// Every condition that must hold before this identity may have a live
    /// peer connection in this channel.
    ///
    /// Shared by the two paths that can open one, because they had drifted and
    /// the WebSocket one had drifted to nothing: `ws/commands/webrtc.rs`
    /// checked channel membership and then called `handle_sdp_offer` directly.
    /// An admin who set `voice_enabled = false` got a 403 on
    /// `POST /voice/join` and no effect whatsoever on a `webrtc_offer` frame —
    /// the operator's kill switch was a client-side suggestion. A member of a
    /// Text channel could open an SFU peer connection in it the same way.
    ///
    /// `can_voice` is checked here for the first time anywhere at voice-join
    /// time. It was consulted only as a channel-level `required_capabilities`
    /// entry, so `PATCH /api/admin/members/{id}/capabilities` advertised a
    /// per-person voice revocation that did nothing at all: the flag defaults
    /// to 1 for every member (see `create_platform_identity`), and clearing it
    /// changed no behaviour on any path. A control that reports success and has
    /// no effect is worse than no control.
    ///
    /// Membership is deliberately NOT part of this: the two callers check it
    /// differently — the HTTP path through `require_membership`, the WebSocket
    /// path through `check_ws_membership` against its own connection — and
    /// folding it in here would mean one of them checking twice.
    pub(crate) async fn ensure_voice_allowed(
        &self,
        identity: &PlatformIdentity,
        channel_id: &str,
    ) -> Result<(), ChannelServiceError> {
        let policy_voice_enabled = self
            .state
            .policy
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .voice_enabled;
        if !policy_voice_enabled {
            return Err(ChannelServiceError::VoiceDisabled);
        }

        if !self.state.voice_service.is_enabled() {
            return Err(ChannelServiceError::VoiceNotConfigured);
        }

        // And, for an agent, whether its alignment still permits voice. This
        // function is the shared gate for BOTH voice paths — the HTTP join and
        // the WebSocket `webrtc_offer` frame — which is why the check goes here
        // rather than at either call site; that drift is what this function was
        // extracted to end.
        {
            let pool = self.state.pool.clone();
            let server_id = self.state.server_id;
            let pid = identity.pseudonym_id.clone();
            let admitted_epoch = identity.token_epoch;
            tokio::task::spawn_blocking(move || -> Result<(), ChannelServiceError> {
                let conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                let current = annex_identity::get_platform_identity(&conn, server_id, &pid)
                    .map_err(|e| ChannelServiceError::Internal(format!("identity: {e}")))?;
                if current.token_epoch != admitted_epoch {
                    return Err(ChannelServiceError::Forbidden(
                        "session was revoked before voice admission".to_string(),
                    ));
                }
                if !current.active || !current.can_voice {
                    return Err(ChannelServiceError::Forbidden(
                        "voice is not enabled for this identity".to_string(),
                    ));
                }
                agent_policy::check_agent_action(
                    &conn,
                    server_id,
                    &pid,
                    agent_policy::AgentAction::VoiceMedia,
                )
                .map_err(ChannelServiceError::from)
            })
            .await
            .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;
        }

        let channel = self.fetch_channel(channel_id.to_string()).await?;
        if channel.channel_type != ChannelType::Voice && channel.channel_type != ChannelType::Hybrid
        {
            return Err(ChannelServiceError::BadRequest(
                "channel does not support voice".to_string(),
            ));
        }

        Ok(())
    }

    /// `POST /api/channels/:id/voice/join` orchestration.
    ///
    /// `is_local_client` is supplied by the handler after inspecting
    /// `connect_info` and `X-Forwarded-For`; the service does not see the
    /// raw socket. Local clients receive `voice_service.get_url_for_local_client()`
    /// (which can be a loopback URL); remote clients receive the publicly
    /// reachable URL.
    pub async fn join_voice_channel(
        &self,
        identity: &PlatformIdentity,
        headers: &HeaderMap,
        channel_id: &str,
        is_local_client: bool,
    ) -> Result<JoinVoiceResponse, ChannelServiceError> {
        self.enforce_zk(identity, headers).await?;
        super::channel_replication::refresh_for_request(
            self.state.clone(),
            identity,
            channel_id,
            None,
        )
        .await?;

        // Share the revocation/registration lock through the final epoch check
        // and grant issuance. Middleware may have admitted this HTTP request
        // before a moderator revoked its session.
        let _lifecycle = self
            .state
            .connection_manager
            .lock_lifecycle(&identity.pseudonym_id)
            .await;

        let webrtc_url = if is_local_client {
            self.state
                .voice_service
                .get_url_for_local_client()
                .to_string()
        } else {
            self.state.voice_service.get_public_url().to_string()
        };

        if webrtc_url.is_empty() {
            return Err(ChannelServiceError::VoiceNotConfigured);
        }

        self.require_membership(&identity.pseudonym_id, channel_id)
            .await?;
        self.ensure_voice_allowed(identity, channel_id).await?;

        let token = self
            .state
            .voice_service
            .generate_join_token(
                channel_id,
                &identity.pseudonym_id,
                &identity.pseudonym_id,
                &self.state.voice_token_secret,
                annex_voice::VOICE_TOKEN_DEFAULT_TTL_SECS,
            )
            .map_err(|e| {
                tracing::error!("failed to generate voice join token: {}", e);
                ChannelServiceError::Internal(format!("token: {e}"))
            })?;

        let ice_servers: Vec<IceServerResponse> = self
            .state
            .voice_service
            .ice_servers()
            .iter()
            .map(|s| IceServerResponse {
                urls: s.urls.clone(),
                username: s.username.clone(),
                credential: s.credential.clone(),
            })
            .collect();

        Ok(JoinVoiceResponse {
            token,
            url: webrtc_url,
            ice_servers,
        })
    }

    /// `POST /api/channels/:id/voice/leave` orchestration. Membership +
    /// channel-type sanity, then a best-effort `remove_participant` on the
    /// voice service. `remove_participant` failures are logged but not
    /// surfaced — leaving a voice room is best-effort cleanup.
    pub async fn leave_voice_channel(
        &self,
        identity: &PlatformIdentity,
        channel_id: &str,
    ) -> Result<(), ChannelServiceError> {
        self.require_membership(&identity.pseudonym_id, channel_id)
            .await?;

        let channel = self.fetch_channel(channel_id.to_string()).await?;
        if channel.channel_type != ChannelType::Voice && channel.channel_type != ChannelType::Hybrid
        {
            return Err(ChannelServiceError::BadRequest(
                "channel does not support voice".to_string(),
            ));
        }

        self.disconnect_agent_voice_client(&identity.pseudonym_id, channel_id);
        {
            if let Err(e) = self
                .state
                .voice_service
                .remove_participant(channel_id, &identity.pseudonym_id)
                .await
            {
                tracing::warn!(
                    "failed to remove participant {} from voice room {}: {}",
                    identity.pseudonym_id,
                    channel_id,
                    e
                );
            }
        }

        self.cleanup_room_transcription(channel_id).await;
        Ok(())
    }

    /// `GET /api/channels/:id/voice/status` orchestration: membership check,
    /// then a participant count from the voice service (defaults to zero
    /// on error).
    pub async fn voice_status(
        &self,
        identity: &PlatformIdentity,
        channel_id: &str,
    ) -> Result<VoiceStatusResponse, ChannelServiceError> {
        self.require_membership(&identity.pseudonym_id, channel_id)
            .await?;

        let participant_ids = self.state.voice_service.participant_ids(channel_id).await;
        // Keep reading the count from its own accessor rather than deriving it
        // from the roster: the two are read at slightly different moments and
        // a caller comparing them can tell that the roster is a snapshot.
        let count = self
            .state
            .voice_service
            .participant_count(channel_id)
            .await
            .unwrap_or(0);

        Ok(VoiceStatusResponse {
            participants: count,
            participant_ids,
            active: count > 0,
        })
    }

    // ─────────────────────────────────────────────────────────────────────
    // Internals
    // ─────────────────────────────────────────────────────────────────────

    /// Bind the ZK membership proof header to the authenticated identity.
    /// Mirrors the `lookup_commitment` + `verify_zk_membership_header` pair
    /// from the previous inline handlers. A `None` commitment is forwarded
    /// to `verify_zk_membership_header`, which decides per the
    /// `enforce_zk_proofs` flag whether that's a hard fail.
    async fn enforce_zk(
        &self,
        identity: &PlatformIdentity,
        headers: &HeaderMap,
    ) -> Result<(), ChannelServiceError> {
        let commitment = self.lookup_commitment(&identity.pseudonym_id).await?;
        verify_zk_membership_header(&self.state, headers, commitment.as_deref()).map_err(|error| {
            match error {
                crate::middleware::MembershipProofError::RootRetired => {
                    ChannelServiceError::ZkRootRetired
                }
                crate::middleware::MembershipProofError::Rejected => {
                    ChannelServiceError::Forbidden("zk proof rejected".to_string())
                }
                crate::middleware::MembershipProofError::Internal => {
                    ChannelServiceError::Internal("zk verification storage failure".into())
                }
            }
        })
    }

    /// Find the registered identity commitment for a pseudonym so the ZK
    /// proof can be bound to the authenticated identity. Returns `None` if
    /// the pseudonym has no commitment (legacy / pre-ZK identity); the
    /// caller decides whether that's allowed.
    async fn lookup_commitment(
        &self,
        pseudonym_id: &str,
    ) -> Result<Option<String>, ChannelServiceError> {
        let pool = self.state.pool.clone();
        let pseudo = pseudonym_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<String>, ChannelServiceError> {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
            find_commitment_for_pseudonym(&conn, &pseudo)
                .map(|opt| opt.map(|(commitment, _topic)| commitment))
                .map_err(|e| ChannelServiceError::Internal(format!("commitment lookup: {e}")))
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    /// Read-only fetch of a channel row, mapping NotFound to the matching
    /// service error variant.
    async fn fetch_channel(&self, channel_id: String) -> Result<Channel, ChannelServiceError> {
        let pool = self.state.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
            get_channel(&conn, &channel_id).map_err(map_channel_err)
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    /// Fail with [`ChannelServiceError::Forbidden`] if `pseudonym_id` is
    /// not in `channel_members` for the configured server. Used as the
    /// gate on every endpoint that exposes channel-scoped data.
    async fn require_membership(
        &self,
        pseudonym_id: &str,
        channel_id: &str,
    ) -> Result<(), ChannelServiceError> {
        self.require_membership_for(pseudonym_id, channel_id, None)
            .await
    }

    /// Membership, and — for an action alignment governs — whether this agent
    /// may still take it.
    ///
    /// Alignment used to be checked once, at the join, and never again.
    /// `recalculate_agent_alignments` exists to cut off an agent whose
    /// principles no longer match the server's; all it did was set
    /// `agent_registrations.active = 0` and drop the socket. It did not touch
    /// `channel_members` and it did not touch `platform_identities.active`, so
    /// the agent's session token still verified, it reconnected, and it kept
    /// sending, editing and deleting in every channel it had already joined —
    /// while `AgentDisconnected` sat in the audit log saying otherwise.
    ///
    /// Both queries run in ONE `spawn_blocking` on ONE pooled connection.
    /// Message send is the busiest path in the app and already pays for a
    /// checkout; a second one per frame would double that.
    async fn require_membership_for(
        &self,
        pseudonym_id: &str,
        channel_id: &str,
        action: Option<agent_policy::AgentAction>,
    ) -> Result<(), ChannelServiceError> {
        let pool = self.state.pool.clone();
        let server_id = self.state.server_id;
        let cid = channel_id.to_string();
        let pid = pseudonym_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<(), ChannelServiceError> {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
            let member = is_member(&conn, server_id, &cid, &pid)
                .map_err(|e| ChannelServiceError::Internal(format!("is_member: {e}")))?;
            if !member {
                return Err(ChannelServiceError::Forbidden(
                    "not a channel member".to_string(),
                ));
            }
            if let Some(action) = action {
                agent_policy::check_agent_action(&conn, server_id, &pid, action)
                    .map_err(ChannelServiceError::from)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    /// AI-agent voice client lifecycle for both join and speech. Idempotent: if a
    /// session for this agent in this channel exists we keep it; otherwise we
    /// connect one, then atomically insert the handle (dropping our copy
    /// if a concurrent request beat us to it). The transcription loop is
    /// only spawned for the winning insert.
    pub(crate) async fn connect_agent_voice_client(
        &self,
        identity: &PlatformIdentity,
        channel_id: &str,
    ) -> Result<Arc<annex_voice::AgentVoiceClient>, ChannelServiceError> {
        super::channel_replication::refresh_for_request(
            self.state.clone(),
            identity,
            channel_id,
            None,
        )
        .await?;
        let _lifecycle = self
            .state
            .connection_manager
            .lock_lifecycle(&identity.pseudonym_id)
            .await;
        self.require_membership(&identity.pseudonym_id, channel_id)
            .await?;
        self.ensure_voice_allowed(identity, channel_id).await?;
        self.ensure_room_transcription(channel_id).await?;
        let client = self
            .connect_voice_client(&identity.pseudonym_id, channel_id, false, true)
            .await?;
        if let Err(error) = self.ensure_voice_allowed(identity, channel_id).await {
            self.disconnect_voice_participant(&identity.pseudonym_id)
                .await;
            return Err(error);
        }
        Ok(client)
    }

    /// One transcription process pipeline per room, shared by humans and agents.
    pub(crate) async fn ensure_room_transcription(
        &self,
        channel_id: &str,
    ) -> Result<(), ChannelServiceError> {
        self.connect_voice_client("", channel_id, true, false)
            .await?;
        Ok(())
    }

    pub(crate) async fn cleanup_room_transcription(&self, channel_id: &str) {
        if self
            .state
            .voice_service
            .participant_count(channel_id)
            .await
            .unwrap_or(0)
            == 0
        {
            self.disconnect_agent_voice_client("", channel_id);
        }
    }

    pub(crate) async fn end_channel_participation(&self, pseudonym: &str, channel: &str) {
        let _lifecycle = self
            .state
            .connection_manager
            .lock_lifecycle(pseudonym)
            .await;
        self.disconnect_agent_voice_client(pseudonym, channel);
        if let Err(error) = self
            .state
            .voice_service
            .remove_participant(channel, pseudonym)
            .await
        {
            tracing::debug!(%error,"departed member had no live media");
        }
        self.cleanup_room_transcription(channel).await;
    }

    /// A durable reconciliation cleanup may run after a crash or explicit
    /// rejoin. Recheck membership under the publisher lifecycle guard so an
    /// obsolete cleanup obligation cannot destroy a newly authorized call.
    pub(crate) async fn end_reconciled_departure(
        &self,
        pseudonym: &str,
        channel: &str,
    ) -> Result<bool, ChannelServiceError> {
        let _lifecycle = self
            .state
            .connection_manager
            .lock_lifecycle(pseudonym)
            .await;
        let member = {
            let conn = self
                .state
                .pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(e.to_string()))?;
            conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_members WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3)",rusqlite::params![self.state.server_id,channel,pseudonym],|r|r.get::<_,bool>(0)).map_err(|e|ChannelServiceError::Internal(e.to_string()))?
        };
        if member {
            return Ok(false);
        }
        self.disconnect_agent_voice_client(pseudonym, channel);
        if let Err(error) = self
            .state
            .voice_service
            .remove_participant(channel, pseudonym)
            .await
        {
            tracing::debug!(%error,"reconciled member had no live media");
        }
        self.cleanup_room_transcription(channel).await;
        Ok(true)
    }

    pub(crate) async fn cleanup_empty_room_transcriptions(&self) {
        let rooms: Vec<_> = self
            .state
            .voice_sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|(pseudonym, _)| pseudonym.is_empty())
            .map(|(_, room)| room.clone())
            .collect();
        for room in rooms {
            self.cleanup_room_transcription(&room).await;
        }
    }

    /// Revoke outstanding publisher handles before closing their SFU tracks.
    /// Used by socket teardown and by moderator actions, including agents that
    /// entered voice through HTTP and have no WebSocket to disconnect.
    pub(crate) async fn disconnect_voice_participant(&self, pseudonym: &str) {
        {
            let mut sessions = self
                .state
                .voice_sessions
                .write()
                .unwrap_or_else(|e| e.into_inner());
            sessions.retain(|(participant, _), client| {
                if participant == pseudonym {
                    client.close();
                    false
                } else {
                    true
                }
            });
        }
        self.state
            .voice_service
            .remove_participant_everywhere(pseudonym)
            .await;
        self.cleanup_empty_room_transcriptions().await;
    }

    pub(crate) async fn disconnect_all_voice(&self) -> Result<(), ChannelServiceError> {
        {
            let mut sessions = self
                .state
                .voice_sessions
                .write()
                .unwrap_or_else(|e| e.into_inner());
            for client in sessions.values() {
                client.close();
            }
            sessions.clear();
        }
        self.state
            .voice_service
            .close_all_rooms()
            .await
            .map_err(|e| ChannelServiceError::Internal(format!("close voice rooms: {e}")))
    }

    async fn connect_voice_client(
        &self,
        pseudonym_id: &str,
        channel_id: &str,
        transcribe: bool,
        publish: bool,
    ) -> Result<Arc<annex_voice::AgentVoiceClient>, ChannelServiceError> {
        let key = (pseudonym_id.to_string(), channel_id.to_string());
        let existing = {
            let sessions =
                self.state.voice_sessions.read().map_err(|e| {
                    ChannelServiceError::Internal(format!("voice_sessions read: {e}"))
                })?;
            sessions.get(&key).cloned()
        };
        if let Some(client) = existing {
            return Ok(client);
        }

        let subject = if pseudonym_id.is_empty() {
            "annex-room-captions"
        } else {
            pseudonym_id
        };
        let token = self
            .state
            .voice_service
            .generate_join_token(
                channel_id,
                subject,
                subject,
                &self.state.voice_token_secret,
                annex_voice::VOICE_TOKEN_DEFAULT_TTL_SECS,
            )
            .map_err(|e| ChannelServiceError::Internal(format!("token: {e}")))?;
        let url = self.state.voice_service.get_url();

        let client = annex_voice::AgentVoiceClient::connect_with_mode(
            url,
            &token,
            channel_id,
            &self.state.voice_token_secret,
            self.state.stt_service.clone(),
            self.state.voice_service.api_key(),
            self.state.voice_service.api_secret(),
            self.state.voice_service.clone(),
            transcribe,
            publish,
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to connect agent voice client: {}", e);
            ChannelServiceError::Internal(format!("connect: {e}"))
        })?;

        let client = Arc::new(client);

        // Double-check under write lock after the async connect gap. If a
        // concurrent request already inserted a session, drop our handle
        // and skip the transcription subscription so we don't ship two
        // copies of every transcript.
        let mut sessions = self
            .state
            .voice_sessions
            .write()
            .map_err(|e| ChannelServiceError::Internal(format!("voice_sessions write: {e}")))?;

        if let std::collections::hash_map::Entry::Vacant(entry) = sessions.entry(key.clone()) {
            if transcribe {
                let mut rx = client.subscribe_transcriptions();
                let state = self.state.clone();
                let live_client = Arc::downgrade(&client);
                tokio::spawn(async move {
                    loop {
                        let received = tokio::select! {
                            _ = state.shutdown.cancelled() => break,
                            event = rx.recv() => event,
                        };
                        let event = match received {
                            Ok(event) => event,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(skipped = n, "room transcription receiver lagged");
                                continue;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        };
                        if !live_client
                            .upgrade()
                            .is_some_and(|client| client.is_connected())
                        {
                            break;
                        }
                        if !state
                            .policy
                            .read()
                            .unwrap_or_else(|e| e.into_inner())
                            .voice_enabled
                        {
                            continue;
                        }
                        let participants =
                            state.voice_service.participant_ids(&event.channel_id).await;
                        let allowed = tokio::task::spawn_blocking({
                            let pool = state.pool.clone();
                            let server_id = state.server_id;
                            let channel_id = event.channel_id.clone();
                            move || -> Result<Vec<String>, String> {
                                let conn = pool.get().map_err(|e| e.to_string())?;
                                let mut allowed = Vec::new();
                                for participant in participants {
                                    let active: bool = conn.query_row(
                                        "SELECT EXISTS(SELECT 1 FROM platform_identities pi JOIN channel_members cm ON pi.server_id = cm.server_id AND pi.pseudonym_id = cm.pseudonym_id WHERE pi.server_id = ?1 AND pi.pseudonym_id = ?2 AND pi.active = 1 AND pi.can_voice = 1 AND cm.channel_id = ?3)",
                                        params![server_id, participant, channel_id], |r| r.get(0),
                                    ).map_err(|e| e.to_string())?;
                                    if active && agent_policy::check_agent_action(&conn, server_id, &participant, agent_policy::AgentAction::VoiceMedia).is_ok() {
                                        allowed.push(participant);
                                    }
                                }
                                Ok(allowed)
                            }
                        }).await;
                        let Ok(Ok(allowed)) = allowed else {
                            tracing::warn!("could not authorize room caption recipients");
                            continue;
                        };
                        let message = crate::api_ws::OutgoingMessage::Transcription {
                            channel_id: event.channel_id,
                            speaker_pseudonym: event.speaker_pseudonym,
                            text: event.text,
                        };
                        if let Ok(json) = serde_json::to_string(&message) {
                            for recipient in allowed {
                                state
                                    .connection_manager
                                    .send(&recipient, json.clone())
                                    .await;
                            }
                        }
                    }
                });
            }

            entry.insert(client);
        }

        Ok(sessions
            .get(&key)
            .expect("voice session inserted under write lock")
            .clone())
    }

    fn disconnect_agent_voice_client(&self, pseudonym_id: &str, channel_id: &str) {
        let key = (pseudonym_id.to_string(), channel_id.to_string());
        let mut sessions = self
            .state
            .voice_sessions
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(client) = sessions.remove(&key) {
            client.close();
        }
    }
}

#[cfg(test)]
mod local_membership_writer_tests {
    use super::*;
    fn fixture() -> (rusqlite::Connection, PlatformIdentity) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'local','Local','{}')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_voice) VALUES(1,'member-1','HUMAN',1)",[]).unwrap();
        conn.execute("INSERT INTO channels(server_id,channel_id,name,channel_type,federation_scope) VALUES(1,'shared','Shared',json_quote('Text'),json_quote('Federated'))",[]).unwrap();
        let identity = annex_identity::get_platform_identity(&conn, 1, "member-1").unwrap();
        (conn, identity)
    }
    #[test]
    fn captured_local_join_and_leave_refuse_authority_installed_before_writer_boundary() {
        let (mut conn, captured) = fixture();
        annex_channels::add_member(&conn, 1, "shared", &captured.pseudonym_id).unwrap();
        // Models a request that selected the legacy branch before the authority
        // commit. Its actual final production writer helper must recheck.
        conn.execute("INSERT INTO channel_authorities(server_id,channel_id,authority_identity,authority_key_hex,authority_url,local_authority) VALUES(1,'shared','authority','key','https://authority.test',0)",[]).unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        for joining in [true, false] {
            assert!(matches!(
                write_local_membership(&tx, 1, &captured, "shared", joining),
                Err(ChannelServiceError::Conflict(_))
            ));
        }
        assert!(annex_channels::is_member(&tx, 1, "shared", &captured.pseudonym_id).unwrap());
    }
    #[test]
    fn captured_local_membership_rechecks_session_activity_and_current_channel_policy() {
        let (mut conn, captured) = fixture();
        assert!(write_local_membership(&conn, 1, &captured, "shared", true).is_err());
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        write_local_membership(&tx, 1, &captured, "shared", true).unwrap();
        tx.execute("UPDATE platform_identities SET token_epoch=1", [])
            .unwrap();
        assert!(write_local_membership(&tx, 1, &captured, "shared", true).is_err());
        assert!(write_local_membership(&tx, 1, &captured, "shared", false).is_err());
        tx.execute("UPDATE platform_identities SET token_epoch=0,active=0", [])
            .unwrap();
        assert!(write_local_membership(&tx, 1, &captured, "shared", true).is_err());
        tx.execute("UPDATE platform_identities SET active=1,can_voice=0", [])
            .unwrap();
        tx.execute(
            "UPDATE channels SET required_capabilities_json=?1",
            [r#"["can_voice"]"#],
        )
        .unwrap();
        assert!(write_local_membership(&tx, 1, &captured, "shared", true).is_err());
        write_local_membership(&tx, 1, &captured, "shared", false).unwrap();
        tx.execute("UPDATE platform_identities SET can_voice=1", [])
            .unwrap();
        write_local_membership(&tx, 1, &captured, "shared", true).unwrap();
    }
}
