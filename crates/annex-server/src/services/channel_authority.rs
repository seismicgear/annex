//! Ordered, signed channel authority operations and durable replica receipts.
use super::federated_upload_types::{
    AcceptedAttachment, AttachmentDescriptor, ReadIntent, SignedReadPermit,
};
use crate::AppState;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use ed25519_dalek::Signer;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const REQUEST_LIFETIME_SECONDS: i64 = 120;
const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEQUENCE: i64 = 9_007_199_254_740_991;

#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Remote(annex_federation::transport::ApplicationError),
    #[error("{0}")]
    Internal(String),
}
impl AuthorityError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Remote(error) => {
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY)
            }
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
    pub fn is_terminal_refusal(&self) -> bool {
        let status = self.status_code().as_u16();
        (400..500).contains(&status) && !matches!(status, 408 | 425 | 429)
    }
}
impl From<super::federation_delivery::DeliveryError> for AuthorityError {
    fn from(error: super::federation_delivery::DeliveryError) -> Self {
        use super::federation_delivery::DeliveryError;
        match error {
            DeliveryError::Unavailable(message) => Self::Unavailable(message),
            DeliveryError::Remote(error) => match error.status {
                400 => Self::BadRequest(error.message),
                403 => Self::Forbidden(error.message),
                404 => Self::NotFound(error.message),
                409 => Self::Conflict(error.message),
                _ => Self::Remote(error),
            },
        }
    }
}
impl From<rusqlite::Error> for AuthorityError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(format!("authority database: {e}"))
    }
}
impl From<crate::at_rest::AtRestError> for AuthorityError {
    fn from(e: crate::at_rest::AtRestError) -> Self {
        Self::Internal(format!("authority storage integrity: {e}"))
    }
}
impl From<super::e2e_service::E2eError> for AuthorityError {
    fn from(e: super::e2e_service::E2eError) -> Self {
        use super::e2e_service::E2eError as E;
        match e {
            E::BadRequest(e) => Self::BadRequest(e),
            E::Forbidden(e) => Self::Forbidden(e),
            E::NotFound(e) => Self::NotFound(e),
            E::Conflict { message, .. } => Self::Conflict(message),
            E::Internal(e) => Self::Internal(e),
            E::Remote(error) => Self::Remote(error),
        }
    }
}
impl IntoResponse for AuthorityError {
    fn into_response(self) -> Response {
        (self.status_code(), Json(json!({"error":self.to_string()}))).into_response()
    }
}

