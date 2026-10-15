//! Durable attachment lifecycle. No file becomes readable until final
//! authorization and metadata commit; every removal leaves a retryable disk
//! cleanup obligation in the same transaction as the logical deletion.

use super::{
    agent_policy::AgentAction,
    channel_service::{check_publish_permission, ChannelServiceError},
};
use crate::{api::ApiError, AppState};
use annex_types::ServerPolicy;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use thiserror::Error;

pub const MAX_ATTACHMENTS: usize = 16;
pub const CIPHERTEXT_OVERHEAD: usize = 28;
// In addition to authenticated HTTP rate limits, bound new speculative durable
// UUID reservations. Existing owned uploads and idempotent cleanup are exempt.
pub const MAX_UNKNOWN_CANCELLATIONS_PER_MEMBER_DAY: i64 = 256;
pub const MAX_UNKNOWN_CANCELLATIONS_PER_SERVER_DAY: i64 = 8192;

#[derive(Debug, Error)]
pub enum UploadError {
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
    Unavailable(String),
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Remote(annex_federation::transport::ApplicationError),
}
impl From<rusqlite::Error> for UploadError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Internal(format!("upload database: {value}"))
    }
}
impl From<std::io::Error> for UploadError {
    fn from(value: std::io::Error) -> Self {
        Self::Internal(format!("upload filesystem: {value}"))
    }
}
impl From<ChannelServiceError> for UploadError {
    fn from(value: ChannelServiceError) -> Self {
        match value {
            ChannelServiceError::BadRequest(message) => Self::BadRequest(message),
            ChannelServiceError::Forbidden(message) => Self::Forbidden(message),
            ChannelServiceError::NotFound(message) => Self::NotFound(message),
            ChannelServiceError::Conflict(message) => Self::Conflict(message),
            ChannelServiceError::ServiceUnavailable(message) => Self::Unavailable(message),
            ChannelServiceError::Remote(error) => Self::Remote(error),
            other => Self::Internal(other.to_string()),
        }
    }
}
impl From<super::e2e_service::E2eError> for UploadError {
    fn from(value: super::e2e_service::E2eError) -> Self {
        match value {
            super::e2e_service::E2eError::BadRequest(message) => Self::BadRequest(message),
            super::e2e_service::E2eError::Forbidden(message) => Self::Forbidden(message),
            super::e2e_service::E2eError::NotFound(message) => Self::NotFound(message),
            super::e2e_service::E2eError::Conflict { message, .. } => Self::Conflict(message),
            super::e2e_service::E2eError::Internal(message) => Self::Internal(message),
            super::e2e_service::E2eError::Remote(error) => Self::Remote(error),
        }
    }
}
impl From<UploadError> for ApiError {
    fn from(value: UploadError) -> Self {
        match value {
            UploadError::BadRequest(message) => Self::BadRequest(message),
            UploadError::Unauthorized(message) => Self::Unauthorized(message),
            UploadError::Forbidden(message) => Self::Forbidden(message),
            UploadError::NotFound(message) => Self::NotFound(message),
            UploadError::Conflict(message) => Self::Conflict(message),
            UploadError::Unavailable(message) => Self::ServiceUnavailable(message),
            UploadError::Internal(message) => Self::InternalServerError(message),
            UploadError::Remote(error) => Self::Remote(error),
        }
    }
}
impl From<UploadError> for ChannelServiceError {
    fn from(value: UploadError) -> Self {
        match value {
            UploadError::BadRequest(message) => Self::BadRequest(message),
            UploadError::Unauthorized(message) | UploadError::Forbidden(message) => {
                Self::Forbidden(message)
            }
            UploadError::NotFound(message) => Self::NotFound(message),
            UploadError::Conflict(message) => Self::Conflict(message),
            UploadError::Unavailable(message) => Self::ServiceUnavailable(message),
            UploadError::Internal(message) => Self::Internal(message),
            UploadError::Remote(error) => Self::Remote(error),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UploadMetadata {
    pub upload_id: String,
    pub channel_id: String,
    pub uploader: String,
    pub token_epoch: i64,
    pub category: String,
    pub original_filename: String,
    pub content_type: String,
    pub extension: String,
    pub encrypted_epoch: Option<(i64, String)>,
}

pub fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value)
}

pub fn category_directory(category: &str) -> Result<&'static str, UploadError> {
    match category {
        "image" => Ok("images"),
        "video" => Ok("videos"),
        "file" => Ok("files"),
        _ => Err(UploadError::BadRequest(
            "category must be image, video, or file".into(),
        )),
    }
}

