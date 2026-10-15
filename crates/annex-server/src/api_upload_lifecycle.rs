//! HTTP parsing for staged ordinary and opaque encrypted attachments.
//! Companions in upload_service own the transactional authorization and disk
//! lifecycle. Encrypted payloads are deliberately never MIME-sniffed here.

use crate::{
    api::ApiError,
    api_upload::{self, UploadCategory},
    middleware::IdentityContext,
    services::upload_service::{self, UploadMetadata},
    AppState,
};
use axum::{
    extract::{Extension, Multipart, Path},
    response::{IntoResponse, Response},
    Json,
};
use std::{collections::HashMap, sync::Arc};

const STREAM_CEILING: usize = 50 * 1024 * 1024;

async fn authorize(
    state: Arc<AppState>,
    channel: String,
    identity: annex_identity::PlatformIdentity,
    epoch: Option<(i64, String)>,
) -> Result<annex_types::ServerPolicy, ApiError> {
    tokio::task::spawn_blocking(move || {
        let mut conn = state
            .pool
            .get()
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
        let policy = upload_service::authorize_upload(
            &tx,
            &state,
            &channel,
            &identity.pseudonym_id,
            identity.token_epoch,
            epoch.as_ref().map(|(epoch, key)| (*epoch, key.as_str())),
        )?;
        tx.commit()
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
        Ok(policy)
    })
    .await
    .map_err(|error| ApiError::InternalServerError(error.to_string()))?
}

pub async fn upload_chat_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let policy = authorize(state.clone(), channel.clone(), identity.clone(), None).await?;
    let mut field = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(format!("multipart error: {error}")))?
        .ok_or_else(|| ApiError::BadRequest("no file provided".into()))?;
    // A caller can reserve the public opaque ID before sending bytes, allowing
    // an independent DELETE to cancel even after commit but before the ACK.
    // Legacy clients which send only a file retain generated-ID behavior.
    let upload_id = if field.name() == Some("attachment_id") {
        if field.file_name().is_some() {
            return Err(ApiError::BadRequest(
                "attachment_id must be a text field".into(),
            ));
        }
        let bytes = api_upload::read_field_capped(&mut field, 36).await?;
        let id = String::from_utf8(bytes)
            .map_err(|_| ApiError::BadRequest("attachment_id must be UTF-8".into()))?;
        if !upload_service::canonical_uuid(&id) {
            return Err(ApiError::BadRequest(
                "attachment_id must be a canonical UUID".into(),
            ));
        }
        drop(field);
        field = multipart
            .next_field()
            .await
            .map_err(|error| ApiError::BadRequest(format!("multipart error: {error}")))?
            .ok_or_else(|| ApiError::BadRequest("no file provided".into()))?;
        id
    } else {
        uuid::Uuid::new_v4().to_string()
    };
    if field.name() != Some("file") {
        return Err(ApiError::BadRequest("expected one file field".into()));
    }
    let original = field.file_name().unwrap_or("upload").to_string();
    if original.len() > 512 || original.contains('\0') {
        return Err(ApiError::BadRequest(
            "filename is invalid or too long".into(),
        ));
    }
    let bytes = api_upload::read_field_capped(&mut field, STREAM_CEILING).await?;
    drop(field);
    if multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?
        .is_some()
    {
        return Err(ApiError::BadRequest(
            "only one attachment per upload request is supported".into(),
        ));
    }
    let (content_type, category) = api_upload::detect_content_type(&bytes)
        .unwrap_or(("application/octet-stream", UploadCategory::File));
    if api_upload::BLOCKED_TYPES.contains(&content_type) {
        return Err(ApiError::BadRequest("file type is not allowed".into()));
    }
    if category == UploadCategory::Image && !api_upload::ALLOWED_IMAGE_TYPES.contains(&content_type)
        || category == UploadCategory::Video
            && !api_upload::ALLOWED_VIDEO_TYPES.contains(&content_type)
    {
        return Err(ApiError::BadRequest(
            "unsupported attachment media type".into(),
        ));
    }
    let limit = upload_service::policy_limit(&policy, category.as_str(), false)?;
    if bytes.len() > limit {
        return Err(ApiError::BadRequest(format!(
            "file too large: maximum {limit} bytes"
        )));
    }
    let cleaned = api_upload::strip_metadata(&bytes, content_type)?;
    let stripped = bytes.len().saturating_sub(cleaned.len());
    let size = cleaned.len();
    let upload = UploadMetadata {
        upload_id,
        channel_id: channel,
        uploader: identity.pseudonym_id,
        token_epoch: identity.token_epoch,
        category: category.as_str().into(),
        original_filename: original.clone(),
        content_type: content_type.into(),
        extension: api_upload::ext_from_content_type(content_type).into(),
        encrypted_epoch: None,
    };
    let upload_id = upload.upload_id.clone();
    let (url, hash) = tokio::task::spawn_blocking(move || {
        upload_service::persist_chat_upload(&state, &upload, &cleaned)
    })
    .await
    .map_err(|error| ApiError::InternalServerError(error.to_string()))??;
    Ok(Json(serde_json::json!({ "status":"ok", "upload_id":upload_id,"url":url,"filename":original,
        "content_type":content_type,"category":category.as_str(),"size":size,"metadata_stripped_bytes":stripped,
        "encrypted":false,"epoch":null,"key_id":null,"ciphertext_sha256":hash })).into_response())
}

