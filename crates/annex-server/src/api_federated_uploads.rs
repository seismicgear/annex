//! Device-bound grants and the bounded federation blob RPC. Registered only
//! with the matching authority and replica storage services.
use crate::services::{
    federated_upload_service::{self, TransferError},
    federated_upload_types::{self as wire, ChunkRequest, ChunkResponse},
};
use crate::{
    api::ApiError,
    api_uploads_access::{UploadGrantResponse, UPLOAD_GRANT_TTL_SECS},
    middleware::IdentityContext,
    AppState,
};
use axum::{
    extract::{Extension, Path, Query},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::sync::Arc;
use subtle::ConstantTimeEq;

impl IntoResponse for TransferError {
    fn into_response(self) -> Response {
        (
            self.status_code(),
            Json(serde_json::json!({"error":self.to_string()})),
        )
            .into_response()
    }
}
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GrantRequest {
    pub device_id: Option<String>,
}
pub struct DeviceGrant {
    pub pseudonym: String,
    pub token_epoch: i64,
    pub device_id: String,
}
fn mac(payload: &str, secret: &[u8; 32]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts a 32-byte key");
    mac.update(b"annex-device-upload-grant-v2\0");
    mac.update(payload.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}
pub fn generate_device_grant(
    pseudonym: &str,
    device: &str,
    epoch: i64,
    secret: &[u8; 32],
    ttl: u64,
) -> Result<String, TransferError> {
    if pseudonym.is_empty()
        || pseudonym.len() > 256
        || pseudonym.contains('|')
        || pseudonym.chars().any(char::is_control)
        || !wire::hex_32(device)
        || epoch < 0
        || ttl > UPLOAD_GRANT_TTL_SECS
    {
        return Err(TransferError::BadRequest(
            "invalid device upload grant identity or lifetime".into(),
        ));
    }
    let expires = chrono::Utc::now()
        .timestamp()
        .checked_add(ttl as i64)
        .ok_or_else(|| TransferError::BadRequest("grant expiry overflow".into()))?;
    let payload = format!("v2|{pseudonym}|{epoch}|{device}|{expires}");
    Ok(URL_SAFE_NO_PAD.encode(format!("{payload}|{}", mac(&payload, secret))))
}
pub fn verify_device_grant(token: &str, secret: &[u8; 32]) -> Option<DeviceGrant> {
    if token.len() > 1024 {
        return None;
    }
    let raw = URL_SAFE_NO_PAD.decode(token).ok()?;
    if URL_SAFE_NO_PAD.encode(&raw) != token {
        return None;
    }
    let text = std::str::from_utf8(&raw).ok()?;
    let (payload, signature) = text.rsplit_once('|')?;
    let expected = mac(payload, secret);
    if !bool::from(expected.as_bytes().ct_eq(signature.as_bytes())) {
        return None;
    }
    let fields = payload.split('|').collect::<Vec<_>>();
    if fields.len() != 5
        || fields[0] != "v2"
        || fields[1].is_empty()
        || fields[1].len() > 256
        || fields[1].chars().any(char::is_control)
        || !wire::hex_32(fields[3])
    {
        return None;
    }
    let epoch: i64 = fields[2].parse().ok()?;
    let expires: i64 = fields[4].parse().ok()?;
    let now = chrono::Utc::now().timestamp();
    if epoch < 0
        || epoch.to_string() != fields[2]
        || expires.to_string() != fields[4]
        || expires <= now
        || expires > now + UPLOAD_GRANT_TTL_SECS as i64
    {
        return None;
    }
    Some(DeviceGrant {
        pseudonym: fields[1].into(),
        token_epoch: epoch,
        device_id: fields[3].into(),
    })
}

/// Replaces the existing route handler without changing its response fields.
/// An omitted body retains the ordinary/local v1 grant; a specified device is
/// validated against the caller's current server identity before signing v2.
pub async fn issue_upload_grant(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    body: Option<Json<GrantRequest>>,
) -> Result<Json<UploadGrantResponse>, ApiError> {
    let device = body.and_then(|Json(body)| body.device_id);
    let token = if let Some(device) = device {
        if !wire::hex_32(&device) {
            return Err(ApiError::BadRequest("invalid device_id".into()));
        }
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        let current:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM member_device_keys d JOIN platform_identities p ON p.server_id=d.server_id AND p.pseudonym_id=d.pseudonym_id WHERE d.server_id=?1 AND d.pseudonym_id=?2 AND d.device_id=?3 AND d.revoked_at IS NULL AND p.active=1 AND p.token_epoch=?4)",rusqlite::params![state.server_id,identity.pseudonym_id,device,identity.token_epoch],|r|r.get(0)).map_err(|e|ApiError::InternalServerError(e.to_string()))?;
        if !current {
            return Err(ApiError::Forbidden(
                "device is not active for this current session identity".into(),
            ));
        }
        crate::services::channel_manifest::require_certified_device(
            &conn,
            &state,
            &identity.pseudonym_id,
            &device,
        )
        .map_err(|error| ApiError::Forbidden(error.to_string()))?;
        generate_device_grant(
            &identity.pseudonym_id,
            &device,
            identity.token_epoch,
            &state.ws_token_secret,
            UPLOAD_GRANT_TTL_SECS,
        )
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
    } else {
        crate::api_uploads_access::generate_upload_grant(
            &identity.pseudonym_id,
            &state.ws_token_secret,
            UPLOAD_GRANT_TTL_SECS,
            identity.token_epoch,
        )
    };
    Ok(Json(UploadGrantResponse {
        token,
        expires_in_secs: UPLOAD_GRANT_TTL_SECS,
    }))
}

pub async fn chunk_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<ChunkRequest>,
) -> Result<Json<ChunkResponse>, TransferError> {
    federated_upload_service::serve_chunk(state, request)
        .await
        .map(Json)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryTransferRequest {
    pub device_id: String,
}

pub async fn retry_member_transfer(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path((channel, attachment)): Path<(String, String)>,
    Json(request): Json<RetryTransferRequest>,
) -> Result<Json<serde_json::Value>, TransferError> {
    let status = federated_upload_service::retry_for_member(
        state,
        &channel,
        &attachment,
        &identity.pseudonym_id,
        &request.device_id,
        identity.token_epoch,
    )
    .await?;
    Ok(Json(
        serde_json::json!({"status":status,"attachment_id":attachment}),
    ))
}

/// Keeps the existing media URL shape. Replica files cannot fall through to
/// the local-only reader, even when a caller supplies an otherwise valid v1
/// grant. V2 local reads also check device revocation in their writer snapshot.
pub async fn serve_chat_upload(
    Extension(state): Extension<Arc<AppState>>,
    Path((category, filename)): Path<(String, String)>,
    Query(query): Query<crate::api_uploads_access::GrantQuery>,
) -> Response {
    use axum::http::{header, StatusCode};
    let Some(token) = query.t.as_deref() else {
        return (
            StatusCode::UNAUTHORIZED,
            "attachment requires a current upload grant",
        )
            .into_response();
    };
    let (pseudonym, epoch, device) =
        if let Some(grant) = verify_device_grant(token, &state.ws_token_secret) {
            (grant.pseudonym, grant.token_epoch, Some(grant.device_id))
        } else if let Some(grant) =
            crate::api_uploads_access::verify_upload_grant(token, &state.ws_token_secret)
        {
            (grant.pseudonym, grant.epoch, None)
        } else {
            return (StatusCode::UNAUTHORIZED, "grant is invalid or expired").into_response();
        };
    fn safe(value: &str) -> bool {
        !value.is_empty()
            && value.len() <= 256
            && !value.contains("..")
            && !value
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '\0'))
    }
    if !safe(&category) || !safe(&filename) {
        return (StatusCode::BAD_REQUEST, "invalid attachment path").into_response();
    }
    let upload_id = filename.split('.').next().unwrap_or("").to_owned();
    let relative = format!("chat/{category}/{filename}");
    if let Some(device) = device.as_deref() {
        if let Err(error) = federated_upload_service::admit_deferred_for_reader(
            state.clone(),
            &upload_id,
            None,
            Some(&relative),
            &pseudonym,
            device,
            epoch,
        )
        .await
        {
            return error.into_response();
        }
    }
    let location = (|| -> Result<(bool, bool), TransferError> {
        let conn = state
            .pool
            .get()
            .map_err(|e| TransferError::Internal(e.to_string()))?;
        Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM federated_upload_transfers WHERE server_id=?1 AND upload_id=?2),EXISTS(SELECT 1 FROM federated_upload_origins WHERE server_id=?1 AND upload_id=?2)", rusqlite::params![state.server_id, upload_id], |r| Ok((r.get(0)?,r.get(1)?)))?)
    })();
    let result = match location {
        Err(error) => return error.into_response(),
        Ok((true, _)) => {
            let Some(device) = device else {
                return (
                    StatusCode::FORBIDDEN,
                    "federated attachment requires a current device-bound grant",
                )
                    .into_response();
            };
            match federated_upload_service::read_replica_for_grant(
                state, &relative, &upload_id, &pseudonym, &device, epoch,
            )
            .await
            {
                Ok(value) => value,
                Err(error) => return error.into_response(),
            }
        }
        Ok((false, true)) => {
            let Some(device) = device else {
                return (
                    StatusCode::FORBIDDEN,
                    "federated attachment requires a current device-bound grant",
                )
                    .into_response();
            };
            match federated_upload_service::read_origin_for_grant(
                state, &relative, &upload_id, &pseudonym, &device, epoch,
            )
            .await
            {
                Ok(value) => value,
                Err(error) => return error.into_response(),
            }
        }
        Ok((false, false)) => {
            match tokio::task::spawn_blocking(move || {
                crate::services::upload_service::read_upload_for_device_grant(
                    &state,
                    &relative,
                    &upload_id,
                    &pseudonym,
                    epoch,
                    device.as_deref(),
                )
            })
            .await
            {
                Ok(Ok(value)) => value,
                Ok(Err(error)) => return ApiError::from(error).into_response(),
                Err(_) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "attachment read task failed",
                    )
                        .into_response()
                }
            }
        }
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, result.1),
            (header::CACHE_CONTROL, "private, no-store".into()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
        ],
        result.0,
    )
        .into_response()
}