pub fn policy_limit(
    policy: &ServerPolicy,
    category: &str,
    encrypted: bool,
) -> Result<usize, UploadError> {
    let (enabled, size) = match category {
        "image" => (policy.images_enabled, policy.max_image_size_mb),
        "video" => (policy.videos_enabled, policy.max_video_size_mb),
        "file" => (policy.files_enabled, policy.max_file_size_mb),
        _ => return Err(UploadError::BadRequest("unknown upload category".into())),
    };
    if !enabled {
        return Err(UploadError::Forbidden(format!(
            "{category} uploads are disabled on this server"
        )));
    }
    Ok((size as usize)
        .saturating_mul(1024 * 1024)
        .saturating_add(if encrypted { CIPHERTEXT_OVERHEAD } else { 0 }))
}

fn current_policy(conn: &Connection, server_id: i64) -> Result<ServerPolicy, UploadError> {
    let policy: String = conn.query_row(
        "SELECT policy_json FROM servers WHERE id = ?1",
        [server_id],
        |row| row.get(0),
    )?;
    serde_json::from_str(&policy)
        .map_err(|error| UploadError::Internal(format!("invalid stored upload policy: {error}")))
}

/// This runs before multipart reading and again in the writer transaction.
pub fn authorize_upload(
    conn: &Connection,
    state: &AppState,
    channel_id: &str,
    uploader: &str,
    token_epoch: i64,
    encrypted_epoch: Option<(i64, &str)>,
) -> Result<ServerPolicy, UploadError> {
    if state.storage_health.writes_blocked() {
        return Err(UploadError::Unavailable(
            "storage is temporarily read-only; retry after operator recovery".into(),
        ));
    }
    check_publish_permission(
        conn,
        state.server_id,
        uploader,
        channel_id,
        AgentAction::SendText,
    )?;
    let current_token_epoch: i64 = conn.query_row(
        "SELECT token_epoch FROM platform_identities WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 1",
        params![state.server_id, uploader], |row| row.get(0))?;
    if current_token_epoch != token_epoch {
        return Err(UploadError::Unauthorized(
            "session was revoked during upload".into(),
        ));
    }
    let encrypted: bool = conn.query_row(
        "SELECT e2e_enabled FROM channels WHERE server_id = ?1 AND channel_id = ?2",
        params![state.server_id, channel_id],
        |row| row.get(0),
    )?;
    if encrypted != encrypted_epoch.is_some() {
        return Err(UploadError::BadRequest(if encrypted {
            "end-to-end encrypted channels require opaque encrypted attachments".into()
        } else {
            "encrypted uploads require an encrypted channel".into()
        }));
    }
    if let Some((epoch, key_id)) = encrypted_epoch {
        super::e2e_service::require_current_encrypted_epoch(
            conn, state, channel_id, uploader, epoch, key_id,
        )?;
    }
    current_policy(conn, state.server_id)
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Only a confined relative path can be opened, renamed or removed. Reject
/// reparse points below the configured canonical upload root.
pub fn confined_path(
    root: &Path,
    relative: &str,
    create_parents: bool,
) -> Result<PathBuf, UploadError> {
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative.contains('\\')
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(UploadError::Internal(
            "invalid stored attachment path".into(),
        ));
    }
    let root = root.canonicalize()?;
    let mut path = root.clone();
    let parts: Vec<_> = relative_path.components().collect();
    for (index, part) in parts.iter().enumerate() {
        path.push(part.as_os_str());
        match fs::symlink_metadata(&path) {
            Ok(metadata) if is_reparse(&metadata) => {
                return Err(UploadError::Internal(
                    "attachment path contains a reparse point".into(),
                ))
            }
            Ok(metadata) if index + 1 < parts.len() && !metadata.is_dir() => {
                return Err(UploadError::Internal(
                    "attachment parent is not a directory".into(),
                ))
            }
            Ok(_) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && create_parents
                    && index + 1 < parts.len() =>
            {
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
                let metadata = fs::symlink_metadata(&path)?;
                if is_reparse(&metadata) || !metadata.is_dir() {
                    return Err(UploadError::Internal(
                        "attachment directory changed during creation".into(),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if !path.starts_with(&root) {
        return Err(UploadError::Internal(
            "attachment path leaves upload directory".into(),
        ));
    }
    Ok(path)
}

/// Reserve the durable intent before writing. Finalize under a new IMMEDIATE
/// transaction after checking the current authority again. The staging row
/// survives process death, and cleanup can remove either side of a rename.
pub fn persist_chat_upload(
    state: &AppState,
    metadata: &UploadMetadata,
    bytes: &[u8],
) -> Result<(String, String), UploadError> {
    persist_upload(state, metadata, bytes, false)
}

fn require_not_canceled(conn: &Connection, state: &AppState, id: &str) -> Result<(), UploadError> {
    let canceled: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM upload_cancellations WHERE server_id=?1 AND upload_id=?2)",
        params![state.server_id, id],
        |r| r.get(0),
    )?;
    if canceled {
        return Err(UploadError::Conflict(
            "attachment upload was explicitly canceled; retry with a new attachment ID".into(),
        ));
    }
    Ok(())
}

pub fn authorize_server_image(
    conn: &Connection,
    state: &AppState,
    uploader: &str,
    token_epoch: i64,
) -> Result<(), UploadError> {
    if state.storage_health.writes_blocked() {
        return Err(UploadError::Unavailable(
            "storage is temporarily read-only; retry after operator recovery".into(),
        ));
    }
    let identity = annex_identity::get_platform_identity(conn, state.server_id, uploader)
        .map_err(|error| UploadError::Internal(error.to_string()))?;
    if !identity.active || !identity.can_moderate {
        return Err(UploadError::Forbidden(
            "only current moderators can change the server image".into(),
        ));
    }
    if identity.token_epoch != token_epoch {
        return Err(UploadError::Unauthorized(
            "session was revoked during upload".into(),
        ));
    }
    Ok(())
}

pub fn persist_server_image(
    state: &AppState,
    metadata: &UploadMetadata,
    bytes: &[u8],
) -> Result<(String, String), UploadError> {
    if metadata.category != "image"
        || metadata.encrypted_epoch.is_some()
        || !["image/jpeg", "image/png", "image/gif", "image/webp"]
            .contains(&metadata.content_type.as_str())
    {
        return Err(UploadError::BadRequest(
            "server image must be a supported ordinary image".into(),
        ));
    }
    persist_upload(state, metadata, bytes, true)
}

fn authorize_intent(
    conn: &Connection,
    state: &AppState,
    metadata: &UploadMetadata,
    branding: bool,
) -> Result<usize, UploadError> {
    if branding {
        authorize_server_image(conn, state, &metadata.uploader, metadata.token_epoch)?;
        Ok(10 * 1024 * 1024)
    } else {
        let epoch = metadata
            .encrypted_epoch
            .as_ref()
            .map(|(epoch, key)| (*epoch, key.as_str()));
        let policy = authorize_upload(
            conn,
            state,
            &metadata.channel_id,
            &metadata.uploader,
            metadata.token_epoch,
            epoch,
        )?;
        policy_limit(&policy, &metadata.category, epoch.is_some())
    }
}

fn persist_upload(
    state: &AppState,
    metadata: &UploadMetadata,
    bytes: &[u8],
    branding: bool,
) -> Result<(String, String), UploadError> {
    if !canonical_uuid(&metadata.upload_id) {
        return Err(UploadError::BadRequest(
            "attachment_id must be a canonical UUID".into(),
        ));
    }
    if metadata.extension.is_empty()
        || !metadata
            .extension
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(UploadError::Internal("invalid attachment extension".into()));
    }
    let directory = category_directory(&metadata.category)?;
    if metadata.encrypted_epoch.is_some()
        && (metadata.original_filename != "encrypted.bin"
            || metadata.content_type != "application/octet-stream"
            || metadata.extension != "bin"
            || bytes.len() < CIPHERTEXT_OVERHEAD)
    {
        return Err(UploadError::BadRequest(
            "encrypted upload metadata must be opaque and ciphertext must include nonce and tag"
                .into(),
        ));
    }
    let relative = if branding {
        format!(
            "server/server_{}.{}",
            metadata.upload_id, metadata.extension
        )
    } else {
        format!(
            "chat/{directory}/{}.{}",
            metadata.upload_id, metadata.extension
        )
    };
    let staged = format!(".staging/{}.part", metadata.upload_id);
    let hash = hex::encode(Sha256::digest(bytes));
    let epoch = metadata
        .encrypted_epoch
        .as_ref()
        .map(|(epoch, key)| (*epoch, key.as_str()));
    let mut conn = state
        .pool
        .get()
        .map_err(|error| UploadError::Internal(error.to_string()))?;
    {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_not_canceled(&tx, state, &metadata.upload_id)?;
        if bytes.len() > authorize_intent(&tx, state, metadata, branding)? {
            return Err(UploadError::BadRequest(
                "attachment exceeds the current category size limit".into(),
            ));
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM upload_ids_used WHERE upload_id = ?1)",
            [&metadata.upload_id],
            |row| row.get(0),
        )?;
        if exists {
            return Err(UploadError::Conflict(
                "attachment_id already exists; use a new attachment ID".into(),
            ));
        }
        tx.execute("INSERT INTO uploads (server_id, upload_id, uploader_pseudonym, original_filename, content_type, size_bytes,
            purpose, channel_id, category, encrypted, epoch, key_id, ciphertext_sha256, storage_path, staging_path, lifecycle)
            VALUES (?1,?2,?3,?4,?5,?6,?15,?7,?8,?9,?10,?11,?12,?13,?14,'staging')",
            params![state.server_id, metadata.upload_id, metadata.uploader, metadata.original_filename, metadata.content_type,
                bytes.len() as i64, if branding {None} else {Some(metadata.channel_id.as_str())}, metadata.category, epoch.is_some(), epoch.map(|value| value.0),
                epoch.map(|value| value.1), hash, relative, staged, if branding {"server_image"} else {"chat"}])?;
        tx.commit()?;
    }
    let result = (|| -> Result<(), UploadError> {
        fs::create_dir_all(&state.upload_dir)?;
        let staging_path = confined_path(Path::new(&state.upload_dir), &staged, true)?;
        let final_path = confined_path(Path::new(&state.upload_dir), &relative, true)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_not_canceled(&tx, state, &metadata.upload_id)?;
        if bytes.len() > authorize_intent(&tx, state, metadata, branding)? {
            return Err(UploadError::BadRequest(
                "attachment exceeds the current category size limit".into(),
            ));
        }
        let still_staged: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM uploads WHERE server_id = ?1 AND upload_id = ?2
            AND lifecycle = 'staging' AND storage_path = ?3 AND staging_path = ?4 AND ciphertext_sha256 = ?5)",
            params![state.server_id, metadata.upload_id, relative, staged, hash], |row| row.get(0))?;
        if !still_staged {
            return Err(UploadError::Conflict(
                "upload was aborted before finalization".into(),
            ));
        }
        if final_path.exists() {
            return Err(UploadError::Conflict(
                "attachment file already exists".into(),
            ));
        }
        fs::rename(&staging_path, &final_path)?;
        #[cfg(unix)]
        {
            fs::File::open(final_path.parent().expect("confined file has parent"))?.sync_all()?;
        }
        tx.execute("UPDATE uploads SET lifecycle = 'ready', staging_path = NULL WHERE server_id = ?1 AND upload_id = ?2 AND lifecycle = 'staging'",
            params![state.server_id, metadata.upload_id])?;
        if branding {
            tx.execute(
                "UPDATE servers SET image_url = ?2 WHERE id = ?1",
                params![state.server_id, format!("/uploads/{relative}")],
            )?;
            // Replacing branding retires the old public bytes atomically with
            // the new URL. Concurrent in-flight staging intents remain valid.
            tx.execute("DELETE FROM uploads WHERE server_id = ?1 AND purpose = 'server_image' AND lifecycle != 'staging' AND upload_id != ?2",
                params![state.server_id,metadata.upload_id])?;
        }
        tx.commit()?;
        Ok(())
    })();
    if result.is_err() {
        // If this removal itself fails, the committed staging intent still
        // remains for the restart worker. Never pretend cleanup succeeded.
        if let Err(error) = conn.execute(
            "DELETE FROM uploads WHERE server_id = ?1 AND upload_id = ?2 AND lifecycle = 'staging'",
            params![state.server_id, metadata.upload_id],
        ) {
            tracing::error!(error = %error, "failed to queue unsuccessful upload cleanup");
        }
        drop(conn);
        let _ = drain_deletion_queue(state);
    }
    result?;
    Ok((format!("/uploads/{relative}"), hash))
}