pub async fn upload_encrypted_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    // Even the small metadata fields are read only for a current publisher.
    let check_state = state.clone();
    let check_channel = channel.clone();
    let check_identity = identity.pseudonym_id.clone();
    tokio::task::spawn_blocking(move || {
        let conn = check_state
            .pool
            .get()
            .map_err(|error| ApiError::InternalServerError(error.to_string()))?;
        crate::services::channel_service::check_publish_permission(
            &conn,
            check_state.server_id,
            &check_identity,
            &check_channel,
            crate::services::agent_policy::AgentAction::SendText,
        )
        .map_err(ApiError::from)
    })
    .await
    .map_err(|error| ApiError::InternalServerError(error.to_string()))??;
    let mut metadata = HashMap::<String, String>::new();
    let mut ciphertext: Option<Vec<u8>> = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::BadRequest(format!("multipart error: {error}")))?
    {
        let name = field
            .name()
            .ok_or_else(|| ApiError::BadRequest("every upload field must be named".into()))?
            .to_string();
        if name == "file" {
            if ciphertext.is_some() || metadata.len() != 4 {
                return Err(ApiError::BadRequest("send attachment_id, epoch, key_id and category before the single ciphertext file".into()));
            }
            let (id, epoch, key, category) = encrypted_metadata(&metadata)?;
            if field.content_type() != Some("application/octet-stream")
                || !matches!(field.file_name(), Some("blob"))
                    && field.file_name() != Some(format!("{id}.bin").as_str())
            {
                return Err(ApiError::BadRequest("ciphertext requires application/octet-stream and an opaque blob or UUID.bin filename".into()));
            }
            let policy = authorize(
                state.clone(),
                channel.clone(),
                identity.clone(),
                Some((epoch, key)),
            )
            .await?;
            let limit = upload_service::policy_limit(&policy, &category, true)?
                .min(STREAM_CEILING + upload_service::CIPHERTEXT_OVERHEAD);
            let bytes = api_upload::read_field_capped(&mut field, limit).await?;
            if bytes.len() < upload_service::CIPHERTEXT_OVERHEAD {
                return Err(ApiError::BadRequest(
                    "ciphertext must include a nonce and authentication tag".into(),
                ));
            }
            ciphertext = Some(bytes);
        } else {
            if ciphertext.is_some()
                || !["attachment_id", "epoch", "key_id", "category"].contains(&name.as_str())
                || metadata.contains_key(&name)
                || field.file_name().is_some()
            {
                return Err(ApiError::BadRequest(
                    "unexpected or duplicate encrypted upload field".into(),
                ));
            }
            let bytes = api_upload::read_field_capped(&mut field, 128).await?;
            let value = String::from_utf8(bytes)
                .map_err(|_| ApiError::BadRequest("upload metadata must be UTF-8".into()))?;
            metadata.insert(name, value);
        }
    }
    let ciphertext =
        ciphertext.ok_or_else(|| ApiError::BadRequest("no ciphertext file provided".into()))?;
    let (id, epoch, key, category) = encrypted_metadata(&metadata)?;
    let size = ciphertext.len();
    let upload = UploadMetadata {
        upload_id: id.clone(),
        channel_id: channel,
        uploader: identity.pseudonym_id,
        token_epoch: identity.token_epoch,
        category: category.clone(),
        original_filename: "encrypted.bin".into(),
        content_type: "application/octet-stream".into(),
        extension: "bin".into(),
        encrypted_epoch: Some((epoch, key.clone())),
    };
    let origin_identity = crate::services::federated_upload_types::stable_identity(
        &state.signing_key.verifying_key().to_bytes(),
    );
    let originating_server = state.get_public_url().trim_end_matches('/').to_owned();
    let (url, hash) = tokio::task::spawn_blocking(move || {
        upload_service::persist_chat_upload(&state, &upload, &ciphertext)
    })
    .await
    .map_err(|error| ApiError::InternalServerError(error.to_string()))??;
    Ok(Json(serde_json::json!({ "status":"ok","upload_id":id,"url":url,"filename":"encrypted.bin",
        "content_type":"application/octet-stream","category":category,"size":size,"metadata_stripped_bytes":0,
        "encrypted":true,"epoch":epoch,"key_id":key,"ciphertext_sha256":hash,
        "origin_identity":origin_identity,"originating_server":originating_server })).into_response())
}

fn encrypted_metadata(
    fields: &HashMap<String, String>,
) -> Result<(String, i64, String, String), ApiError> {
    let get = |name: &str| {
        fields
            .get(name)
            .cloned()
            .ok_or_else(|| ApiError::BadRequest(format!("missing {name}")))
    };
    let id = get("attachment_id")?;
    if !upload_service::canonical_uuid(&id) {
        return Err(ApiError::BadRequest(
            "attachment_id must be a canonical UUID".into(),
        ));
    }
    let encoded_epoch = get("epoch")?;
    let epoch = encoded_epoch
        .parse::<i64>()
        .map_err(|_| ApiError::BadRequest("epoch must be a positive canonical integer".into()))?;
    if epoch <= 0 || epoch > 9_007_199_254_740_991 || epoch.to_string() != encoded_epoch {
        return Err(ApiError::BadRequest(
            "epoch must be a positive canonical integer".into(),
        ));
    }
    let key = get("key_id")?;
    if key.len() != 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ApiError::BadRequest(
            "key_id must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    let category = get("category")?;
    upload_service::category_directory(&category)?;
    Ok((id, epoch, key, category))
}

pub async fn abort_upload_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path((channel, upload_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    tokio::task::spawn_blocking(move || {
        upload_service::abort_upload(
            &state,
            &channel,
            &identity.pseudonym_id,
            &upload_id,
            identity.token_epoch,
        )
    })
    .await
    .map_err(|error| ApiError::InternalServerError(error.to_string()))??;
    Ok(Json(serde_json::json!({"status":"ok"})).into_response())
}