/// Key ordering is explicit and recursive; hashes never depend on serde_json's
/// optional preserve_order feature or the sender's object insertion order.
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort();
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|key| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("JSON string"),
                        canonical_json(&values[key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        _ => value.to_string(),
    }
}
pub fn digest(value: &Value) -> String {
    hex::encode(Sha256::digest(canonical_json(value).as_bytes()))
}
pub fn server_identity(public_key_hex: &str) -> Result<String, AuthorityError> {
    let bytes = decode32(public_key_hex)?;
    Ok(format!(
        "annex-server:{}",
        hex::encode(Sha256::digest(bytes))
    ))
}
fn decode32(value: &str) -> Result<[u8; 32], AuthorityError> {
    if !super::e2e_service::lower_hex(value) {
        return Err(AuthorityError::BadRequest(
            "expected canonical 32-byte lowercase hex".into(),
        ));
    }
    hex::decode(value)
        .map_err(|_| AuthorityError::BadRequest("invalid hex".into()))?
        .try_into()
        .map_err(|_| AuthorityError::BadRequest("invalid key length".into()))
}
pub(crate) fn verify(
    public_key: &str,
    signature: &str,
    value: &Value,
) -> Result<(), AuthorityError> {
    let key = ed25519_dalek::VerifyingKey::from_bytes(&decode32(public_key)?)
        .map_err(|_| AuthorityError::Forbidden("invalid authority key".into()))?;
    if signature.len() != 128
        || !signature
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(AuthorityError::Forbidden(
            "invalid signature encoding".into(),
        ));
    }
    let bytes = hex::decode(signature)
        .map_err(|_| AuthorityError::Forbidden("invalid signature".into()))?;
    let signature = ed25519_dalek::Signature::from_slice(&bytes)
        .map_err(|_| AuthorityError::Forbidden("invalid signature length".into()))?;
    key.verify_strict(canonical_json(value).as_bytes(), &signature)
        .map_err(|_| AuthorityError::Forbidden("authority signature does not verify".into()))
}
fn sign(state: &AppState, value: &Value) -> String {
    hex::encode(
        state
            .signing_key
            .sign(canonical_json(value).as_bytes())
            .to_bytes(),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Genesis,
    StateChanged,
    EpochCommitted,
    DevicesAdmitted,
    HistoryAdmitted,
    MessageAccepted,
    MessageEdited,
    MessageDeleted,
    ChannelDeleted,
    HistoryPruned,
    ManifestChanged,
    HistoryImported,
    HistoryEditImported,
    DeviceHistoryImported,
    KeyHistoryImported,
    ManifestHistoryImported,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventHeader {
    pub version: u8,
    pub authority_identity: String,
    pub channel_id: String,
    pub seq: i64,
    pub previous_hash: String,
    pub event_kind: EventKind,
    pub payload_hash: String,
    pub created_at: String,
}
impl EventHeader {
    fn signing_value(&self) -> Value {
        json!([
            "annex-channel-authority-event-v1",
            self.version,
            self.authority_identity,
            self.channel_id,
            self.seq,
            self.previous_hash,
            self.event_kind,
            self.payload_hash,
            self.created_at
        ])
    }
    pub fn event_hash(&self) -> String {
        digest(&self.signing_value())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityEvent {
    pub header: EventHeader,
    pub event_hash: String,
    pub signature: String,
    pub payload: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBinding {
    pub channel_id: String,
    pub authority_identity: String,
    pub authority_key_hex: String,
    pub authority_url: String,
    pub authority_peer_id: Option<i64>,
    pub local_authority: bool,
    pub head_seq: i64,
    pub head_hash: String,
    pub deleted_seq: Option<i64>,
}
pub fn binding(
    conn: &Connection,
    state: &AppState,
    channel: &str,
) -> Result<AuthorityBinding, AuthorityError> {
    conn.query_row("SELECT channel_id,authority_identity,authority_key_hex,authority_url,authority_peer_id,local_authority,head_seq,head_hash,deleted_seq FROM channel_authorities WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel],|r|Ok(AuthorityBinding{channel_id:r.get(0)?,authority_identity:r.get(1)?,authority_key_hex:r.get(2)?,authority_url:r.get(3)?,authority_peer_id:r.get(4)?,local_authority:r.get(5)?,head_seq:r.get(6)?,head_hash:r.get(7)?,deleted_seq:r.get(8)?})).optional()?.ok_or_else(||AuthorityError::NotFound("channel authority is not attached".into()))
}
/// Explicit local creation only. Never infer or replace a remote binding.
pub fn register_local(
    conn: &Transaction<'_>,
    state: &AppState,
    channel: &str,
) -> Result<AuthorityBinding, AuthorityError> {
    let scope: String = conn
        .query_row(
            "SELECT federation_scope FROM channels WHERE server_id=?1 AND channel_id=?2",
            params![state.server_id, channel],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| AuthorityError::NotFound("channel not found".into()))?;
    if serde_json::from_str::<annex_types::FederationScope>(&scope).ok()
        != Some(annex_types::FederationScope::Federated)
    {
        return Err(AuthorityError::Forbidden("channel is not federated".into()));
    }
    let key = hex::encode(state.signing_key.verifying_key().to_bytes());
    let identity = server_identity(&key)?;
    let existing:Option<String>=conn.query_row("SELECT authority_identity FROM channel_authorities WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel],|r|r.get(0)).optional()?;
    if existing.as_ref().is_some_and(|value| value != &identity) {
        return Err(AuthorityError::Conflict(
            "channel already belongs to a different authority".into(),
        ));
    }
    conn.execute("INSERT OR IGNORE INTO channel_authorities(server_id,channel_id,authority_identity,authority_key_hex,authority_url,local_authority) VALUES(?1,?2,?3,?4,?5,1)",params![state.server_id,channel,identity,key,state.get_public_url()])?;
    binding(conn, state, channel)
}

/// Append is part of the caller's writer transaction. No success can escape
/// without both materialized state and the exact signed delivery obligation.
pub fn append_event(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    kind: EventKind,
    payload: Value,
) -> Result<AuthorityEvent, AuthorityError> {
    let bound = binding(tx, state, channel)?;
    if !bound.local_authority {
        return Err(AuthorityError::Forbidden(
            "only the pinned authority may accept this operation".into(),
        ));
    }
    if bound.deleted_seq.is_some() {
        return Err(AuthorityError::NotFound("channel has been deleted".into()));
    }
    if bound.head_seq >= MAX_SEQUENCE {
        return Err(AuthorityError::Unavailable(
            "channel event sequence exhausted".into(),
        ));
    }
    let payload_json = canonical_json(&payload);
    if payload_json.len() > MAX_EVENT_BYTES {
        return Err(AuthorityError::BadRequest(
            "authority event exceeds the bounded payload limit".into(),
        ));
    }
    let header = EventHeader {
        version: 1,
        authority_identity: bound.authority_identity,
        channel_id: channel.into(),
        seq: bound.head_seq + 1,
        previous_hash: bound.head_hash,
        event_kind: kind,
        payload_hash: digest(&payload),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };
    let event = AuthorityEvent {
        event_hash: header.event_hash(),
        signature: sign(state, &header.signing_value()),
        header,
        payload,
    };
    let header_json = serde_json::to_string(&event.header)
        .map_err(|e| AuthorityError::Internal(e.to_string()))?;
    let stored = state.message_cipher().encrypt(&payload_json)?;
    tx.execute("INSERT INTO channel_authority_events(server_id,channel_id,event_seq,event_kind,event_hash,payload_hash,header_json,signature,payload_json,created_at,message_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![state.server_id,channel,event.header.seq,serde_json::to_string(&event.header.event_kind).map_err(|e|AuthorityError::Internal(e.to_string()))?,event.event_hash,event.header.payload_hash,header_json,event.signature,stored,event.header.created_at,event.payload["message"]["message_id"].as_str()])?;
    tx.execute("UPDATE channel_authorities SET head_seq=?3,head_hash=?4,deleted_seq=CASE WHEN ?5 THEN ?3 ELSE deleted_seq END WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel,event.header.seq,event.event_hash,event.header.event_kind==EventKind::ChannelDeleted])?;
    Ok(event)
}

pub fn verify_event(
    bound: &AuthorityBinding,
    event: &AuthorityEvent,
) -> Result<(), AuthorityError> {
    let h = &event.header;
    if h.version != 1
        || h.channel_id != bound.channel_id
        || h.authority_identity != bound.authority_identity
        || h.seq <= 0
        || h.seq > MAX_SEQUENCE
    {
        return Err(AuthorityError::Forbidden(
            "event does not name the pinned channel authority".into(),
        ));
    }
    if h.payload_hash != digest(&event.payload) || event.event_hash != h.event_hash() {
        return Err(AuthorityError::Forbidden(
            "authority event hash mismatch".into(),
        ));
    }
    if canonical_json(&event.payload).len() > MAX_EVENT_BYTES {
        return Err(AuthorityError::BadRequest("event is oversized".into()));
    }
    chrono::DateTime::parse_from_rfc3339(&h.created_at)
        .map_err(|_| AuthorityError::BadRequest("invalid event timestamp".into()))?;
    verify(
        &bound.authority_key_hex,
        &event.signature,
        &h.signing_value(),
    )?;
    validate_snapshot_identifiers(&event.payload)
}

/// A pinned authority signs content, not permission to collide with this
/// server's virtual graph vertices. Apply the same identifier grammar as HTTP
/// authentication to every typed participant field, including historical rows.
pub(crate) fn validate_snapshot_identifiers(value: &Value) -> Result<(), AuthorityError> {
    match value {
        Value::Array(values) => {
            for value in values {
                validate_snapshot_identifiers(value)?;
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(
                    key.as_str(),
                    "pseudonym_id"
                        | "sender_pseudonym"
                        | "recipient_pseudonym_id"
                        | "sender_pseudonym_id"
                        | "signer_pseudonym_id"
                        | "committed_by_pseudonym"
                        | "member_pseudonym"
                ) && !value.is_null()
                    && !value
                        .as_str()
                        .is_some_and(crate::middleware::is_valid_pseudonym_format)
                {
                    return Err(AuthorityError::Forbidden(format!(
                        "invalid snapshot participant identifier in {key}"
                    )));
                }
                validate_snapshot_identifiers(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The closure performs the complete operation-specific mutation. Its effects,
/// receipt and ordered cursor share the transaction; any failure rolls all back.
pub fn apply_event<F>(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
    materialize: F,
) -> Result<bool, AuthorityError>
where
    F: FnOnce(&Transaction<'_>, &AuthorityEvent) -> Result<(), AuthorityError>,
{
    let bound = binding(tx, state, &event.header.channel_id)?;
    verify_event(&bound, event)?;
    if bound.local_authority {
        return Err(AuthorityError::Forbidden(
            "local authority cannot import a replica event".into(),
        ));
    }
    if event.header.seq <= bound.head_seq {
        let hash:Option<String>=tx.query_row("SELECT event_hash FROM channel_authority_events WHERE server_id=?1 AND channel_id=?2 AND event_seq=?3",params![state.server_id,bound.channel_id,event.header.seq],|r|r.get(0)).optional()?;
        return if hash.as_deref() == Some(&event.event_hash) {
            Ok(false)
        } else {
            Err(AuthorityError::Conflict(
                "historical sequence has a different event hash".into(),
            ))
        };
    }
    if event.header.seq != bound.head_seq + 1 || event.header.previous_hash != bound.head_hash {
        return Err(AuthorityError::Conflict(
            "missing or reordered authority events".into(),
        ));
    }
    if bound.deleted_seq.is_some() {
        return Err(AuthorityError::Forbidden(
            "deleted channel cannot accept later content".into(),
        ));
    }
    materialize(tx, event)?;
    let stored = state
        .message_cipher()
        .encrypt(&canonical_json(&event.payload))?;
    tx.execute("INSERT INTO channel_authority_events(server_id,channel_id,event_seq,event_kind,event_hash,payload_hash,header_json,signature,payload_json,created_at,message_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![state.server_id,bound.channel_id,event.header.seq,serde_json::to_string(&event.header.event_kind).map_err(|e|AuthorityError::Internal(e.to_string()))?,event.event_hash,event.header.payload_hash,serde_json::to_string(&event.header).map_err(|e|AuthorityError::Internal(e.to_string()))?,event.signature,stored,event.header.created_at,event.payload["message"]["message_id"].as_str()])?;
    tx.execute("UPDATE channel_authorities SET head_seq=?3,head_hash=?4,deleted_seq=CASE WHEN ?5 THEN ?3 ELSE deleted_seq END WHERE server_id=?1 AND channel_id=?2",params![state.server_id,bound.channel_id,event.header.seq,event.event_hash,event.header.event_kind==EventKind::ChannelDeleted])?;
    Ok(true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedRequest {
    pub version: u8,
    pub requesting_identity: String,
    pub originating_server: String,
    pub authority_identity: String,
    pub channel_id: String,
    pub pseudonym_id: String,
    pub device_id: Option<String>,
    pub operation_id: String,
    pub operation: Value,
    pub nonce: String,
    pub created_at: String,
    pub signature: String,
}
impl SignedRequest {
    pub fn signing_value(&self) -> Value {
        json!([
            "annex-channel-authority-request-v1",
            self.version,
            self.requesting_identity,
            self.originating_server,
            self.authority_identity,
            self.channel_id,
            self.pseudonym_id,
            self.device_id,
            self.operation_id,
            self.operation,
            self.nonce,
            self.created_at
        ])
    }
    /// Retry identity excludes nonce/time/signature but includes every requested
    /// operation and principal field. Fresh transport authentication can retry an
    /// exact durable operation; it cannot change its semantics under the same ID.
    pub fn operation_hash(&self) -> String {
        digest(&json!([
            "annex-channel-authority-operation-v1",
            self.requesting_identity,
            self.authority_identity,
            self.channel_id,
            self.pseudonym_id,
            self.device_id,
            self.operation_id,
            self.operation
        ]))
    }
}
#[derive(Debug, Clone)]
pub struct RequestPrincipal {
    pub peer_id: i64,
    pub pseudonym_id: String,
    pub device_id: Option<String>,
}

/// Called inside the authority writer transaction on every request, including
/// requests over an already-established data channel. The nonce is committed
/// only with the authorized mutation/receipt; a failed transaction consumes none.
pub fn authenticate_request(
    tx: &Transaction<'_>,
    state: &AppState,
    request: &SignedRequest,
) -> Result<RequestPrincipal, AuthorityError> {
    authenticate_request_with_access(tx, state, request, RequestAccess::CurrentMember)
}
#[derive(Clone, Copy)]
enum RequestAccess {
    CurrentMember,
    PriorMember,
    ReplicaCleanup,
}
fn authenticate_request_with_access(
    tx: &Transaction<'_>,
    state: &AppState,
    request: &SignedRequest,
    access: RequestAccess,
) -> Result<RequestPrincipal, AuthorityError> {
    let require_current = matches!(access, RequestAccess::CurrentMember);
    require_federation_enabled(tx, state)?;
    if request.version != 1
        || request.operation_id.is_empty()
        || request.operation_id.len() > 128
        || request.nonce.len() != 64
        || !super::e2e_service::lower_hex(&request.nonce)
    {
        return Err(AuthorityError::BadRequest(
            "invalid authority request version, ID or nonce".into(),
        ));
    }
    let timestamp = chrono::DateTime::parse_from_rfc3339(&request.created_at)
        .map_err(|_| AuthorityError::BadRequest("invalid request timestamp".into()))?
        .timestamp();
    let now = chrono::Utc::now().timestamp();
    if timestamp > now + 30 || timestamp < now - REQUEST_LIFETIME_SECONDS {
        return Err(AuthorityError::Forbidden(
            "authority request is stale".into(),
        ));
    }
    let bound = binding(tx, state, &request.channel_id)?;
    if !bound.local_authority || bound.authority_identity != request.authority_identity {
        return Err(AuthorityError::Forbidden(
            "request targets a different authority".into(),
        ));
    }
    let peer:Option<(i64,String)>=tx.query_row("SELECT i.id,i.public_key FROM instances i JOIN federation_agreements a ON a.remote_instance_id=i.id WHERE i.base_url=?1 AND i.status='ACTIVE' AND a.local_server_id=?2 AND a.active=1 AND a.transfer_scope<>'NO_TRANSFER'",params![request.originating_server,state.server_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (peer_id, key) =
        peer.ok_or_else(|| AuthorityError::Forbidden("no current federation agreement".into()))?;
    if server_identity(&key)? != request.requesting_identity {
        return Err(AuthorityError::Forbidden(
            "requesting identity does not match pinned peer key".into(),
        ));
    }
    verify(&key, &request.signature, &request.signing_value())?;
    let attested:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM federated_identities f JOIN platform_identities p ON p.server_id=f.server_id AND p.pseudonym_id=f.pseudonym_id WHERE f.server_id=?1 AND f.remote_instance_id=?2 AND f.pseudonym_id=?3 AND (?4=0 OR p.active=1))",params![state.server_id,peer_id,request.pseudonym_id,require_current],|r|r.get(0))?;
    if !attested && !matches!(access, RequestAccess::ReplicaCleanup) {
        return Err(AuthorityError::Forbidden(
            "request member is not currently attested for this peer".into(),
        ));
    }
    if require_current
        && !super::human_federation::current_remote_membership(
            tx,
            state,
            peer_id,
            &request.channel_id,
            &request.pseudonym_id,
            request.device_id.as_deref(),
            now,
        )?
    {
        return Err(AuthorityError::Forbidden(
            "a fresh typed human membership attestation is required".into(),
        ));
    }
    tx.execute(
        "DELETE FROM channel_authority_nonces WHERE expires_at<?1",
        [now],
    )?;
    let seen:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_authority_nonces WHERE server_id=?1 AND requesting_identity=?2 AND nonce=?3)",params![state.server_id,request.requesting_identity,request.nonce],|r|r.get(0))?;
    if seen {
        return Err(AuthorityError::Forbidden(
            "authority request nonce was already used".into(),
        ));
    }
    tx.execute("INSERT INTO channel_authority_nonces(server_id,requesting_identity,nonce,request_hash,expires_at) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,request.requesting_identity,request.nonce,digest(&request.signing_value()),now+REQUEST_LIFETIME_SECONDS])?;
    Ok(RequestPrincipal {
        peer_id,
        pseudonym_id: request.pseudonym_id.clone(),
        device_id: request.device_id.clone(),
    })
}

pub fn prior_operation(
    tx: &Transaction<'_>,
    state: &AppState,
    request: &SignedRequest,
) -> Result<Option<i64>, AuthorityError> {
    let prior:Option<(String,i64)>=tx.query_row("SELECT operation_hash,accepted_seq FROM channel_authority_operations WHERE server_id=?1 AND channel_id=?2 AND requesting_identity=?3 AND pseudonym_id=?4 AND operation_id=?5",params![state.server_id,request.channel_id,request.requesting_identity,request.pseudonym_id,request.operation_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    match prior {
        None => Ok(None),
        Some((hash, seq)) if hash == request.operation_hash() => Ok(Some(seq)),
        Some(_) => Err(AuthorityError::Conflict(
            "operation ID already names a different accepted request".into(),
        )),
    }
}
pub fn remember_operation(
    tx: &Transaction<'_>,
    state: &AppState,
    request: &SignedRequest,
    seq: i64,
) -> Result<(), AuthorityError> {
    tx.execute("INSERT INTO channel_authority_operations(server_id,channel_id,requesting_identity,pseudonym_id,operation_id,operation_hash,accepted_seq) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![state.server_id,request.channel_id,request.requesting_identity,request.pseudonym_id,request.operation_id,request.operation_hash(),seq])?;
    Ok(())
}

pub fn event_by_sequence(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    seq: i64,
) -> Result<AuthorityEvent, AuthorityError> {
    let row:Option<(String,String,Option<String>,String)>=conn.query_row("SELECT header_json,signature,payload_json,event_hash FROM channel_authority_events WHERE server_id=?1 AND channel_id=?2 AND event_seq=?3",params![state.server_id,channel,seq],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (header, signature, payload, hash) =
        row.ok_or_else(|| AuthorityError::NotFound("event not found".into()))?;
    let payload = payload.ok_or_else(|| {
        AuthorityError::NotFound(
            "event payload was removed; request its signed removal certificate".into(),
        )
    })?;
    let event = AuthorityEvent {
        header: serde_json::from_str(&header)
            .map_err(|_| AuthorityError::Internal("stored event header is corrupt".into()))?,
        signature,
        event_hash: hash,
        payload: serde_json::from_str(&state.message_cipher().decrypt(&payload)?)
            .map_err(|_| AuthorityError::Internal("stored event payload is corrupt".into()))?,
    };
    verify_event(&binding(conn, state, channel)?, &event)?;
    Ok(event)
}

/// Preserve mutation-time state ordering before an accepted message/key event.
pub fn drain_pending_state(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
) -> Result<(), AuthorityError> {
    let pending = {
        let mut stmt=tx.prepare("SELECT pending_id,revision,state_json,members_json,devices_json FROM channel_authority_pending_state WHERE server_id=?1 AND channel_id=?2 ORDER BY pending_id")?;
        let rows = stmt
            .query_map(params![state.server_id, channel], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (id, revision, s, m, d) in pending {
        let parse = |text: &str| {
            serde_json::from_str::<Value>(text)
                .map_err(|_| AuthorityError::Internal("pending authority state is corrupt".into()))
        };
        append_event(
            tx,
            state,
            channel,
            EventKind::StateChanged,
            json!({"revision":revision,"state":parse(&s)?,"members":parse(&m)?,"devices":parse(&d)?}),
        )?;
        tx.execute(
            "DELETE FROM channel_authority_pending_state WHERE pending_id=?1",
            [id],
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthorityOperation {
    Join,
    Leave,
    PublishDevice {
        certificate: Box<crate::device_certificates::DeviceCertificate>,
    },
    Manifest {
        event: Box<super::channel_manifest::SignedManifestEvent>,
    },
    RevokeDevice {
        device_id: String,
    },
    SetEncryption {
        enabled: bool,
    },
    StoreWraps {
        request: Value,
    },
    Send {
        content: String,
        reply_to: Option<String>,
        #[serde(default)]
        attachments: Vec<AttachmentDescriptor>,
    },
    Edit {
        message_id: String,
        content: String,
        attachments: Option<Vec<AttachmentDescriptor>>,
    },
    Delete {
        message_id: String,
    },
}
pub fn is_own_departure(operation: &Value, pid: &str) -> bool {
    match serde_json::from_value::<AuthorityOperation>(operation.clone()) {
        Ok(AuthorityOperation::Leave) => true,
        Ok(AuthorityOperation::Manifest { event }) => {
            super::channel_manifest::removed_member(&event) == Some(pid)
        }
        _ => false,
    }
}
fn map_channel(error: annex_channels::ChannelError) -> AuthorityError {
    match error {
        annex_channels::ChannelError::NotFound(e) => AuthorityError::NotFound(e),
        e => AuthorityError::Internal(e.to_string()),
    }
}
fn map_publication(error: super::channel_service::ChannelServiceError) -> AuthorityError {
    use super::channel_service::ChannelServiceError as E;
    match error {
        E::BadRequest(e) => AuthorityError::BadRequest(e),
        E::Forbidden(e) => AuthorityError::Forbidden(e),
        E::NotFound(e) => AuthorityError::NotFound(e),
        E::Conflict(e) => AuthorityError::Conflict(e),
        e => AuthorityError::Internal(e.to_string()),
    }
}
fn require_moderator(
    tx: &Transaction<'_>,
    state: &AppState,
    pid: &str,
) -> Result<(), AuthorityError> {
    let permitted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2 AND active=1 AND can_moderate=1)",params![state.server_id,pid],|r|r.get(0))?;
    if !permitted {
        return Err(AuthorityError::Forbidden(
            "current moderator permission is required".into(),
        ));
    }
    Ok(())
}
fn message_payload(
    state: &AppState,
    mut message: annex_channels::Message,
) -> Result<Value, AuthorityError> {
    if message.deleted_at.is_none() {
        state
            .message_cipher()
            .decrypt_in_place(&mut message.content)?;
    }
    let expires = message.expires_at.clone();
    let mut value =
        serde_json::to_value(message).map_err(|e| AuthorityError::Internal(e.to_string()))?;
    value["expires_at"] = json!(expires);
    Ok(value)
}
/// Authority-side admission for a single authenticated remote intent. All policy
/// reads and accepted-message writes serialize against the same state transaction.
/// Attachment descriptors are added by the phase055 typed operation extension;
/// unknown fields already fail closed instead of being silently discarded.
pub fn accept_operation(
    tx: &Transaction<'_>,
    state: &AppState,
    request: &SignedRequest,
) -> Result<AuthorityEvent, AuthorityError> {
    let principal = authenticate_request(tx, state, request)?;
    let operation: AuthorityOperation = serde_json::from_value(request.operation.clone())
        .map_err(|e| AuthorityError::BadRequest(format!("invalid authority operation: {e}")))?;
    let channel = &request.channel_id;
    let pid = &principal.pseudonym_id;
    if !matches!(operation, AuthorityOperation::Join) {
        super::e2e_service::require_member(tx, state, channel, pid)?;
    }
    if let Some(seq) = prior_operation(tx, state, request)? {
        return event_by_sequence(tx, state, channel, seq);
    }
    drain_pending_state(tx, state, channel)?;
    let (kind, payload) = match operation {
        AuthorityOperation::Join => {
            let channel_row = annex_channels::get_channel(tx, channel).map_err(map_channel)?;
            let identity = annex_identity::get_platform_identity(tx, state.server_id, pid)
                .map_err(|e| AuthorityError::Forbidden(e.to_string()))?;
            super::channel_policy::check_join_policy(tx, state.server_id, &channel_row, &identity)
                .map_err(|e| match e {
                    super::channel_policy::JoinRefusal::Forbidden(e) => {
                        AuthorityError::Forbidden(e)
                    }
                    super::channel_policy::JoinRefusal::Internal(e) => AuthorityError::Internal(e),
                })?;
            annex_channels::add_member(tx, state.server_id, channel, pid).map_err(map_channel)?;
            tx.execute("INSERT INTO channel_authority_replicas(server_id,channel_id,peer_instance_id) VALUES(?1,?2,?3) ON CONFLICT(server_id,channel_id,peer_instance_id) DO UPDATE SET revoked_at=NULL",params![state.server_id,channel,principal.peer_id])?;
            drain_pending_state(tx, state, channel)?;
            (
                EventKind::StateChanged,
                json!({"action":"member_joined","pseudonym_id":pid,"requesting_identity":request.requesting_identity,"state":super::e2e_service::read_state(tx,state,channel)?}),
            )
        }
        AuthorityOperation::Leave => {
            super::channel_manifest::require_signed_departure(tx, state, channel, pid)?;
            annex_channels::remove_member(tx, state.server_id, channel, pid)
                .map_err(map_channel)?;
            drain_pending_state(tx, state, channel)?;
            (
                EventKind::StateChanged,
                json!({"action":"member_left","pseudonym_id":pid,"requesting_identity":request.requesting_identity,"state":super::e2e_service::read_state(tx,state,channel)?}),
            )
        }
        AuthorityOperation::PublishDevice { certificate } => {
            if certificate.server_identity != request.requesting_identity
                || certificate.pseudonym_id != *pid
            {
                return Err(AuthorityError::Forbidden(
                    "device certificate is not owned by the signed originating identity".into(),
                ));
            }
            import_verified_certificate(tx, state, &certificate, None)?;
            drain_pending_state(tx, state, channel)?;
            (
                EventKind::DevicesAdmitted,
                json!({"devices":[{"pseudonym_id":pid,"device_id":certificate.device_id,"x25519_pub_hex":certificate.x25519_pub_hex,"device_certificate":certificate,"revoked_at":null}],"state":super::e2e_service::read_state(tx,state,channel)?}),
            )
        }
        AuthorityOperation::Manifest { event } => {
            if principal.device_id.as_deref() != Some(event.signer_device_id.as_str()) {
                return Err(AuthorityError::Forbidden(
                    "manifest signer differs from the signed device context".into(),
                ));
            }
            let result = super::channel_manifest::append(tx, state, channel, pid, &event)?;
            drain_pending_state(tx, state, channel)?;
            (
                EventKind::ManifestChanged,
                json!({"manifest_event":event,"result":result,"state":super::e2e_service::read_state(tx,state,channel)?}),
            )
        }
        AuthorityOperation::RevokeDevice { device_id } => {
            super::e2e_service::require_device(tx, state.server_id, pid, &device_id)?;
            tx.execute("UPDATE member_device_keys SET revoked_at=datetime('now') WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3 AND revoked_at IS NULL",params![state.server_id,pid,device_id])?;
            drain_pending_state(tx, state, channel)?;
            (
                EventKind::DevicesAdmitted,
                json!({"action":"device_revoked","pseudonym_id":pid,"device_id":device_id,"state":super::e2e_service::read_state(tx,state,channel)?}),
            )
        }
        AuthorityOperation::SetEncryption { enabled } => {
            require_moderator(tx, state, pid)?;
            let manifest = super::channel_manifest::projection(tx, state, channel)?;
            if manifest.as_ref().is_none_or(|m| m.e2e_enabled != enabled) {
                return Err(AuthorityError::Forbidden(
                    "encryption change lacks a matching member-signed policy".into(),
                ));
            }
            tx.execute(
                "UPDATE channels SET e2e_enabled=?3 WHERE server_id=?1 AND channel_id=?2",
                params![state.server_id, channel, enabled],
            )?;
            drain_pending_state(tx, state, channel)?;
            (
                EventKind::StateChanged,
                json!({"action":"encryption_changed","state":super::e2e_service::read_state(tx,state,channel)?}),
            )
        }
        AuthorityOperation::StoreWraps { request: wrap_json } => {
            let wraps: super::e2e_service::PostWrapsRequest =
                serde_json::from_value(wrap_json.clone())
                    .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
            if principal.device_id.as_deref() != Some(wraps.sender_device_id.as_str()) {
                return Err(AuthorityError::Forbidden(
                    "wrap sender device differs from authenticated request".into(),
                ));
            }
            let result = super::e2e_service::store_wraps(tx, state, channel, pid, &wraps)?;
            drain_pending_state(tx, state, channel)?;
            let kind = if wraps.operation == "commit_epoch" {
                EventKind::EpochCommitted
            } else if wraps.operation.contains("history") || wraps.operation.contains("legacy") {
                EventKind::HistoryAdmitted
            } else {
                EventKind::DevicesAdmitted
            };
            (
                kind,
                json!({"pseudonym_id":pid,"wrap_request":wrap_json,"result":result,"state":super::e2e_service::read_state(tx,state,channel)?}),
            )
        }
        AuthorityOperation::Send {
            content,
            reply_to,
            attachments,
        } => {
            super::channel_service::check_publish_permission(
                tx,
                state.server_id,
                pid,
                channel,
                super::agent_policy::AgentAction::SendText,
            )
            .map_err(map_publication)?;
            validate_message_input(&content)?;
            super::e2e_service::validate_message_body(tx, state, channel, pid, &content)?;
            validate_attachment_descriptors(tx, state, request, None, &content, &attachments)?;
            let id = uuid::Uuid::new_v4().to_string();
            let stored = state.message_cipher().encrypt(&content)?;
            let message = annex_channels::create_message(
                tx,
                &annex_channels::CreateMessageParams {
                    channel_id: channel.clone(),
                    message_id: id,
                    sender_pseudonym: pid.clone(),
                    content: stored,
                    reply_to_message_id: reply_to,
                },
            )
            .map_err(map_channel)?;
            (
                EventKind::MessageAccepted,
                json!({"message":message_payload(state,message)?,"attachments":attachments}),
            )
        }
        AuthorityOperation::Edit {
            message_id,
            content,
            attachments,
        } => {
            super::channel_service::check_publish_permission(
                tx,
                state.server_id,
                pid,
                channel,
                super::agent_policy::AgentAction::EditText,
            )
            .map_err(map_publication)?;
            validate_message_input(&content)?;
            super::e2e_service::validate_message_body(tx, state, channel, pid, &content)?;
            let prior = annex_channels::get_message(tx, &message_id).map_err(map_channel)?;
            if prior.channel_id != *channel {
                return Err(AuthorityError::NotFound(
                    "message belongs to another channel".into(),
                ));
            }
            state.message_cipher().decrypt(&prior.content)?;
            if let Some(ref descriptors) = attachments {
                validate_attachment_descriptors(
                    tx,
                    state,
                    request,
                    Some(&message_id),
                    &content,
                    descriptors,
                )?;
            }
            let stored = state.message_cipher().encrypt(&content)?;
            let updated =
                annex_channels::edit_message_in_transaction(tx, &message_id, pid, &stored)
                    .map_err(map_channel)?;
            (
                EventKind::MessageEdited,
                json!({"message":message_payload(state,updated)?,"attachments":attachments}),
            )
        }
        AuthorityOperation::Delete { message_id } => {
            super::agent_policy::check_agent_action(
                tx,
                state.server_id,
                pid,
                super::agent_policy::AgentAction::DeleteText,
            )
            .map_err(super::channel_service::ChannelServiceError::from)
            .map_err(map_publication)?;
            let prior = annex_channels::get_message(tx, &message_id).map_err(map_channel)?;
            if prior.channel_id != *channel {
                return Err(AuthorityError::NotFound(
                    "message belongs to another channel".into(),
                ));
            }
            let updated = annex_channels::delete_message_in_transaction(tx, &message_id, pid)
                .map_err(map_channel)?;
            (
                EventKind::MessageDeleted,
                json!({"message":message_payload(state,updated)?}),
            )
        }
    };
    let event = append_event(tx, state, channel, kind, payload)?;
    remember_operation(tx, state, request, event.header.seq)?;
    if matches!(
        event.header.event_kind,
        EventKind::MessageAccepted | EventKind::MessageEdited
    ) && event.payload["attachments"].is_array()
    {
        let descriptors: Vec<AttachmentDescriptor> =
            serde_json::from_value(event.payload["attachments"].clone())
                .map_err(|e| AuthorityError::Internal(e.to_string()))?;
        bind_accepted_attachments(tx, state, &event, &descriptors)?;
        materialize_accepted_attachments(tx, state, &event, &descriptors)?;
    }
    if event.header.event_kind == EventKind::MessageDeleted {
        let id = event.payload["message"]["message_id"]
            .as_str()
            .ok_or_else(|| {
                AuthorityError::Internal("deleted event lacks message identity".into())
            })?;
        scrub_message_events(tx, state, channel, id, event.header.seq, "deleted")?;
        tx.execute("DELETE FROM channel_authority_pending_message_removals WHERE server_id=?1 AND channel_id=?2 AND message_id=?3",params![state.server_id,channel,id])?;
    }
    Ok(event)
}
fn validate_message_input(content: &str) -> Result<(), AuthorityError> {
    if content.trim().is_empty() || content.len() > 65_536 {
        return Err(AuthorityError::BadRequest(
            "message must contain 1..65536 bytes".into(),
        ));
    }
    Ok(())
}

/// Redaction removes every prior send/edit payload while retaining its signed
/// digest and a reference to the later durable removal event. No old plaintext,
/// encrypted body or attachment key descriptor remains in the history ledger.
pub fn scrub_message_events(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    message: &str,
    removal_seq: i64,
    reason: &str,
) -> Result<(), AuthorityError> {
    tx.execute("UPDATE channel_authority_events SET payload_json=NULL,removed_reason=?4,removal_event_seq=?5 WHERE server_id=?1 AND channel_id=?2 AND message_id=?3 AND event_seq<?5",params![state.server_id,channel,message,reason,removal_seq])?;
    tx.execute("UPDATE channel_authority_attachment_bindings SET removed_reason=?4,removal_seq=?5 WHERE server_id=?1 AND channel_id=?2 AND message_id=?3 AND removed_reason IS NULL",params![state.server_id,channel,message,reason,removal_seq])?;
    Ok(())
}

pub async fn receive_operation(
    state: Arc<AppState>,
    request: SignedRequest,
) -> Result<OperationReceipt, AuthorityError> {
    let working_state = state.clone();
    let (receipt, inserted) = tokio::task::spawn_blocking(move || {
        let state = working_state;
        let mut conn = state
            .pool
            .get()
            .map_err(|e| AuthorityError::Internal(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = prior_operation(&tx, &state, &request)?;
        let (seq, inserted) = if let Some(seq) = previous {
            authenticate_request(&tx, &state, &request)?;
            if !is_own_departure(&request.operation, &request.pseudonym_id) {
                super::e2e_service::require_member(
                    &tx,
                    &state,
                    &request.channel_id,
                    &request.pseudonym_id,
                )?;
            }
            (seq, false)
        } else {
            (accept_operation(&tx, &state, &request)?.header.seq, true)
        };
        drain_pending_removals(&tx, &state, &request.channel_id)?;
        let record = event_record(&tx, &state, &request.channel_id, seq)?;
        let mut receipt = OperationReceipt {
            version: 1,
            authority_identity: request.authority_identity.clone(),
            channel_id: request.channel_id.clone(),
            request_hash: digest(&request.signing_value()),
            operation_hash: request.operation_hash(),
            accepted_seq: seq,
            accepted_hash: record.hash().into(),
            record,
            signature: String::new(),
        };
        receipt.signature = sign(&state, &receipt.signing_value());
        tx.commit()?;
        Ok::<_, AuthorityError>((receipt, inserted))
    })
    .await
    .map_err(|e| AuthorityError::Internal(e.to_string()))??;
    if inserted {
        if let Err(error) =
            notify_applied_records(state, std::slice::from_ref(&receipt.record)).await
        {
            tracing::warn!(%error,"committed authority notification deferred");
        }
    }
    Ok(receipt)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationReceipt {
    pub version: u8,
    pub authority_identity: String,
    pub channel_id: String,
    pub request_hash: String,
    pub operation_hash: String,
    pub accepted_seq: i64,
    pub accepted_hash: String,
    pub record: EventRecord,
    pub signature: String,
}
impl OperationReceipt {
    fn signing_value(&self) -> Value {
        json!([
            "annex-authority-operation-receipt-v1",
            self.version,
            self.authority_identity,
            self.channel_id,
            self.request_hash,
            self.operation_hash,
            self.accepted_seq,
            self.accepted_hash,
            self.record
        ])
    }
    pub fn verify(
        &self,
        bound: &AuthorityBinding,
        request: &SignedRequest,
    ) -> Result<(), AuthorityError> {
        if self.version != 1
            || self.authority_identity != bound.authority_identity
            || self.channel_id != bound.channel_id
            || self.request_hash != digest(&request.signing_value())
            || self.operation_hash != request.operation_hash()
            || self.accepted_seq != self.record.header().seq
            || self.accepted_hash != self.record.hash()
        {
            return Err(AuthorityError::Forbidden(
                "authority receipt does not match the exact authenticated request".into(),
            ));
        }
        self.verify_signature(bound)
    }
    pub fn verify_signature(&self, bound: &AuthorityBinding) -> Result<(), AuthorityError> {
        if self.version != 1
            || self.authority_identity != bound.authority_identity
            || self.channel_id != bound.channel_id
            || self.accepted_seq != self.record.header().seq
            || self.accepted_hash != self.record.hash()
        {
            return Err(AuthorityError::Forbidden(
                "authority receipt binding mismatch".into(),
            ));
        }
        verify(
            &bound.authority_key_hex,
            &self.signature,
            &self.signing_value(),
        )?;
        verify_record(bound, &self.record)
    }
}

/// Notifications happen only after durable application. Catch-up may repeat a
/// notification after a crash; message IDs remain the client deduplication key.
pub async fn notify_applied_records(
    state: Arc<AppState>,
    records: &[EventRecord],
) -> Result<(), AuthorityError> {
    use crate::ws::protocol::OutgoingMessage;
    let mut keys = std::collections::BTreeSet::new();
    for record in records {
        let EventRecord::Present(event) = record else {
            continue;
        };
        let channel = &event.header.channel_id;
        let out = match event.header.event_kind {
            EventKind::MessageAccepted | EventKind::MessageEdited | EventKind::MessageDeleted
                if event.payload["message"].is_object() =>
            {
                let message: annex_channels::Message =
                    serde_json::from_value(event.payload["message"].clone())
                        .map_err(|e| AuthorityError::Internal(e.to_string()))?;
                Some(match event.header.event_kind {
                    EventKind::MessageAccepted => OutgoingMessage::Message(message.into()),
                    EventKind::MessageEdited => OutgoingMessage::MessageEdited(message.into()),
                    _ => OutgoingMessage::MessageDeleted(message.into()),
                })
            }
            EventKind::ChannelDeleted => {
                {
                    let mut sessions = state
                        .voice_sessions
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    sessions.retain(|(_, room), client| {
                        if room == channel {
                            client.close();
                            false
                        } else {
                            true
                        }
                    });
                }
                state
                    .voice_service
                    .delete_room(channel)
                    .await
                    .map_err(|e| AuthorityError::Internal(e.to_string()))?;
                Some(OutgoingMessage::ChannelDeleted {
                    channel_id: channel.clone(),
                })
            }
            EventKind::StateChanged
            | EventKind::EpochCommitted
            | EventKind::DevicesAdmitted
            | EventKind::HistoryAdmitted => {
                keys.insert(channel.clone());
                None
            }
            EventKind::ManifestChanged => {
                let event: super::channel_manifest::SignedManifestEvent =
                    serde_json::from_value(event.payload["manifest_event"].clone())
                        .map_err(|e| AuthorityError::Internal(e.to_string()))?;
                super::channel_manifest::notify_departure(state.clone(), channel, &event).await?;
                keys.insert(channel.clone());
                None
            }
            _ => None,
        };
        if let Some(out) = out {
            state
                .connection_manager
                .broadcast(
                    channel,
                    serde_json::to_string(&out)
                        .map_err(|e| AuthorityError::Internal(e.to_string()))?,
                )
                .await;
        }
    }
    for channel in keys {
        super::e2e_service::notify_channel(state.clone(), channel).await?;
    }
    Ok(())
}

pub fn pinned_authority_key(
    conn: &Connection,
    state: &AppState,
    channel: &str,
) -> Result<String, AuthorityError> {
    let bound = binding(conn, state, channel)?;
    if server_identity(&bound.authority_key_hex)? != bound.authority_identity {
        return Err(AuthorityError::Internal(
            "stored authority pin is inconsistent".into(),
        ));
    }
    Ok(bound.authority_key_hex)
}

fn validate_attachment_descriptors(
    tx: &Transaction<'_>,
    state: &AppState,
    request: &SignedRequest,
    existing_message: Option<&str>,
    body: &str,
    descriptors: &[AttachmentDescriptor],
) -> Result<(), AuthorityError> {
    if descriptors.len() > 16 {
        return Err(AuthorityError::BadRequest(
            "at most 16 attachments per message".into(),
        ));
    }
    let mut ids = std::collections::HashSet::new();
    let policy_json: String = tx.query_row(
        "SELECT policy_json FROM servers WHERE id=?1",
        [state.server_id],
        |r| r.get(0),
    )?;
    let policy: annex_types::ServerPolicy =
        serde_json::from_str(&policy_json).map_err(|e| AuthorityError::Internal(e.to_string()))?;
    for descriptor in descriptors {
        descriptor
            .validate()
            .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
        if !ids.insert(&descriptor.attachment_id) {
            return Err(AuthorityError::BadRequest(
                "duplicate attachment descriptor".into(),
            ));
        }
        let existing:Option<(String,String,String,Option<String>)>=tx.query_row("SELECT channel_id,message_id,descriptor_json,removed_reason FROM channel_authority_attachment_bindings WHERE server_id=?1 AND attachment_id=?2",params![state.server_id,descriptor.attachment_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if let Some((channel, message, stored, removed)) = existing {
            let prior: AttachmentDescriptor = serde_json::from_str(&stored).map_err(|_| {
                AuthorityError::Internal("stored attachment descriptor is corrupt".into())
            })?;
            if channel == request.channel_id
                && existing_message == Some(message.as_str())
                && prior == *descriptor
                && removed.is_none()
            {
                continue;
            }
            return Err(AuthorityError::Conflict(
                "attachment ID already has another immutable or removed binding".into(),
            ));
        }
        if descriptor.origin_identity != request.requesting_identity
            || descriptor.originating_server != request.originating_server
        {
            return Err(AuthorityError::Forbidden(
                "new attachment origin differs from authenticated submitting peer".into(),
            ));
        }
        if descriptor.is_plaintext() {
            let encrypted: bool = tx.query_row(
                "SELECT e2e_enabled FROM channels WHERE server_id=?1 AND channel_id=?2",
                params![state.server_id, request.channel_id],
                |r| r.get(0),
            )?;
            if encrypted || super::e2e_service::parse_message_body(body).is_ok() {
                return Err(AuthorityError::Conflict(
                    "ordinary attachment cannot be newly associated with an encrypted message"
                        .into(),
                ));
            }
        } else {
            let (epoch, key) = super::e2e_service::parse_message_body(body)?;
            if descriptor.epoch != epoch as u64 || descriptor.key_id != key {
                return Err(AuthorityError::Conflict(
                    "new attachment does not use the accepted message epoch".into(),
                ));
            }
        }
        let limit = super::upload_service::policy_limit(
            &policy,
            &descriptor.category,
            !descriptor.is_plaintext(),
        )
        .map_err(super::channel_service::ChannelServiceError::from)
        .map_err(map_publication)?;
        if descriptor.size_bytes > limit as u64 {
            return Err(AuthorityError::BadRequest(
                "attachment exceeds authority media policy".into(),
            ));
        }
    }
    Ok(())
}
pub fn bind_accepted_attachments(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
    descriptors: &[AttachmentDescriptor],
) -> Result<(), AuthorityError> {
    let message = event.payload["message"]["message_id"]
        .as_str()
        .ok_or_else(|| AuthorityError::BadRequest("attachment event lacks message ID".into()))?;
    let previous = {
        let mut stmt=tx.prepare("SELECT attachment_id FROM channel_authority_attachment_bindings WHERE server_id=?1 AND channel_id=?2 AND message_id=?3 AND removed_reason IS NULL")?;
        let rows = stmt
            .query_map(
                params![state.server_id, event.header.channel_id, message],
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for id in previous {
        if !descriptors.iter().any(|d| d.attachment_id == id) {
            tx.execute("UPDATE channel_authority_attachment_bindings SET removed_reason='detached',removal_seq=?4 WHERE server_id=?1 AND channel_id=?2 AND attachment_id=?3",params![state.server_id,event.header.channel_id,id,event.header.seq])?;
        }
    }
    for descriptor in descriptors {
        let prior:Option<(String,Option<String>,String,String)>=tx.query_row("SELECT descriptor_json,removed_reason,channel_id,message_id FROM channel_authority_attachment_bindings WHERE server_id=?1 AND attachment_id=?2",params![state.server_id,descriptor.attachment_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if let Some((prior, removed, prior_channel, prior_message)) = prior {
            let prior: AttachmentDescriptor = serde_json::from_str(&prior).map_err(|_| {
                AuthorityError::Internal("stored attachment descriptor is corrupt".into())
            })?;
            if prior != *descriptor
                || removed.is_some()
                || prior_channel != event.header.channel_id
                || prior_message != message
            {
                return Err(AuthorityError::Conflict(
                    "attachment binding cannot be replaced or restored".into(),
                ));
            }
            // Existing attachment retains its original accepted event context and AAD.
            continue;
        }
        tx.execute("INSERT INTO channel_authority_attachment_bindings(server_id,channel_id,message_id,attachment_id,accepted_seq,accepted_hash,descriptor_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![state.server_id,event.header.channel_id,message,descriptor.attachment_id,event.header.seq,event.event_hash,serde_json::to_string(descriptor).map_err(|e|AuthorityError::Internal(e.to_string()))?])?;
    }
    Ok(())
}

pub const READ_PERMIT_ENDPOINT: &str = "/api/federation/channel-authority/read-permit";

/// The forwarding origin authenticates its own transport request while retaining
/// the requester's independent intent signature. An origin cannot impersonate a
/// member's server when the authority is a third server.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadPermitRequest {
    pub version: u8,
    pub originating_server: String,
    pub originating_identity: String,
    pub intent: ReadIntent,
    pub intent_signature: String,
    pub nonce: String,
    pub created_at: i64,
    pub signature: String,
}
impl ReadPermitRequest {
    fn signing_value(&self) -> Value {
        json!([
            if self.version == 2 {
                "annex-channel-authority-read-request-v2"
            } else {
                "annex-channel-authority-read-request-v1"
            },
            self.version,
            self.originating_server,
            self.originating_identity,
            self.intent,
            self.intent_signature,
            self.nonce,
            self.created_at
        ])
    }
}

fn require_federation_enabled(conn: &Connection, state: &AppState) -> Result<(), AuthorityError> {
    let raw: String = conn.query_row(
        "SELECT policy_json FROM servers WHERE id=?1",
        [state.server_id],
        |r| r.get(0),
    )?;
    let policy: annex_types::ServerPolicy =
        serde_json::from_str(&raw).map_err(|e| AuthorityError::Internal(e.to_string()))?;
    if !policy.federation_enabled {
        return Err(AuthorityError::Forbidden(
            "federation is disabled by current authority policy".into(),
        ));
    }
    Ok(())
}
fn active_peer_by_url(
    conn: &Connection,
    state: &AppState,
    url: &str,
    identity: &str,
) -> Result<(i64, String), AuthorityError> {
    require_federation_enabled(conn, state)?;
    let row: Option<(i64,String)> = conn.query_row(
        "SELECT i.id,i.public_key FROM instances i JOIN federation_agreements a ON a.remote_instance_id=i.id WHERE i.base_url=?1 AND i.status='ACTIVE' AND a.local_server_id=?2 AND a.active=1 AND a.transfer_scope<>'NO_TRANSFER'",
        params![url,state.server_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (id, key) = row.ok_or_else(|| {
        AuthorityError::Forbidden("no current agreement for the attachment peer".into())
    })?;
    if super::federation_repository::active_agreement_transfer_scope(conn, state.server_id, id)?
        .as_deref()
        == Some("NO_TRANSFER")
    {
        return Err(AuthorityError::Forbidden(
            "attachment transfer is prohibited by the current agreement".into(),
        ));
    }
    if server_identity(&key)? != identity {
        return Err(AuthorityError::Forbidden(
            "attachment peer identity does not match its pinned key".into(),
        ));
    }
    Ok((id, key))
}

fn permit_for_intent(
    tx: &Transaction<'_>,
    state: &AppState,
    intent: &ReadIntent,
    requester_signature: &str,
) -> Result<SignedReadPermit, AuthorityError> {
    use super::federated_upload_types as wire;
    let now = chrono::Utc::now().timestamp();
    intent
        .validate(now)
        .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
    let channel = &intent.accepted.channel_id;
    let bound = binding(tx, state, channel)?;
    if !bound.local_authority || bound.authority_identity != intent.accepted.authority_identity {
        return Err(AuthorityError::Forbidden(
            "attachment read targets a different authority".into(),
        ));
    }
    if bound.deleted_seq.is_some() {
        return Err(AuthorityError::NotFound("channel was deleted".into()));
    }
    let local_key = hex::encode(state.signing_key.verifying_key().to_bytes());
    let local_identity = server_identity(&local_key)?;
    let requester_key = if intent.requesting_identity == local_identity {
        if intent.requesting_server != state.get_public_url().trim_end_matches('/') {
            return Err(AuthorityError::Forbidden(
                "local attachment requester URL mismatch".into(),
            ));
        }
        let local: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM platform_identities p WHERE p.server_id=?1 AND p.pseudonym_id=?2 AND p.active=1 AND NOT EXISTS(SELECT 1 FROM federated_identities f WHERE f.server_id=p.server_id AND f.pseudonym_id=p.pseudonym_id))",params![state.server_id,intent.member_pseudonym],|r|r.get(0))?;
        if !local {
            return Err(AuthorityError::Forbidden(
                "requester does not own this local member".into(),
            ));
        }
        local_key
    } else {
        let (peer, key) = active_peer_by_url(
            tx,
            state,
            &intent.requesting_server,
            &intent.requesting_identity,
        )?;
        let attested: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM federated_identities f JOIN platform_identities p ON p.server_id=f.server_id AND p.pseudonym_id=f.pseudonym_id WHERE f.server_id=?1 AND f.remote_instance_id=?2 AND f.pseudonym_id=?3 AND p.active=1)",params![state.server_id,peer,intent.member_pseudonym],|r|r.get(0))?;
        if !attested {
            return Err(AuthorityError::Forbidden(
                "requester is not the member's currently attested server".into(),
            ));
        }
        if !super::human_federation::current_remote_membership(
            tx,
            state,
            peer,
            channel,
            &intent.member_pseudonym,
            Some(&intent.device_id),
            now,
        )? {
            return Err(AuthorityError::Forbidden(
                "attachment read requires a fresh typed human membership attestation".into(),
            ));
        }
        key
    };
    let signed_bytes = wire::intent_signing_input(intent)
        .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
    wire::verify(&requester_key, requester_signature, &signed_bytes)
        .map_err(|e| AuthorityError::Forbidden(e.to_string()))?;
    super::e2e_service::require_member(tx, state, channel, &intent.member_pseudonym)?;
    let descriptor = &intent.accepted.descriptor;
    let certificate = super::channel_manifest::require_certified_device(
        tx,
        state,
        &intent.member_pseudonym,
        &intent.device_id,
    )?;
    if certificate.server_identity != intent.requesting_identity {
        return Err(AuthorityError::Forbidden(
            "attachment device belongs to a different origin".into(),
        ));
    }
    if !descriptor.is_plaintext() {
        super::channel_manifest::require_admitted_device(
            tx,
            state,
            channel,
            &intent.member_pseudonym,
            &intent.device_id,
        )?;
        let authorized_key: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_device_key_wraps WHERE server_id=?1 AND channel_id=?2 AND recipient_pseudonym_id=?3 AND recipient_device_id=?4 AND epoch=?5 AND key_id=?6)",params![state.server_id,channel,intent.member_pseudonym,intent.device_id,descriptor.epoch,descriptor.key_id],|r|r.get(0))?;
        if !authorized_key {
            return Err(AuthorityError::Forbidden(
                "device is not entitled to the attachment's encryption epoch".into(),
            ));
        }
    }
    let row: Option<(String,i64,String,Option<String>)> = tx.query_row(
        "SELECT b.descriptor_json,b.accepted_seq,b.accepted_hash,b.removed_reason FROM channel_authority_attachment_bindings b JOIN messages m ON m.server_id=b.server_id AND m.channel_id=b.channel_id AND m.message_id=b.message_id WHERE b.server_id=?1 AND b.channel_id=?2 AND b.message_id=?3 AND b.attachment_id=?4 AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now'))",
        params![state.server_id,channel,intent.accepted.message_id,descriptor.attachment_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (stored, seq, hash, removed) = row.ok_or_else(|| {
        AuthorityError::NotFound("accepted attachment is absent or its message was deleted".into())
    })?;
    let actual: AttachmentDescriptor =
        serde_json::from_str(&stored).map_err(|e| AuthorityError::Internal(e.to_string()))?;
    if removed.is_some() {
        return Err(AuthorityError::NotFound("attachment was removed".into()));
    }
    if &actual != descriptor
        || seq as u64 != intent.accepted.accepted_event_seq
        || hash != intent.accepted.accepted_event_hash
    {
        return Err(AuthorityError::Forbidden(
            "attachment read does not match its immutable accepted binding".into(),
        ));
    }
    // Checking source admission here prevents a stale source or replica from
    // continuing a read after its federation agreement has been revoked.
    if descriptor.origin_identity != local_identity {
        active_peer_by_url(
            tx,
            state,
            &descriptor.originating_server,
            &descriptor.origin_identity,
        )?;
    }
    let intent_hash =
        wire::intent_digest(intent).map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
    tx.execute(
        "DELETE FROM channel_authority_nonces WHERE expires_at<?1",
        [now],
    )?;
    let prior: Option<String> = tx.query_row("SELECT request_hash FROM channel_authority_nonces WHERE server_id=?1 AND requesting_identity=?2 AND nonce=?3",params![state.server_id,intent.requesting_identity,intent.nonce],|r|r.get(0)).optional()?;
    if prior.is_some() {
        return Err(AuthorityError::Forbidden(
            "attachment read nonce was already used".into(),
        ));
    }
    tx.execute("INSERT INTO channel_authority_nonces(server_id,requesting_identity,nonce,request_hash,expires_at) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,intent.requesting_identity,intent.nonce,intent_hash,now+REQUEST_LIFETIME_SECONDS])?;
    drain_pending_state(tx, state, channel)?;
    let bound = binding(tx, state, channel)?;
    let revision: i64 = tx.query_row(
        "SELECT revision FROM channel_key_state WHERE server_id=?1 AND channel_id=?2",
        params![state.server_id, channel],
        |r| r.get(0),
    )?;
    let mut permit = SignedReadPermit {
        domain: wire::permit_domain(intent.version).into(),
        version: intent.version,
        authority_identity: bound.authority_identity,
        intent_sha256: intent_hash,
        intent: intent.clone(),
        current_revision: revision as u64,
        head_seq: bound.head_seq as u64,
        head_hash: bound.head_hash,
        issued_at: now,
        expires_at: now + wire::PERMIT_SECONDS,
        signature: String::new(),
    };
    permit.signature = wire::sign(
        &state.signing_key,
        &wire::permit_signing_input(&permit)
            .map_err(|e| AuthorityError::Internal(e.to_string()))?,
    );
    Ok(permit)
}

pub async fn receive_read_permit(
    state: Arc<AppState>,
    request: ReadPermitRequest,
) -> Result<SignedReadPermit, AuthorityError> {
    tokio::task::spawn_blocking(move || {
        let now = chrono::Utc::now().timestamp();
        if request.version != request.intent.accepted.descriptor.protocol_version()
            || request.version != request.intent.version
            || !super::federated_upload_types::canonical_uuid(&request.nonce)
            || request.created_at.abs_diff(now) > 90
        {
            return Err(AuthorityError::BadRequest(
                "invalid read permit request".into(),
            ));
        }
        let mut conn = state
            .pool
            .get()
            .map_err(|e| AuthorityError::Internal(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (_, key) = active_peer_by_url(
            &tx,
            &state,
            &request.originating_server,
            &request.originating_identity,
        )?;
        verify(&key, &request.signature, &request.signing_value())?;
        let intent = &request.intent;
        if request.originating_identity != intent.requesting_identity
            && request.originating_identity != intent.accepted.descriptor.origin_identity
        {
            return Err(AuthorityError::Forbidden(
                "only the requester or accepted blob origin may forward a read intent".into(),
            ));
        }
        let result = permit_for_intent(&tx, &state, intent, &request.intent_signature)?;
        tx.commit()?;
        Ok(result)
    })
    .await
    .map_err(|e| AuthorityError::Internal(e.to_string()))?
}

/// Every chunk, completed replica association and browser download obtains a
/// new nonce-bound authority decision. A disconnected authority is unavailable;
/// a replica's last materialized membership is never a substitute for permission.
pub async fn authorize_attachment_read(
    state: Arc<AppState>,
    intent: ReadIntent,
    requester_signature: Option<String>,
) -> Result<SignedReadPermit, AuthorityError> {
    use super::federated_upload_types as wire;
    intent
        .validate(chrono::Utc::now().timestamp())
        .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
    let local_identity = wire::stable_identity(&state.signing_key.verifying_key().to_bytes());
    let local_request = requester_signature.is_none();
    let intent_signature = match requester_signature {
        Some(signature) => signature,
        None => {
            if intent.requesting_identity != local_identity
                || intent.requesting_server != state.get_public_url().trim_end_matches('/')
            {
                return Err(AuthorityError::Forbidden(
                    "local server cannot sign another requester's intent".into(),
                ));
            }
            wire::sign(
                &state.signing_key,
                &wire::intent_signing_input(&intent)
                    .map_err(|e| AuthorityError::BadRequest(e.to_string()))?,
            )
        }
    };
    let bound = {
        let conn = state
            .pool
            .get()
            .map_err(|e| AuthorityError::Internal(e.to_string()))?;
        binding(&conn, &state, &intent.accepted.channel_id)?
    };
    if bound.local_authority {
        return tokio::task::spawn_blocking(move || {
            let mut conn = state
                .pool
                .get()
                .map_err(|e| AuthorityError::Internal(e.to_string()))?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let result = permit_for_intent(&tx, &state, &intent, &intent_signature)?;
            tx.commit()?;
            Ok(result)
        })
        .await
        .map_err(|e| AuthorityError::Internal(e.to_string()))?;
    }
    if bound.authority_identity != intent.accepted.authority_identity || bound.deleted_seq.is_some()
    {
        return Err(AuthorityError::Forbidden(
            "attachment authority binding is absent or deleted".into(),
        ));
    }
    if local_request {
        super::channel_replication::refresh_attachment_attestation(
            state.clone(),
            &intent.accepted.channel_id,
            &intent.member_pseudonym,
            &intent.device_id,
        )
        .await?;
    }
    let peer = bound
        .authority_peer_id
        .ok_or_else(|| AuthorityError::Unavailable("authority peer is not configured".into()))?;
    let mut request = ReadPermitRequest {
        version: intent.version,
        originating_server: state.get_public_url().trim_end_matches('/').to_owned(),
        originating_identity: local_identity,
        intent: intent.clone(),
        intent_signature,
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: chrono::Utc::now().timestamp(),
        signature: String::new(),
    };
    request.signature = sign(&state, &request.signing_value());
    let body =
        serde_json::to_string(&request).map_err(|e| AuthorityError::Internal(e.to_string()))?;
    let response =
        super::federation_delivery::request_typed(state.clone(), peer, READ_PERMIT_ENDPOINT, &body)
            .await
            .map_err(AuthorityError::from)?;
    let permit: SignedReadPermit = serde_json::from_str(&response)
        .map_err(|e| AuthorityError::Forbidden(format!("invalid authority read permit: {e}")))?;
    wire::verify_permit(
        &permit,
        &intent,
        &bound.authority_key_hex,
        chrono::Utc::now().timestamp(),
    )
    .map_err(|e| AuthorityError::Forbidden(e.to_string()))?;
    Ok(permit)
}

/// A removed record retains its original authority signature and digest, plus a
/// separately signed removal decision. Catch-up can advance in exact order
/// without reconstructing deleted plaintext or wrapping keys from an old body.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemovedEvent {
    pub header: EventHeader,
    pub event_hash: String,
    pub signature: String,
    pub message_id: Option<String>,
    pub reason: String,
    pub removal_seq: i64,
    pub removal_hash: String,
    pub removal_signature: String,
}
impl RemovedEvent {
    fn signing_value(&self) -> Value {
        json!([
            "annex-channel-authority-removed-v1",
            self.header.authority_identity,
            self.header.channel_id,
            self.header.seq,
            self.event_hash,
            self.header.payload_hash,
            self.message_id,
            self.reason,
            self.removal_seq,
            self.removal_hash
        ])
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "record_type", rename_all = "snake_case")]
pub enum EventRecord {
    Present(AuthorityEvent),
    Removed(RemovedEvent),
}
impl EventRecord {
    pub fn header(&self) -> &EventHeader {
        match self {
            Self::Present(e) => &e.header,
            Self::Removed(e) => &e.header,
        }
    }
    pub fn hash(&self) -> &str {
        match self {
            Self::Present(e) => &e.event_hash,
            Self::Removed(e) => &e.event_hash,
        }
    }
}

pub fn drain_pending_removals(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
) -> Result<(), AuthorityError> {
    let channel_deleted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_authority_pending_deletions WHERE server_id=?1 AND channel_id=?2)",params![state.server_id,channel],|r|r.get(0))?;
    if channel_deleted {
        let event = append_event(
            tx,
            state,
            channel,
            EventKind::ChannelDeleted,
            json!({"channel_id":channel,"reason":"channel_deleted"}),
        )?;
        tx.execute("UPDATE channel_authority_events SET payload_json=NULL,removed_reason='channel_deleted',removal_event_seq=?3 WHERE server_id=?1 AND channel_id=?2 AND event_seq<?3",params![state.server_id,channel,event.header.seq])?;
        tx.execute("UPDATE channel_authority_attachment_bindings SET removed_reason='channel_deleted',removal_seq=?3 WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel,event.header.seq])?;
        tx.execute("DELETE FROM channel_authority_pending_message_removals WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel])?;
        tx.execute(
            "DELETE FROM channel_authority_pending_deletions WHERE server_id=?1 AND channel_id=?2",
            params![state.server_id, channel],
        )?;
        return Ok(());
    }
    let rows = {
        let mut stmt=tx.prepare("SELECT message_id,reason FROM channel_authority_pending_message_removals WHERE server_id=?1 AND channel_id=?2 ORDER BY message_id")?;
        let rows = stmt
            .query_map(params![state.server_id, channel], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (message, reason) in rows {
        let kind = if reason == "retention" {
            EventKind::HistoryPruned
        } else {
            EventKind::MessageDeleted
        };
        let event = append_event(
            tx,
            state,
            channel,
            kind,
            json!({"message_id":message,"reason":reason}),
        )?;
        scrub_message_events(tx, state, channel, &message, event.header.seq, &reason)?;
        tx.execute("DELETE FROM channel_authority_pending_message_removals WHERE server_id=?1 AND channel_id=?2 AND message_id=?3",params![state.server_id,channel,message])?;
    }
    Ok(())
}

pub fn event_record(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    seq: i64,
) -> Result<EventRecord, AuthorityError> {
    type StoredAuthorityEvent = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
    );
    let row:Option<StoredAuthorityEvent>=conn.query_row(
        "SELECT header_json,event_hash,signature,payload_json,removed_reason,removal_event_seq,message_id FROM channel_authority_events WHERE server_id=?1 AND channel_id=?2 AND event_seq=?3",
        params![state.server_id,channel,seq],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    let (header, hash, signature, payload, reason, removal_seq, message_id) =
        row.ok_or_else(|| AuthorityError::NotFound("authority event is absent".into()))?;
    let header: EventHeader =
        serde_json::from_str(&header).map_err(|e| AuthorityError::Internal(e.to_string()))?;
    if let Some(payload) = payload {
        let payload = state.message_cipher().decrypt(&payload)?;
        let event = AuthorityEvent {
            header,
            event_hash: hash,
            signature,
            payload: serde_json::from_str(&payload)
                .map_err(|e| AuthorityError::Internal(e.to_string()))?,
        };
        verify_event(&binding(conn, state, channel)?, &event)?;
        return Ok(EventRecord::Present(event));
    }
    let bound = binding(conn, state, channel)?;
    if !bound.local_authority {
        let raw:Option<String>=conn.query_row("SELECT removal_certificate_json FROM channel_authority_events WHERE server_id=?1 AND channel_id=?2 AND event_seq=?3",params![state.server_id,channel,seq],|r|r.get(0))?;
        let record: EventRecord = serde_json::from_str(&raw.ok_or_else(|| {
            AuthorityError::Unavailable(
                "replica lacks the authority's original removal certificate".into(),
            )
        })?)
        .map_err(|e| AuthorityError::Internal(e.to_string()))?;
        verify_record(&bound, &record)?;
        return Ok(record);
    }
    let removal_seq = removal_seq.ok_or_else(|| {
        AuthorityError::Unavailable(
            "pending removal decision must be signed before catch-up".into(),
        )
    })?;
    let removal_hash:String=conn.query_row("SELECT event_hash FROM channel_authority_events WHERE server_id=?1 AND channel_id=?2 AND event_seq=?3",params![state.server_id,channel,removal_seq],|r|r.get(0))?;
    let mut removed = RemovedEvent {
        header,
        event_hash: hash,
        signature,
        message_id,
        reason: reason
            .ok_or_else(|| AuthorityError::Internal("removed event lacks reason".into()))?,
        removal_seq,
        removal_hash,
        removal_signature: String::new(),
    };
    removed.removal_signature = sign(state, &removed.signing_value());
    Ok(EventRecord::Removed(removed))
}

pub fn verify_record(bound: &AuthorityBinding, record: &EventRecord) -> Result<(), AuthorityError> {
    match record {
        EventRecord::Present(event) => verify_event(bound, event),
        EventRecord::Removed(event) => {
            let h = &event.header;
            if h.version != 1
                || h.channel_id != bound.channel_id
                || h.authority_identity != bound.authority_identity
                || h.seq <= 0
                || h.seq > MAX_SEQUENCE
                || h.event_hash() != event.event_hash
                || event.removal_seq <= h.seq
                || event.removal_seq > MAX_SEQUENCE
                || !super::e2e_service::lower_hex(&event.removal_hash)
                || !matches!(
                    event.reason.as_str(),
                    "deleted" | "retention" | "channel_deleted"
                )
            {
                return Err(AuthorityError::Forbidden(
                    "invalid removed authority event".into(),
                ));
            }
            verify(
                &bound.authority_key_hex,
                &event.signature,
                &h.signing_value(),
            )?;
            verify(
                &bound.authority_key_hex,
                &event.removal_signature,
                &event.signing_value(),
            )
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatchupBatch {
    pub version: u8,
    pub authority_identity: String,
    pub channel_id: String,
    pub request_hash: String,
    pub after_seq: i64,
    pub head_seq: i64,
    pub head_hash: String,
    pub records: Vec<EventRecord>,
    pub access_revoked: bool,
    pub more: bool,
    pub signature: String,
}
impl CatchupBatch {
    pub(crate) fn signing_value(&self) -> Value {
        json!([
            "annex-channel-authority-batch-v1",
            self.version,
            self.authority_identity,
            self.channel_id,
            self.request_hash,
            self.after_seq,
            self.head_seq,
            self.head_hash,
            self.records,
            self.access_revoked,
            self.more
        ])
    }
}
/// SignedRequest.operation for this read is {"type":"catch_up","after_seq":N,"limit":N}.
/// Reuse the same principal/signature/nonce checks as mutation, then revalidate
/// current membership before any retained message or encrypted wrap leaves disk.
pub async fn receive_catchup(
    state: Arc<AppState>,
    request: SignedRequest,
) -> Result<CatchupBatch, AuthorityError> {
    tokio::task::spawn_blocking(move||{
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Pull {r#type:String,after_seq:i64,limit:usize}
        let pull:Pull=serde_json::from_value(request.operation.clone()).map_err(|e|AuthorityError::BadRequest(e.to_string()))?;
        let cleanup_only = pull.r#type == "cleanup";
        if !matches!(pull.r#type.as_str(),"catch_up"|"cleanup")||pull.after_seq<0||pull.after_seq>MAX_SEQUENCE||pull.limit==0||pull.limit>64 {
            return Err(AuthorityError::BadRequest("invalid catch-up cursor or batch limit".into()));
        }
        let mut conn=state.pool.get().map_err(|e|AuthorityError::Internal(e.to_string()))?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let principal=authenticate_request_with_access(&tx,&state,&request,
            if cleanup_only { RequestAccess::ReplicaCleanup } else { RequestAccess::PriorMember })?;
        // Expiry is a read boundary, not the next background sweep. Only the
        // requested bounded page can be retired by this request. The normal
        // deletion triggers durably purge bodies/wrap descriptors and enqueue
        // cleanup; signed removal records advance the same ordered cursor.
        tx.execute("DELETE FROM messages WHERE server_id=?1 AND channel_id=?2 AND expires_at IS NOT NULL AND (datetime(expires_at) IS NULL OR datetime(expires_at)<=datetime('now')) AND message_id IN (SELECT message_id FROM channel_authority_events WHERE server_id=?1 AND channel_id=?2 AND event_seq>?3 ORDER BY event_seq LIMIT ?4)",params![state.server_id,request.channel_id,pull.after_seq,pull.limit])?;
        drain_pending_removals(&tx,&state,&request.channel_id)?;
        let before=binding(&tx,&state,&request.channel_id)?;
        // Deletion catch-up is restricted to previously admitted replicas and
        // returns only signed removal records; ordinary history still needs a
        // current admitted member, even on an established transport.
        if before.deleted_seq.is_none() {

            drain_pending_state(&tx,&state,&request.channel_id)?;
        }
        let admitted:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_authority_replicas WHERE server_id=?1 AND channel_id=?2 AND peer_instance_id=?3 AND revoked_at IS NULL)",params![state.server_id,request.channel_id,principal.peer_id],|r|r.get(0))?;
        if !admitted{return Err(AuthorityError::Forbidden("peer is not an admitted channel replica".into()));}
        drain_pending_removals(&tx,&state,&request.channel_id)?;
        let bound=binding(&tx,&state,&request.channel_id)?;
        if pull.after_seq>bound.head_seq{return Err(AuthorityError::Conflict("replica cursor is beyond authority head".into()));}
        // A previously admitted server retains a cleanup obligation after its
        // last member/device leaves. Its signed control request may retrieve
        // only a deleted channel's tombstones, never live history or key wraps.
        let may_read = if cleanup_only {
            bound.deleted_seq.is_some()
        } else {
            let member=before.deleted_seq.is_some() || super::e2e_service::require_member(&tx,&state,&request.channel_id,&principal.pseudonym_id).is_ok();
            let active:bool=tx.query_row("SELECT active=1 FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2",params![state.server_id,principal.pseudonym_id],|r|r.get(0))?;
            let fresh=super::human_federation::current_remote_membership(&tx,&state,principal.peer_id,&request.channel_id,&principal.pseudonym_id,principal.device_id.as_deref(),chrono::Utc::now().timestamp())?;
            member && active && fresh
        };
        if !may_read {
            let mut batch=CatchupBatch{version:1,authority_identity:bound.authority_identity,channel_id:request.channel_id.clone(),request_hash:digest(&request.signing_value()),after_seq:pull.after_seq,head_seq:bound.head_seq,head_hash:bound.head_hash,records:Vec::new(),access_revoked:true,more:false,signature:String::new()};
            batch.signature=sign(&state,&batch.signing_value());tx.commit()?;return Ok(batch);
        }
        let mut records=Vec::new();let mut bytes=0usize;
        for seq in (pull.after_seq+1)..=bound.head_seq {
            if records.len()>=pull.limit { break; }
            let record=event_record(&tx,&state,&request.channel_id,seq)?;
            if cleanup_only && !cleanup_record(&record) {
                return Err(AuthorityError::Internal("cleanup must never disclose retained history".into()));
            }
            let size=serde_json::to_vec(&record).map_err(|e|AuthorityError::Internal(e.to_string()))?.len();
            if !records.is_empty()&&(bytes+size>MAX_EVENT_BYTES||records.len()>=pull.limit){break;}
            if size>MAX_EVENT_BYTES+16_384{return Err(AuthorityError::Internal("stored authority event exceeds protocol bound".into()));}
            bytes+=size;records.push(record);
        }
        let last=records.last().map_or(pull.after_seq,|e|e.header().seq);
        let mut batch=CatchupBatch{version:1,authority_identity:bound.authority_identity,channel_id:request.channel_id.clone(),request_hash:digest(&request.signing_value()),after_seq:pull.after_seq,head_seq:bound.head_seq,head_hash:bound.head_hash,records,access_revoked:false,more:last<bound.head_seq,signature:String::new()};
        batch.signature=sign(&state,&batch.signing_value());tx.commit()?;Ok(batch)
    }).await.map_err(|e|AuthorityError::Internal(e.to_string()))?
}

/// Initial ownership is an explicit moderator operation. A preexisting channel
/// on another server is never silently reassigned to whichever peer spoke first.
pub fn initialize_local(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    moderator: &str,
) -> Result<AuthorityEvent, AuthorityError> {
    require_moderator(tx, state, moderator)?;
    let bound = register_local(tx, state, channel)?;
    if bound.head_seq > 0 {
        return event_by_sequence(tx, state, channel, 1);
    }
    let row = annex_channels::get_channel(tx, channel).map_err(map_channel)?;
    let members = {
        let mut stmt=tx.prepare("SELECT p.pseudonym_id,p.participant_type,p.active,p.can_moderate FROM channel_members m JOIN platform_identities p ON p.server_id=m.server_id AND p.pseudonym_id=m.pseudonym_id WHERE m.server_id=?1 AND m.channel_id=?2 ORDER BY p.pseudonym_id")?;
        let rows=stmt.query_map(params![state.server_id,channel],|r|Ok(json!({"pseudonym_id":r.get::<_,String>(0)?,"participant_type":r.get::<_,String>(1)?,"active":r.get::<_,bool>(2)?,"can_moderate":r.get::<_,bool>(3)?})))?.collect::<Result<Vec<_>,_>>()?;
        rows
    };
    let devices = {
        let mut stmt=tx.prepare("SELECT d.pseudonym_id,d.device_id,d.x25519_pub_hex,d.revoked_at,d.device_certificate FROM member_device_keys d WHERE d.server_id=?1 AND EXISTS(SELECT 1 FROM channel_members m WHERE m.server_id=d.server_id AND m.channel_id=?2 AND m.pseudonym_id=d.pseudonym_id) ORDER BY d.pseudonym_id,d.device_id")?;
        let rows=stmt.query_map(params![state.server_id,channel],|r|Ok(json!({"pseudonym_id":r.get::<_,String>(0)?,"device_id":r.get::<_,String>(1)?,"x25519_pub_hex":r.get::<_,String>(2)?,"revoked_at":r.get::<_,Option<String>>(3)?,"device_certificate":r.get::<_,Option<String>>(4)?})))?.collect::<Result<Vec<_>,_>>()?;
        rows
    };
    let mut key_state = super::e2e_service::read_state(tx, state, channel)?;
    key_state.authority_server = state.get_public_url().trim_end_matches('/').into();
    tx.execute(
        "UPDATE channel_key_state SET authority_server=?3 WHERE server_id=?1 AND channel_id=?2",
        params![state.server_id, channel, key_state.authority_server],
    )?;
    let event = append_event(
        tx,
        state,
        channel,
        EventKind::Genesis,
        json!({"channel":row,"state":key_state,"members":members,"devices":devices}),
    )?;
    tx.execute(
        "DELETE FROM channel_authority_pending_state WHERE server_id=?1 AND channel_id=?2",
        params![state.server_id, channel],
    )?;
    super::authority_bootstrap::append_retained_keys(tx, state, channel)?;
    super::authority_bootstrap::append_retained_messages(tx, state, channel)?;
    // Importing historical device records on a replica may fire its ordinary
    // revision triggers. Restore the exact current authority snapshot after
    // all historical rows, without rewriting any epoch's authenticated AAD.
    append_event(
        tx,
        state,
        channel,
        EventKind::StateChanged,
        json!({"state":key_state}),
    )?;
    tx.execute("UPDATE federation_outbox SET status='failed',last_error='channel adopted ordered authority; use authority catch-up',updated_at=datetime('now') WHERE channel_id=?1 AND envelope_kind<>'rtx' AND status IN('pending','failed')",[channel])?;
    Ok(event)
}

fn import_key_state(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    value: &Value,
) -> Result<(), AuthorityError> {
    let number = |name: &str| {
        value[name]
            .as_i64()
            .filter(|n| (0..=MAX_SEQUENCE).contains(n))
            .ok_or_else(|| AuthorityError::BadRequest(format!("invalid authority state {name}")))
    };
    let revision = number("revision")?;
    let current = number("current_epoch")?;
    let required = number("required_epoch")?;
    let key = value["current_key_id"].as_str();
    if required == 0
        || required < current
        || value["history_policy"] != "all_members"
        || key.is_some_and(|s| !super::e2e_service::lower_hex(s))
    {
        return Err(AuthorityError::BadRequest(
            "invalid authority epoch state".into(),
        ));
    }
    let enabled = value["e2e_enabled"]
        .as_bool()
        .or_else(|| value["e2e_enabled"].as_i64().map(|n| n != 0))
        .ok_or_else(|| AuthorityError::BadRequest("missing encryption mode".into()))?;
    tx.execute(
        "UPDATE channels SET e2e_enabled=?3 WHERE server_id=?1 AND channel_id=?2",
        params![state.server_id, channel, enabled],
    )?;
    let authority = binding(tx, state, channel)?.authority_url;
    tx.execute("UPDATE channel_key_state SET authority_server=?3,revision=?4,current_epoch=?5,required_epoch=?6,current_key_id=?7,history_policy='all_members' WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel,authority,revision,current,required,key])?;
    Ok(())
}

fn import_members_and_devices(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    payload: &Value,
) -> Result<(), AuthorityError> {
    if let Some(members) = payload["members"].as_array() {
        if members.len() > 10_000 {
            return Err(AuthorityError::BadRequest(
                "authority membership exceeds limit".into(),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for member in members {
            let pid = member["pseudonym_id"]
                .as_str()
                .filter(|s| crate::middleware::is_valid_pseudonym_format(s))
                .ok_or_else(|| AuthorityError::BadRequest("invalid member pseudonym".into()))?;
            let active = member["active"]
                .as_bool()
                .or_else(|| member["active"].as_i64().map(|v| v != 0))
                .ok_or_else(|| AuthorityError::BadRequest("member lacks active state".into()))?;
            if !active {
                continue;
            }
            if !ids.insert(pid) {
                return Err(AuthorityError::BadRequest(
                    "duplicate authority member".into(),
                ));
            }
            let kind = member["participant_type"].as_str().ok_or_else(|| {
                AuthorityError::BadRequest("member lacks participant type".into())
            })?;
            if !matches!(
                kind,
                "HUMAN" | "AI_AGENT" | "COLLECTIVE" | "BRIDGE" | "SERVICE"
            ) {
                return Err(AuthorityError::BadRequest(
                    "invalid participant type".into(),
                ));
            }
            // A channel authority never grants this server's global operator
            // powers. Existing local identities retain their independent roles.
            tx.execute("INSERT OR IGNORE INTO platform_identities(server_id,pseudonym_id,participant_type) VALUES(?1,?2,?3)",params![state.server_id,pid,kind])?;
        }
        let existing = {
            let mut stmt = tx.prepare(
                "SELECT pseudonym_id FROM channel_members WHERE server_id=?1 AND channel_id=?2",
            )?;
            let rows = stmt
                .query_map(params![state.server_id, channel], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for pid in existing {
            if !ids.contains(pid.as_str()) {
                annex_channels::remove_member(tx, state.server_id, channel, &pid)
                    .map_err(map_channel)?;
            }
        }
        for pid in ids {
            annex_channels::add_member(tx, state.server_id, channel, pid).map_err(map_channel)?;
        }
    }
    if let Some(devices) = payload["devices"].as_array() {
        if devices.len() > 10_000 {
            return Err(AuthorityError::BadRequest(
                "authority device directory exceeds limit".into(),
            ));
        }
        for device in devices {
            let pid = device["pseudonym_id"]
                .as_str()
                .ok_or_else(|| AuthorityError::BadRequest("device lacks member".into()))?;
            let key = device["x25519_pub_hex"]
                .as_str()
                .ok_or_else(|| AuthorityError::BadRequest("device lacks key".into()))?;
            let id = super::e2e_service::device_id(key)?;
            if device["device_id"].as_str() != Some(&id) {
                return Err(AuthorityError::Forbidden("device key hash mismatch".into()));
            }
            let revoked = device["revoked_at"].as_str();
            let cert = if device["device_certificate"].is_null() {
                None
            } else if let Some(s) = device["device_certificate"].as_str() {
                Some(s.to_owned())
            } else {
                Some(canonical_json(&device["device_certificate"]))
            };
            if let Some(raw) = &cert {
                let certificate: crate::device_certificates::DeviceCertificate =
                    serde_json::from_str(raw).map_err(|_| {
                        AuthorityError::Forbidden(
                            "authority device certificate is malformed".into(),
                        )
                    })?;
                if certificate.pseudonym_id != pid
                    || certificate.device_id != id
                    || certificate.x25519_pub_hex != key
                {
                    return Err(AuthorityError::Forbidden(
                        "authority certificate differs from directory key".into(),
                    ));
                }
                import_verified_certificate(tx, state, &certificate, revoked)?;
                continue;
            }

            let prior:Option<Option<String>>=tx.query_row("SELECT revoked_at FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3",params![state.server_id,pid,id],|r|r.get(0)).optional()?;
            if matches!(prior, Some(Some(_))) && revoked.is_none() {
                return Err(AuthorityError::Forbidden(
                    "authority attempted to restore a revoked device".into(),
                ));
            }
            tx.execute("INSERT OR IGNORE INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex,device_certificate,revoked_at) VALUES(?1,?2,?3,?4,?5,?6)",params![state.server_id,pid,id,key,cert,revoked])?;
            if let Some(revoked) = revoked {
                tx.execute("UPDATE member_device_keys SET revoked_at=?4 WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3 AND revoked_at IS NULL",params![state.server_id,pid,id,revoked])?;
            }
        }
    }
    Ok(())
}

pub(crate) fn transfer_error(
    error: super::federated_upload_service::TransferError,
) -> AuthorityError {
    match error.status_code() {
        StatusCode::BAD_REQUEST => AuthorityError::BadRequest(error.to_string()),
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => {
            AuthorityError::Forbidden(error.to_string())
        }
        StatusCode::NOT_FOUND => AuthorityError::NotFound(error.to_string()),
        StatusCode::CONFLICT => AuthorityError::Conflict(error.to_string()),
        StatusCode::SERVICE_UNAVAILABLE => AuthorityError::Unavailable(error.to_string()),
        _ => AuthorityError::Internal(error.to_string()),
    }
}
fn remove_materialized_message(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    message: &str,
    reason: &str,
    seq: i64,
    hash: &str,
) -> Result<(), AuthorityError> {
    tx.execute("UPDATE messages SET content='',deleted_at=COALESCE(deleted_at,datetime('now')) WHERE server_id=?1 AND channel_id=?2 AND message_id=?3",params![state.server_id,channel,message])?;
    tx.execute("DELETE FROM message_edits WHERE message_id IN(SELECT message_id FROM messages WHERE server_id=?1 AND channel_id=?2 AND message_id=?3)",params![state.server_id,channel,message])?;
    super::federated_upload_service::remove_expected(
        tx,
        state.server_id,
        channel,
        Some(message),
        None,
        reason,
        (seq as u64, hash),
    )
    .map_err(transfer_error)?;
    Ok(())
}
fn import_wraps(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
) -> Result<(), AuthorityError> {
    let Some(request) = event.payload.get("wrap_request") else {
        return Ok(());
    };
    let req: super::e2e_service::PostWrapsRequest = serde_json::from_value(request.clone())
        .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
    let pid = event.payload["pseudonym_id"]
        .as_str()
        .ok_or_else(|| AuthorityError::BadRequest("wrap event lacks sender".into()))?;
    let channel = &event.header.channel_id;
    let receipt: super::channel_manifest::WrapReceipt =
        serde_json::from_value(event.payload["result"]["authorization"].clone()).map_err(|_| {
            AuthorityError::Forbidden(
                "accepted key event lacks independent member authorization".into(),
            )
        })?;
    let manifest = super::channel_manifest::projection(tx, state, channel)?.ok_or_else(|| {
        AuthorityError::Forbidden("accepted key event precedes its signed member manifest".into())
    })?;
    super::channel_manifest::verify_wrap_receipt(&manifest, &receipt)?;
    if receipt.sender_pseudonym_id != pid
        || receipt.sender_device_id != req.sender_device_id
        || receipt.epoch != req.epoch
        || receipt.key_id != req.key_id
        || receipt.operation != req.operation
        || receipt.expected_revision != req.expected_revision
    {
        return Err(AuthorityError::Forbidden(
            "accepted wrap request differs from its member authorization".into(),
        ));
    }
    use base64::{engine::general_purpose::STANDARD, Engine};
    let mut digests = Vec::new();
    for wrap in &req.wraps {
        let bytes = STANDARD
            .decode(&wrap.wrapped_key_b64)
            .map_err(|_| AuthorityError::BadRequest("invalid accepted key base64".into()))?;
        if bytes.len() < 60
            || wrap.wrapped_key_b64.len() > 4096
            || STANDARD.encode(&bytes) != wrap.wrapped_key_b64
        {
            return Err(AuthorityError::BadRequest(
                "invalid accepted sealed key".into(),
            ));
        }
        let device = manifest
            .admitted(&wrap.recipient_pseudonym_id, &wrap.recipient_device_id)
            .ok_or_else(|| {
                AuthorityError::Forbidden("accepted key recipient is not admitted".into())
            })?;
        digests.push((
            wrap.recipient_pseudonym_id.clone(),
            wrap.recipient_device_id.clone(),
            device.certificate_sha256.clone(),
            hex::encode(Sha256::digest(bytes)),
        ));
    }
    digests.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    if digests != receipt.wrap_digests {
        return Err(AuthorityError::Forbidden(
            "accepted wrap bytes differ from their signed digests".into(),
        ));
    }
    tx.execute("INSERT OR IGNORE INTO channel_key_authorizations(server_id,channel_id,authorization_sha256,receipt_json) VALUES(?1,?2,?3,?4)",params![state.server_id,channel,receipt.authorization_sha256,serde_json::to_string(&receipt).map_err(|e|AuthorityError::Internal(e.to_string()))?])?;
    if req.version != 2
        || req.epoch <= 0
        || req.epoch > MAX_SEQUENCE
        || !super::e2e_service::lower_hex(&req.key_id)
        || req.wraps.len() > 10_000
    {
        return Err(AuthorityError::BadRequest(
            "invalid accepted key-wrap event".into(),
        ));
    }
    if req.operation == "admit_legacy_devices" {
        tx.execute("INSERT OR IGNORE INTO channel_legacy_key_candidates(server_id,channel_id,epoch,key_id,authority_namespace) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,channel,req.epoch,req.key_id,event.payload["result"]["state"]["authority_server"].as_str().ok_or_else(||AuthorityError::BadRequest("legacy key namespace is missing".into()))?])?;
    } else {
        if req.operation == "commit_epoch" {
            tx.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device,manifest_hash,authority_namespace) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![state.server_id,channel,req.epoch,req.key_id,req.expected_revision,pid,req.sender_device_id,receipt.manifest_hash,receipt.authority_identity])?;
        } else {
            let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_key_epochs WHERE server_id=?1 AND channel_id=?2 AND epoch=?3 AND key_id=?4)",params![state.server_id,channel,req.epoch,req.key_id],|r|r.get(0))?;
            if !matches {
                return Err(AuthorityError::Conflict(
                    "missing committed key epoch before admission".into(),
                ));
            }
        }
    }
    for wrap in req.wraps {
        let (table, legacy) = if req.operation == "admit_legacy_devices" {
            ("channel_legacy_device_wraps", true)
        } else {
            ("channel_device_key_wraps", false)
        };
        // Table identifiers are fixed constants above; no request text is SQL.
        let sql=format!("SELECT key_id,wrapped_key_b64 FROM {table} WHERE server_id=?1 AND channel_id=?2 AND epoch=?3 AND recipient_pseudonym_id=?4 AND recipient_device_id=?5{}",if legacy{" AND key_id=?6"}else{" AND (?6 IS NOT NULL)"});
        let prior: Option<(String, String)> = tx
            .query_row(
                &sql,
                params![
                    state.server_id,
                    channel,
                    req.epoch,
                    wrap.recipient_pseudonym_id,
                    wrap.recipient_device_id,
                    req.key_id
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((key, body)) = prior {
            if key != req.key_id || body != wrap.wrapped_key_b64 {
                return Err(AuthorityError::Conflict(
                    "immutable key wrap was replaced".into(),
                ));
            }
            continue;
        }
        let sql=format!("INSERT INTO {table}(server_id,channel_id,epoch,key_id,recipient_pseudonym_id,recipient_device_id,sender_pseudonym_id,sender_device_id,wrapped_key_b64,authorization_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)");
        tx.execute(
            &sql,
            params![
                state.server_id,
                channel,
                req.epoch,
                req.key_id,
                wrap.recipient_pseudonym_id,
                wrap.recipient_device_id,
                pid,
                req.sender_device_id,
                wrap.wrapped_key_b64,
                receipt.authorization_sha256
            ],
        )?;
    }
    Ok(())
}

fn materialize_present(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
    _fetch_member: &str,
    _fetch_device: &str,
) -> Result<(), AuthorityError> {
    let channel = &event.header.channel_id;
    match event.header.event_kind {
        EventKind::HistoryImported => {
            super::authority_bootstrap::materialize_message(tx, state, event)?
        }
        EventKind::HistoryEditImported => {
            super::authority_bootstrap::materialize_edit(tx, state, event)?
        }
        EventKind::DeviceHistoryImported => {
            super::authority_bootstrap::materialize_device(tx, state, event)?
        }
        EventKind::KeyHistoryImported => {
            super::authority_bootstrap::materialize_key(tx, state, event)?
        }
        EventKind::ManifestHistoryImported => {
            super::authority_bootstrap::materialize_manifest(tx, state, event)?
        }
        EventKind::ManifestChanged => {
            let manifest_event: super::channel_manifest::SignedManifestEvent =
                serde_json::from_value(event.payload["manifest_event"].clone())
                    .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
            let previous = super::channel_manifest::projection(tx, state, channel)?;
            let key = state.membership_vkey_v2.as_ref().ok_or_else(|| {
                AuthorityError::Forbidden("membership_v2 verifier unavailable".into())
            })?;
            let next = super::channel_manifest::verify_transition(
                previous.as_ref(),
                &manifest_event,
                key,
            )?;
            if manifest_event.authority_identity != event.header.authority_identity
                || manifest_event.channel_id != *channel
            {
                return Err(AuthorityError::Forbidden(
                    "manifest differs from accepted authority context".into(),
                ));
            }
            super::channel_manifest::persist(tx, state, channel, &manifest_event, &next)?;
            if let Some(removed) = super::channel_manifest::removed_member(&manifest_event) {
                annex_channels::remove_member(tx, state.server_id, channel, removed)
                    .map_err(map_channel)?;
                tx.execute("DELETE FROM graph_edges WHERE server_id=?1 AND from_node=?2 AND to_node=?3 AND kind IN('MEMBER_OF','MODERATES','AGENT_SERVING')",params![state.server_id,removed,annex_graph::channel_vertex(state.server_id,channel)])?;
            }
            import_key_state(tx, state, channel, &event.payload["state"])?;
        }
        EventKind::Genesis => {
            let row: annex_channels::Channel =
                serde_json::from_value(event.payload["channel"].clone())
                    .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
            if row.channel_id != *channel
                || row.federation_scope != annex_types::FederationScope::Federated
            {
                return Err(AuthorityError::Forbidden(
                    "genesis channel binding mismatch".into(),
                ));
            }
            if tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM channels WHERE channel_id=?1)",
                [channel],
                |r| r.get::<_, bool>(0),
            )? {
                let guarded: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_reconciliation_guards WHERE server_id=?1 AND channel_id=?2)", params![state.server_id,channel], |r| r.get(0))?;
                if !guarded {
                    return Err(AuthorityError::Conflict(
                        "replica genesis would overwrite an existing channel".into(),
                    ));
                }
                tx.execute("UPDATE channels SET name=?3,channel_type=?4,topic=?5,vrp_topic_binding=?6,required_capabilities_json=?7,agent_min_alignment=?8,retention_days=?9,created_at=?10 WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel,row.name,serde_json::to_string(&row.channel_type).map_err(|e|AuthorityError::Internal(e.to_string()))?,row.topic,row.vrp_topic_binding,row.required_capabilities_json,row.agent_min_alignment.map(|v|serde_json::to_string(&v)).transpose().map_err(|e|AuthorityError::Internal(e.to_string()))?,row.retention_days,row.created_at])?;
            } else {
                annex_channels::create_channel(
                    tx,
                    &annex_channels::CreateChannelParams {
                        server_id: state.server_id,
                        channel_id: channel.clone(),
                        name: row.name,
                        channel_type: row.channel_type,
                        topic: row.topic,
                        vrp_topic_binding: row.vrp_topic_binding,
                        required_capabilities_json: row.required_capabilities_json,
                        agent_min_alignment: row.agent_min_alignment,
                        retention_days: row.retention_days,
                        federation_scope: row.federation_scope,
                    },
                )
                .map_err(map_channel)?;
            }
            import_members_and_devices(tx, state, channel, &event.payload)?;
            import_key_state(tx, state, channel, &event.payload["state"])?;
        }
        EventKind::StateChanged
        | EventKind::EpochCommitted
        | EventKind::DevicesAdmitted
        | EventKind::HistoryAdmitted => {
            import_members_and_devices(tx, state, channel, &event.payload)?;
            import_wraps(tx, state, event)?;
            if event.payload.get("state").is_some() {
                import_key_state(tx, state, channel, &event.payload["state"])?;
            }
        }
        EventKind::MessageAccepted | EventKind::MessageEdited => {
            let message: annex_channels::Message =
                serde_json::from_value(event.payload["message"].clone())
                    .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
            if message.channel_id != *channel || message.deleted_at.is_some() {
                return Err(AuthorityError::Forbidden(
                    "accepted message channel/deletion mismatch".into(),
                ));
            }
            if event.header.event_kind == EventKind::MessageAccepted {
                let stored = state.message_cipher().encrypt(&message.content)?;
                annex_channels::create_message(
                    tx,
                    &annex_channels::CreateMessageParams {
                        channel_id: channel.clone(),
                        message_id: message.message_id.clone(),
                        sender_pseudonym: message.sender_pseudonym.clone(),
                        content: stored,
                        reply_to_message_id: message.reply_to_message_id.clone(),
                    },
                )
                .map_err(map_channel)?;
                tx.execute(
                    "UPDATE messages SET created_at=?2,expires_at=?3 WHERE message_id=?1",
                    params![message.message_id, message.created_at, message.expires_at],
                )?;
            } else {
                let prior =
                    annex_channels::get_message(tx, &message.message_id).map_err(map_channel)?;
                if prior.channel_id != *channel
                    || prior.sender_pseudonym != message.sender_pseudonym
                    || prior.created_at != message.created_at
                    || prior.reply_to_message_id != message.reply_to_message_id
                    || prior.deleted_at.is_some()
                {
                    return Err(AuthorityError::Forbidden(
                        "edit changed the original message identity".into(),
                    ));
                }
                state.message_cipher().decrypt(&prior.content)?;
                let stored = state.message_cipher().encrypt(&message.content)?;
                // The authority enforced the edit deadline at acceptance.
                // Reapplying that deadline at catch-up time would permanently
                // strand a legitimate edit during a long receiver outage.
                let edited = message.edited_at.as_deref().ok_or_else(|| {
                    AuthorityError::Forbidden("accepted edit lacks timestamp".into())
                })?;
                let created =
                    chrono::NaiveDateTime::parse_from_str(&message.created_at, "%Y-%m-%d %H:%M:%S")
                        .map_err(|_| {
                            AuthorityError::Forbidden(
                                "accepted edit has invalid original timestamp".into(),
                            )
                        })?;
                let edited_time =
                    chrono::NaiveDateTime::parse_from_str(edited, "%Y-%m-%d %H:%M:%S").map_err(
                        |_| AuthorityError::Forbidden("accepted edit timestamp is invalid".into()),
                    )?;
                if edited_time < created {
                    return Err(AuthorityError::Forbidden(
                        "accepted edit predates original message".into(),
                    ));
                }
                tx.execute(
                    "INSERT INTO message_edits(message_id,old_content,edited_at) VALUES(?1,?2,?3)",
                    params![message.message_id, prior.content, edited],
                )?;
                tx.execute(
                    "UPDATE messages SET edited_at=?2,content=?3 WHERE message_id=?1",
                    params![message.message_id, edited, stored],
                )?;
            }
            if let Some(attachments) = event.payload["attachments"].as_array() {
                let descriptors: Vec<AttachmentDescriptor> =
                    serde_json::from_value(Value::Array(attachments.clone()))
                        .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
                let previous = {
                    let mut stmt=tx.prepare("SELECT attachment_id FROM channel_authority_attachment_bindings WHERE server_id=?1 AND channel_id=?2 AND message_id=?3 AND removed_reason IS NULL")?;
                    let rows = stmt
                        .query_map(params![state.server_id, channel, message.message_id], |r| {
                            r.get::<_, String>(0)
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    rows
                };
                let detached: Vec<_> = previous
                    .into_iter()
                    .filter(|id| !descriptors.iter().any(|d| &d.attachment_id == id))
                    .collect();
                if !detached.is_empty() {
                    super::federated_upload_service::remove_expected(
                        tx,
                        state.server_id,
                        channel,
                        Some(&message.message_id),
                        Some(&detached),
                        "detached",
                        (event.header.seq as u64, &event.event_hash),
                    )
                    .map_err(transfer_error)?;
                }
                bind_accepted_attachments(tx, state, event, &descriptors)?;
                materialize_accepted_attachments(tx, state, event, &descriptors)?;
            }
        }
        EventKind::MessageDeleted | EventKind::HistoryPruned => {
            let message = event.payload["message_id"]
                .as_str()
                .or_else(|| event.payload["message"]["message_id"].as_str())
                .ok_or_else(|| {
                    AuthorityError::BadRequest("removal event lacks message ID".into())
                })?;
            let reason = if event.header.event_kind == EventKind::HistoryPruned {
                "retention"
            } else {
                "deleted"
            };
            remove_materialized_message(
                tx,
                state,
                channel,
                message,
                reason,
                event.header.seq,
                &event.event_hash,
            )?;
            scrub_message_events(tx, state, channel, message, event.header.seq, reason)?;
        }
        EventKind::ChannelDeleted => {
            super::federated_upload_service::remove_expected(
                tx,
                state.server_id,
                channel,
                None,
                None,
                "channel_deleted",
                (event.header.seq as u64, &event.event_hash),
            )
            .map_err(transfer_error)?;
            tx.execute("DELETE FROM graph_edges WHERE server_id=?1 AND to_node=?2 AND kind IN('AGENT_SERVING','MEMBER_OF','MODERATES')",params![state.server_id,annex_graph::channel_vertex(state.server_id,channel)])?;
            for sql in [
                "DELETE FROM messages WHERE server_id=?1 AND channel_id=?2",
                "DELETE FROM channel_members WHERE server_id=?1 AND channel_id=?2",
                "DELETE FROM channel_key_wraps WHERE server_id=?1 AND channel_id=?2",
                "DELETE FROM channels WHERE server_id=?1 AND channel_id=?2",
            ] {
                tx.execute(sql, params![state.server_id, channel])?;
            }
            tx.execute("UPDATE channel_authority_events SET payload_json=NULL,removed_reason='channel_deleted',removal_event_seq=?3 WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel,event.header.seq])?;
        }
    }
    Ok(())
}

pub fn apply_batch(
    tx: &Transaction<'_>,
    state: &AppState,
    request: &SignedRequest,
    batch: &CatchupBatch,
    fetch_member: &str,
    fetch_device: &str,
) -> Result<usize, AuthorityError> {
    let bound = binding(tx, state, &request.channel_id)?;
    if bound.local_authority
        || batch.version != 1
        || batch.authority_identity != bound.authority_identity
        || batch.channel_id != bound.channel_id
        || batch.request_hash != digest(&request.signing_value())
        || batch.after_seq != bound.head_seq
        || batch.records.len() > 64
        || batch.head_seq < bound.head_seq
        || batch.head_seq > MAX_SEQUENCE
    {
        return Err(AuthorityError::Forbidden(
            "catch-up batch does not match request, pin or current cursor".into(),
        ));
    }
    verify(
        &bound.authority_key_hex,
        &batch.signature,
        &batch.signing_value(),
    )?;
    let cleanup_only = request.operation["type"] == "cleanup";
    if cleanup_only && batch.records.iter().any(|record| !cleanup_record(record)) {
        return Err(AuthorityError::Forbidden(
            "cleanup contains retained history".into(),
        ));
    }
    if batch.access_revoked {
        if !batch.records.is_empty() || batch.more {
            return Err(AuthorityError::Forbidden(
                "revocation receipt contains forbidden retained history".into(),
            ));
        }
        if cleanup_only {
            return Ok(0);
        }
        annex_channels::remove_member(tx, state.server_id, &bound.channel_id, fetch_member)
            .map_err(map_channel)?;
        tx.execute("UPDATE channel_replica_readers SET revoked_at=unixepoch(),last_error='authority revoked channel access' WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3",params![state.server_id,bound.channel_id,fetch_member])?;
        tx.execute("UPDATE channel_authority_outgoing SET status='cancelled',operation_ciphertext=NULL,last_error='authority revoked channel access' WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3 AND status='pending'",params![state.server_id,bound.channel_id,fetch_member])?;
        return Ok(0);
    }
    let mut seq = bound.head_seq;
    let mut previous = bound.head_hash.clone();
    for record in &batch.records {
        apply_verified_record(
            tx,
            state,
            &bound.channel_id,
            record,
            fetch_member,
            fetch_device,
        )?;
        seq = record.header().seq;
        previous = record.hash().to_owned();
    }
    if seq > batch.head_seq
        || batch.more != (seq < batch.head_seq)
        || (seq == batch.head_seq && previous != batch.head_hash)
    {
        return Err(AuthorityError::Forbidden(
            "catch-up head or continuation is inconsistent".into(),
        ));
    }
    Ok(batch.records.len())
}

fn cleanup_record(record: &EventRecord) -> bool {
    matches!(record, EventRecord::Removed(_))
        || matches!(record, EventRecord::Present(event) if event.header.event_kind == EventKind::ChannelDeleted)
}

/// Apply one independently signed record only after its ordered predecessor.
/// Reconciliation separately verifies a fresh, nonce-bound complete head before
/// entering its writer transaction; ordinary catch-up verifies the whole batch.
pub(crate) fn apply_verified_record(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    record: &EventRecord,
    fetch_member: &str,
    fetch_device: &str,
) -> Result<(), AuthorityError> {
    let bound = binding(tx, state, channel)?;
    if bound.local_authority {
        return Err(AuthorityError::Forbidden(
            "cannot import into a local authority".into(),
        ));
    }
    verify_record(&bound, record)?;
    if record.header().seq != bound.head_seq + 1 || record.header().previous_hash != bound.head_hash
    {
        return Err(AuthorityError::Conflict(
            "missing or reordered authority record".into(),
        ));
    }
    match record {
        EventRecord::Present(event) => {
            apply_event(tx, state, event, |tx, event| {
                materialize_present(tx, state, event, fetch_member, fetch_device)
            })?;
        }
        EventRecord::Removed(event) => {
            if let Some(message) = &event.message_id {
                remove_materialized_message(
                    tx,
                    state,
                    &bound.channel_id,
                    message,
                    &event.reason,
                    event.removal_seq,
                    &event.removal_hash,
                )?;
            }
            tx.execute("INSERT INTO channel_authority_events(server_id,channel_id,event_seq,message_id,event_kind,event_hash,payload_hash,header_json,signature,payload_json,removed_reason,removal_event_seq,created_at,removal_certificate_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,?10,?11,?12,?13)",params![state.server_id,bound.channel_id,event.header.seq,event.message_id,serde_json::to_string(&event.header.event_kind).map_err(|e|AuthorityError::Internal(e.to_string()))?,event.event_hash,event.header.payload_hash,serde_json::to_string(&event.header).map_err(|e|AuthorityError::Internal(e.to_string()))?,event.signature,event.reason,event.removal_seq,event.header.created_at,serde_json::to_string(record).map_err(|e|AuthorityError::Internal(e.to_string()))?])?;
            tx.execute("UPDATE channel_authorities SET head_seq=?3,head_hash=?4 WHERE server_id=?1 AND channel_id=?2",params![state.server_id,bound.channel_id,event.header.seq,event.event_hash])?;
        }
    }
    Ok(())
}

/// Import independently verified ownership without assigning group membership.
pub(crate) fn import_verified_certificate(
    tx: &Transaction<'_>,
    state: &AppState,
    certificate: &crate::device_certificates::DeviceCertificate,
    revoked: Option<&str>,
) -> Result<(), AuthorityError> {
    let key = state
        .membership_vkey_v2
        .as_ref()
        .ok_or_else(|| AuthorityError::Forbidden("membership_v2 verifier unavailable".into()))?;
    let digest = crate::device_certificates::verify_certificate(
        certificate,
        &certificate.server_identity,
        &certificate.pseudonym_id,
        key,
    )?;
    let prior:Option<(Option<String>,Option<String>,String)>=tx.query_row("SELECT revoked_at,certificate_sha256,x25519_pub_hex FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3",params![state.server_id,certificate.pseudonym_id,certificate.device_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    if let Some((old_revoked, old_digest, old_key)) = &prior {
        if old_key != &certificate.x25519_pub_hex
            || old_digest.as_ref().is_some_and(|d| d != &digest)
            || old_revoked.is_some() && revoked.is_none()
        {
            return Err(AuthorityError::Forbidden(
                "accepted device changes an immutable or revoked certificate".into(),
            ));
        }
    }
    let raw = serde_json::to_string(certificate)
        .map_err(|e| AuthorityError::BadRequest(e.to_string()))?;
    tx.execute("INSERT INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex,device_certificate,certificate_sha256,revoked_at) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(server_id,pseudonym_id,device_id) DO UPDATE SET device_certificate=CASE WHEN member_device_keys.device_certificate IS NULL THEN excluded.device_certificate ELSE member_device_keys.device_certificate END,certificate_sha256=COALESCE(member_device_keys.certificate_sha256,excluded.certificate_sha256),revoked_at=COALESCE(member_device_keys.revoked_at,excluded.revoked_at)",params![state.server_id,certificate.pseudonym_id,certificate.device_id,certificate.x25519_pub_hex,raw,digest,revoked])?;
    Ok(())
}
pub fn record_local_manifest(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    pid: &str,
    event: &super::channel_manifest::SignedManifestEvent,
    result: &Value,
) -> Result<(), AuthorityError> {
    let row = annex_channels::get_channel(tx, channel).map_err(map_channel)?;
    if row.federation_scope != annex_types::FederationScope::Federated {
        return Ok(());
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM channel_authorities WHERE server_id=?1 AND channel_id=?2)",
        params![state.server_id, channel],
        |r| r.get(0),
    )?;
    if !exists {
        initialize_local(tx, state, channel, pid)?;
        // Initialization imports the complete already-committed signed
        // lineage, including this event, inside this same transaction.
        return Ok(());
    }
    if !binding(tx, state, channel)?.local_authority {
        return Err(AuthorityError::Forbidden(
            "replica manifest mutations must be accepted by its pinned authority".into(),
        ));
    }
    if result["applied"] == false {
        return Ok(());
    }
    drain_pending_state(tx, state, channel)?;
    append_event(
        tx,
        state,
        channel,
        EventKind::ManifestChanged,
        json!({"manifest_event":event,"state":super::e2e_service::read_state(tx,state,channel)?}),
    )?;
    Ok(())
}
pub fn record_local_wraps(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    pid: &str,
    request: &super::e2e_service::PostWrapsRequest,
    result: &Value,
) -> Result<(), AuthorityError> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM channel_authorities WHERE server_id=?1 AND channel_id=?2)",
        params![state.server_id, channel],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    if !binding(tx, state, channel)?.local_authority {
        return Err(AuthorityError::Forbidden(
            "replica key mutations must be accepted by its pinned authority".into(),
        ));
    }
    if result["inserted"] == 0 {
        return Ok(());
    }
    drain_pending_state(tx, state, channel)?;
    let kind = if request.operation == "commit_epoch" {
        EventKind::EpochCommitted
    } else {
        EventKind::HistoryAdmitted
    };
    append_event(
        tx,
        state,
        channel,
        kind,
        json!({"pseudonym_id":pid,"wrap_request":request,"result":result,"state":super::e2e_service::read_state(tx,state,channel)?}),
    )?;
    Ok(())
}

/// A local authority's ordinary message path participates in the same ordered
/// ledger transaction as remote intents. Legacy channels without an explicit
/// authority binding keep their existing relay protocol.
pub fn record_local_message(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    message: &annex_channels::Message,
    kind: EventKind,
    attachment_ids: Option<&[String]>,
) -> Result<bool, AuthorityError> {
    let bound = match binding(tx, state, channel) {
        Ok(bound) => bound,
        Err(AuthorityError::NotFound(_)) => return Ok(false),
        Err(e) => return Err(e),
    };
    if !bound.local_authority {
        return Err(AuthorityError::Forbidden(
            "replica messages must be accepted by their pinned authority".into(),
        ));
    }
    if bound.head_seq == 0 {
        return Err(AuthorityError::Conflict(
            "initialize the channel authority before publishing".into(),
        ));
    }
    drain_pending_state(tx, state, channel)?;
    let descriptors = attachment_ids
        .map(|ids| {
            super::federated_upload_service::descriptors_for_origin_message(
                tx,
                state,
                channel,
                &message.sender_pseudonym,
                ids,
                Some(&message.message_id),
            )
        })
        .transpose()
        .map_err(transfer_error)?;
    let mut value =
        serde_json::to_value(message).map_err(|e| AuthorityError::Internal(e.to_string()))?;
    value["expires_at"] = json!(message.expires_at);
    let event = append_event(
        tx,
        state,
        channel,
        kind.clone(),
        json!({"message":value,"attachments":descriptors}),
    )?;
    if let Some(descriptors) = descriptors {
        bind_accepted_attachments(tx, state, &event, &descriptors)?;
        materialize_accepted_attachments(tx, state, &event, &descriptors)?;
    }
    if kind == EventKind::MessageDeleted {
        scrub_message_events(
            tx,
            state,
            channel,
            &message.message_id,
            event.header.seq,
            "deleted",
        )?;
        tx.execute("DELETE FROM channel_authority_pending_message_removals WHERE server_id=?1 AND channel_id=?2 AND message_id=?3",params![state.server_id,channel,message.message_id])?;
    }
    Ok(true)
}

pub(crate) fn materialize_accepted_attachments(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
    descriptors: &[AttachmentDescriptor],
) -> Result<(), AuthorityError> {
    if descriptors.is_empty() {
        return Ok(());
    }
    let channel = &event.header.channel_id;
    let message = event.payload["message"]["message_id"]
        .as_str()
        .ok_or_else(|| AuthorityError::Internal("accepted attachment lacks message".into()))?;
    let author = event.payload["message"]["sender_pseudonym"]
        .as_str()
        .ok_or_else(|| AuthorityError::Internal("accepted attachment lacks author".into()))?;
    let local = crate::device_certificates::server_identity(state);
    for descriptor in descriptors {
        let retained_replica: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2)",
            params![state.server_id, descriptor.attachment_id],
            |row| row.get(0),
        )?;
        let reader = if descriptor.origin_identity == local || retained_replica {
            // This records committed bytes and immutable origin context, not
            // download permission. A retained replica is validated against its
            // original immutable binding before restoring its association.
            // Replay may precede current membership; it never invents a reader.
            Some((String::new(), String::new()))
        } else {
            let mut statement=tx.prepare("SELECT d.pseudonym_id,d.device_id FROM member_device_keys d JOIN channel_members m ON m.server_id=d.server_id AND m.pseudonym_id=d.pseudonym_id JOIN platform_identities p ON p.server_id=d.server_id AND p.pseudonym_id=d.pseudonym_id WHERE d.server_id=?1 AND m.channel_id=?2 AND p.active=1 AND d.revoked_at IS NULL AND d.device_certificate IS NOT NULL AND NOT EXISTS(SELECT 1 FROM federated_identities f WHERE f.server_id=d.server_id AND f.pseudonym_id=d.pseudonym_id) ORDER BY d.pseudonym_id,d.device_id")?;
            let candidates = statement.query_map(params![state.server_id, channel], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut selected = None;
            for candidate in candidates {
                let (pid, device) = candidate?;
                let certificate =
                    super::channel_manifest::require_certified_device(tx, state, &pid, &device)?;
                if certificate.server_identity != local {
                    continue;
                }
                if !descriptor.is_plaintext() {
                    if super::channel_manifest::require_admitted_device(
                        tx, state, channel, &pid, &device,
                    )
                    .is_err()
                    {
                        continue;
                    }
                    let wrapped:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_device_key_wraps WHERE server_id=?1 AND channel_id=?2 AND recipient_pseudonym_id=?3 AND recipient_device_id=?4 AND epoch=?5 AND key_id=?6)",params![state.server_id,channel,pid,device,descriptor.epoch,descriptor.key_id],|r|r.get(0))?;
                    if !wrapped {
                        continue;
                    }
                }
                selected = Some((pid, device));
                break;
            }
            selected
        };
        // Initial catch-up may import history before this reader's Join or
        // signed admission. An authenticated lazy read starts transfer later.
        let Some((member, device)) = reader else {
            continue;
        };
        let (seq,hash):(i64,String)=tx.query_row("SELECT accepted_seq,accepted_hash FROM channel_authority_attachment_bindings WHERE server_id=?1 AND channel_id=?2 AND message_id=?3 AND attachment_id=?4 AND removed_reason IS NULL",params![state.server_id,channel,message,descriptor.attachment_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let accepted = AcceptedAttachment {
            authority_identity: event.header.authority_identity.clone(),
            channel_id: channel.clone(),
            message_id: message.into(),
            accepted_event_seq: seq as u64,
            accepted_event_hash: hash,
            descriptor: descriptor.clone(),
        };
        super::federated_upload_service::record_expected(
            tx, state, &accepted, author, &member, &device,
        )
        .map_err(transfer_error)?;
    }
    Ok(())
}
pub async fn http_operation(
    axum::extract::Extension(state): axum::extract::Extension<Arc<AppState>>,
    Json(request): Json<SignedRequest>,
) -> Result<Json<OperationReceipt>, AuthorityError> {
    receive_operation(state, request).await.map(Json)
}
pub async fn http_catchup(
    axum::extract::Extension(state): axum::extract::Extension<Arc<AppState>>,
    Json(request): Json<SignedRequest>,
) -> Result<Json<CatchupBatch>, AuthorityError> {
    receive_catchup(state, request).await.map(Json)
}
pub async fn http_read_permit(
    axum::extract::Extension(state): axum::extract::Extension<Arc<AppState>>,
    Json(request): Json<ReadPermitRequest>,
) -> Result<Json<SignedReadPermit>, AuthorityError> {
    receive_read_permit(state, request).await.map(Json)
}

#[cfg(test)]
mod snapshot_identifier_tests {
    use super::*;
    #[test]
    fn directory_author_device_and_key_owner_fields_share_authenticated_identifier_grammar() {
        for field in [
            "pseudonym_id",
            "sender_pseudonym",
            "recipient_pseudonym_id",
            "sender_pseudonym_id",
            "signer_pseudonym_id",
            "committed_by_pseudonym",
            "member_pseudonym",
        ] {
            for valid in [
                "user-1".to_owned(),
                "Agent_01".to_owned(),
                "a".repeat(64),
                "b".repeat(128),
            ] {
                validate_snapshot_identifiers(&json!({"nested":[{field:valid}]})).unwrap();
            }
            for invalid in [
                "annex:channel:1:secret".to_owned(),
                "".into(),
                "bad space".into(),
                "a".repeat(129),
            ] {
                assert!(
                    validate_snapshot_identifiers(&json!({"nested":[{field:invalid}]})).is_err()
                );
            }
        }
        validate_snapshot_identifiers(&json!({"content":"pseudonym_id: annex:channel:1:secret","authority_identity":"annex-server:hash"})).unwrap();
    }
}