/// Caller must hold the same writer transaction that publishes/edits message.
/// An omitted edit list should not invoke this method: that preserves the old
/// references; an explicit empty list removes them.
pub fn associate_message_uploads(
    conn: &Connection,
    state: &AppState,
    channel_id: &str,
    pseudonym: &str,
    message_id: &str,
    attachment_ids: &[String],
    encrypted_epoch: Option<(i64, &str)>,
) -> Result<(), UploadError> {
    if conn.is_autocommit() {
        return Err(UploadError::Internal(
            "attachment association requires the message transaction".into(),
        ));
    }
    if attachment_ids.len() > MAX_ATTACHMENTS {
        return Err(UploadError::BadRequest(
            "too many message attachments".into(),
        ));
    }
    let current_message: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE server_id = ?1
        AND channel_id = ?2 AND message_id = ?3 AND sender_pseudonym = ?4 AND deleted_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>datetime('now')))",
        params![state.server_id, channel_id, message_id, pseudonym],
        |row| row.get(0),
    )?;
    if !current_message {
        return Err(UploadError::Forbidden(
            "attachment association requires the author's current message".into(),
        ));
    }
    let mut unique = HashSet::new();
    for id in attachment_ids {
        if !canonical_uuid(id) || !unique.insert(id.as_str()) {
            return Err(UploadError::BadRequest(
                "attachment IDs must be unique canonical UUIDs".into(),
            ));
        }
        type UploadAssociationRow = (String, String, bool, Option<i64>, Option<String>, String);
        let upload: Option<UploadAssociationRow> = conn.query_row(
            "SELECT channel_id, uploader_pseudonym, encrypted, epoch, key_id, lifecycle FROM uploads WHERE server_id = ?1 AND upload_id = ?2 AND purpose = 'chat'",
            params![state.server_id, id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))).optional()?;
        let Some((channel, owner, encrypted, epoch, key_id, lifecycle)) = upload else {
            return Err(UploadError::NotFound("attachment not found".into()));
        };
        if channel != channel_id || owner != pseudonym {
            return Err(UploadError::Forbidden(
                "attachment belongs to another member or channel".into(),
            ));
        }
        if lifecycle != "ready" && lifecycle != "legacy" {
            return Err(UploadError::Conflict("attachment is not finalized".into()));
        }
        // An edit may reseal an existing descriptor under a newer message
        // epoch while retaining the file's original authenticated context.
        // This exception is bound to the exact existing message association;
        // it cannot attach an unpublished or another message's historical blob.
        let retained: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM message_uploads WHERE server_id = ?1 AND message_id = ?2 AND upload_id = ?3)",
            params![state.server_id,message_id,id], |row| row.get(0))?;
        if !retained
            && (encrypted != encrypted_epoch.is_some()
                || encrypted_epoch.is_some_and(|(expected, key)| {
                    epoch != Some(expected) || key_id.as_deref() != Some(key)
                }))
        {
            return Err(UploadError::Conflict(
                "attachment does not belong to this encrypted message epoch".into(),
            ));
        }
        conn.execute("INSERT OR IGNORE INTO message_uploads(server_id,message_id,upload_id) VALUES (?1,?2,?3)", params![state.server_id,message_id,id])?;
    }
    let old = {
        let mut statement = conn.prepare(
            "SELECT upload_id FROM message_uploads WHERE server_id = ?1 AND message_id = ?2",
        )?;
        let rows = statement.query_map(params![state.server_id, message_id], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for id in old {
        if !unique.contains(id.as_str()) {
            conn.execute("DELETE FROM message_uploads WHERE server_id = ?1 AND message_id = ?2 AND upload_id = ?3", params![state.server_id,message_id,id])?;
        }
    }
    Ok(())
}

pub fn abort_upload(
    state: &AppState,
    channel: &str,
    pseudonym: &str,
    upload_id: &str,
    token_epoch: i64,
) -> Result<(), UploadError> {
    if !canonical_uuid(upload_id) {
        return Err(UploadError::BadRequest(
            "attachment_id must be a canonical UUID".into(),
        ));
    }
    if state.storage_health.writes_blocked() {
        return Err(UploadError::Unavailable(
            "storage is temporarily read-only; retry after operator recovery".into(),
        ));
    }
    let mut conn = state
        .pool
        .get()
        .map_err(|error| UploadError::Internal(error.to_string()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    check_publish_permission(
        &tx,
        state.server_id,
        pseudonym,
        channel,
        AgentAction::SendText,
    )?;
    let current_epoch: Option<i64> = tx.query_row(
        "SELECT token_epoch FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2 AND active=1",
        params![state.server_id,pseudonym], |r|r.get(0),
    ).optional()?;
    if current_epoch != Some(token_epoch) {
        return Err(UploadError::Unauthorized(
            "session was revoked during upload cancellation".into(),
        ));
    }
    let canceled: Option<(String,String)> = tx.query_row(
        "SELECT uploader_pseudonym,channel_id FROM upload_cancellations WHERE server_id=?1 AND upload_id=?2",
        params![state.server_id,upload_id], |r|Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    if let Some((owner, actual)) = canceled {
        if owner != pseudonym || actual != channel {
            return Err(UploadError::NotFound(
                "unpublished attachment not found".into(),
            ));
        }
        tx.commit()?;
        drop(conn);
        drain_deletion_queue(state)?;
        return Ok(());
    }
    let owner: Option<(String,String)> = tx.query_row("SELECT uploader_pseudonym,channel_id FROM uploads WHERE server_id = ?1 AND upload_id = ?2 AND purpose = 'chat'",
        params![state.server_id,upload_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
    if owner
        .as_ref()
        .is_some_and(|(owner, actual)| owner != pseudonym || actual != channel)
    {
        return Err(UploadError::NotFound(
            "unpublished attachment not found".into(),
        ));
    }
    let unknown = owner.is_none();
    if unknown {
        let consumed: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM upload_ids_used WHERE upload_id=?1)",
            [upload_id],
            |r| r.get(0),
        )?;
        if consumed {
            return Err(UploadError::NotFound(
                "unpublished attachment not found".into(),
            ));
        }
        let (member,total):(i64,i64)=tx.query_row(
            "SELECT COALESCE(SUM(uploader_pseudonym=?2),0),COUNT(*) FROM upload_cancellations WHERE server_id=?1 AND was_unpersisted=1 AND canceled_at>unixepoch()-86400",
            params![state.server_id,pseudonym],|r|Ok((r.get(0)?,r.get(1)?)),
        )?;
        if member >= MAX_UNKNOWN_CANCELLATIONS_PER_MEMBER_DAY
            || total >= MAX_UNKNOWN_CANCELLATIONS_PER_SERVER_DAY
        {
            return Err(UploadError::Unavailable("unpersisted upload cancellation capacity reached; retry after the rolling daily window".into()));
        }
    }
    let associated: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM message_uploads WHERE server_id = ?1 AND upload_id = ?2)",
        params![state.server_id, upload_id],
        |row| row.get(0),
    )?;
    if associated {
        return Err(UploadError::Conflict(
            "attachment is already published; delete its message instead".into(),
        ));
    }
    tx.execute("INSERT INTO upload_cancellations(server_id,upload_id,channel_id,uploader_pseudonym,was_unpersisted) VALUES(?1,?2,?3,?4,?5)",
        params![state.server_id,upload_id,channel,pseudonym,unknown])?;
    if unknown {
        tx.execute(
            "INSERT INTO upload_ids_used(server_id,upload_id) VALUES(?1,?2)",
            params![state.server_id, upload_id],
        )?;
    }
    tx.execute(
        "DELETE FROM uploads WHERE server_id = ?1 AND upload_id = ?2",
        params![state.server_id, upload_id],
    )?;
    tx.commit()?;
    drop(conn);
    drain_deletion_queue(state)?;
    Ok(())
}

/// Download permission and disk read share one serialized authorization point.
/// New unpublished uploads are previewable only by their uploader. Legacy
/// files retain their prior member access until association backfill finishes.
pub fn read_upload_for_grant(
    state: &AppState,
    relative: &str,
    upload_id: &str,
    pseudonym: &str,
    token_epoch: i64,
) -> Result<(Vec<u8>, String), UploadError> {
    read_upload_for_device_grant(state, relative, upload_id, pseudonym, token_epoch, None)
}

/// Device-bound reads serialize session, device and epoch entitlement with the
/// actual file read. Ordinary local files remain compatible with v1 grants.
pub fn read_upload_for_device_grant(
    state: &AppState,
    relative: &str,
    upload_id: &str,
    pseudonym: &str,
    token_epoch: i64,
    device_id: Option<&str>,
) -> Result<(Vec<u8>, String), UploadError> {
    let mut conn = state
        .pool
        .get()
        .map_err(|error| UploadError::Internal(error.to_string()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let identity_epoch: Option<i64> = tx.query_row("SELECT token_epoch FROM platform_identities WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 1",
        params![state.server_id,pseudonym], |row| row.get(0)).optional()?;
    if identity_epoch != Some(token_epoch) {
        return Err(UploadError::Unauthorized("grant has been revoked".into()));
    }
    if let Some(device) = device_id {
        super::channel_manifest::require_certified_device(&tx, state, pseudonym, device)
            .map_err(|error| UploadError::Forbidden(error.to_string()))?;
    }
    type Row = (
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        bool,
        Option<i64>,
        Option<String>,
    );
    let upload: Option<Row> = tx.query_row("SELECT channel_id,uploader_pseudonym,lifecycle,storage_path,content_type,size_bytes,ciphertext_sha256,encrypted,epoch,key_id
        FROM uploads WHERE server_id = ?1 AND upload_id = ?2 AND purpose = 'chat'",
        params![state.server_id,upload_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?))).optional()?;
    let Some((
        channel,
        owner,
        lifecycle,
        stored_path,
        content_type,
        size,
        hash,
        encrypted,
        epoch,
        key_id,
    )) = upload
    else {
        return Err(UploadError::NotFound("not found".into()));
    };
    if relative != stored_path
        || lifecycle == "staging"
        || !annex_channels::is_member(&tx, state.server_id, &channel, pseudonym)
            .map_err(|error| UploadError::Internal(error.to_string()))?
    {
        return Err(UploadError::NotFound("not found".into()));
    }
    let published: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM message_uploads u JOIN messages m ON m.message_id = u.message_id
        WHERE u.server_id = ?1 AND u.upload_id = ?2 AND m.server_id = ?1 AND m.channel_id = ?3 AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now')))",
        params![state.server_id,upload_id,channel], |row| row.get(0))?;
    let associated: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM message_uploads WHERE server_id=?1 AND upload_id=?2)",
        params![state.server_id, upload_id],
        |row| row.get(0),
    )?;
    // The owner's unpublished preview privilege cannot resurrect bytes from
    // an expired message while the slower physical retention sweep is pending.
    if associated && !published {
        return Err(UploadError::NotFound("not found".into()));
    }
    if lifecycle != "legacy" && owner != pseudonym && !published {
        return Err(UploadError::NotFound("not found".into()));
    }
    if encrypted {
        let federated_origin:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2)",params![state.server_id,upload_id],|r|r.get(0))?;
        if federated_origin {
            return Err(UploadError::Forbidden(
                "federated origin attachment requires a fresh authority read permit".into(),
            ));
        }
        let device = device_id.ok_or_else(|| {
            UploadError::Forbidden(
                "encrypted attachment requires a current device-bound grant".into(),
            )
        })?;
        super::channel_manifest::require_admitted_device(&tx, state, &channel, pseudonym, device)
            .map_err(|error| UploadError::Forbidden(error.to_string()))?;
        let entitled:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_device_key_wraps WHERE server_id=?1 AND channel_id=?2 AND recipient_pseudonym_id=?3 AND recipient_device_id=?4 AND epoch=?5 AND key_id=?6)",params![state.server_id,channel,pseudonym,device,epoch,key_id],|r|r.get(0))?;
        if !entitled {
            return Err(UploadError::Forbidden(
                "device is not entitled to this attachment's encryption epoch".into(),
            ));
        }
    }
    let path = confined_path(Path::new(&state.upload_dir), relative, false)?;
    let metadata = match fs::metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(UploadError::NotFound("not found".into()))
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file()
        || size < 0
        || metadata.len() != size as u64
        || metadata.len() > (50 * 1024 * 1024 + CIPHERTEXT_OVERHEAD) as u64
    {
        return Err(UploadError::Internal(
            "stored attachment length is invalid".into(),
        ));
    }
    let bytes = fs::read(path)?;
    if let Some(hash) = hash {
        if hex::encode(Sha256::digest(&bytes)) != hash {
            return Err(UploadError::Internal(
                "stored attachment integrity check failed".into(),
            ));
        }
    }
    tx.commit()?;
    Ok((
        bytes,
        if encrypted {
            "application/octet-stream".into()
        } else {
            content_type
        },
    ))
}

