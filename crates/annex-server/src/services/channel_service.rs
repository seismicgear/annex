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
    add_member, create_channel, create_message, delete_channel, delete_message, edit_message,
    get_channel, get_edit_history, get_message, is_member, list_channels, list_messages,
    remove_member, Channel, ChannelError, CreateChannelParams, CreateMessageParams, Message,
    MessageEdit,
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
use annex_graph::{create_edge, delete_edge};
use annex_identity::PlatformIdentity;
use annex_types::{AlignmentStatus, ChannelType, EdgeKind, FederationScope, RoleCode};
use axum::http::{HeaderMap, StatusCode};
use rusqlite::{params, OptionalExtension};
use thiserror::Error;

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
    /// 403 voice-specific — server policy `voice_enabled = false`.
    /// Carries no payload because the JSON body is fixed.
    #[error("voice disabled by server policy")]
    VoiceDisabled,
    /// 503 voice-specific — WebRTC URL / credentials are not configured.
    /// Carries no payload because the JSON body is fixed.
    #[error("voice not configured")]
    VoiceNotConfigured,
}

impl ChannelServiceError {
    /// HTTP status the handler should reply with for this variant. Voice
    /// handlers consult this only as a fallback — the structured-JSON
    /// variants are matched explicitly.
    pub fn status_code(&self) -> StatusCode {
        match self {
            ChannelServiceError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ChannelServiceError::Forbidden(_) | ChannelServiceError::VoiceDisabled => {
                StatusCode::FORBIDDEN
            }
            ChannelServiceError::NotFound(_) => StatusCode::NOT_FOUND,
            ChannelServiceError::Conflict(_) => StatusCode::CONFLICT,
            ChannelServiceError::ServiceUnavailable(_)
            | ChannelServiceError::VoiceNotConfigured => StatusCode::SERVICE_UNAVAILABLE,
            ChannelServiceError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
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
        })?
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

        if let Some(caps_json) = &channel.required_capabilities_json {
            let required: Vec<String> = serde_json::from_str(caps_json).map_err(|e| {
                ChannelServiceError::Internal(format!("malformed required_capabilities_json: {e}"))
            })?;

            for req in required {
                let has_cap = match req.as_str() {
                    "can_voice" => identity.can_voice,
                    "can_moderate" => identity.can_moderate,
                    "can_invite" => identity.can_invite,
                    "can_federate" => identity.can_federate,
                    "can_bridge" => identity.can_bridge,
                    _ => false, // Unknown capability required -> deny
                };
                if !has_cap {
                    return Err(ChannelServiceError::Forbidden(
                        "missing required capability".to_string(),
                    ));
                }
            }
        }

        // Agent channels are restricted to AI agents only. Allowing humans
        // would let them bypass agent-specific policy controls (alignment,
        // VRP handshake, transfer scope).
        if channel.channel_type == ChannelType::Agent
            && identity.participant_type != RoleCode::AiAgent
        {
            return Err(ChannelServiceError::Forbidden(
                "agent channel requires AiAgent participant".to_string(),
            ));
        }

        if identity.participant_type == RoleCode::AiAgent {
            let alignment_status: Option<String> = tokio::task::spawn_blocking({
                let pool = self.state.pool.clone();
                let server_id = self.state.server_id;
                let pseudo = identity.pseudonym_id.clone();
                move || -> Result<Option<String>, ChannelServiceError> {
                    let conn = pool
                        .get()
                        .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                    conn.query_row(
                        "SELECT alignment_status FROM agent_registrations WHERE server_id = ?1 AND pseudonym_id = ?2",
                        params![server_id, pseudo],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| ChannelServiceError::Internal(format!("alignment query: {e}")))
                }
            })
            .await
            .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;

            let status_str = alignment_status.ok_or_else(|| {
                ChannelServiceError::Forbidden("agent not registered".to_string())
            })?;
            let status: AlignmentStatus = serde_json::from_str(&status_str)
                .or_else(|_| serde_json::from_str(&format!("\"{status_str}\"")))
                .map_err(|e| ChannelServiceError::Internal(format!("alignment parse: {e}")))?;

            if status == AlignmentStatus::Conflict {
                return Err(ChannelServiceError::Forbidden(
                    "conflict-aligned agents may not join channels".to_string(),
                ));
            }

            if status == AlignmentStatus::Partial && channel.channel_type != ChannelType::Text {
                return Err(ChannelServiceError::Forbidden(
                    "partial-aligned agents are restricted to text channels".to_string(),
                ));
            }

            if let Some(min_alignment) = channel.agent_min_alignment {
                let allowed = match min_alignment {
                    AlignmentStatus::Conflict => true,
                    AlignmentStatus::Partial => status != AlignmentStatus::Conflict,
                    AlignmentStatus::Aligned => status == AlignmentStatus::Aligned,
                };
                if !allowed {
                    return Err(ChannelServiceError::Forbidden(
                        "channel min alignment not met".to_string(),
                    ));
                }
            }
        }