const OPERATOR_UNION:&str="SELECT o.id,o.peer_instance_id,o.message_id,o.status,o.attempts,o.next_retry_at,o.last_error,o.created_at,o.updated_at,LENGTH(o.envelope_json) AS envelope_bytes,o.envelope_kind AS operation_kind,NULL AS attachment_id,NULL AS transferred_bytes,NULL AS total_bytes FROM federation_outbox o UNION ALL SELECT -f.id,f.origin_peer_id,f.message_id,CASE f.status WHEN 'receiving' THEN 'pending' WHEN 'ready' THEN 'delivered' WHEN 'removed' THEN 'cancelled' ELSE f.status END,f.attempts,datetime(f.next_retry_at,'unixepoch'),f.last_error,datetime(f.created_at,'unixepoch'),datetime(f.updated_at,'unixepoch'),f.size_bytes,'attachment',f.upload_id,f.committed_bytes,f.size_bytes FROM federated_upload_transfers f WHERE f.server_id=?1";

/// Both outbound envelopes and inbound verified blobs use one existing
/// operator surface. Pagination and counts apply to the same combined rows.
pub async fn list_delivery(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Query(query): Query<crate::api_admin::OutboxListParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to inspect federation delivery".into(),
        ));
    }
    if query.status.as_ref().is_some_and(|s| {
        !matches!(
            s.as_str(),
            "pending" | "delivered" | "failed" | "paused" | "cancelled"
        )
    }) {
        return Err(ApiError::BadRequest(
            "invalid delivery status filter".into(),
        ));
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0).max(0);
    let state = state.clone();
    tokio::task::spawn_blocking(move||{
        let conn=state.pool.get().map_err(|e|ApiError::InternalServerError(e.to_string()))?;
        let db_error=|e:rusqlite::Error|ApiError::InternalServerError(e.to_string());
        let mut counts=serde_json::Map::new();
        {let mut stmt=conn.prepare(&format!("SELECT status,COUNT(*) FROM({OPERATOR_UNION}) GROUP BY status")).map_err(db_error)?;
        for row in stmt.query_map([state.server_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).map_err(db_error)?{let (status,count)=row.map_err(db_error)?;counts.insert(status,count.into());}}
        let sql=format!("SELECT d.id,d.peer_instance_id,i.base_url,i.label,d.message_id,d.status,d.attempts,d.next_retry_at,d.last_error,d.created_at,d.updated_at,d.envelope_bytes,d.operation_kind,d.attachment_id,d.transferred_bytes,d.total_bytes FROM({OPERATOR_UNION}) d LEFT JOIN instances i ON i.id=d.peer_instance_id WHERE (?2 IS NULL OR d.status=?2) ORDER BY d.created_at DESC,d.id DESC LIMIT ?3 OFFSET ?4");
        let mut stmt=conn.prepare(&sql).map_err(db_error)?;
        let rows=stmt.query_map(rusqlite::params![state.server_id,query.status,limit,offset],|r|Ok(serde_json::json!({
            "id":r.get::<_,i64>(0)?,"peer_instance_id":r.get::<_,i64>(1)?,"peer_base_url":r.get::<_,Option<String>>(2)?,"peer_label":r.get::<_,Option<String>>(3)?,"message_id":r.get::<_,String>(4)?,"status":r.get::<_,String>(5)?,"attempts":r.get::<_,i64>(6)?,"next_retry_at":r.get::<_,String>(7)?,"last_error":r.get::<_,Option<String>>(8)?,"created_at":r.get::<_,String>(9)?,"updated_at":r.get::<_,String>(10)?,"envelope_bytes":r.get::<_,i64>(11)?,"operation_kind":r.get::<_,String>(12)?,"attachment_id":r.get::<_,Option<String>>(13)?,"transferred_bytes":r.get::<_,Option<u64>>(14)?,"total_bytes":r.get::<_,Option<u64>>(15)?
        }))).map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?;
        Ok(Json(serde_json::json!({"entries":rows,"counts":counts,"limit":limit,"offset":offset})))
    }).await.map_err(|e|ApiError::InternalServerError(e.to_string()))?
}

