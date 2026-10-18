//! Restart-safe, authority-gated ciphertext transfer. This module never accepts
//! an arbitrary path or substitutes local replica membership for authority.

use super::{federated_upload_types as wire, federation_repository as peers, upload_service};
use crate::AppState;
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;
use wire::{
    AcceptedAttachment, ChunkOutcome, ChunkRequest, ChunkResponse, ReadIntent, ReadPurpose,
};

const MAX_FAILURES: i64 = 8;
const LEASE_SECONDS: i64 = 120;
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Integrity(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Remote(annex_federation::transport::ApplicationError),
}
impl TransferError {
    pub fn status_code(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode as S;
        match self {
            Self::BadRequest(_) => S::BAD_REQUEST,
            Self::Unauthorized(_) => S::UNAUTHORIZED,
            Self::Forbidden(_) => S::FORBIDDEN,
            Self::NotFound(_) => S::NOT_FOUND,
            Self::Conflict(_) | Self::Integrity(_) => S::CONFLICT,
            Self::Unavailable(_) => S::SERVICE_UNAVAILABLE,
            Self::Internal(_) => S::INTERNAL_SERVER_ERROR,
            Self::Remote(error) => S::from_u16(error.status).unwrap_or(S::BAD_GATEWAY),
        }
    }
}
impl From<wire::WireError> for TransferError {
    fn from(e: wire::WireError) -> Self {
        match e {
            wire::WireError::Authentication => Self::Unauthorized(e.to_string()),
            _ => Self::BadRequest(e.to_string()),
        }
    }
}
impl From<rusqlite::Error> for TransferError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(format!("attachment database: {e}"))
    }
}
impl From<std::io::Error> for TransferError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(format!("attachment filesystem: {e}"))
    }
}
impl From<serde_json::Error> for TransferError {
    fn from(_: serde_json::Error) -> Self {
        Self::BadRequest("invalid attachment metadata".into())
    }
}
impl From<upload_service::UploadError> for TransferError {
    fn from(e: upload_service::UploadError) -> Self {
        use upload_service::UploadError as E;
        match e {
            E::BadRequest(e) => Self::BadRequest(e),
            E::Unauthorized(e) => Self::Unauthorized(e),
            E::Forbidden(e) => Self::Forbidden(e),
            E::NotFound(e) => Self::NotFound(e),
            E::Conflict(e) => Self::Conflict(e),
            E::Unavailable(e) => Self::Unavailable(e),
            E::Internal(e) => Self::Internal(e),
            E::Remote(error) => Self::Remote(error),
        }
    }
}
impl From<TransferError> for crate::api::ApiError {
    fn from(e: TransferError) -> Self {
        match e {
            TransferError::BadRequest(e) => Self::BadRequest(e),
            TransferError::Unauthorized(e) => Self::Unauthorized(e),
            TransferError::Forbidden(e) => Self::Forbidden(e),
            TransferError::NotFound(e) => Self::NotFound(e),
            TransferError::Conflict(e) | TransferError::Integrity(e) => Self::Conflict(e),
            TransferError::Unavailable(e) => Self::ServiceUnavailable(e),
            TransferError::Internal(e) => Self::InternalServerError(e),
            TransferError::Remote(error) => Self::Remote(error),
        }
    }
}

fn local_identity(state: &AppState) -> String {
    wire::stable_identity(&state.signing_key.verifying_key().to_bytes())
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn pool_error(e: impl std::fmt::Display) -> TransferError {
    TransferError::Internal(e.to_string())
}
fn authority_error(e: super::channel_authority::AuthorityError) -> TransferError {
    use super::channel_authority::AuthorityError as E;
    match e {
        E::BadRequest(e) => TransferError::BadRequest(e),
        E::Forbidden(e) => TransferError::Forbidden(e),
        E::NotFound(e) => TransferError::NotFound(e),
        E::Conflict(e) => TransferError::Conflict(e),
        E::Unavailable(e) => TransferError::Unavailable(e),
        E::Internal(e) => TransferError::Internal(e),
        E::Remote(error) => TransferError::Remote(error),
    }
}
fn require_transaction(conn: &Connection) -> Result<(), TransferError> {
    if conn.is_autocommit() {
        return Err(TransferError::Internal(
            "attachment event changes require the caller's transaction".into(),
        ));
    }
    Ok(())
}

fn replica_paths(upload_id: &str) -> (String, String) {
    let generation = uuid::Uuid::new_v4();
    (
        format!("federated/{upload_id}/{generation}.part"),
        format!("federated/{upload_id}/{generation}.bin"),
    )
}

/// Resolve routing only through a current trusted peer and its pinned key.
/// A descriptor URL is never itself permission to open a network connection.
fn peer_for(
    conn: &Connection,
    state: &AppState,
    identity: &str,
    url: &str,
) -> Result<(i64, String), TransferError> {
    let peer = peers::find_instance_by_base_url(conn, url)?
        .ok_or_else(|| TransferError::Forbidden("unknown attachment peer".into()))?;
    let key: [u8; 32] = hex::decode(&peer.public_key_hex)
        .map_err(|_| TransferError::Forbidden("invalid pinned peer key".into()))?
        .try_into()
        .map_err(|_| TransferError::Forbidden("invalid pinned peer key".into()))?;
    let scope = peers::active_agreement_transfer_scope(conn, state.server_id, peer.id)?;
    if peer.status != "ACTIVE"
        || scope.is_none()
        || scope.as_deref() == Some("NO_TRANSFER")
        || wire::stable_identity(&key) != identity
    {
        return Err(TransferError::Forbidden(
            "attachment peer has no current transfer agreement".into(),
        ));
    }
    Ok((peer.id, peer.public_key_hex))
}

fn require_authority_agreement(
    conn: &Connection,
    state: &AppState,
    channel: &str,
) -> Result<String, TransferError> {
    let bound = super::channel_authority::binding(conn, state, channel).map_err(authority_error)?;
    if bound.deleted_seq.is_some() {
        return Err(TransferError::Forbidden(
            "channel authority has been deleted".into(),
        ));
    }
    if bound.local_authority {
        if bound.authority_identity != local_identity(state) {
            return Err(TransferError::Forbidden(
                "local authority identity is inconsistent".into(),
            ));
        }
    } else {
        let (id, key) = peer_for(
            conn,
            state,
            &bound.authority_identity,
            bound.authority_url.trim_end_matches('/'),
        )?;
        if Some(id) != bound.authority_peer_id || key != bound.authority_key_hex {
            return Err(TransferError::Forbidden(
                "authority peer pin changed".into(),
            ));
        }
    }
    super::channel_authority::pinned_authority_key(conn, state, channel).map_err(authority_error)
}

/// Resolve only committed local-origin metadata. Absence of encryption fields
/// is accepted only with the explicit ordinary descriptor type; it cannot
/// silently downgrade an opaque upload or assign a foreign origin to it.
fn local_origin_descriptor(
    conn: &Connection,
    state: &AppState,
    id: &str,
) -> Result<Option<(String, String, wire::AttachmentDescriptor)>, TransferError> {
    type Row = (
        String,
        String,
        u64,
        String,
        String,
        bool,
        Option<u64>,
        Option<String>,
        String,
        String,
    );
    let row: Option<Row> = conn.query_row(
        "SELECT channel_id,uploader_pseudonym,size_bytes,ciphertext_sha256,category,encrypted,epoch,key_id,original_filename,content_type FROM uploads WHERE server_id=?1 AND upload_id=?2 AND purpose='chat' AND lifecycle='ready'",
        params![state.server_id,id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?)),
    ).optional()?;
    let Some((channel, owner, size, hash, category, encrypted, epoch, key, filename, content_type)) =
        row
    else {
        return Ok(None);
    };
    let (epoch, key_id, plaintext) = if encrypted {
        (
            epoch.ok_or_else(|| {
                TransferError::Conflict("encrypted origin upload has no epoch".into())
            })?,
            key.ok_or_else(|| {
                TransferError::Conflict("encrypted origin upload has no key identity".into())
            })?,
            None,
        )
    } else {
        if epoch.is_some() || key.is_some() {
            return Err(TransferError::Conflict(
                "ordinary upload contains encryption metadata".into(),
            ));
        }
        (
            0,
            String::new(),
            Some(wire::PlainAttachmentMetadata {
                filename,
                content_type,
            }),
        )
    };
    let descriptor = wire::AttachmentDescriptor {
        attachment_id: id.into(),
        origin_identity: local_identity(state),
        originating_server: state.get_public_url().trim_end_matches('/').into(),
        size_bytes: size,
        ciphertext_sha256: hash,
        category,
        epoch,
        key_id,
        plaintext,
    };
    descriptor.validate()?;
    Ok(Some((channel, owner, descriptor)))
}

/// The authenticated origin calls this before signing a forwarded send/edit.
/// A URL and digest supplied by a browser are not evidence that it owns a file.
/// Authority validation separately checks the currently accepted epoch/policy.
pub fn validate_origin_descriptor(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    uploader: &str,
    d: &wire::AttachmentDescriptor,
) -> Result<(), TransferError> {
    d.validate()?;
    if d.origin_identity != local_identity(state)
        || d.originating_server != state.get_public_url().trim_end_matches('/')
    {
        return Err(TransferError::Forbidden(
            "new attachment is not owned by this origin".into(),
        ));
    }
    super::channel_service::check_publish_permission(
        conn,
        state.server_id,
        uploader,
        channel,
        super::agent_policy::AgentAction::SendText,
    )
    .map_err(|e| TransferError::Forbidden(e.to_string()))?;
    let Some((actual_channel, owner, actual)) =
        local_origin_descriptor(conn, state, &d.attachment_id)?
    else {
        return Err(TransferError::NotFound(
            "committed origin upload not found".into(),
        ));
    };
    if actual_channel != channel || owner != uploader || actual != *d {
        return Err(TransferError::Conflict(
            "origin upload ownership or immutable metadata does not match".into(),
        ));
    }
    Ok(())
}