        // Insert membership row and, for agents, an AgentServing edge.
        tokio::task::spawn_blocking({
            let pool = self.state.pool.clone();
            let server_id = self.state.server_id;
            let cid = channel_id.to_string();
            let pid = identity.pseudonym_id.clone();
            let is_agent = identity.participant_type == RoleCode::AiAgent;
            move || -> Result<(), ChannelServiceError> {
                let conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                add_member(&conn, server_id, &cid, &pid).map_err(map_channel_err)?;

                if is_agent {
                    create_edge(&conn, server_id, &pid, &cid, EdgeKind::AgentServing, 1.0)
                        .map_err(|e| ChannelServiceError::Internal(format!("create_edge: {e}")))?;
                }
                Ok(())
            }
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;

        if identity.participant_type == RoleCode::AiAgent
            && (channel.channel_type == ChannelType::Voice
                || channel.channel_type == ChannelType::Hybrid)
        {
            self.connect_agent_voice_client(&identity.pseudonym_id, channel_id)
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

        tokio::task::spawn_blocking({
            let pool = self.state.pool.clone();
            let server_id = self.state.server_id;
            let cid = channel_id.to_string();
            let pid = identity.pseudonym_id.clone();
            let is_agent = identity.participant_type == RoleCode::AiAgent;
            move || -> Result<(), ChannelServiceError> {
                let conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                remove_member(&conn, server_id, &cid, &pid).map_err(map_channel_err)?;

                if is_agent {
                    delete_edge(&conn, server_id, &pid, &cid, EdgeKind::AgentServing)
                        .map_err(|e| ChannelServiceError::Internal(format!("delete_edge: {e}")))?;
                }
                Ok(())
            }
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;

        if (channel.channel_type == ChannelType::Voice
            || channel.channel_type == ChannelType::Hybrid)
            && self.state.voice_service.is_enabled()
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
                cipher.decrypt_in_place(&mut m.content);
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
    ) -> Result<Vec<Message>, ChannelServiceError> {
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
            self.require_membership(&identity.pseudonym_id, cid).await?;
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
            let scan = |cid: &str| -> Result<Vec<Message>, ChannelServiceError> {
                let mut hits = Vec::new();
                let candidates = annex_channels::scan_messages(&conn, server_id, cid, SEARCH_SCAN_CAP)
                    .map_err(map_channel_err)?;
                for mut m in candidates {
                    cipher.decrypt_in_place(&mut m.content);
                    if m.content.to_lowercase().contains(&needle) {
                        hits.push(m);
                        if hits.len() >= limit as usize {
                            break;
                        }
                    }
                }
                Ok(hits)
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
                    return Ok(vec![]);
                }

                let mut all_results = Vec::new();
                for cid in &member_channels {
                    all_results.append(&mut scan(cid)?);
                }
                all_results.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                all_results.truncate(limit as usize);
                Ok(all_results)
            } else {
                scan(channel_id.as_deref().unwrap())
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
                cipher.decrypt_in_place(&mut e.old_content);
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
        self.require_membership(sender_pseudonym, channel_id)
            .await?;

        let server_id = self.state.server_id;
        let pool = self.state.pool.clone();
        let cid = channel_id.to_string();
        let sender = sender_pseudonym.to_string();
        let request_id = client_request_id;
        let cipher = self.state.message_cipher();
        tokio::task::spawn_blocking(
            move || -> Result<(Message, bool, SendOutcome), ChannelServiceError> {
                let mut conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;

                // Idempotency lookup: if the same sender repeats the same
                // client_request_id, return the original message instead
                // of inserting a duplicate. Scope is
                // (server_id, sender_pseudonym, client_request_id) — see
                // migration 035 for the rationale.
                if let Some(ref rid) = request_id {
                    let existing_message_id: Option<String> = conn
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
                        let mut msg = get_message(&conn, &mid).map_err(map_channel_err)?;
                        cipher.decrypt_in_place(&mut msg.content);
                        let channel = get_channel(&conn, &cid).map_err(map_channel_err)?;
                        let is_federated =
                            matches!(channel.federation_scope, FederationScope::Federated);
                        return Ok((msg, is_federated, SendOutcome::Replayed));
                    }
                }

                // Fresh send: open a transaction so the message and its
                // idempotency row are durable together. A concurrent racer
                // with the same (sender, request_id) will lose on the
                // UNIQUE constraint and fall back to the lookup branch on
                // its next attempt.
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
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(|e| ChannelServiceError::Internal(format!("tx begin: {e}")))?;

                let message_id = uuid::Uuid::new_v4().to_string();
                // Encrypt the body at rest; keep the plaintext to return so the
                // broadcast + federation relay see cleartext.
                let plaintext = content;
                let params = CreateMessageParams {
                    channel_id: cid.clone(),
                    message_id: message_id.clone(),
                    sender_pseudonym: sender.clone(),
                    content: cipher.encrypt(&plaintext),
                    reply_to_message_id: reply_to,
                };
                let mut msg = create_message(&tx, &params).map_err(map_channel_err)?;
                msg.content = plaintext;

                if let Some(ref rid) = request_id {
                    // Ignore UNIQUE conflicts: another in-flight request
                    // raced us in. We keep our own message (the racer's
                    // commit may have already inserted theirs as well —
                    // both are valid messages, only the second mapping is
                    // dropped). This degrades to two messages under a true
                    // race; clients send a single message under a stable
                    // request_id in practice.
                    let _ = tx.execute(
                        "INSERT OR IGNORE INTO message_request_ids \
                         (server_id, channel_id, sender_pseudonym, client_request_id, message_id) \
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![server_id, &cid, &sender, rid, &message_id],
                    );
                }

                let channel = get_channel(&tx, &cid).map_err(map_channel_err)?;
                let is_federated = matches!(channel.federation_scope, FederationScope::Federated);

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
    /// edit window). Caller owns the broadcast.
    ///
    /// Returns the updated message plus whether the channel is
    /// `FEDERATED`-scoped, so the caller can relay the edit to
    /// federation peers — same shape as [`Self::delete_message`].
    pub async fn edit_message(
        &self,
        sender_pseudonym: &str,
        channel_id: &str,
        message_id: &str,
        new_content: &str,
    ) -> Result<(Message, bool), ChannelServiceError> {
        self.require_membership(sender_pseudonym, channel_id)
            .await?;

        let pool = self.state.pool.clone();
        let mid = message_id.to_string();
        let cid = channel_id.to_string();
        let pseudo = sender_pseudonym.to_string();
        let content = new_content.to_string();
        let cipher = self.state.message_cipher();
        tokio::task::spawn_blocking(move || -> Result<(Message, bool), ChannelServiceError> {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
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
            let existing = get_message(&conn, &mid).map_err(map_channel_err)?;
            if existing.channel_id != cid {
                return Err(ChannelServiceError::NotFound(format!(
                    "message {mid} is not in channel {cid}"
                )));
            }
            let stored = cipher.encrypt(&content);
            let mut msg = edit_message(&conn, &mid, &pseudo, &stored).map_err(map_channel_err)?;
            msg.content = content;
            let channel = get_channel(&conn, &cid).map_err(map_channel_err)?;
            let is_federated = matches!(channel.federation_scope, FederationScope::Federated);
            Ok((msg, is_federated))
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    /// `IncomingMessage::DeleteMessage` orchestration. Membership check +
    /// `annex_channels::delete_message` (ownership + window enforced).
    /// Caller owns the broadcast.
    ///
    /// Returns the updated (blanked) message plus whether the channel is
    /// `FEDERATED`-scoped, so the caller can enqueue a redaction
    /// tombstone for federation peers (ADR-0011) — same shape as
    /// [`Self::send_message`]'s federation flag.
    pub async fn delete_message(
        &self,
        sender_pseudonym: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(Message, bool), ChannelServiceError> {
        self.require_membership(sender_pseudonym, channel_id)
            .await?;

        let pool = self.state.pool.clone();
        let mid = message_id.to_string();
        let cid = channel_id.to_string();
        let pseudo = sender_pseudonym.to_string();
        tokio::task::spawn_blocking(move || -> Result<(Message, bool), ChannelServiceError> {
            let conn = pool
                .get()
                .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
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
            let existing = get_message(&conn, &mid).map_err(map_channel_err)?;
            if existing.channel_id != cid {
                return Err(ChannelServiceError::NotFound(format!(
                    "message {mid} is not in channel {cid}"
                )));
            }
            let msg = delete_message(&conn, &mid, &pseudo).map_err(map_channel_err)?;
            let channel = get_channel(&conn, &cid).map_err(map_channel_err)?;
            let is_federated = matches!(channel.federation_scope, FederationScope::Federated);
            Ok((msg, is_federated))
        })
        .await
        .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))?
    }

    // ─────────────────────────────────────────────────────────────────────
    // Voice
    // ─────────────────────────────────────────────────────────────────────

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

        let webrtc_url = if is_local_client {
            self.state
                .voice_service
                .get_url_for_local_client()
                .to_string()
        } else {
            self.state.voice_service.get_public_url().to_string()
        };

        let policy_voice_enabled = self
            .state
            .policy
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .voice_enabled;
        if !policy_voice_enabled {
            return Err(ChannelServiceError::VoiceDisabled);
        }

        if !self.state.voice_service.is_enabled() || webrtc_url.is_empty() {
            return Err(ChannelServiceError::VoiceNotConfigured);
        }

        self.require_membership(&identity.pseudonym_id, channel_id)
            .await?;

        let channel = self.fetch_channel(channel_id.to_string()).await?;
        if channel.channel_type != ChannelType::Voice && channel.channel_type != ChannelType::Hybrid
        {
            return Err(ChannelServiceError::BadRequest(
                "channel does not support voice".to_string(),
            ));
        }

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

        if self.state.voice_service.is_enabled() {
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
        verify_zk_membership_header(&self.state, headers, commitment.as_deref()).map_err(|status| {
            match status {
                StatusCode::FORBIDDEN => {
                    ChannelServiceError::Forbidden("zk proof rejected".to_string())
                }
                _ => ChannelServiceError::Internal(format!("zk verify status {status}")),
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
        let pool = self.state.pool.clone();
        let server_id = self.state.server_id;
        let cid = channel_id.to_string();
        let pid = pseudonym_id.to_string();
        let member: bool =
            tokio::task::spawn_blocking(move || -> Result<bool, ChannelServiceError> {
                let conn = pool
                    .get()
                    .map_err(|e| ChannelServiceError::Internal(format!("pool: {e}")))?;
                is_member(&conn, server_id, &cid, &pid)
                    .map_err(|e| ChannelServiceError::Internal(format!("is_member: {e}")))
            })
            .await
            .map_err(|e| ChannelServiceError::Internal(format!("join: {e}")))??;

        if !member {
            return Err(ChannelServiceError::Forbidden(
                "not a channel member".to_string(),
            ));
        }
        Ok(())
    }

    /// AI-agent voice client lifecycle for the join path. Idempotent: if a
    /// session for the agent already exists we keep it; otherwise we
    /// connect one, then atomically insert the handle (dropping our copy
    /// if a concurrent request beat us to it). The transcription loop is
    /// only spawned for the winning insert.
    async fn connect_agent_voice_client(
        &self,
        pseudonym_id: &str,
        channel_id: &str,
    ) -> Result<(), ChannelServiceError> {
        let already_exists = {
            let sessions =
                self.state.voice_sessions.read().map_err(|e| {
                    ChannelServiceError::Internal(format!("voice_sessions read: {e}"))
                })?;
            sessions.contains_key(pseudonym_id)
        };
        if already_exists {
            return Ok(());
        }

        let token = self
            .state
            .voice_service
            .generate_join_token(
                channel_id,
                pseudonym_id,
                pseudonym_id,
                &self.state.voice_token_secret,
                annex_voice::VOICE_TOKEN_DEFAULT_TTL_SECS,
            )
            .map_err(|e| ChannelServiceError::Internal(format!("token: {e}")))?;
        let url = self.state.voice_service.get_url();

        let client = annex_voice::AgentVoiceClient::connect(
            url,
            &token,
            channel_id,
            &self.state.voice_token_secret,
            self.state.stt_service.clone(),
            self.state.voice_service.api_key(),
            self.state.voice_service.api_secret(),
            self.state.voice_service.clone(),
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

        if let std::collections::hash_map::Entry::Vacant(entry) =
            sessions.entry(pseudonym_id.to_string())
        {
            let mut rx = client.subscribe_transcriptions();
            let cm = self.state.connection_manager.clone();
            let p_clone = entry.key().clone();

            // Differentiate `Lagged` from `Closed` so a brief burst that
            // overflows the 256-deep broadcast window does NOT terminate
            // this forwarder permanently — see [F36].
            tokio::spawn(async move {
                loop {
                    let event = match rx.recv().await {
                        Ok(e) => e,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                pseudonym = %p_clone,
                                skipped = n,
                                "transcription broadcast lagged; some events skipped",
                            );
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    };
                    let msg = crate::api_ws::OutgoingMessage::Transcription {
                        channel_id: event.channel_id,
                        speaker_pseudonym: event.speaker_pseudonym,
                        text: event.text,
                    };

                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            cm.send(&p_clone, json).await;
                        }
                        Err(e) => {
                            tracing::error!("failed to serialize transcription message: {}", e);
                        }
                    }
                }
            });

            entry.insert(client);
        }

        Ok(())
    }
}