/// Branding is deliberately public, but a staging, retired or orphaned file
/// is not the server's published image merely because it exists on disk.
pub fn read_public_server_image(
    state: &AppState,
    filename: &str,
) -> Result<(Vec<u8>, String), UploadError> {
    let relative = format!("server/{filename}");
    let mut conn = state
        .pool
        .get()
        .map_err(|error| UploadError::Internal(error.to_string()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let row:Option<(String,i64,Option<String>)> = tx.query_row("SELECT u.content_type,u.size_bytes,u.ciphertext_sha256 FROM uploads u JOIN servers s ON s.id = u.server_id
        WHERE u.server_id = ?1 AND u.purpose = 'server_image' AND u.storage_path = ?2 AND u.lifecycle != 'staging' AND s.image_url = '/uploads/' || u.storage_path",
        params![state.server_id,relative],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
    let Some((mime, size, hash)) = row else {
        return Err(UploadError::NotFound("not found".into()));
    };
    if !(0..=10 * 1024 * 1024).contains(&size)
        || !["image/jpeg", "image/png", "image/gif", "image/webp"].contains(&mime.as_str())
    {
        return Err(UploadError::Internal(
            "invalid stored server image metadata".into(),
        ));
    }
    let path = confined_path(Path::new(&state.upload_dir), &relative, false)?;
    let metadata = fs::metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            UploadError::NotFound("not found".into())
        } else {
            error.into()
        }
    })?;
    if !metadata.is_file() || metadata.len() != size as u64 {
        return Err(UploadError::Internal(
            "stored server image length is invalid".into(),
        ));
    }
    let bytes = fs::read(path)?;
    if hash.is_some_and(|value| value != hex::encode(Sha256::digest(&bytes))) {
        return Err(UploadError::Internal(
            "stored server image integrity check failed".into(),
        ));
    }
    tx.commit()?;
    Ok((bytes, mime))
}