/// Derive the public, immutable descriptor from committed origin metadata. A
/// caller cannot promote an arbitrary browser URL or filename into a signed
/// authority event. The caller's writer transaction is also the send/edit
/// transaction; a retained descriptor stays bound to its original message.
pub(crate) fn descriptors_for_origin_message(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pseudonym: &str,
    attachment_ids: &[String],
    existing_message: Option<&str>,
) -> Result<Vec<wire::AttachmentDescriptor>, TransferError> {
    require_transaction(conn)?;
    if attachment_ids.len() > 16 {
        return Err(TransferError::BadRequest(
            "too many message attachments".into(),
        ));
    }
    let mut unique = std::collections::HashSet::new();
    for id in attachment_ids {
        if !wire::canonical_uuid(id) || !unique.insert(id.as_str()) {
            return Err(TransferError::BadRequest(
                "attachment IDs must be unique canonical UUIDs".into(),
            ));
        }
    }
    if let Some(message) = existing_message {
        let owns: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE server_id=?1 AND channel_id=?2 AND message_id=?3 AND sender_pseudonym=?4 AND deleted_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>datetime('now')))",
            params![state.server_id, channel, message, pseudonym], |r| r.get(0),
        )?;
        if !owns {
            return Err(TransferError::Forbidden(
                "attachment edit requires the author's live message in this channel".into(),
            ));
        }
    }
    if !attachment_ids.is_empty() {
        super::channel_service::check_publish_permission(
            conn,
            state.server_id,
            pseudonym,
            channel,
            super::agent_policy::AgentAction::SendText,
        )
        .map_err(|error| TransferError::Forbidden(error.to_string()))?;
    }
    let mut descriptors = Vec::with_capacity(attachment_ids.len());
    for id in attachment_ids {
        // Include removed rows in these lookups: their tombstones must never
        // fall through to a new-upload path and resurrect the same UUID.
        let bound: Option<(String, Option<String>, String, String)> = conn.query_row(
            "SELECT descriptor_json,removed_reason,channel_id,message_id FROM channel_authority_attachment_bindings WHERE server_id=?1 AND attachment_id=?2",
            params![state.server_id, id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        ).optional()?;
        let origin: Option<(String, Option<String>)> = conn.query_row(
            "SELECT accepted_json,removed_reason FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2",
            params![state.server_id, id], |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        let mut retained: Option<wire::AttachmentDescriptor> = None;
        if let Some((encoded, removed, prior_channel, prior_message)) = bound {
            if removed.is_some()
                || prior_channel != channel
                || Some(prior_message.as_str()) != existing_message
            {
                return Err(TransferError::Conflict(
                    "attachment is removed or belongs to another message".into(),
                ));
            }
            let descriptor: wire::AttachmentDescriptor = serde_json::from_str(&encoded)?;
            descriptor.validate()?;
            if descriptor.attachment_id != *id {
                return Err(TransferError::Conflict(
                    "stored attachment binding is inconsistent".into(),
                ));
            }
            retained = Some(descriptor);
        }
        if let Some((encoded, removed)) = origin {
            let accepted: AcceptedAttachment = serde_json::from_str(&encoded)?;
            accepted.validate()?;
            if removed.is_some()
                || accepted.channel_id != channel
                || Some(accepted.message_id.as_str()) != existing_message
                || accepted.descriptor.attachment_id != *id
                || retained
                    .as_ref()
                    .is_some_and(|prior| *prior != accepted.descriptor)
            {
                return Err(TransferError::Conflict(
                    "origin attachment is removed or has a different immutable binding".into(),
                ));
            }
            retained = Some(accepted.descriptor);
        }
        if let Some(descriptor) = retained {
            descriptors.push(descriptor);
            continue;
        }
        let reused: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM message_uploads WHERE server_id=?1 AND upload_id=?2 AND (?3 IS NULL OR message_id<>?3) UNION ALL SELECT 1 FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2)",
            params![state.server_id,id,existing_message], |r|r.get(0),
        )?;
        if reused {
            return Err(TransferError::Conflict(
                "attachment is already bound to another message or has no accepted descriptor"
                    .into(),
            ));
        }
        let (actual_channel, owner, descriptor) = local_origin_descriptor(conn, state, id)?
            .ok_or_else(|| TransferError::NotFound("ready local attachment not found".into()))?;
        if actual_channel != channel || owner != pseudonym {
            return Err(TransferError::Forbidden(
                "attachment ownership or channel does not match".into(),
            ));
        }
        validate_origin_descriptor(conn, state, channel, pseudonym, &descriptor)?;
        descriptors.push(descriptor);
    }
    Ok(descriptors)
}