pub async fn retry_delivery(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    if id >= 0 {
        return crate::api_admin::retry_federation_outbox_handler(
            Extension(state),
            Extension(IdentityContext(identity)),
            Path(id),
        )
        .await;
    }
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to retry attachment delivery".into(),
        ));
    }
    let transfer_id = id
        .checked_neg()
        .ok_or_else(|| ApiError::BadRequest("invalid delivery id".into()))?;
    let conn = state
        .pool
        .get()
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
    federated_upload_service::retry_transfer(&conn, state.server_id, transfer_id)?;
    let message: String = conn
        .query_row(
            "SELECT message_id FROM federated_upload_transfers WHERE server_id=?1 AND id=?2",
            rusqlite::params![state.server_id, transfer_id],
            |r| r.get(0),
        )
        .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
    crate::emit_and_broadcast(&conn,state.server_id,&identity.pseudonym_id,&annex_observe::EventPayload::ModerationAction{moderator_pseudonym:identity.pseudonym_id.clone(),action_type:"federation_attachment_retry".into(),target_pseudonym:None,description:format!("Attachment transfer {transfer_id} for message {message} returned to retry rotation")},&state.observe_tx,&state.signing_key);
    Ok(Json(serde_json::json!({"status":"ok","outbox_id":id,"message_id":message,"new_status":"pending"})).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_grants_bind_identity_device_epoch_expiry_and_secret() {
        let secret = [9u8; 32];
        let device = "ab".repeat(32);
        let token = generate_device_grant("alice", &device, 17, &secret, 300).unwrap();
        let verified = verify_device_grant(&token, &secret).unwrap();
        assert_eq!(verified.pseudonym, "alice");
        assert_eq!(verified.device_id, device);
        assert_eq!(verified.token_epoch, 17);
        assert!(verify_device_grant(&token, &[8u8; 32]).is_none());
        let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(&token).unwrap()).unwrap();
        for altered in [
            raw.replace("alice", "bob"),
            raw.replace("|17|", "|18|"),
            raw.replace(&device, &"cd".repeat(32)),
        ] {
            assert!(verify_device_grant(&URL_SAFE_NO_PAD.encode(altered), &secret).is_none());
        }
    }
    #[test]
    fn legacy_and_device_grants_have_distinct_formats_and_domains() {
        let secret = [4u8; 32];
        let device = "ab".repeat(32);
        let legacy = crate::api_uploads_access::generate_upload_grant("alice", &secret, 300, 0);
        assert!(verify_device_grant(&legacy, &secret).is_none());
        let device_grant = generate_device_grant("alice", &device, 0, &secret, 300).unwrap();
        assert!(crate::api_uploads_access::verify_upload_grant(&device_grant, &secret).is_none());
        let expired = generate_device_grant("alice", &device, 0, &secret, 0).unwrap();
        assert!(verify_device_grant(&expired, &secret).is_none());
    }
    #[test]
    fn grant_validation_refuses_ambiguous_fields_oversize_and_unbounded_lifetime() {
        assert!(
            generate_device_grant("alice|another", &"ab".repeat(32), 0, &[0; 32], 300).is_err()
        );
        assert!(generate_device_grant("alice", &"AB".repeat(32), 0, &[0; 32], 300).is_err());
        assert!(generate_device_grant("alice", &"ab".repeat(32), -1, &[0; 32], 300).is_err());
        assert!(generate_device_grant(
            "alice",
            &"ab".repeat(32),
            0,
            &[0; 32],
            UPLOAD_GRANT_TTL_SECS + 1
        )
        .is_err());
        assert!(verify_device_grant(&"a".repeat(1025), &[0; 32]).is_none());
    }
}