pub fn drain_deletion_queue(state: &AppState) -> Result<usize, UploadError> {
    let conn = state
        .pool
        .get()
        .map_err(|error| UploadError::Internal(error.to_string()))?;
    let rows = {
        let mut statement = conn.prepare("SELECT relative_path FROM upload_deletion_queue WHERE server_id = ?1 ORDER BY queued_at LIMIT 256")?;
        let rows = statement.query_map([state.server_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut removed = 0;
    for relative in rows {
        let result =
            confined_path(Path::new(&state.upload_dir), &relative, false).and_then(|path| {
                match fs::remove_file(path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error.into()),
                }
            });
        match result {
            Ok(()) => {
                conn.execute(
                    "DELETE FROM upload_deletion_queue WHERE server_id = ?1 AND relative_path = ?2",
                    params![state.server_id, relative],
                )?;
                removed += 1;
            }
            Err(error) => {
                conn.execute("UPDATE upload_deletion_queue SET attempts = attempts + 1, last_error = ?3 WHERE server_id = ?1 AND relative_path = ?2", params![state.server_id,relative,error.to_string()])?;
                tracing::warn!(error = %error, "attachment cleanup will retry");
            }
        }
    }
    Ok(removed)
}

pub fn expire_abandoned_uploads(state: &AppState) -> Result<(), UploadError> {
    let mut conn = state
        .pool
        .get()
        .map_err(|error| UploadError::Internal(error.to_string()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM uploads WHERE server_id = ?1 AND (
        (purpose <> 'federated_chat' AND lifecycle = 'staging' AND created_at < datetime('now', '-1 hour')) OR
        (purpose = 'chat' AND lifecycle = 'ready' AND created_at < datetime('now', '-1 day') AND NOT EXISTS (
            SELECT 1 FROM message_uploads WHERE message_uploads.server_id = uploads.server_id AND message_uploads.upload_id = uploads.upload_id)))",
        [state.server_id])?;
    tx.commit()?;
    Ok(())
}

/// Upgrade old URL-only attachment references without treating at-rest
/// corruption as an empty message. The supplied decryptor must fail on marked
/// invalid ciphertext. Failure rolls back every association/status mutation;
/// historical files remain present and keep their legacy authorization.
pub fn backfill_legacy_associations<F>(state: &AppState, decrypt: F) -> Result<(), UploadError>
where
    F: Fn(&str) -> Result<String, UploadError>,
{
    let mut conn = state
        .pool
        .get()
        .map_err(|error| UploadError::Internal(error.to_string()))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let complete: bool = tx.query_row("SELECT COALESCE((SELECT legacy_associations_complete FROM upload_migration_state WHERE server_id = ?1),0)",
        [state.server_id], |row| row.get(0))?;
    if complete {
        return Ok(());
    }
    let mut by_channel = std::collections::HashMap::<String, Vec<(String, String)>>::new();
    {
        let mut statement = tx.prepare("SELECT channel_id,upload_id,storage_path FROM uploads WHERE server_id = ?1 AND purpose = 'chat' AND lifecycle = 'legacy' AND channel_id IS NOT NULL")?;
        let rows = statement.query_map([state.server_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (channel, id, path) = row?;
            by_channel
                .entry(channel)
                .or_default()
                .push((id, format!("/uploads/{path}")));
        }
    }
    for (channel, uploads) in by_channel {
        // Keep historical associations even when their message already expired:
        // they prevent the uploader-preview exception from reviving old bytes,
        // and allow the physical retention sweep to reclaim the linked file.
        // No expired message body is returned to a caller by this migration.
        let mut statement = tx.prepare("SELECT message_id,content FROM messages WHERE server_id = ?1 AND channel_id = ?2 AND deleted_at IS NULL ORDER BY id")?;
        let rows = statement.query_map(params![state.server_id, channel], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (message, body) = row?;
            let plaintext = decrypt(&body)?;
            for (upload, url) in &uploads {
                if plaintext.contains(url) {
                    tx.execute("INSERT OR IGNORE INTO message_uploads(server_id,message_id,upload_id) VALUES (?1,?2,?3)",params![state.server_id,message,upload])?;
                }
            }
        }
    }
    tx.execute("UPDATE uploads SET lifecycle = 'ready' WHERE server_id = ?1 AND purpose = 'chat' AND lifecycle = 'legacy'",[state.server_id])?;
    tx.execute(
        "INSERT INTO upload_migration_state(server_id,legacy_associations_complete) VALUES (?1,1)
        ON CONFLICT(server_id) DO UPDATE SET legacy_associations_complete = 1",
        [state.server_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub async fn start_upload_cleanup_task(state: Arc<AppState>) {
    loop {
        let current = state.clone();
        match tokio::task::spawn_blocking(move || {
            let backfill = backfill_legacy_associations(&current, |body| {
                current
                    .message_cipher()
                    .decrypt(body)
                    .map_err(|error| UploadError::Internal(error.to_string()))
            });
            // Legacy records remain excluded from expiry if their backfill
            // cannot authenticate stored message bodies. Independent deletion
            // obligations still run, so one damaged history row does not keep
            // newly deleted files indefinitely.
            expire_abandoned_uploads(&current)?;
            let removed = drain_deletion_queue(&current)?;
            backfill?;
            Ok::<usize, UploadError>(removed)
        })
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::error!(error = %error, "attachment cleanup failed; persisted obligations retained")
            }
            Err(error) => tracing::error!(error = %error, "attachment cleanup task failed"),
        }
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {},
        }
    }
}