/// Adopt retained attachments only from actual message associations. This is
/// historical import, so a departed author's current publication rights do not
/// erase their retained history. Existing immutable origin/epoch/AAD metadata
/// is preserved; URLs embedded in message text never become blob evidence.
pub(crate) fn descriptors_for_bootstrap_message(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    message: &str,
) -> Result<Vec<wire::AttachmentDescriptor>, TransferError> {
    require_transaction(conn)?;
    let author:Option<String>=conn.query_row("SELECT sender_pseudonym FROM messages WHERE server_id=?1 AND channel_id=?2 AND message_id=?3 AND deleted_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>datetime('now'))",
        params![state.server_id,channel,message],|r|r.get(0)).optional()?;
    let author = author.ok_or_else(|| {
        TransferError::NotFound("retained attachment message is absent or deleted".into())
    })?;
    let mut statement=conn.prepare("SELECT upload_id FROM message_uploads WHERE server_id=?1 AND message_id=?2 ORDER BY upload_id")?;
    let ids = statement
        .query_map(params![state.server_id, message], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if ids.len() > 16 {
        return Err(TransferError::Conflict(
            "retained message has too many attachments".into(),
        ));
    }
    let mut descriptors = Vec::with_capacity(ids.len());
    for id in ids {
        if !wire::canonical_uuid(&id) {
            return Err(TransferError::Conflict(
                "retained attachment ID is not canonical".into(),
            ));
        }
        let mut retained: Option<wire::AttachmentDescriptor> = None;
        let binding:Option<(String,String,String,Option<String>)>=conn.query_row(
            "SELECT channel_id,message_id,descriptor_json,removed_reason FROM channel_authority_attachment_bindings WHERE server_id=?1 AND attachment_id=?2",
            params![state.server_id,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if let Some((bound_channel, bound_message, encoded, removed)) = binding {
            if bound_channel != channel || bound_message != message || removed.is_some() {
                return Err(TransferError::Conflict(
                    "retained attachment has a removed or different message binding".into(),
                ));
            }
            let descriptor: wire::AttachmentDescriptor = serde_json::from_str(&encoded)?;
            descriptor.validate()?;
            retained = Some(descriptor);
        }
        let mut refs=conn.prepare("SELECT accepted_json,removed_reason IS NOT NULL FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2 UNION ALL SELECT accepted_json,status='removed' FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2")?;
        let references = refs
            .query_map(params![state.server_id, id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(refs);
        for (encoded, removed) in references {
            let accepted: AcceptedAttachment = serde_json::from_str(&encoded)?;
            accepted.validate()?;
            if removed
                || accepted.channel_id != channel
                || accepted.message_id != message
                || retained
                    .as_ref()
                    .is_some_and(|prior| *prior != accepted.descriptor)
            {
                return Err(TransferError::Conflict(
                    "retained origin/replica reference is removed or inconsistent".into(),
                ));
            }
            retained = Some(accepted.descriptor);
        }
        let local = local_origin_descriptor(conn, state, &id)?;
        let descriptor = match (retained, local) {
            (Some(prior), Some((actual_channel, owner, actual))) => {
                if actual_channel != channel || owner != author || prior != actual {
                    return Err(TransferError::Conflict(
                        "retained local attachment metadata changed".into(),
                    ));
                }
                prior
            }
            (Some(prior), None) if prior.origin_identity != local_identity(state) => prior,
            (None, Some((actual_channel, owner, actual))) => {
                if actual_channel != channel || owner != author {
                    return Err(TransferError::Conflict(
                        "historical local attachment belongs to a different author or channel"
                            .into(),
                    ));
                }
                actual
            }
            _ => return Err(TransferError::Conflict(
                "retained attachment lacks verified origin metadata; legacy URLs cannot be adopted"
                    .into(),
            )),
        };
        if descriptor.attachment_id != id {
            return Err(TransferError::Conflict(
                "retained attachment UUID does not match its association".into(),
            ));
        }
        let reused:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM message_uploads WHERE server_id=?1 AND upload_id=?2 AND message_id<>?3)",params![state.server_id,id,message],|r|r.get(0))?;
        if reused {
            return Err(TransferError::Conflict(
                "retained attachment is shared across different messages".into(),
            ));
        }
        if descriptor.origin_identity == local_identity(state) {
            let relative: String = conn.query_row(
                "SELECT storage_path FROM uploads WHERE server_id=?1 AND upload_id=?2",
                params![state.server_id, id],
                |r| r.get(0),
            )?;
            let path =
                upload_service::confined_path(Path::new(&state.upload_dir), &relative, false)?;
            let mut file = File::open(path)?;
            if !file.metadata()?.is_file()
                || file.metadata()?.len() != descriptor.size_bytes
                || hash_file(&mut file, descriptor.size_bytes)? != descriptor.ciphertext_sha256
            {
                return Err(TransferError::Integrity(
                    "retained local attachment bytes do not match committed metadata".into(),
                ));
            }
        }
        descriptors.push(descriptor);
    }
    Ok(descriptors)
}

/// Called only by the authenticated authority event applier in the exact
/// transaction that accepts/materializes the containing message. Local-origin
/// descriptors are verified against already committed opaque upload metadata.
pub fn record_expected(
    conn: &Connection,
    state: &AppState,
    accepted: &AcceptedAttachment,
    author: &str,
    fetch_member: &str,
    fetch_device: &str,
) -> Result<(), TransferError> {
    require_transaction(conn)?;
    accepted.validate()?;
    if author.is_empty() {
        return Err(TransferError::BadRequest(
            "attachment requires its original message author".into(),
        ));
    }
    let message_live: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE server_id=?1 AND channel_id=?2 AND message_id=?3 AND sender_pseudonym=?4 AND deleted_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>datetime('now')))",
        params![state.server_id,accepted.channel_id,accepted.message_id,author],|r|r.get(0))?;
    if !message_live {
        return Err(TransferError::Conflict(
            "attachment message is not live in this transaction".into(),
        ));
    }
    let d = &accepted.descriptor;
    if d.origin_identity == local_identity(state) {
        let Some((channel, owner, actual)) =
            local_origin_descriptor(conn, state, &d.attachment_id)?
        else {
            return Err(TransferError::NotFound(
                "accepted attachment does not exist at its origin".into(),
            ));
        };
        if channel != accepted.channel_id || owner != author || actual != *d {
            return Err(TransferError::Conflict(
                "origin upload does not match the accepted descriptor".into(),
            ));
        }
        let reused: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM message_uploads WHERE server_id=?1 AND upload_id=?2 AND message_id<>?3 UNION ALL SELECT 1 FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2 AND message_id<>?3)",params![state.server_id,d.attachment_id,accepted.message_id],|r|r.get(0))?;
        if reused {
            return Err(TransferError::Conflict(
                "origin attachment is already bound to another message".into(),
            ));
        }
        let old: Option<(String,Option<String>)> = conn.query_row("SELECT accepted_json,removed_reason FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2 AND message_id=?3",
            params![state.server_id,d.attachment_id,accepted.message_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((old, removed)) = &old {
            let prior: AcceptedAttachment = serde_json::from_str(old)?;
            if removed.is_some()
                || prior.descriptor != *d
                || prior.authority_identity != accepted.authority_identity
                || prior.channel_id != accepted.channel_id
            {
                return Err(TransferError::Conflict(
                    "origin attachment binding is immutable or removed".into(),
                ));
            }
        }
        conn.execute("INSERT OR IGNORE INTO message_uploads(server_id,message_id,upload_id) VALUES(?1,?2,?3)",params![state.server_id,accepted.message_id,d.attachment_id])?;
        // Retaining a descriptor in a later edit does not change its first
        // accepted reference; authority permits may name that original event.
        if old.is_some() {
            return Ok(());
        }
        conn.execute("INSERT INTO federated_upload_origins(server_id,upload_id,authority_identity,channel_id,message_id,accepted_event_seq,accepted_event_hash,accepted_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![state.server_id,d.attachment_id,accepted.authority_identity,accepted.channel_id,accepted.message_id,accepted.accepted_event_seq,accepted.accepted_event_hash,serde_json::to_string(accepted)?])?;
        return Ok(());
    }
    let existing: Option<(String,String)> = conn.query_row("SELECT accepted_json,status FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2",params![state.server_id,d.attachment_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((old, status)) = existing {
        let prior: AcceptedAttachment = serde_json::from_str(&old)?;
        if prior.descriptor != *d
            || prior.authority_identity != accepted.authority_identity
            || prior.message_id != accepted.message_id
            || prior.channel_id != accepted.channel_id
            || status == "removed"
        {
            return Err(TransferError::Conflict(
                "attachment UUID already has a different or removed binding".into(),
            ));
        }
        // Reconciliation rebuilds messages under a cleanup guard. Restore the
        // cascade-removed association only when the retained replica metadata
        // still matches the authenticated message and immutable descriptor.
        let coherent: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM uploads u JOIN federated_upload_transfers f
             ON f.server_id=u.server_id AND f.upload_id=u.upload_id
             WHERE u.server_id=?1 AND u.upload_id=?2 AND u.channel_id=?3
               AND u.uploader_pseudonym=?4 AND u.purpose='federated_chat'
               AND u.original_filename=?5 AND u.content_type=?6 AND u.size_bytes=?7
               AND u.category=?8 AND u.encrypted=?9 AND u.epoch IS ?10 AND u.key_id IS ?11
               AND u.ciphertext_sha256=?12 AND u.storage_path=f.storage_path
               AND ((f.status='ready' AND u.lifecycle='ready' AND u.staging_path IS NULL)
                 OR (f.status IN('pending','receiving','failed') AND u.lifecycle='staging' AND u.staging_path=f.staging_path))
               AND f.message_id=?13 AND f.channel_id=?3 AND f.size_bytes=u.size_bytes
               AND f.ciphertext_sha256=u.ciphertext_sha256 AND f.status<>'removed')",
            params![state.server_id,d.attachment_id,accepted.channel_id,author,d.filename(),d.content_type(),
                d.size_bytes,d.category,!d.is_plaintext(),(!d.is_plaintext()).then_some(d.epoch),
                (!d.is_plaintext()).then_some(d.key_id.as_str()),d.ciphertext_sha256,accepted.message_id],
            |row|row.get(0))?;
        let reused: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM message_uploads WHERE server_id=?1 AND upload_id=?2 AND message_id<>?3)",
            params![state.server_id,d.attachment_id,accepted.message_id], |row|row.get(0))?;
        if !coherent || reused {
            return Err(TransferError::Conflict(
                "retained replica upload metadata or message ownership changed".into(),
            ));
        }
        conn.execute("INSERT OR IGNORE INTO message_uploads(server_id,message_id,upload_id) VALUES(?1,?2,?3)",
            params![state.server_id,accepted.message_id,d.attachment_id])?;
        return Ok(());
    }
    if !wire::hex_32(fetch_device) || fetch_member.is_empty() {
        return Err(TransferError::BadRequest(
            "replica attachment requires an admitted fetch member and device".into(),
        ));
    }
    let (origin_peer, key) = peer_for(conn, state, &d.origin_identity, &d.originating_server)?;
    let used: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM upload_ids_used WHERE upload_id=?1)",
        [&d.attachment_id],
        |r| r.get(0),
    )?;
    if used {
        return Err(TransferError::Conflict(
            "attachment UUID was already consumed".into(),
        ));
    }
    // Public URLs still name the authenticated attachment UUID. Internal paths
    // include a generation so a delayed cleanup attempt can never remove a
    // fresh retry's bytes, even when two cleanup drains overlapped earlier.
    let (staging, storage) = replica_paths(&d.attachment_id);
    conn.execute("INSERT INTO uploads(server_id,upload_id,uploader_pseudonym,original_filename,content_type,size_bytes,purpose,channel_id,category,encrypted,epoch,key_id,ciphertext_sha256,storage_path,staging_path,lifecycle) VALUES(?1,?2,?3,?4,?5,?6,'federated_chat',?7,?8,?9,?10,?11,?12,?13,?14,'staging')",
        params![state.server_id,d.attachment_id,author,d.filename(),d.content_type(),d.size_bytes,accepted.channel_id,d.category,!d.is_plaintext(),
            (!d.is_plaintext()).then_some(d.epoch),(!d.is_plaintext()).then_some(d.key_id.as_str()),d.ciphertext_sha256,storage,staging])?;
    conn.execute(
        "INSERT INTO message_uploads(server_id,message_id,upload_id) VALUES(?1,?2,?3)",
        params![state.server_id, accepted.message_id, d.attachment_id],
    )?;
    conn.execute("INSERT INTO federated_upload_transfers(server_id,upload_id,authority_identity,channel_id,message_id,accepted_event_seq,accepted_event_hash,accepted_json,origin_identity,origin_peer_id,origin_public_key,member_pseudonym,device_id,staging_path,storage_path,size_bytes,ciphertext_sha256,prefix_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
        params![state.server_id,d.attachment_id,accepted.authority_identity,accepted.channel_id,accepted.message_id,accepted.accepted_event_seq,accepted.accepted_event_hash,serde_json::to_string(accepted)?,d.origin_identity,origin_peer,key,fetch_member,fetch_device,staging,storage,d.size_bytes,d.ciphertext_sha256,EMPTY_SHA256])?;
    Ok(())
}

