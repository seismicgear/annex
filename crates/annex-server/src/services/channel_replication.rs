//! Durable origin intents and ordered replica synchronization. A transport write
//! never completes a user action: the authority receipt and its signed ledger
//! must both be verified and committed locally before success is returned.
use super::{
    channel_authority as authority, channel_manifest, federation_delivery, human_federation,
};
use crate::AppState;
use authority::{
    AuthorityBinding, AuthorityError as Error, AuthorityOperation, EventRecord, SignedRequest,
};
use ed25519_dalek::Signer;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, Weak},
    time::Duration,
};
use tokio::sync::{Mutex, OwnedMutexGuard};

const MAX_PENDING: i64 = 256;
const MAX_BATCHES_PER_ATTEMPT: usize = 64;
const BACKOFF_SECONDS: i64 = 3;
const MAX_INTENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reader {
    pub pseudonym_id: String,
    pub device_id: String,
    pub token_epoch: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardResult {
    pub operation_id: String,
    pub accepted_seq: i64,
    pub accepted_hash: String,
    pub event: EventRecord,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachRequest {
    pub peer_id: i64,
    pub authority_identity: String,
    pub device_id: String,
}

pub fn remote_binding(
    conn: &Connection,
    state: &AppState,
    channel: &str,
) -> Result<Option<AuthorityBinding>, Error> {
    match authority::binding(conn, state, channel) {
        Ok(bound) if !bound.local_authority => Ok(Some(bound)),
        Ok(_) | Err(Error::NotFound(_)) => Ok(None),
        Err(error) => Err(error),
    }
}
pub fn reader_for_request(
    conn: &Connection,
    state: &AppState,
    identity: &annex_identity::PlatformIdentity,
    channel: &str,
    device: Option<&str>,
) -> Result<Reader, Error> {
    human_federation::current_local_identity(
        conn,
        state,
        &identity.pseudonym_id,
        identity.token_epoch,
    )?;
    let device = if let Some(device) = device {
        device.to_owned()
    } else {
        conn.query_row("SELECT d.device_id FROM member_device_keys d LEFT JOIN channel_replica_readers r ON r.server_id=d.server_id AND r.pseudonym_id=d.pseudonym_id AND r.device_id=d.device_id AND r.channel_id=?3 WHERE d.server_id=?1 AND d.pseudonym_id=?2 AND d.revoked_at IS NULL AND d.device_certificate IS NOT NULL ORDER BY r.updated_at DESC,d.device_id LIMIT 1",params![state.server_id,identity.pseudonym_id,channel],|r|r.get(0)).optional()?.ok_or_else(||Error::Forbidden("publish a certified device before accessing a remote channel".into()))?
    };
    let reader = Reader {
        pseudonym_id: identity.pseudonym_id.clone(),
        device_id: device,
        token_epoch: identity.token_epoch,
    };
    authorize_reader(conn, state, &reader)?;
    Ok(reader)
}
/// Every replica read gets fresh authority state before consulting its cache.
/// A stopped authority or revoked agreement therefore cannot silently turn a
/// stale replica membership into a new read authorization.
pub async fn refresh_for_request(
    state: Arc<AppState>,
    identity: &annex_identity::PlatformIdentity,
    channel: &str,
    device: Option<&str>,
) -> Result<(), Error> {
    let reader = {
        let conn = state.pool.get().map_err(pool_error)?;
        if remote_binding(&conn, &state, channel)?.is_none() {
            return Ok(());
        }
        reader_for_request(&conn, &state, identity, channel, device)?
    };
    synchronize(state, channel, &reader).await
}
pub async fn maybe_forward(
    state: Arc<AppState>,
    identity: &annex_identity::PlatformIdentity,
    channel: &str,
    device: Option<&str>,
    operation_id: Option<&str>,
    operation: Value,
) -> Result<Option<ForwardResult>, Error> {
    let reader = {
        let conn = state.pool.get().map_err(pool_error)?;
        if remote_binding(&conn, &state, channel)?.is_none() {
            return Ok(None);
        }
        reader_for_request(&conn, &state, identity, channel, device)?
    };
    let id = operation_id.map(str::to_owned).unwrap_or_else(|| {
        authority::digest(&json!([
            "annex-client-control-operation-v1",
            identity.pseudonym_id,
            operation
        ]))
    });
    forward(state, channel, reader, &id, operation)
        .await
        .map(Some)
}
pub fn result_value(
    state: &AppState,
    channel: &str,
    result: &ForwardResult,
) -> Result<Value, Error> {
    let EventRecord::Present(event) = &result.event else {
        return Err(Error::Conflict(
            "the accepted operation was subsequently removed".into(),
        ));
    };
    let mut value = event.payload["result"].clone();
    if !value.is_object() {
        return Err(Error::Internal(
            "authority control operation lacks a result".into(),
        ));
    }
    // The result belongs to its accepted revision. Preserve that snapshot and
    // its authority namespace, while identifying the browser's local server.
    value["state"]["server_identity"] = json!(crate::device_certificates::server_identity(state));
    if event.header.channel_id != channel {
        return Err(Error::Forbidden("authority result channel mismatch".into()));
    }
    Ok(value)
}

/// Ordinary WebSocket actions keep their authenticated session epoch all the
/// way to the durable origin intent. A replica never writes a provisional local
/// message and acknowledges it before the authority accepts it.
pub async fn maybe_message(
    state: Arc<AppState>,
    identity: &annex_identity::PlatformIdentity,
    channel: &str,
    operation_id: Option<&str>,
    mut operation: Value,
    attachment_ids: Option<&[String]>,
) -> Result<Option<annex_channels::Message>, Error> {
    let reader = {
        let mut conn = state.pool.get().map_err(pool_error)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if remote_binding(&tx, &state, channel)?.is_none() {
            return Ok(None);
        }
        let reader = reader_for_request(&tx, &state, identity, channel, None)?;
        super::e2e_service::require_member(&tx, &state, channel, &identity.pseudonym_id)?;
        if let Some(ids) = attachment_ids {
            let existing = operation["message_id"].as_str();
            let descriptors = super::federated_upload_service::descriptors_for_origin_message(
                &tx,
                &state,
                channel,
                &identity.pseudonym_id,
                ids,
                existing,
            )
            .map_err(authority::transfer_error)?;
            operation["attachments"] = serde_json::to_value(descriptors).map_err(pool_error)?;
        }
        tx.commit()?;
        reader
    };
    let operation_id = operation_id
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let result = forward(state.clone(), channel, reader, &operation_id, operation).await?;
    let EventRecord::Present(event) = result.event else {
        return Err(Error::Conflict(
            "message was removed after the authority accepted this operation".into(),
        ));
    };
    let message: annex_channels::Message =
        serde_json::from_value(event.payload["message"].clone()).map_err(pool_error)?;
    if message.channel_id != channel || message.sender_pseudonym != identity.pseudonym_id {
        return Err(Error::Forbidden(
            "authority message differs from authenticated action".into(),
        ));
    }
    Ok(Some(message))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn pool_error(error: impl std::fmt::Display) -> Error {
    Error::Internal(error.to_string())
}
pub(crate) async fn channel_guard(
    state: &AppState,
    channel: &str,
) -> Result<OwnedMutexGuard<()>, Error> {
    type Locks = std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>;
    static LOCKS: OnceLock<Locks> = OnceLock::new();
    let name = format!(
        "{}:{}:{channel}",
        crate::device_certificates::server_identity(state),
        state.server_id
    );
    let lock = {
        let mut locks = LOCKS
            .get_or_init(Default::default)
            .lock()
            .map_err(pool_error)?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&name).and_then(Weak::upgrade) {
            lock
        } else {
            if locks.len() >= 4096 {
                return Err(Error::Unavailable(
                    "too many concurrent channel operations".into(),
                ));
            }
            let lock = Arc::new(Mutex::new(()));
            locks.insert(name, Arc::downgrade(&lock));
            lock
        }
    };
    Ok(lock.lock_owned().await)
}
pub(crate) fn authorize_reader(
    conn: &Connection,
    state: &AppState,
    reader: &Reader,
) -> Result<(), Error> {
    human_federation::current_local_identity(
        conn,
        state,
        &reader.pseudonym_id,
        reader.token_epoch,
    )?;
    let cert = channel_manifest::require_certified_device(
        conn,
        state,
        &reader.pseudonym_id,
        &reader.device_id,
    )?;
    if cert.server_identity != crate::device_certificates::server_identity(state) {
        return Err(Error::Forbidden(
            "replica reader must be a locally registered device".into(),
        ));
    }
    Ok(())
}
pub(crate) fn current_peer(
    conn: &Connection,
    state: &AppState,
    bound: &AuthorityBinding,
) -> Result<i64, Error> {
    let policy: String = conn.query_row(
        "SELECT policy_json FROM servers WHERE id=?1",
        [state.server_id],
        |r| r.get(0),
    )?;
    let policy: annex_types::ServerPolicy = serde_json::from_str(&policy).map_err(pool_error)?;
    if !policy.federation_enabled {
        return Err(Error::Forbidden(
            "federation is disabled by current local policy".into(),
        ));
    }
    let peer = bound
        .authority_peer_id
        .ok_or_else(|| Error::Conflict("channel has no remote authority".into()))?;
    let current:Option<(String,String)>=conn.query_row("SELECT i.base_url,i.public_key FROM instances i JOIN federation_agreements a ON a.remote_instance_id=i.id WHERE i.id=?1 AND i.status='ACTIVE' AND a.local_server_id=?2 AND a.active=1 AND a.transfer_scope<>'NO_TRANSFER'",params![peer,state.server_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (url, key) =
        current.ok_or_else(|| Error::Forbidden("authority agreement was revoked".into()))?;
    if bound.local_authority
        || url.trim_end_matches('/') != bound.authority_url.trim_end_matches('/')
        || key != bound.authority_key_hex
        || authority::server_identity(&key)? != bound.authority_identity
    {
        return Err(Error::Forbidden(
            "authority routing or pinned key changed".into(),
        ));
    }
    Ok(peer)
}
pub(crate) fn signed_request(
    state: &AppState,
    bound: &AuthorityBinding,
    reader: &Reader,
    operation_id: &str,
    operation: Value,
) -> SignedRequest {
    signed_principal_request(state, bound, Some(reader), operation_id, operation)
}

fn signed_principal_request(
    state: &AppState,
    bound: &AuthorityBinding,
    reader: Option<&Reader>,
    operation_id: &str,
    operation: Value,
) -> SignedRequest {
    let mut request = SignedRequest {
        version: 1,
        requesting_identity: crate::device_certificates::server_identity(state),
        originating_server: state.get_public_url().trim_end_matches('/').into(),
        authority_identity: bound.authority_identity.clone(),
        channel_id: bound.channel_id.clone(),
        pseudonym_id: reader.map(|r| r.pseudonym_id.clone()).unwrap_or_default(),
        device_id: reader.map(|r| r.device_id.clone()),
        operation_id: operation_id.into(),
        operation,
        nonce: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        created_at: chrono::Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    request.signature = hex::encode(
        state
            .signing_key
            .sign(authority::canonical_json(&request.signing_value()).as_bytes())
            .to_bytes(),
    );
    request
}
pub(crate) async fn attest_human(
    state: Arc<AppState>,
    bound: &AuthorityBinding,
    reader: &Reader,
) -> Result<(), Error> {
    let (peer, request) = {
        let conn = state.pool.get().map_err(pool_error)?;
        authorize_reader(&conn, &state, reader)?;
        let identity = human_federation::current_local_identity(
            &conn,
            &state,
            &reader.pseudonym_id,
            reader.token_epoch,
        )?;
        if identity.participant_type != annex_types::RoleCode::Human {
            return Ok(());
        }
        (
            current_peer(&conn, &state, bound)?,
            human_federation::build(
                &conn,
                &state,
                &reader.pseudonym_id,
                reader.token_epoch,
                &reader.device_id,
                &bound.channel_id,
                &bound.authority_identity,
            )?,
        )
    };
    let body = serde_json::to_string(&request).map_err(pool_error)?;
    let response = federation_delivery::request_typed(
        state.clone(),
        peer,
        "/api/federation/human-membership",
        &body,
    )
    .await
    .map_err(Error::from)?;
    let receipt: human_federation::MembershipReceipt =
        serde_json::from_str(&response).map_err(pool_error)?;
    receipt.verify(&request, &bound.authority_key_hex)?;
    let conn = state.pool.get().map_err(pool_error)?;
    authorize_reader(&conn, &state, reader)?;
    current_peer(&conn, &state, bound)?;
    Ok(())
}

/// Refresh the requesting server's independently verified human registration
/// before an attachment read. This is also used by the durable blob worker,
/// whose origin cannot sign a membership attestation for its requester.
pub(crate) async fn refresh_attachment_attestation(
    state: Arc<AppState>,
    channel: &str,
    pseudonym: &str,
    device: &str,
) -> Result<(), Error> {
    let (bound, reader) = {
        let conn = state.pool.get().map_err(pool_error)?;
        let identity = annex_identity::get_platform_identity(&conn, state.server_id, pseudonym)
            .map_err(|_| {
                Error::Forbidden("attachment requester membership is unavailable".into())
            })?;
        let reader = Reader {
            pseudonym_id: pseudonym.into(),
            device_id: device.into(),
            token_epoch: identity.token_epoch,
        };
        authorize_reader(&conn, &state, &reader)?;
        let bound = authority::binding(&conn, &state, channel)?;
        (bound, reader)
    };
    if bound.local_authority {
        return Ok(());
    }
    attest_human(state, &bound, &reader).await
}

/// Explicit attachment pins the selected already-agreed peer. Existing local
/// channels cannot be overwritten by a peer's first message or first response.
pub async fn attach(
    state: Arc<AppState>,
    identity: &annex_identity::PlatformIdentity,
    channel: &str,
    input: AttachRequest,
) -> Result<ForwardResult, Error> {
    if channel.is_empty() || channel.len() > 128 || channel.chars().any(char::is_control) {
        return Err(Error::BadRequest("invalid channel identity".into()));
    }
    let reader = Reader {
        pseudonym_id: identity.pseudonym_id.clone(),
        device_id: input.device_id,
        token_epoch: identity.token_epoch,
    };
    {
        let mut conn = state.pool.get().map_err(pool_error)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        authorize_reader(&tx, &state, &reader)?;
        let peer:Option<(String,String)>=tx.query_row("SELECT i.base_url,i.public_key FROM instances i JOIN federation_agreements a ON a.remote_instance_id=i.id WHERE i.id=?1 AND i.status='ACTIVE' AND a.local_server_id=?2 AND a.active=1 AND a.transfer_scope<>'NO_TRANSFER'",params![input.peer_id,state.server_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (url, key) =
            peer.ok_or_else(|| Error::Forbidden("selected peer has no active agreement".into()))?;
        if authority::server_identity(&key)? != input.authority_identity {
            return Err(Error::Forbidden(
                "selected authority fingerprint does not match pinned peer".into(),
            ));
        }
        match authority::binding(&tx, &state, channel) {
            Ok(bound) => {
                if bound.local_authority
                    || bound.authority_identity != input.authority_identity
                    || bound.authority_peer_id != Some(input.peer_id)
                {
                    return Err(Error::Conflict(
                        "channel is already bound to a different authority".into(),
                    ));
                }
            }
            Err(Error::NotFound(_)) => {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM channels WHERE channel_id=?1)",
                    [channel],
                    |r| r.get(0),
                )?;
                if exists {
                    return Err(Error::Conflict(
                        "existing channel requires explicit historical authority migration".into(),
                    ));
                }
                tx.execute("INSERT INTO channel_authorities(server_id,channel_id,authority_identity,authority_key_hex,authority_url,authority_peer_id,local_authority) VALUES(?1,?2,?3,?4,?5,?6,0)",params![state.server_id,channel,input.authority_identity,key,url.trim_end_matches('/'),input.peer_id])?;
            }
            Err(error) => return Err(error),
        }
        tx.commit()?;
    }
    forward(
        state,
        channel,
        reader,
        &uuid::Uuid::new_v4().to_string(),
        json!({"kind":"join"}),
    )
    .await
}

fn enqueue(
    conn: &mut Connection,
    state: &AppState,
    channel: &str,
    reader: &Reader,
    id: &str,
    operation: &Value,
) -> Result<i64, Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    authorize_reader(&tx, state, reader)?;
    let bound = authority::binding(&tx, state, channel)?;
    current_peer(&tx, state, &bound)?;
    if bound.deleted_seq.is_some() {
        return Err(Error::NotFound("authority deleted this channel".into()));
    }
    let parsed: AuthorityOperation =
        serde_json::from_value(operation.clone()).map_err(|e| Error::BadRequest(e.to_string()))?;
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return Err(Error::BadRequest("invalid operation id".into()));
    }
    let canonical = authority::canonical_json(operation);
    if canonical.len() > MAX_INTENT_BYTES {
        return Err(Error::BadRequest(
            "authority operation exceeds size limit".into(),
        ));
    }
    let request = signed_request(state, &bound, reader, id, operation.clone());
    let digest = request.operation_hash();
    let prior:Option<(i64,String)>=tx.query_row("SELECT id,operation_hash FROM channel_authority_outgoing WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3 AND operation_id=?4",params![state.server_id,channel,reader.pseudonym_id,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let row = if let Some((row, prior)) = prior {
        if prior != digest {
            return Err(Error::Conflict(
                "operation id already belongs to another request".into(),
            ));
        }
        row
    } else {
        if !matches!(parsed, AuthorityOperation::Join) {
            super::e2e_service::require_member(&tx, state, channel, &reader.pseudonym_id)?;
        }
        let count:i64=tx.query_row("SELECT COUNT(*) FROM channel_authority_outgoing WHERE server_id=?1 AND status='pending'",[state.server_id],|r|r.get(0))?;
        if count >= MAX_PENDING {
            return Err(Error::Unavailable(
                "authority operation queue is full".into(),
            ));
        }
        let ciphertext = state.message_cipher().encrypt(&canonical)?;
        tx.execute("INSERT INTO channel_authority_outgoing(server_id,channel_id,pseudonym_id,device_id,token_epoch,operation_id,operation_hash,operation_ciphertext,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![state.server_id,channel,reader.pseudonym_id,reader.device_id,reader.token_epoch,id,digest,ciphertext,now()])?;
        tx.last_insert_rowid()
    };
    tx.execute("INSERT INTO channel_replica_readers(server_id,channel_id,pseudonym_id,device_id,token_epoch,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(server_id,channel_id,pseudonym_id,device_id) DO UPDATE SET token_epoch=excluded.token_epoch,revoked_at=NULL,updated_at=excluded.updated_at",params![state.server_id,channel,reader.pseudonym_id,reader.device_id,reader.token_epoch,now()])?;
    tx.commit()?;
    Ok(row)
}

pub async fn forward(
    state: Arc<AppState>,
    channel: &str,
    reader: Reader,
    id: &str,
    operation: Value,
) -> Result<ForwardResult, Error> {
    let _guard = channel_guard(&state, channel).await?;
    let row = {
        let mut conn = state.pool.get().map_err(pool_error)?;
        enqueue(&mut conn, &state, channel, &reader, id, &operation)?
    };
    let result = deliver_locked(state.clone(), row).await;
    if let Err(error) = &result {
        record_delivery_failure(&state, row, error)?;
    }
    result
}

fn record_delivery_failure(state: &AppState, row: i64, error: &Error) -> Result<(), Error> {
    let conn = state.pool.get().map_err(pool_error)?;
    conn.execute("UPDATE channel_authority_outgoing SET attempts=attempts+1,next_attempt_at=?3,last_error=?4,status=CASE WHEN ?5 THEN 'failed' ELSE status END,operation_ciphertext=CASE WHEN ?5 THEN NULL ELSE operation_ciphertext END WHERE server_id=?1 AND id=?2 AND status='pending'",params![state.server_id,row,now()+BACKOFF_SECONDS,error.to_string(),error.is_terminal_refusal()])?;
    Ok(())
}

async fn deliver_locked(state: Arc<AppState>, row: i64) -> Result<ForwardResult, Error> {
    let (bound, reader, id, operation_hash, operation, accepted, departure) = {
        let conn = state.pool.get().map_err(pool_error)?;
        type StoredOutgoingOperation = (
            String,
            String,
            String,
            i64,
            String,
            String,
            Option<String>,
            String,
            Option<i64>,
            Option<String>,
            Option<String>,
        );
        let (channel,pid,device,epoch,id,op_hash,raw,status,seq,hash,departure):StoredOutgoingOperation=
            conn.query_row("SELECT channel_id,pseudonym_id,device_id,token_epoch,operation_id,operation_hash,operation_ciphertext,status,accepted_seq,accepted_hash,departure_receipt_ciphertext FROM channel_authority_outgoing WHERE server_id=?1 AND id=?2",params![state.server_id,row],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?)))?;
        let reader = Reader {
            pseudonym_id: pid,
            device_id: device,
            token_epoch: epoch,
        };
        authorize_reader(&conn, &state, &reader)?;
        let bound = authority::binding(&conn, &state, &channel)?;
        current_peer(&conn, &state, &bound)?;
        let accepted = if status == "accepted" {
            Some((
                seq.ok_or_else(|| Error::Internal("accepted operation lacks sequence".into()))?,
                hash.ok_or_else(|| Error::Internal("accepted operation lacks hash".into()))?,
            ))
        } else {
            None
        };
        if status != "pending" && status != "accepted" {
            return Err(Error::Forbidden(
                "authority operation was cancelled or refused".into(),
            ));
        }
        let operation = raw
            .map(|raw| state.message_cipher().decrypt(&raw))
            .transpose()?
            .map(|raw| serde_json::from_str(&raw))
            .transpose()
            .map_err(pool_error)?;
        let departure = departure
            .map(|raw| state.message_cipher().decrypt(&raw))
            .transpose()?
            .map(|raw| serde_json::from_str::<authority::OperationReceipt>(&raw))
            .transpose()
            .map_err(pool_error)?;
        (bound, reader, id, op_hash, operation, accepted, departure)
    };
    if let Some(receipt) = departure {
        return finish_departure(state, &bound, &reader, id, &operation_hash, receipt).await;
    }
    let (seq, hash) = if let Some(accepted) = accepted {
        accepted
    } else {
        attest_human(state.clone(), &bound, &reader).await?;
        let operation = operation
            .ok_or_else(|| Error::Internal("pending authority operation has no body".into()))?;
        let is_departure = authority::is_own_departure(&operation, &reader.pseudonym_id);
        let request = signed_request(&state, &bound, &reader, &id, operation);
        let body = serde_json::to_string(&request).map_err(pool_error)?;
        let response = federation_delivery::request_typed(
            state.clone(),
            bound.authority_peer_id.unwrap(),
            "/api/federation/channel-authority/operations",
            &body,
        )
        .await
        .map_err(Error::from)?;
        let receipt: authority::OperationReceipt =
            serde_json::from_str(&response).map_err(pool_error)?;
        receipt.verify(&bound, &request)?;
        {
            let mut conn = state.pool.get().map_err(pool_error)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            authorize_reader(&tx, &state, &reader)?;
            current_peer(&tx, &state, &bound)?;
            let departure = if is_departure {
                Some(
                    state
                        .message_cipher()
                        .encrypt(&serde_json::to_string(&receipt).map_err(pool_error)?)?,
                )
            } else {
                None
            };
            tx.execute("UPDATE channel_authority_outgoing SET status='accepted',operation_ciphertext=NULL,accepted_seq=?3,accepted_hash=?4,departure_receipt_ciphertext=?5,last_error=NULL WHERE server_id=?1 AND id=?2 AND status='pending'",params![state.server_id,row,receipt.accepted_seq,receipt.accepted_hash,departure])?;
            tx.commit()?;
        }
        if is_departure {
            return finish_departure(state, &bound, &reader, id, &operation_hash, receipt).await;
        }
        (receipt.accepted_seq, receipt.accepted_hash)
    };
    sync_locked(state.clone(), &bound.channel_id, &reader, Some(seq)).await?;
    let conn = state.pool.get().map_err(pool_error)?;
    let event = authority::event_record(&conn, &state, &bound.channel_id, seq)?;
    if event.hash() != hash {
        return Err(Error::Forbidden(
            "accepted operation differs from ordered authority ledger".into(),
        ));
    }
    Ok(ForwardResult {
        operation_id: id,
        accepted_seq: seq,
        accepted_hash: hash,
        event,
    })
}

/// A signed self-departure receipt is sufficient to remove this local reader.
/// It grants no post-departure catch-up and never advances the ordered cursor
/// over unseen events. The exact control receipt is retained for crash retries.
async fn finish_departure(
    state: Arc<AppState>,
    bound: &AuthorityBinding,
    reader: &Reader,
    id: String,
    operation_hash: &str,
    receipt: authority::OperationReceipt,
) -> Result<ForwardResult, Error> {
    receipt.verify_signature(bound)?;
    if receipt.operation_hash != operation_hash {
        return Err(Error::Forbidden(
            "stored departure receipt differs from intent".into(),
        ));
    }
    let EventRecord::Present(event) = &receipt.record else {
        return Err(Error::Forbidden("departure receipt was removed".into()));
    };
    let own = match event.header.event_kind {
        authority::EventKind::StateChanged => {
            event.payload["action"] == "member_left"
                && event.payload["pseudonym_id"] == reader.pseudonym_id
        }
        authority::EventKind::ManifestChanged => serde_json::from_value::<
            channel_manifest::SignedManifestEvent,
        >(event.payload["manifest_event"].clone())
        .ok()
        .as_ref()
        .is_some_and(|e| channel_manifest::removed_member(e) == Some(reader.pseudonym_id.as_str())),
        _ => false,
    };
    if !own {
        return Err(Error::Forbidden(
            "receipt is not this reader's accepted departure".into(),
        ));
    }
    {
        let mut conn = state.pool.get().map_err(pool_error)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        authorize_reader(&tx, &state, reader)?;
        current_peer(&tx, &state, bound)?;
        annex_channels::remove_member(
            &tx,
            state.server_id,
            &bound.channel_id,
            &reader.pseudonym_id,
        )
        .map_err(pool_error)?;
        tx.execute("UPDATE channel_replica_readers SET revoked_at=?4,last_error='accepted self departure' WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3",params![state.server_id,bound.channel_id,reader.pseudonym_id,now()])?;
        tx.execute("UPDATE channel_authority_outgoing SET status='cancelled',operation_ciphertext=NULL,last_error='accepted self departure' WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3 AND status='pending'",params![state.server_id,bound.channel_id,reader.pseudonym_id])?;
        tx.commit()?;
    }
    state.connection_manager.send(&reader.pseudonym_id,json!({"type":"channel_membership_changed","channelId":bound.channel_id,"pseudonymId":reader.pseudonym_id,"member":false}).to_string()).await;
    state
        .connection_manager
        .unsubscribe(&bound.channel_id, &reader.pseudonym_id)
        .await;
    super::ChannelService::new(state.clone())
        .end_channel_participation(&reader.pseudonym_id, &bound.channel_id)
        .await;
    Ok(ForwardResult {
        operation_id: id,
        accepted_seq: receipt.accepted_seq,
        accepted_hash: receipt.accepted_hash,
        event: receipt.record,
    })
}

async fn sync_locked(
    state: Arc<AppState>,
    channel: &str,
    reader: &Reader,
    through: Option<i64>,
) -> Result<(), Error> {
    for _ in 0..MAX_BATCHES_PER_ATTEMPT {
        let bound = {
            let conn = state.pool.get().map_err(pool_error)?;
            authorize_reader(&conn, &state, reader)?;
            let bound = authority::binding(&conn, &state, channel)?;
            current_peer(&conn, &state, &bound)?;
            bound
        };
        if bound.deleted_seq.is_some() || through.is_some_and(|seq| bound.head_seq >= seq) {
            return Ok(());
        }
        // A signed cleanup receipt must remain obtainable after remote access
        // is revoked. Failure to refresh membership never grants history: the
        // authority returns only a nonce-bound revocation receipt in that case.
        if let Err(error) = attest_human(state.clone(), &bound, reader).await {
            tracing::debug!(%error,"human membership refresh did not authorize history");
        }
        let request = signed_request(
            &state,
            &bound,
            reader,
            &uuid::Uuid::new_v4().to_string(),
            json!({"type":"catch_up","after_seq":bound.head_seq,"limit":64}),
        );
        let body = serde_json::to_string(&request).map_err(pool_error)?;
        let raw = federation_delivery::request_typed(
            state.clone(),
            bound.authority_peer_id.unwrap(),
            "/api/federation/channel-authority/catch-up",
            &body,
        )
        .await
        .map_err(Error::from)?;
        let batch: authority::CatchupBatch = serde_json::from_str(&raw).map_err(pool_error)?;
        {
            let mut conn = state.pool.get().map_err(pool_error)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            authorize_reader(&tx, &state, reader)?;
            current_peer(&tx, &state, &bound)?;
            authority::apply_batch(
                &tx,
                &state,
                &request,
                &batch,
                &reader.pseudonym_id,
                &reader.device_id,
            )?;
            tx.execute("UPDATE channel_replica_readers SET next_attempt_at=?5,last_error=NULL,updated_at=?6 WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3 AND device_id=?4",params![state.server_id,channel,reader.pseudonym_id,reader.device_id,now()+BACKOFF_SECONDS,now()])?;
            tx.commit()?;
        }
        authority::notify_applied_records(state.clone(), &batch.records).await?;
        if batch.access_revoked {
            state.connection_manager.send(&reader.pseudonym_id,json!({"type":"channel_membership_changed","channelId":channel,"pseudonymId":reader.pseudonym_id,"member":false}).to_string()).await;
            state
                .connection_manager
                .unsubscribe(channel, &reader.pseudonym_id)
                .await;
            super::ChannelService::new(state.clone())
                .end_channel_participation(&reader.pseudonym_id, channel)
                .await;
            return Err(Error::Forbidden("authority revoked channel access".into()));
        }
        if !batch.more {
            if through.is_some_and(|seq| seq > batch.head_seq) {
                return Err(Error::Conflict(
                    "authority receipt is beyond its signed current head".into(),
                ));
            }
            return Ok(());
        }
    }
    Err(Error::Unavailable(
        "channel synchronization continues in the background".into(),
    ))
}

pub async fn synchronize(
    state: Arc<AppState>,
    channel: &str,
    reader: &Reader,
) -> Result<(), Error> {
    let _guard = channel_guard(&state, channel).await?;
    sync_locked(state, channel, reader, None).await
}

/// Complete an admitted replica's deletion obligation without restoring any
/// departed identity, device, membership or entitlement to retained history.
pub async fn cleanup_replica(state: Arc<AppState>, channel: &str) -> Result<bool, Error> {
    let _guard = channel_guard(&state, channel).await?;
    for _ in 0..MAX_BATCHES_PER_ATTEMPT {
        let bound = {
            let conn = state.pool.get().map_err(pool_error)?;
            let bound = authority::binding(&conn, &state, channel)?;
            if bound.local_authority {
                return Err(Error::Forbidden(
                    "cleanup is only for remote replicas".into(),
                ));
            }
            if bound.deleted_seq.is_some() {
                return Ok(true);
            }
            current_peer(&conn, &state, &bound)?;
            bound
        };
        let request = signed_principal_request(
            &state,
            &bound,
            None,
            &uuid::Uuid::new_v4().to_string(),
            json!({"type":"cleanup","after_seq":bound.head_seq,"limit":64}),
        );
        let raw = federation_delivery::request_typed(
            state.clone(),
            bound.authority_peer_id.unwrap(),
            "/api/federation/channel-authority/catch-up",
            &serde_json::to_string(&request).map_err(pool_error)?,
        )
        .await
        .map_err(Error::from)?;
        let batch: authority::CatchupBatch = serde_json::from_str(&raw).map_err(pool_error)?;
        {
            let mut conn = state.pool.get().map_err(pool_error)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            current_peer(&tx, &state, &bound)?;
            authority::apply_batch(&tx, &state, &request, &batch, "", "")?;
            tx.commit()?;
        }
        authority::notify_applied_records(state.clone(), &batch.records).await?;
        if batch.access_revoked {
            return Ok(false);
        }
        if !batch.more {
            let conn = state.pool.get().map_err(pool_error)?;
            return Ok(authority::binding(&conn, &state, channel)?
                .deleted_seq
                .is_some());
        }
    }
    Err(Error::Unavailable(
        "deletion cleanup continues in the background".into(),
    ))
}

async fn cleanup_departed_replicas(state: Arc<AppState>) -> Result<(), Error> {
    let channels = {
        let conn = state.pool.get().map_err(pool_error)?;
        let mut stmt = conn.prepare("SELECT a.channel_id FROM channel_authorities a JOIN channel_replica_readers r ON r.server_id=a.server_id AND r.channel_id=a.channel_id WHERE a.server_id=?1 AND a.local_authority=0 AND a.deleted_seq IS NULL AND NOT EXISTS(SELECT 1 FROM channel_replica_readers live WHERE live.server_id=a.server_id AND live.channel_id=a.channel_id AND live.revoked_at IS NULL) GROUP BY a.channel_id HAVING MIN(r.next_attempt_at)<=?2 ORDER BY MIN(r.next_attempt_at),a.channel_id LIMIT 32")?;
        let rows = stmt
            .query_map(params![state.server_id, now()], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let mut tasks = tokio::task::JoinSet::new();
    for channel in channels {
        while tasks.len() >= 4 {
            tokio::select! { _=state.shutdown.cancelled()=>{tasks.abort_all();return Ok(());}, _=tasks.join_next()=>{} }
        }
        let state = state.clone();
        tasks.spawn(async move {
            let result = cleanup_replica(state.clone(), &channel).await;
            if let Ok(conn) = state.pool.get() {
                let _ = conn.execute("UPDATE channel_replica_readers SET next_attempt_at=?3 WHERE server_id=?1 AND channel_id=?2 AND revoked_at IS NOT NULL",params![state.server_id,channel,now()+30]);
            }
            if let Err(error) = result { tracing::debug!(%channel,%error,"replica deletion cleanup deferred"); }
        });
    }
    while !tasks.is_empty() {
        tokio::select! { _=state.shutdown.cancelled()=>{tasks.abort_all();return Ok(());}, _=tasks.join_next()=>{} }
    }
    Ok(())
}

async fn run_channel(state: Arc<AppState>, channel: String, reader: Reader) -> Result<(), Error> {
    let _guard = channel_guard(&state, &channel).await?;
    let rows = {
        let conn = state.pool.get().map_err(pool_error)?;
        let mut stmt=conn.prepare("SELECT id FROM channel_authority_outgoing WHERE server_id=?1 AND channel_id=?2 AND status='pending' AND next_attempt_at<=?3 ORDER BY id LIMIT 8")?;
        let rows = stmt
            .query_map(params![state.server_id, channel, now()], |r| {
                r.get::<_, i64>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for row in rows {
        if let Err(error) = deliver_locked(state.clone(), row).await {
            record_delivery_failure(&state, row, &error)?;
        }
    }
    sync_locked(state, &channel, &reader, None).await
}
pub async fn start_sync_task(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(3));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cleanup_ticker = tokio::time::interval(Duration::from_secs(30));
    cleanup_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Departed peers can be offline. Their bounded cleanup attempts must not
    // stall synchronization for channels that still have active readers.
    let mut cleanup_jobs = tokio::task::JoinSet::new();
    loop {
        let cleanup = tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = ticker.tick() => false,
            _ = cleanup_ticker.tick() => true,
            _ = cleanup_jobs.join_next(), if !cleanup_jobs.is_empty() => continue,
        };
        if cleanup {
            if cleanup_jobs.is_empty() {
                let state = state.clone();
                cleanup_jobs.spawn(async move {
                    if let Err(error) = cleanup_departed_replicas(state).await {
                        tracing::warn!(%error,"replica deletion cleanup scan failed");
                    }
                });
            }
            continue;
        }
        if let Err(error) = super::authority_reconciliation::drain_departures(state.clone()).await {
            tracing::warn!(%error,"reconciliation departure cleanup failed");
        }
        let readers = (|| -> Result<Vec<(String, Reader)>, Error> {
            let conn = state.pool.get().map_err(pool_error)?;
            super::authority_reconciliation::expire_staging(&conn, &state)?;
            let mut stmt=conn.prepare("SELECT r.channel_id,r.pseudonym_id,r.device_id,r.token_epoch FROM channel_replica_readers r JOIN channel_authorities a ON a.server_id=r.server_id AND a.channel_id=r.channel_id WHERE r.server_id=?1 AND r.revoked_at IS NULL AND r.next_attempt_at<=?2 AND a.local_authority=0 AND a.deleted_seq IS NULL ORDER BY r.next_attempt_at,r.updated_at LIMIT 32")?;
            let rows = stmt
                .query_map(params![state.server_id, now()], |r| {
                    Ok((
                        r.get(0)?,
                        Reader {
                            pseudonym_id: r.get(1)?,
                            device_id: r.get(2)?,
                            token_epoch: r.get(3)?,
                        },
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })();
        let readers = match readers {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error,"authority synchronization scan failed");
                continue;
            }
        };
        let mut tasks = tokio::task::JoinSet::new();
        for (channel, reader) in readers {
            while tasks.len() >= 4 {
                tokio::select! {_=state.shutdown.cancelled()=>{tasks.abort_all();return;},_=tasks.join_next()=>{}}
            }
            let state = state.clone();
            tasks.spawn(async move{
                if let Err(error)=run_channel(state.clone(),channel.clone(),reader.clone()).await{
                    let terminal=matches!(error,Error::Forbidden(_));
                    if let Ok(conn)=state.pool.get(){let _=conn.execute("UPDATE channel_replica_readers SET next_attempt_at=?5,last_error=?6,revoked_at=CASE WHEN ?7 THEN ?8 ELSE revoked_at END WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3 AND device_id=?4",params![state.server_id,channel,reader.pseudonym_id,reader.device_id,now()+BACKOFF_SECONDS,error.to_string(),terminal,now()]);}
                    tracing::debug!(%channel,%error,"authority synchronization deferred");
                }
            });
        }
        while !tasks.is_empty() {
            tokio::select! {_=state.shutdown.cancelled()=>{tasks.abort_all();return;},_=tasks.join_next()=>{}}
        }
    }
}

pub async fn http_attach(
    axum::Extension(state): axum::Extension<Arc<AppState>>,
    axum::Extension(crate::middleware::IdentityContext(identity)): axum::Extension<
        crate::middleware::IdentityContext,
    >,
    axum::extract::Path(channel): axum::extract::Path<String>,
    axum::Json(input): axum::Json<AttachRequest>,
) -> Result<axum::Json<ForwardResult>, Error> {
    attach(state, &identity, &channel, input)
        .await
        .map(axum::Json)
}