/// Ordered authority removal and all disk obligations commit atomically. The
/// transfer row survives so old accepted events cannot recreate the same UUID.
pub fn remove_expected(
    conn: &Connection,
    server_id: i64,
    channel: &str,
    message: Option<&str>,
    ids: Option<&[String]>,
    reason: &str,
    event: (u64, &str),
) -> Result<usize, TransferError> {
    let (seq, hash) = event;
    require_transaction(conn)?;
    if !matches!(
        reason,
        "deleted" | "retention" | "detached" | "channel_deleted"
    ) || seq == 0
        || !wire::hex_32(hash)
    {
        return Err(TransferError::BadRequest(
            "invalid attachment removal event".into(),
        ));
    }
    let mut stmt=conn.prepare("SELECT upload_id FROM federated_upload_transfers WHERE server_id=?1 AND channel_id=?2 AND (?3 IS NULL OR message_id=?3) UNION SELECT upload_id FROM federated_upload_origins WHERE server_id=?1 AND channel_id=?2 AND (?3 IS NULL OR message_id=?3)")?;
    let all = stmt
        .query_map(params![server_id, channel, message], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut count = 0;
    for id in all {
        if ids.is_some_and(|ids| !ids.contains(&id)) {
            continue;
        }
        conn.execute("UPDATE federated_upload_transfers SET status='removed',removed_reason=?3,tombstone_seq=?4,tombstone_hash=?5,lease_token=NULL,lease_expires_at=NULL,updated_at=unixepoch() WHERE server_id=?1 AND upload_id=?2",params![server_id,id,reason,seq,hash])?;
        conn.execute("UPDATE federated_upload_origins SET removed_reason=?3,tombstone_seq=?4,tombstone_hash=?5 WHERE server_id=?1 AND upload_id=?2",params![server_id,id,reason,seq,hash])?;
        conn.execute("INSERT OR IGNORE INTO upload_deletion_queue(server_id,relative_path) SELECT server_id,staging_path FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2 UNION SELECT server_id,storage_path FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2",params![server_id,id])?;
        conn.execute(
            "DELETE FROM uploads WHERE server_id=?1 AND upload_id=?2",
            params![server_id, id],
        )?;
        count += 1;
    }
    Ok(count)
}

fn require_origin_binding(
    conn: &Connection,
    state: &AppState,
    intent: &ReadIntent,
) -> Result<(), TransferError> {
    let a = &intent.accepted;
    let stored:Option<(String,Option<String>)>=conn.query_row("SELECT accepted_json,removed_reason FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2 AND message_id=?3",params![state.server_id,a.descriptor.attachment_id,a.message_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((stored, removed)) = stored else {
        return Err(TransferError::NotFound(
            "attachment is not bound to an accepted event".into(),
        ));
    };
    let stored: AcceptedAttachment = serde_json::from_str(&stored)?;
    if stored != *a || removed.is_some() {
        return Err(TransferError::Forbidden(
            "attachment event binding is changed or removed".into(),
        ));
    }
    let associated:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM message_uploads mu JOIN messages m ON m.message_id=mu.message_id JOIN uploads u ON u.upload_id=mu.upload_id WHERE mu.server_id=?1 AND mu.message_id=?2 AND mu.upload_id=?3 AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now')) AND m.channel_id=?4 AND u.encrypted=?5 AND u.uploader_pseudonym=m.sender_pseudonym AND u.lifecycle='ready')",params![state.server_id,a.message_id,a.descriptor.attachment_id,a.channel_id,!a.descriptor.is_plaintext()],|r|r.get(0))?;
    if !associated {
        return Err(TransferError::Forbidden(
            "attachment is no longer associated with a live message".into(),
        ));
    }
    Ok(())
}

fn origin_outcome(
    conn: &Connection,
    state: &AppState,
    intent: &ReadIntent,
) -> Result<Option<ChunkOutcome>, TransferError> {
    type OriginOutcomeRow = (String, Option<String>, Option<u64>, Option<String>);
    let row: Option<OriginOutcomeRow> = conn.query_row("SELECT accepted_json,removed_reason,tombstone_seq,tombstone_hash FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2 AND message_id=?3",params![state.server_id,intent.accepted.descriptor.attachment_id,intent.accepted.message_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((stored, reason, seq, hash)) = row else {
        // The authority can request bytes immediately after committing an
        // origin-authored message, before that origin has applied its receipt.
        // This is a pending ordered event, not a permanent missing attachment.
        // Return no bytes or read permit until the exact binding is applied.
        let accepted = &intent.accepted;
        require_authority_agreement(conn, state, &accepted.channel_id)?;
        let bound = super::channel_authority::binding(conn, state, &accepted.channel_id)
            .map_err(authority_error)?;
        if !bound.local_authority
            && bound.authority_identity == accepted.authority_identity
            && bound.head_seq >= 0
            && (bound.head_seq as u64) < accepted.accepted_event_seq
            && local_origin_descriptor(conn, state, &accepted.descriptor.attachment_id)?
                .is_some_and(|(channel, _, descriptor)| {
                    channel == accepted.channel_id && descriptor == accepted.descriptor
                })
        {
            return Ok(Some(ChunkOutcome::Pending {
                retry_after_seconds: 3,
            }));
        }
        return Err(TransferError::NotFound(
            "attachment is not bound to an accepted event".into(),
        ));
    };
    let stored: AcceptedAttachment = serde_json::from_str(&stored)?;
    if stored != intent.accepted {
        return Err(TransferError::Forbidden(
            "origin attachment event context does not match".into(),
        ));
    }
    if let Some(reason) = reason {
        return Ok(Some(match (seq, hash) {
            (Some(tombstone_seq), Some(tombstone_hash)) => ChunkOutcome::Removed {
                reason,
                tombstone_seq,
                tombstone_hash,
            },
            _ => ChunkOutcome::Pending {
                retry_after_seconds: 3,
            },
        }));
    }
    require_origin_binding(conn, state, intent)?;
    Ok(None)
}
fn signed_response(
    state: &AppState,
    request: ChunkRequest,
    permit: Option<wire::SignedReadPermit>,
    outcome: ChunkOutcome,
) -> Result<ChunkResponse, TransferError> {
    let mut response = ChunkResponse {
        version: request.version,
        originating_identity: local_identity(state),
        originating_server: state.get_public_url().trim_end_matches('/').into(),
        request_sha256: wire::request_digest(&request)?,
        intent: request.intent,
        permit,
        outcome,
        created_at: now(),
        signature: String::new(),
    };
    response.signature = wire::sign(
        &state.signing_key,
        &wire::response_signing_input(&response)?,
    );
    Ok(response)
}

/// The origin itself asks the authority, rather than trusting a reusable permit
/// handed in by the requester. Permission applies to exactly this one range.
pub async fn serve_chunk(
    state: Arc<AppState>,
    request: ChunkRequest,
) -> Result<ChunkResponse, TransferError> {
    let current = now();
    let outcome = {
        let conn = state.pool.get().map_err(pool_error)?;
        let (_, key) = peer_for(
            &conn,
            &state,
            &request.intent.requesting_identity,
            &request.requesting_server,
        )?;
        wire::verify_request(&request, &key, current)?;
        origin_outcome(&conn, &state, &request.intent)?
    };
    if request.intent.accepted.descriptor.origin_identity != local_identity(&state) {
        return Err(TransferError::Forbidden(
            "this server is not the blob origin".into(),
        ));
    }
    // Persist replay consumption before the await; duplicate requests cannot
    // race two successful reads through separate HTTP connections.
    {
        let conn = state.pool.get().map_err(pool_error)?;
        conn.execute(
            "DELETE FROM federated_upload_read_nonces WHERE server_id=?1 AND expires_at<?2",
            params![state.server_id, current],
        )?;
        let inserted=conn.execute("INSERT OR IGNORE INTO federated_upload_read_nonces(server_id,requesting_identity,nonce,intent_sha256,expires_at) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,request.intent.requesting_identity,request.intent.nonce,wire::intent_digest(&request.intent)?,current+wire::MAX_CLOCK_SKEW_SECONDS*2])?;
        if inserted != 1 {
            return Err(TransferError::Conflict(
                "attachment read nonce was already consumed".into(),
            ));
        }
    }
    if let Some(outcome) = outcome {
        return signed_response(&state, request, None, outcome);
    }
    let permit = super::channel_authority::authorize_attachment_read(
        state.clone(),
        request.intent.clone(),
        Some(request.intent_signature.clone()),
    )
    .await
    .map_err(authority_error)?;
    let authority_key = {
        let conn = state.pool.get().map_err(pool_error)?;
        super::channel_authority::pinned_authority_key(
            &conn,
            &state,
            &request.intent.accepted.channel_id,
        )
        .map_err(|e| TransferError::Forbidden(e.to_string()))?
    };
    wire::verify_permit(&permit, &request.intent, &authority_key, now())?;
    let bytes = {
        let mut conn = state.pool.get().map_err(pool_error)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_authority_agreement(&tx, &state, &request.intent.accepted.channel_id)?;
        peer_for(
            &tx,
            &state,
            &request.intent.requesting_identity,
            &request.requesting_server,
        )?;
        require_origin_binding(&tx, &state, &request.intent)?;
        let (relative,size,hash):(String,u64,String)=tx.query_row("SELECT storage_path,size_bytes,ciphertext_sha256 FROM uploads WHERE server_id=?1 AND upload_id=?2 AND lifecycle='ready'",params![state.server_id,request.intent.accepted.descriptor.attachment_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let d = &request.intent.accepted.descriptor;
        if size != d.size_bytes || hash != d.ciphertext_sha256 {
            return Err(TransferError::Conflict("origin metadata changed".into()));
        }
        let path = upload_service::confined_path(Path::new(&state.upload_dir), &relative, false)?;
        let mut file = File::open(path)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() != size {
            return Err(TransferError::Conflict("origin file length changed".into()));
        }
        file.seek(SeekFrom::Start(request.intent.offset.ok_or_else(|| {
            TransferError::BadRequest("missing range".into())
        })?))?;
        let mut bytes = vec![
            0;
            request
                .intent
                .length
                .ok_or_else(|| TransferError::BadRequest("missing length".into()))?
                as usize
        ];
        file.read_exact(&mut bytes)?;
        tx.commit()?;
        bytes
    };
    signed_response(
        &state,
        request,
        Some(permit),
        ChunkOutcome::Ready {
            count: bytes.len() as u64,
            chunk_sha256: hex::encode(Sha256::digest(&bytes)),
            bytes_base64: STANDARD.encode(&bytes),
        },
    )
}

#[derive(Clone, Debug)]
struct Transfer {
    id: i64,
    accepted: AcceptedAttachment,
    origin_peer: i64,
    origin_key: String,
    member: String,
    device: String,
    staging: String,
    storage: String,
    committed: u64,
    prefix_hash: String,
    lease: String,
}
fn claim(state: &AppState) -> Result<Option<Transfer>, TransferError> {
    let mut conn = state.pool.get().map_err(pool_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = now();
    type TransferRow = (
        i64,
        String,
        i64,
        String,
        String,
        String,
        String,
        String,
        u64,
        String,
    );
    let item: Option<TransferRow> = tx.query_row("SELECT id,accepted_json,origin_peer_id,origin_public_key,member_pseudonym,device_id,staging_path,storage_path,committed_bytes,prefix_sha256 FROM federated_upload_transfers WHERE server_id=?1 AND status IN ('pending','receiving') AND next_retry_at<=?2 AND (lease_expires_at IS NULL OR lease_expires_at<=?2) AND EXISTS(SELECT 1 FROM message_uploads mu JOIN messages m ON m.server_id=mu.server_id AND m.message_id=mu.message_id WHERE mu.server_id=federated_upload_transfers.server_id AND mu.upload_id=federated_upload_transfers.upload_id AND mu.message_id=federated_upload_transfers.message_id AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now'))) ORDER BY next_retry_at,id LIMIT 1",params![state.server_id,current],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?))).optional()?;
    let Some((
        id,
        accepted,
        origin_peer,
        origin_key,
        member,
        device,
        staging,
        storage,
        committed,
        prefix_hash,
    )) = item
    else {
        return Ok(None);
    };
    let accepted: AcceptedAttachment = serde_json::from_str(&accepted)?;
    accepted.validate()?;
    let lease = uuid::Uuid::new_v4().to_string();
    tx.execute("UPDATE federated_upload_transfers SET status='receiving',lease_token=?2,lease_expires_at=?3,updated_at=?4 WHERE id=?1",params![id,lease,current+LEASE_SECONDS,current])?;
    tx.commit()?;
    Ok(Some(Transfer {
        id,
        accepted,
        origin_peer,
        origin_key,
        member,
        device,
        staging,
        storage,
        committed,
        prefix_hash,
        lease,
    }))
}
fn require_live_lease(
    conn: &Connection,
    state: &AppState,
    t: &Transfer,
) -> Result<(), TransferError> {
    require_authority_agreement(conn, state, &t.accepted.channel_id)?;
    super::e2e_service::require_member(conn, state, &t.accepted.channel_id, &t.member)
        .map_err(|error| TransferError::Forbidden(error.to_string()))?;
    super::e2e_service::require_device(conn, state.server_id, &t.member, &t.device)
        .map_err(|error| TransferError::Forbidden(error.to_string()))?;
    let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM federated_upload_transfers f JOIN message_uploads mu ON mu.upload_id=f.upload_id AND mu.message_id=f.message_id JOIN messages m ON m.message_id=f.message_id WHERE f.id=?1 AND f.server_id=?2 AND f.status='receiving' AND f.lease_token=?3 AND f.committed_bytes=?4 AND f.prefix_sha256=?5 AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now')))",params![t.id,state.server_id,t.lease,t.committed,t.prefix_hash],|r|r.get(0))?;
    if !valid {
        return Err(TransferError::Conflict(
            "attachment transfer was removed, changed, or reclaimed".into(),
        ));
    }
    let (id, key) = peer_for(
        conn,
        state,
        &t.accepted.descriptor.origin_identity,
        &t.accepted.descriptor.originating_server,
    )?;
    if id != t.origin_peer || key != t.origin_key {
        return Err(TransferError::Forbidden(
            "attachment origin pin changed".into(),
        ));
    }
    Ok(())
}
fn hash_file(file: &mut File, length: u64) -> Result<String, TransferError> {
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut remaining = length;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let take = buffer.len().min(remaining as usize);
        file.read_exact(&mut buffer[..take])?;
        hash.update(&buffer[..take]);
        remaining -= take as u64;
    }
    Ok(hex::encode(hash.finalize()))
}
/// Restore exactly the committed prefix. Uncommitted trailing bytes are
/// truncated, but a missing/changed committed prefix is never silently blessed.
fn open_prefix(root: &Path, t: &Transfer) -> Result<(File, PathBuf), TransferError> {
    let path = upload_service::confined_path(root, &t.staging, true)?;
    let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && t.committed == 0 => {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(TransferError::Integrity(
                "committed staged ciphertext is missing".into(),
            ));
        }
        Err(e) => return Err(e.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() < t.committed {
        return Err(TransferError::Integrity(
            "staged file is shorter than committed progress".into(),
        ));
    }
    if hash_file(&mut file, t.committed)? != t.prefix_hash {
        return Err(TransferError::Integrity(
            "staged committed prefix failed integrity check".into(),
        ));
    }
    if metadata.len() > t.committed {
        file.set_len(t.committed)?;
        file.sync_all()?;
    }
    file.seek(SeekFrom::Start(t.committed))?;
    Ok((file, path))
}
fn persist_chunk(state: &AppState, t: &Transfer, bytes: &[u8]) -> Result<(), TransferError> {
    let expected =
        wire::CHUNK_BYTES.min(t.accepted.descriptor.size_bytes.saturating_sub(t.committed));
    if bytes.len() as u64 != expected || expected == 0 {
        return Err(TransferError::BadRequest(
            "chunk does not match committed progress".into(),
        ));
    }
    let mut conn = state.pool.get().map_err(pool_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_live_lease(&tx, state, t)?;
    if state.storage_health.writes_blocked() {
        return Err(TransferError::Unavailable("storage is read-only".into()));
    }
    let (mut file, _) = open_prefix(Path::new(&state.upload_dir), t)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let committed = t.committed + bytes.len() as u64;
    let hash = hash_file(&mut file, committed)?;
    if committed == t.accepted.descriptor.size_bytes
        && hash != t.accepted.descriptor.ciphertext_sha256
    {
        return Err(TransferError::Integrity(
            "completed ciphertext hash does not match its accepted descriptor".into(),
        ));
    }
    tx.execute("UPDATE federated_upload_transfers SET committed_bytes=?2,prefix_sha256=?3,status='pending',attempts=0,last_error=NULL,lease_token=NULL,lease_expires_at=NULL,next_retry_at=?4,updated_at=?4 WHERE id=?1",params![t.id,committed,hash,now()])?;
    tx.commit()?;
    Ok(())
}
fn make_intent(state: &AppState, t: &Transfer, purpose: ReadPurpose) -> ReadIntent {
    let chunk = purpose == ReadPurpose::Chunk;
    ReadIntent {
        version: t.accepted.descriptor.protocol_version(),
        purpose,
        requesting_identity: local_identity(state),
        requesting_server: state.get_public_url().trim_end_matches('/').into(),
        member_pseudonym: t.member.clone(),
        device_id: t.device.clone(),
        accepted: t.accepted.clone(),
        offset: chunk.then_some(t.committed),
        length: chunk.then_some(
            wire::CHUNK_BYTES.min(t.accepted.descriptor.size_bytes.saturating_sub(t.committed)),
        ),
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: now(),
    }
}
async fn finalize(state: Arc<AppState>, t: &Transfer) -> Result<(), TransferError> {
    let intent = make_intent(&state, t, ReadPurpose::Finalize);
    let permit =
        super::channel_authority::authorize_attachment_read(state.clone(), intent.clone(), None)
            .await
            .map_err(authority_error)?;
    let authority_key = {
        let conn = state.pool.get().map_err(pool_error)?;
        super::channel_authority::pinned_authority_key(&conn, &state, &t.accepted.channel_id)
            .map_err(|e| TransferError::Forbidden(e.to_string()))?
    };
    wire::verify_permit(&permit, &intent, &authority_key, now())?;
    let mut conn = state.pool.get().map_err(pool_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_live_lease(&tx, &state, t)?;
    if state.storage_health.writes_blocked() {
        return Err(TransferError::Unavailable("storage is read-only".into()));
    }
    let root = Path::new(&state.upload_dir);
    let stage = upload_service::confined_path(root, &t.staging, false)?;
    let target = upload_service::confined_path(root, &t.storage, true)?;
    // A previous process may have died after rename but before SQLite commit.
    // Only this exact intent's path, full length and authenticated digest can
    // recover that state; an arbitrary preexisting target is never overwritten.
    let source = if stage.exists() {
        stage.clone()
    } else {
        target.clone()
    };
    // FlushFileBuffers on Windows requires a writable handle. A read-only
    // integrity handle cannot durably finalize a staged file on that platform.
    let mut file = OpenOptions::new().read(true).write(true).open(&source)?;
    if !file.metadata()?.is_file()
        || file.metadata()?.len() != t.accepted.descriptor.size_bytes
        || hash_file(&mut file, t.accepted.descriptor.size_bytes)?
            != t.accepted.descriptor.ciphertext_sha256
    {
        return Err(TransferError::Integrity(
            "final ciphertext integrity check failed".into(),
        ));
    }
    file.sync_all()?;
    drop(file);
    if source != target {
        if target.exists() {
            return Err(TransferError::Conflict(
                "attachment destination already exists".into(),
            ));
        }
        fs::rename(&source, &target)?;
    }
    tx.execute("UPDATE uploads SET lifecycle='ready',staging_path=NULL WHERE server_id=?1 AND upload_id=?2 AND purpose='federated_chat'",params![state.server_id,t.accepted.descriptor.attachment_id])?;
    tx.execute("UPDATE federated_upload_transfers SET status='ready',attempts=0,last_error=NULL,lease_token=NULL,lease_expires_at=NULL,updated_at=?2 WHERE id=?1",params![t.id,now()])?;
    tx.commit()?;
    Ok(())
}
fn fail_attempt(
    state: &AppState,
    t: &Transfer,
    error: &TransferError,
) -> Result<(), TransferError> {
    let mut conn = state.pool.get().map_err(pool_error)?;
    // Store a bounded diagnostic with no request, credential or file content.
    let message = error.to_string().chars().take(240).collect::<String>();
    if matches!(error, TransferError::Integrity(_)) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed=tx.execute("UPDATE federated_upload_transfers SET status='failed',attempts=?4,committed_bytes=0,prefix_sha256=?5,cleanup_pending=1,lease_token=NULL,lease_expires_at=NULL,last_error=?6,updated_at=unixepoch() WHERE server_id=?1 AND id=?2 AND status='receiving' AND lease_token=?3",params![state.server_id,t.id,t.lease,MAX_FAILURES,EMPTY_SHA256,message])?;
        if changed == 1 {
            tx.execute("INSERT OR IGNORE INTO upload_deletion_queue(server_id,relative_path) VALUES(?1,?2),(?1,?3)",params![state.server_id,t.staging,t.storage])?;
        }
        tx.commit()?;
        return Ok(());
    }
    // Authenticated permanent refusals need an operator/member action before
    // retrying. Keep the exact verified prefix for that explicit retry.
    if matches!(error, TransferError::Remote(remote)
        if (400..500).contains(&remote.status) && !matches!(remote.status, 408 | 425 | 429))
    {
        conn.execute("UPDATE federated_upload_transfers SET attempts=attempts+1,status='failed',last_error=?3,lease_token=NULL,lease_expires_at=NULL,updated_at=unixepoch() WHERE id=?1 AND server_id=?2 AND status='receiving' AND lease_token=?4",params![t.id,state.server_id,message,t.lease])?;
        return Ok(());
    }
    conn.execute("UPDATE federated_upload_transfers SET attempts=attempts+1,status=CASE WHEN attempts+1>=?4 THEN 'failed' ELSE 'pending' END,next_retry_at=?3+MIN(300,5*(1 << MIN(attempts,6))),last_error=?5,lease_token=NULL,lease_expires_at=NULL,updated_at=?3 WHERE id=?1 AND server_id=?2 AND status='receiving' AND lease_token=?6",params![t.id,state.server_id,now(),MAX_FAILURES,message,t.lease])?;
    Ok(())
}
/// One bounded unit of work. Authority events keep applying independently.
pub async fn run_once(state: Arc<AppState>) -> Result<bool, TransferError> {
    let Some(t) = claim(&state)? else {
        return Ok(false);
    };
    let result = async {
        if t.committed == t.accepted.descriptor.size_bytes {
            return finalize(state.clone(), &t).await;
        }
        super::channel_replication::refresh_attachment_attestation(
            state.clone(),
            &t.accepted.channel_id,
            &t.member,
            &t.device,
        )
        .await
        .map_err(authority_error)?;
        let intent = make_intent(&state, &t, ReadPurpose::Chunk);
        let intent_signature =
            wire::sign(&state.signing_key, &wire::intent_signing_input(&intent)?);
        let mut request = ChunkRequest {
            version: intent.version,
            requesting_server: intent.requesting_server.clone(),
            intent,
            intent_signature,
            signature: String::new(),
        };
        request.signature = wire::sign(&state.signing_key, &wire::request_signing_input(&request)?);
        let body = super::federation_delivery::request_typed(
            state.clone(),
            t.origin_peer,
            wire::CHUNK_ENDPOINT,
            &serde_json::to_string(&request)?,
        )
        .await
        .map_err(|error| match error {
            super::federation_delivery::DeliveryError::Remote(error) => {
                TransferError::Remote(error)
            }
            super::federation_delivery::DeliveryError::Unavailable(message) => {
                TransferError::Unavailable(message)
            }
        })?;
        if body.len() > wire::MAX_CHUNK_RESPONSE_BYTES {
            return Err(TransferError::BadRequest(
                "attachment response exceeds limit".into(),
            ));
        }
        let response: ChunkResponse = serde_json::from_str(&body)?;
        let key = {
            let conn = state.pool.get().map_err(pool_error)?;
            super::channel_authority::pinned_authority_key(&conn, &state, &t.accepted.channel_id)
                .map_err(|e| TransferError::Forbidden(e.to_string()))?
        };
        match wire::verify_response(&response, &request, &t.origin_key, &key, now())? {
            Some(bytes) => persist_chunk(&state, &t, &bytes),
            None => Err(TransferError::Unavailable(match response.outcome {
                ChunkOutcome::Pending { .. } => "origin attachment is pending".into(),
                _ => "origin reports removal; awaiting ordered authority tombstone".into(),
            })),
        }
    }
    .await;
    if let Err(error) = result {
        fail_attempt(&state, &t, &error)?;
        return Err(error);
    }
    Ok(true)
}
pub async fn start_transfer_task(state: Arc<AppState>) {
    loop {
        if let Err(error) = notify_transfer_status(state.clone()).await {
            tracing::warn!(error=%error,"federated attachment status hint remains pending");
        }
        if let Err(error) = expire_failed_staging(&state) {
            tracing::warn!(error=%error,"expired federated attachment cleanup remains pending");
        }
        let delay = match run_once(state.clone()).await {
            Ok(true) => 0,
            Ok(false) => 2,
            Err(error) => {
                tracing::warn!(error=%error,"federated attachment transfer retained for retry");
                2
            }
        };
        tokio::select! {_=state.shutdown.cancelled()=>break,_=tokio::time::sleep(std::time::Duration::from_secs(delay))=>{}}
    }
}

/// Hints trigger fresh reads; they grant neither membership nor bytes. A crash
/// after finalization but before this send leaves a durable notification, and
/// disconnected clients discover ready state through their ordinary refetch.
pub async fn notify_transfer_status(state: Arc<AppState>) -> Result<(), TransferError> {
    type Row = (i64, i64, String, String, String, String, u64, u64);
    let rows = {
        let conn = state.pool.get().map_err(pool_error)?;
        let mut stmt=conn.prepare("SELECT id,notification_seq,channel_id,message_id,upload_id,status,committed_bytes,size_bytes FROM federated_upload_transfers WHERE server_id=?1 AND notification_pending=1 ORDER BY id LIMIT 100")?;
        let rows = stmt
            .query_map([state.server_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?
            .collect::<Result<Vec<Row>, _>>()?;
        rows
    };
    for (id, revision, channel, message, upload, status, transferred, total) in rows {
        let members = {
            let conn = state.pool.get().map_err(pool_error)?;
            let mut stmt=conn.prepare("SELECT m.pseudonym_id FROM channel_members m JOIN platform_identities p ON p.server_id=m.server_id AND p.pseudonym_id=m.pseudonym_id WHERE m.server_id=?1 AND m.channel_id=?2 AND p.active=1")?;
            let rows = stmt
                .query_map(params![state.server_id, channel], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let status = if status == "receiving" {
            "pending"
        } else {
            status.as_str()
        };
        let frame=serde_json::json!({"type":"attachment_transfer_changed","channelId":channel,"messageId":message,"attachmentId":upload,"status":status,"transferredBytes":transferred,"totalBytes":total}).to_string();
        for member in members {
            state.connection_manager.send(&member, frame.clone()).await;
        }
        let conn = state.pool.get().map_err(pool_error)?;
        conn.execute("UPDATE federated_upload_transfers SET notification_pending=0 WHERE server_id=?1 AND id=?2 AND notification_seq=?3",params![state.server_id,id,revision])?;
    }
    Ok(())
}

/// Operator recovery retains the exact accepted descriptor and committed
/// prefix. Changing recipient credentials requires a separate authenticated
/// current-member request, never administrator impersonation.
pub fn retry_transfer(conn: &Connection, server_id: i64, id: i64) -> Result<(), TransferError> {
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let row:Option<(String,bool)>=tx.query_row("SELECT upload_id,cleanup_pending FROM federated_upload_transfers WHERE server_id=?1 AND id=?2 AND status='failed' AND EXISTS(SELECT 1 FROM message_uploads mu JOIN messages m ON m.server_id=mu.server_id AND m.message_id=mu.message_id WHERE mu.server_id=federated_upload_transfers.server_id AND mu.upload_id=federated_upload_transfers.upload_id AND mu.message_id=federated_upload_transfers.message_id AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now')))",params![server_id,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((upload_id, reset_paths)) = row else {
        return Err(TransferError::Conflict(
            "only a failed, still-associated attachment transfer can be retried".into(),
        ));
    };
    let changed=tx.execute("UPDATE federated_upload_transfers SET status='pending',attempts=0,next_retry_at=unixepoch(),last_error=NULL,lease_token=NULL,lease_expires_at=NULL,cleanup_pending=0,updated_at=unixepoch() WHERE server_id=?1 AND id=?2 AND status='failed' AND NOT EXISTS(SELECT 1 FROM upload_deletion_queue q WHERE q.server_id=federated_upload_transfers.server_id AND q.relative_path IN(federated_upload_transfers.staging_path,federated_upload_transfers.storage_path))",params![server_id,id])?;
    if changed != 1 {
        return Err(TransferError::Conflict(
            "only a failed, still-associated attachment transfer can be retried".into(),
        ));
    }
    if reset_paths {
        let (stage, storage) = replica_paths(&upload_id);
        tx.execute("UPDATE federated_upload_transfers SET staging_path=?3,storage_path=?4 WHERE server_id=?1 AND id=?2",params![server_id,id,stage,storage])?;
        tx.execute("UPDATE uploads SET staging_path=?3,storage_path=?4,lifecycle='staging' WHERE server_id=?1 AND upload_id=?2 AND purpose='federated_chat'",params![server_id,upload_id,stage,storage])?;
    }
    tx.commit()?;
    Ok(())
}

/// Terminally failed partial files have a bounded retention lifetime. The
/// accepted descriptor remains retryable, but path cleanup must finish before
/// another attempt can reuse that path, including after a Windows lock failure.
pub fn expire_failed_staging(state: &AppState) -> Result<(), TransferError> {
    let mut conn = state.pool.get().map_err(pool_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("INSERT OR IGNORE INTO upload_deletion_queue(server_id,relative_path) SELECT server_id,staging_path FROM federated_upload_transfers WHERE server_id=?1 AND status='failed' AND cleanup_pending=0 AND updated_at<unixepoch()-604800 UNION SELECT server_id,storage_path FROM federated_upload_transfers WHERE server_id=?1 AND status='failed' AND cleanup_pending=0 AND updated_at<unixepoch()-604800",[state.server_id])?;
    tx.execute("UPDATE federated_upload_transfers SET committed_bytes=0,prefix_sha256=?2,cleanup_pending=1,last_error='staged bytes expired; cleanup must finish before retry',updated_at=unixepoch() WHERE server_id=?1 AND status='failed' AND cleanup_pending=0 AND updated_at<unixepoch()-604800",params![state.server_id,EMPTY_SHA256])?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct TransferDeliveryRow {
    pub id: i64,
    pub message_id: String,
    pub channel_id: String,
    pub remote_instance_id: i64,
    pub status: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub committed_bytes: u64,
    pub size_bytes: u64,
    pub operation_kind: &'static str,
}
pub fn list_transfers(
    conn: &Connection,
    server_id: i64,
    limit: u32,
) -> Result<Vec<TransferDeliveryRow>, TransferError> {
    let mut stmt=conn.prepare("SELECT id,message_id,channel_id,origin_peer_id,status,attempts,last_error,committed_bytes,size_bytes FROM federated_upload_transfers WHERE server_id=?1 ORDER BY id DESC LIMIT ?2")?;
    let rows = stmt
        .query_map(params![server_id, limit.min(200)], |r| {
            Ok(TransferDeliveryRow {
                id: -r.get::<_, i64>(0)?,
                message_id: r.get(1)?,
                channel_id: r.get(2)?,
                remote_instance_id: r.get(3)?,
                status: r.get(4)?,
                attempts: r.get(5)?,
                last_error: r.get(6)?,
                committed_bytes: r.get(7)?,
                size_bytes: r.get(8)?,
                operation_kind: "attachment",
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn local_reader(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pseudonym: &str,
    device: &str,
    token_epoch: i64,
) -> Result<(), TransferError> {
    if !wire::hex_32(device) {
        return Err(TransferError::BadRequest(
            "a current device-bound upload grant is required".into(),
        ));
    }
    let permitted:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM platform_identities p JOIN channel_members m ON m.server_id=p.server_id AND m.pseudonym_id=p.pseudonym_id JOIN member_device_keys d ON d.server_id=p.server_id AND d.pseudonym_id=p.pseudonym_id WHERE p.server_id=?1 AND p.pseudonym_id=?2 AND p.active=1 AND p.token_epoch=?3 AND m.channel_id=?4 AND d.device_id=?5 AND d.revoked_at IS NULL)",params![state.server_id,pseudonym,token_epoch,channel,device],|r|r.get(0))?;
    if !permitted {
        return Err(TransferError::Forbidden(
            "attachment reader membership, session, or device was revoked".into(),
        ));
    }
    let certificate =
        super::channel_manifest::require_certified_device(conn, state, pseudonym, device)
            .map_err(|error| TransferError::Forbidden(error.to_string()))?;
    if certificate.server_identity != local_identity(state) {
        return Err(TransferError::Forbidden(
            "attachment reader is not certified for this local server".into(),
        ));
    }
    Ok(())
}

fn reader_for_descriptor(
    conn: &Connection,
    state: &AppState,
    accepted: &AcceptedAttachment,
    pseudonym: &str,
    device: &str,
    token_epoch: i64,
) -> Result<(), TransferError> {
    local_reader(
        conn,
        state,
        &accepted.channel_id,
        pseudonym,
        device,
        token_epoch,
    )?;
    if !accepted.descriptor.is_plaintext() {
        super::channel_manifest::require_admitted_device(
            conn,
            state,
            &accepted.channel_id,
            pseudonym,
            device,
        )
        .map_err(|error| TransferError::Forbidden(error.to_string()))?;
        let entitled:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_device_key_wraps WHERE server_id=?1 AND channel_id=?2 AND recipient_pseudonym_id=?3 AND recipient_device_id=?4 AND epoch=?5 AND key_id=?6)",params![state.server_id,accepted.channel_id,pseudonym,device,accepted.descriptor.epoch,accepted.descriptor.key_id],|r|r.get(0))?;
        if !entitled {
            return Err(TransferError::Forbidden(
                "device has no wrap for the accepted attachment epoch".into(),
            ));
        }
    }
    Ok(())
}

/// A signed accepted descriptor can precede the first entitled local reader.
/// Resolve it without inventing a local author/device or accepting browser
/// metadata. Removed bindings and deleted messages are never eligible.
fn deferred_attachment(
    conn: &Connection,
    state: &AppState,
    upload_id: &str,
) -> Result<Option<(AcceptedAttachment, String)>, TransferError> {
    let row: Option<(String,String,String,u64,String,String,String)> = conn.query_row(
        "SELECT a.authority_identity,b.channel_id,b.message_id,b.accepted_seq,b.accepted_hash,b.descriptor_json,m.sender_pseudonym FROM channel_authority_attachment_bindings b JOIN channel_authorities a ON a.server_id=b.server_id AND a.channel_id=b.channel_id JOIN messages m ON m.server_id=b.server_id AND m.channel_id=b.channel_id AND m.message_id=b.message_id WHERE b.server_id=?1 AND b.attachment_id=?2 AND b.removed_reason IS NULL AND a.deleted_seq IS NULL AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now'))",
        params![state.server_id,upload_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)),
    ).optional()?;
    let Some((
        authority_identity,
        channel_id,
        message_id,
        accepted_event_seq,
        accepted_event_hash,
        encoded,
        author,
    )) = row
    else {
        return Ok(None);
    };
    let accepted = AcceptedAttachment {
        authority_identity,
        channel_id,
        message_id,
        accepted_event_seq,
        accepted_event_hash,
        descriptor: serde_json::from_str(&encoded)?,
    };
    accepted.validate()?;
    if accepted.descriptor.attachment_id != upload_id {
        return Err(TransferError::Conflict(
            "accepted attachment binding is inconsistent".into(),
        ));
    }
    Ok(Some((accepted, author)))
}

/// Lazily materialize only an immutable accepted remote attachment for an
/// actual local member/device. A fresh authority permit is mandatory before
/// creating a transfer row; no remote author's identity is used as a reader.
pub(crate) async fn admit_deferred_for_reader(
    state: Arc<AppState>,
    upload_id: &str,
    expected_channel: Option<&str>,
    expected_relative: Option<&str>,
    pseudonym: &str,
    device: &str,
    token_epoch: i64,
) -> Result<bool, TransferError> {
    if !wire::canonical_uuid(upload_id) {
        return Err(TransferError::BadRequest("invalid attachment ID".into()));
    }
    let pending = {
        let conn = state.pool.get().map_err(pool_error)?;
        let existing:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM uploads WHERE server_id=?1 AND upload_id=?2 UNION ALL SELECT 1 FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2 UNION ALL SELECT 1 FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2)",params![state.server_id,upload_id],|r|r.get(0))?;
        if existing {
            return Ok(false);
        }
        let Some((accepted, author)) = deferred_attachment(&conn, &state, upload_id)? else {
            return Ok(false);
        };
        if accepted.descriptor.origin_identity == local_identity(&state) {
            return Err(TransferError::NotFound(
                "accepted origin attachment is not available".into(),
            ));
        }
        if expected_channel.is_some_and(|channel| channel != accepted.channel_id) {
            return Err(TransferError::NotFound(
                "attachment is not in this channel".into(),
            ));
        }
        let relative = accepted.descriptor.public_path();
        if expected_relative.is_some_and(|path| path != relative) {
            return Err(TransferError::NotFound(
                "attachment path does not match its accepted descriptor".into(),
            ));
        }
        reader_for_descriptor(&conn, &state, &accepted, pseudonym, device, token_epoch)?;
        let certificate =
            super::channel_manifest::require_certified_device(&conn, &state, pseudonym, device)
                .map_err(|error| TransferError::Forbidden(error.to_string()))?;
        if certificate.server_identity != local_identity(&state) {
            return Err(TransferError::Forbidden(
                "attachment reader device is not certified for this requesting server".into(),
            ));
        }
        (accepted, author)
    };
    if state.storage_health.writes_blocked() {
        return Err(TransferError::Unavailable(
            "attachment transfer admission awaits storage recovery".into(),
        ));
    }
    let (accepted, author) = pending;
    let intent = ReadIntent {
        version: accepted.descriptor.protocol_version(),
        purpose: ReadPurpose::Finalize,
        requesting_identity: local_identity(&state),
        requesting_server: state.get_public_url().trim_end_matches('/').into(),
        member_pseudonym: pseudonym.into(),
        device_id: device.into(),
        accepted: accepted.clone(),
        offset: None,
        length: None,
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: now(),
    };
    let permit =
        super::channel_authority::authorize_attachment_read(state.clone(), intent.clone(), None)
            .await
            .map_err(authority_error)?;
    let mut conn = state.pool.get().map_err(pool_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if state.storage_health.writes_blocked() {
        return Err(TransferError::Unavailable(
            "attachment transfer admission awaits storage recovery".into(),
        ));
    }
    reader_for_descriptor(&tx, &state, &accepted, pseudonym, device, token_epoch)?;
    let current = deferred_attachment(&tx, &state, upload_id)?
        .ok_or_else(|| TransferError::NotFound("accepted attachment was removed".into()))?;
    if current != (accepted.clone(), author.clone()) {
        return Err(TransferError::Conflict(
            "accepted attachment changed during reader admission".into(),
        ));
    }
    let key = require_authority_agreement(&tx, &state, &accepted.channel_id)?;
    wire::verify_permit(&permit, &intent, &key, now())?;
    record_expected(&tx, &state, &accepted, &author, pseudonym, device)?;
    tx.commit()?;
    Ok(true)
}

/// A current member can resume a stalled replica under their own device's
/// authority entitlement. The accepted descriptor and verified prefix stay
/// immutable; this does not let an operator impersonate another member.
pub async fn retry_for_member(
    state: Arc<AppState>,
    channel: &str,
    upload_id: &str,
    pseudonym: &str,
    device: &str,
    token_epoch: i64,
) -> Result<&'static str, TransferError> {
    if !wire::canonical_uuid(upload_id) {
        return Err(TransferError::BadRequest("invalid attachment ID".into()));
    }
    if admit_deferred_for_reader(
        state.clone(),
        upload_id,
        Some(channel),
        None,
        pseudonym,
        device,
        token_epoch,
    )
    .await?
    {
        return Ok("pending");
    }
    // The same browser component renders local uploads, local origins of a
    // federated channel, and downloaded replicas. Retry must revalidate the
    // matching real read path; absence of a replica job is not a missing local
    // file, and an origin must never bypass its fresh authority permit.
    let local_target = {
        let conn = state.pool.get().map_err(pool_error)?;
        local_reader(&conn, &state, channel, pseudonym, device, token_epoch)?;
        let replica: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2)",
            params![state.server_id,upload_id], |r|r.get(0),
        )?;
        if replica {
            None
        } else {
            let target: Option<(String,bool)> = conn.query_row(
                "SELECT u.storage_path,EXISTS(SELECT 1 FROM federated_upload_origins f WHERE f.server_id=u.server_id AND f.upload_id=u.upload_id) FROM uploads u WHERE u.server_id=?1 AND u.channel_id=?2 AND u.upload_id=?3 AND u.purpose='chat' AND u.lifecycle='ready'",
                params![state.server_id,channel,upload_id], |r|Ok((r.get(0)?,r.get(1)?)),
            ).optional()?;
            Some(
                target.ok_or_else(|| {
                    TransferError::NotFound("encrypted attachment not found".into())
                })?,
            )
        }
    };
    if let Some((relative, origin)) = local_target {
        if origin {
            read_origin_for_grant(state, &relative, upload_id, pseudonym, device, token_epoch)
                .await?;
        } else {
            // Includes the current session, certified device, signed channel
            // admission, exact epoch entitlement and on-disk hash/length.
            let id = upload_id.to_string();
            let member = pseudonym.to_string();
            let device = device.to_string();
            tokio::task::spawn_blocking(move || {
                upload_service::read_upload_for_device_grant(
                    &state,
                    &relative,
                    &id,
                    &member,
                    token_epoch,
                    Some(&device),
                )
            })
            .await
            .map_err(|_| {
                TransferError::Internal("attachment retry validation task failed".into())
            })??;
        }
        return Ok("ready");
    }
    let accepted = {
        let conn = state.pool.get().map_err(pool_error)?;
        local_reader(&conn, &state, channel, pseudonym, device, token_epoch)?;
        let stored:Option<String>=conn.query_row("SELECT accepted_json FROM federated_upload_transfers WHERE server_id=?1 AND channel_id=?2 AND upload_id=?3 AND status<>'removed'",params![state.server_id,channel,upload_id],|r|r.get(0)).optional()?;
        let stored = stored
            .ok_or_else(|| TransferError::NotFound("attachment transfer not found".into()))?;
        let accepted: AcceptedAttachment = serde_json::from_str(&stored)?;
        accepted.validate()?;
        accepted
    };
    let intent = ReadIntent {
        version: accepted.descriptor.protocol_version(),
        purpose: ReadPurpose::Finalize,
        requesting_identity: local_identity(&state),
        requesting_server: state.get_public_url().trim_end_matches('/').into(),
        member_pseudonym: pseudonym.into(),
        device_id: device.into(),
        accepted: accepted.clone(),
        offset: None,
        length: None,
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: now(),
    };
    let permit =
        super::channel_authority::authorize_attachment_read(state.clone(), intent.clone(), None)
            .await
            .map_err(authority_error)?;
    let mut conn = state.pool.get().map_err(pool_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    reader_for_descriptor(&tx, &state, &accepted, pseudonym, device, token_epoch)?;
    let key = require_authority_agreement(&tx, &state, channel)?;
    wire::verify_permit(&permit, &intent, &key, now())?;
    peer_for(
        &tx,
        &state,
        &accepted.descriptor.origin_identity,
        &accepted.descriptor.originating_server,
    )?;
    let row:Option<(i64,String,bool,String,String)>=tx.query_row("SELECT f.id,f.status,f.cleanup_pending,f.staging_path,f.storage_path FROM federated_upload_transfers f JOIN message_uploads mu ON mu.upload_id=f.upload_id AND mu.message_id=f.message_id JOIN messages m ON m.message_id=f.message_id WHERE f.server_id=?1 AND f.upload_id=?2 AND f.accepted_json=?3 AND f.status<>'removed' AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now'))",params![state.server_id,upload_id,serde_json::to_string(&accepted)?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((id, status, cleanup, stage, storage)) = row else {
        return Err(TransferError::NotFound(
            "attachment transfer was removed or changed".into(),
        ));
    };
    if status == "ready" {
        tx.commit()?;
        return Ok("ready");
    }
    let queued:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM upload_deletion_queue WHERE server_id=?1 AND relative_path IN(?2,?3))",params![state.server_id,stage,storage],|r|r.get(0))?;
    if queued {
        return Err(TransferError::Conflict(
            "attachment cleanup must finish before retry".into(),
        ));
    }
    if cleanup {
        let (stage, storage) = replica_paths(upload_id);
        tx.execute("UPDATE federated_upload_transfers SET staging_path=?3,storage_path=?4 WHERE server_id=?1 AND id=?2",params![state.server_id,id,stage,storage])?;
        tx.execute("UPDATE uploads SET staging_path=?3,storage_path=?4,lifecycle='staging' WHERE server_id=?1 AND upload_id=?2 AND purpose='federated_chat'",params![state.server_id,upload_id,stage,storage])?;
    }
    tx.execute("UPDATE federated_upload_transfers SET member_pseudonym=?3,device_id=?4,status='pending',attempts=0,next_retry_at=unixepoch(),last_error=NULL,lease_token=NULL,lease_expires_at=NULL,cleanup_pending=0,updated_at=unixepoch() WHERE server_id=?1 AND id=?2",params![state.server_id,id,pseudonym,device])?;
    tx.commit()?;
    Ok("pending")
}

/// An origin can itself be a replica of another channel authority. Its local
/// copy of the file and roster cannot authorize a new browser download while
/// that authority is unavailable or has removed the member/device/attachment.
pub async fn read_origin_for_grant(
    state: Arc<AppState>,
    relative: &str,
    upload_id: &str,
    pseudonym: &str,
    device: &str,
    token_epoch: i64,
) -> Result<(Vec<u8>, String), TransferError> {
    let accepted = {
        let conn = state.pool.get().map_err(pool_error)?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2",
            params![state.server_id, upload_id],
            |r| r.get(0),
        )?;
        if count != 1 {
            return Err(TransferError::NotFound(
                "attachment origin binding is absent or ambiguous".into(),
            ));
        }
        let stored:Option<String>=conn.query_row("SELECT accepted_json FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2 AND removed_reason IS NULL",params![state.server_id,upload_id],|r|r.get(0)).optional()?;
        let accepted: AcceptedAttachment = serde_json::from_str(
            &stored.ok_or_else(|| TransferError::NotFound("attachment was removed".into()))?,
        )?;
        accepted.validate()?;
        if accepted.descriptor.attachment_id != upload_id
            || accepted.descriptor.origin_identity != local_identity(&state)
        {
            return Err(TransferError::Forbidden(
                "attachment does not belong to this origin".into(),
            ));
        }
        reader_for_descriptor(&conn, &state, &accepted, pseudonym, device, token_epoch)?;
        accepted
    };
    let intent = ReadIntent {
        version: accepted.descriptor.protocol_version(),
        purpose: ReadPurpose::BrowserDownload,
        requesting_identity: local_identity(&state),
        requesting_server: state.get_public_url().trim_end_matches('/').into(),
        member_pseudonym: pseudonym.into(),
        device_id: device.into(),
        accepted: accepted.clone(),
        offset: None,
        length: None,
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: now(),
    };
    let permit =
        super::channel_authority::authorize_attachment_read(state.clone(), intent.clone(), None)
            .await
            .map_err(authority_error)?;
    let mut conn = state.pool.get().map_err(pool_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let key = require_authority_agreement(&tx, &state, &accepted.channel_id)?;
    wire::verify_permit(&permit, &intent, &key, now())?;
    reader_for_descriptor(&tx, &state, &accepted, pseudonym, device, token_epoch)?;
    require_origin_binding(&tx, &state, &intent)?;
    let (stored,size,hash):(String,u64,String)=tx.query_row("SELECT storage_path,size_bytes,ciphertext_sha256 FROM uploads WHERE server_id=?1 AND upload_id=?2 AND lifecycle='ready' AND purpose='chat'",params![state.server_id,upload_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if stored != relative
        || size != accepted.descriptor.size_bytes
        || hash != accepted.descriptor.ciphertext_sha256
    {
        return Err(TransferError::NotFound(
            "attachment path or accepted metadata changed".into(),
        ));
    }
    let path = upload_service::confined_path(Path::new(&state.upload_dir), relative, false)?;
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() != size {
        return Err(TransferError::Integrity(
            "origin ciphertext length changed".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    (&mut file).take(size + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size || hex::encode(Sha256::digest(&bytes)) != hash {
        return Err(TransferError::Integrity(
            "origin ciphertext integrity check failed".into(),
        ));
    }
    tx.commit()?;
    Ok((
        bytes,
        if accepted.descriptor.category == "file" {
            "application/octet-stream".into()
        } else {
            accepted.descriptor.content_type().into()
        },
    ))
}

fn invalidate_ready(
    conn: &Connection,
    state: &AppState,
    upload_id: &str,
    reason: &str,
) -> Result<(), TransferError> {
    require_transaction(conn)?;
    let changed=conn.execute("UPDATE federated_upload_transfers SET status='failed',attempts=?3,committed_bytes=0,prefix_sha256=?4,cleanup_pending=1,last_error=?5,updated_at=unixepoch() WHERE server_id=?1 AND upload_id=?2 AND status='ready'",params![state.server_id,upload_id,MAX_FAILURES,EMPTY_SHA256,reason])?;
    if changed == 1 {
        conn.execute("UPDATE uploads SET lifecycle='staging',staging_path=(SELECT staging_path FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2) WHERE server_id=?1 AND upload_id=?2 AND purpose='federated_chat'",params![state.server_id,upload_id])?;
        conn.execute("INSERT OR IGNORE INTO upload_deletion_queue(server_id,relative_path) SELECT server_id,staging_path FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2 UNION SELECT server_id,storage_path FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2",params![state.server_id,upload_id])?;
    }
    Ok(())
}

/// Downloading already assembled replica bytes still requires a fresh read
/// decision from the authority. A stale local roster or old signed URL cannot
/// authorize a read while the authority is unavailable or has revoked access.
pub async fn read_replica_for_grant(
    state: Arc<AppState>,
    relative: &str,
    upload_id: &str,
    pseudonym: &str,
    device: &str,
    token_epoch: i64,
) -> Result<(Vec<u8>, String), TransferError> {
    let (accepted, stored_path) = {
        let conn = state.pool.get().map_err(pool_error)?;
        let row:Option<(String,String,String)>=conn.query_row("SELECT accepted_json,status,storage_path FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2",params![state.server_id,upload_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((accepted, status, path)) = row else {
            return Err(TransferError::NotFound("attachment not found".into()));
        };
        if status == "removed" {
            return Err(TransferError::NotFound("attachment not found".into()));
        }
        if status != "ready" {
            return Err(TransferError::Unavailable(
                "attachment transfer is not complete".into(),
            ));
        }
        let accepted: AcceptedAttachment = serde_json::from_str(&accepted)?;
        accepted.validate()?;
        let public_path = accepted.descriptor.public_path();
        if relative != public_path {
            return Err(TransferError::NotFound(
                "attachment path does not match its authenticated descriptor".into(),
            ));
        }
        reader_for_descriptor(&conn, &state, &accepted, pseudonym, device, token_epoch)?;
        (accepted, path)
    };
    let intent = ReadIntent {
        version: accepted.descriptor.protocol_version(),
        purpose: ReadPurpose::BrowserDownload,
        requesting_identity: local_identity(&state),
        requesting_server: state.get_public_url().trim_end_matches('/').into(),
        member_pseudonym: pseudonym.into(),
        device_id: device.into(),
        accepted: accepted.clone(),
        offset: None,
        length: None,
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: now(),
    };
    let permit =
        super::channel_authority::authorize_attachment_read(state.clone(), intent.clone(), None)
            .await
            .map_err(authority_error)?;
    let mut conn = state.pool.get().map_err(pool_error)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let key = require_authority_agreement(&tx, &state, &accepted.channel_id)?;
    wire::verify_permit(&permit, &intent, &key, now())?;
    reader_for_descriptor(&tx, &state, &accepted, pseudonym, device, token_epoch)?;
    peer_for(
        &tx,
        &state,
        &accepted.descriptor.origin_identity,
        &accepted.descriptor.originating_server,
    )?;
    let live:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM federated_upload_transfers f JOIN uploads u ON u.upload_id=f.upload_id JOIN message_uploads mu ON mu.upload_id=f.upload_id AND mu.message_id=f.message_id JOIN messages m ON m.message_id=f.message_id WHERE f.server_id=?1 AND f.upload_id=?2 AND f.status='ready' AND f.accepted_json=?3 AND f.storage_path=?4 AND u.storage_path=f.storage_path AND u.purpose='federated_chat' AND u.lifecycle='ready' AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now')))",params![state.server_id,upload_id,serde_json::to_string(&accepted)?,stored_path],|r|r.get(0))?;
    if !live {
        return Err(TransferError::NotFound(
            "attachment was removed or changed".into(),
        ));
    }
    let path = upload_service::confined_path(Path::new(&state.upload_dir), &stored_path, false)?;
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            invalidate_ready(
                &tx,
                &state,
                upload_id,
                "ready replica ciphertext is missing",
            )?;
            tx.commit()?;
            return Err(TransferError::Integrity(
                "ready replica ciphertext is missing; operator retry required".into(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() != accepted.descriptor.size_bytes {
        drop(file);
        invalidate_ready(&tx, &state, upload_id, "replica ciphertext length changed")?;
        tx.commit()?;
        return Err(TransferError::Integrity(
            "replica ciphertext length changed; operator retry required".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(accepted.descriptor.size_bytes as usize);
    (&mut file)
        .take(accepted.descriptor.size_bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != accepted.descriptor.size_bytes
        || hex::encode(Sha256::digest(&bytes)) != accepted.descriptor.ciphertext_sha256
    {
        drop(file);
        invalidate_ready(
            &tx,
            &state,
            upload_id,
            "replica ciphertext integrity check failed",
        )?;
        tx.commit()?;
        return Err(TransferError::Integrity(
            "replica ciphertext integrity check failed; operator retry required".into(),
        ));
    }
    tx.commit()?;
    Ok((
        bytes,
        if accepted.descriptor.category == "file" {
            "application/octet-stream".into()
        } else {
            accepted.descriptor.content_type().into()
        },
    ))
}

#[cfg(test)]
#[path = "federated_upload_service_tests.rs"]
mod tests;
