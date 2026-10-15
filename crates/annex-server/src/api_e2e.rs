//! Versioned, device-scoped channel key distribution. Clients alone open keys.
use crate::{
    middleware::IdentityContext,
    services::{
        channel_replication as replication,
        e2e_service::{self as e2e, E2eError, PageQuery, PostWrapsRequest},
    },
    AppState,
};
use annex_identity::PlatformIdentity;
use axum::{
    extract::{Extension, Path, Query},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

async fn transaction<F>(
    state: Arc<AppState>,
    identity: PlatformIdentity,
    operation: F,
) -> Result<Value, E2eError>
where
    F: FnOnce(&rusqlite::Transaction<'_>, &AppState, &str) -> Result<Value, E2eError>
        + Send
        + 'static,
{
    tokio::task::spawn_blocking(move||{
        let mut conn=state.pool.get().map_err(|e|E2eError::Internal(e.to_string()))?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let epoch:Option<i64>=tx.query_row("SELECT token_epoch FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2 AND active=1",params![state.server_id,identity.pseudonym_id],|r|r.get(0)).optional()?;
        if epoch!=Some(identity.token_epoch) {return Err(E2eError::Forbidden("session is inactive or revoked".into()));}
        let value=operation(&tx,&state,&identity.pseudonym_id)?;
        tx.commit()?;Ok(value)
    }).await.map_err(|e|E2eError::Internal(e.to_string()))?
}
fn affected(conn: &Connection, state: &AppState, pid: &str) -> Result<Vec<Value>, E2eError> {
    let channels = {
        let mut stmt=conn.prepare("SELECT channel_id FROM channel_members WHERE server_id=?1 AND pseudonym_id=?2 ORDER BY channel_id")?;
        let rows = stmt
            .query_map(params![state.server_id, pid], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    channels
        .into_iter()
        .map(|channel| {
            let mut v = json!(e2e::read_state(conn, state, &channel)?);
            v["channel_id"] = json!(channel);
            Ok(v)
        })
        .collect()
}
async fn notify_affected(state: Arc<AppState>, value: &Value) {
    if let Some(channels) = value["affected_channels"].as_array() {
        for channel in channels {
            if let Some(id) = channel["channel_id"].as_str() {
                if let Err(error) = e2e::notify_channel(state.clone(), id.to_owned()).await {
                    tracing::warn!(%error,"key notification deferred");
                }
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutMyKeyRequest {
    pub version: u8,
    pub x25519_pub_hex: String,
    pub device_certificate: Option<Value>,
}
pub async fn get_certificate_context_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, E2eError> {
    Ok(Json(
        transaction(state, identity, |conn, state, pid| {
            crate::device_certificates::context(conn, state, pid)
        })
        .await?,
    )
    .into_response())
}
pub async fn put_my_key_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(req): Json<PutMyKeyRequest>,
) -> Result<Response, E2eError> {
    if req.version != 2 {
        return Err(E2eError::BadRequest(
            "version2 with a verified device certificate is required".into(),
        ));
    }
    let id = e2e::device_id(&req.x25519_pub_hex)?;
    let certificate: crate::device_certificates::DeviceCertificate =
        serde_json::from_value(req.device_certificate.ok_or_else(|| {
            E2eError::BadRequest("a verified device certificate is required".into())
        })?)
        .map_err(|_| E2eError::BadRequest("malformed device certificate".into()))?;
    if certificate.device_id != id || certificate.x25519_pub_hex != req.x25519_pub_hex {
        return Err(E2eError::BadRequest(
            "certificate must bind the published device key".into(),
        ));
    }
    let result = transaction(state.clone(), identity, move |conn, state, pid| {
        let created = crate::device_certificates::publish(conn, state, pid, &certificate)?;
        Ok(json!({"device_id":id,"created":created,"affected_channels":affected(conn,state,pid)?}))
    })
    .await?;
    notify_affected(state, &result).await;
    Ok(Json(result).into_response())
}
pub async fn revoke_my_device_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(id): Path<String>,
) -> Result<Response, E2eError> {
    if !e2e::lower_hex(&id) {
        return Err(E2eError::BadRequest("invalid device_id".into()));
    }
    let result=transaction(state.clone(),identity,move|conn,state,pid|{
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3)",params![state.server_id,pid,id],|r|r.get(0))?;
        if !exists {return Err(E2eError::NotFound("device not found".into()));}
        crate::services::channel_manifest::require_signed_device_departure(conn,state,pid,&id)?;
        conn.execute("UPDATE member_device_keys SET revoked_at=datetime('now') WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3 AND revoked_at IS NULL",params![state.server_id,pid,id])?;
        Ok(json!({"device_id":id,"revoked":true,"affected_channels":affected(conn,state,pid)?}))
    }).await?;
    notify_affected(state, &result).await;
    Ok(Json(result).into_response())
}
pub async fn get_member_key_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(pid): Path<String>,
) -> Result<Response, E2eError> {
    let result=transaction(state,identity,move|conn,state,_|{
        e2e::import_legacy_devices(conn,state.server_id)?;
        let mut stmt=conn.prepare("SELECT d.device_id,d.x25519_pub_hex,d.device_certificate FROM member_device_keys d JOIN platform_identities p ON p.server_id=d.server_id AND p.pseudonym_id=d.pseudonym_id WHERE d.server_id=?1 AND d.pseudonym_id=?2 AND p.active=1 AND d.revoked_at IS NULL ORDER BY d.device_id")?;
        let raw=stmt.query_map(params![state.server_id,pid],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        let rows=raw.into_iter().map(|(id,key,certificate)| {
            let certificate=certificate.map(|value|serde_json::from_str::<crate::device_certificates::DeviceCertificate>(&value)
                .map_err(|_|E2eError::Internal("stored device certificate is corrupt".into()))).transpose()?;
            Ok(json!({"device_id":id,"x25519_pub_hex":key,"device_certificate":certificate}))
        }).collect::<Result<Vec<Value>,E2eError>>()?;
        Ok(json!({"pseudonym_id":pid,"devices":rows}))
    }).await?;
    Ok(Json(result).into_response())
}
pub async fn list_channel_member_keys_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, E2eError> {
    replication::refresh_for_request(state.clone(), &identity, &channel, None)
        .await
        .map_err(authority_error)?;
    Ok(Json(
        transaction(state, identity, move |conn, state, pid| {
            e2e::directory(conn, state, &channel, pid, &query)
        })
        .await?,
    )
    .into_response())
}
pub async fn get_channel_key_wraps_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, E2eError> {
    replication::refresh_for_request(
        state.clone(),
        &identity,
        &channel,
        query.device_id.as_deref(),
    )
    .await
    .map_err(authority_error)?;
    Ok(Json(
        transaction(state, identity, move |conn, state, pid| {
            e2e::get_wraps(conn, state, &channel, pid, &query)
        })
        .await?,
    )
    .into_response())
}
pub async fn post_channel_key_wraps_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    Json(req): Json<PostWrapsRequest>,
) -> Result<Response, E2eError> {
    if let Some(result) = replication::maybe_forward(
        state.clone(),
        &identity,
        &channel,
        Some(&req.sender_device_id),
        None,
        json!({"kind":"store_wraps","request":req}),
    )
    .await
    .map_err(authority_error)?
    {
        return Ok(Json(
            replication::result_value(&state, &channel, &result).map_err(authority_error)?,
        )
        .into_response());
    }
    let id = channel.clone();
    let result = transaction(state.clone(), identity, move |conn, state, pid| {
        let result = e2e::store_wraps(conn, state, &id, pid, &req)?;
        crate::services::channel_authority::record_local_wraps(
            conn, state, &id, pid, &req, &result,
        )
        .map_err(authority_error)?;
        Ok(result)
    })
    .await?;
    if let Err(error) = e2e::notify_channel(state, channel).await {
        tracing::warn!(%error,"key notification deferred");
    }
    Ok(Json(result).into_response())
}
pub async fn get_channel_key_status_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
) -> Result<Response, E2eError> {
    replication::refresh_for_request(state.clone(), &identity, &channel, None)
        .await
        .map_err(authority_error)?;
    Ok(Json(
        transaction(state, identity, move |conn, state, pid| {
            e2e::require_member(conn, state, &channel, pid)?;
            Ok(json!(e2e::read_state(conn, state, &channel)?))
        })
        .await?,
    )
    .into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetE2eRequest {
    pub enabled: bool,
    pub manifest_event: Option<crate::services::channel_manifest::SignedManifestEvent>,
}
pub async fn set_channel_e2e_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    Json(req): Json<SetE2eRequest>,
) -> Result<Response, E2eError> {
    let id = channel.clone();
    let enabled = req.enabled;
    let result=transaction(state.clone(),identity,move|conn,state,pid|{
        let moderator:bool=conn.query_row("SELECT can_moderate FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2 AND active=1",params![state.server_id,pid],|r|r.get(0))?;
        if !moderator{return Err(E2eError::Forbidden("moderation capability required to change E2E setting".into()));}
        if let Some(event)=&req.manifest_event {let result=crate::services::channel_manifest::append(conn,state,&id,pid,event)?;crate::services::channel_authority::record_local_manifest(conn,state,&id,pid,event,&result).map_err(authority_error)?;}
        let manifest=crate::services::channel_manifest::projection(conn,state,&id)?;
        if manifest.as_ref().is_none_or(|p|p.e2e_enabled!=enabled){return Err(E2eError::Forbidden("encryption changes require the matching member-signed manifest policy".into()));}
        if conn.execute("UPDATE channels SET e2e_enabled=?3 WHERE server_id=?1 AND channel_id=?2",params![state.server_id,id,enabled])?==0{return Err(E2eError::NotFound("channel not found".into()));}
        Ok(json!({"status":"ok","e2e_enabled":enabled,"state":e2e::read_state(conn,state,&id)?}))
    }).await?;
    // Toggle notification remains compatible with existing channel lists.
    state
        .connection_manager
        .broadcast(
            &channel,
            json!({"type":"channel_e2e_changed","channelId":channel,"e2eEnabled":enabled})
                .to_string(),
        )
        .await;
    if let Err(error) = e2e::notify_channel(state, channel).await {
        tracing::warn!(%error,"key notification deferred");
    }
    Ok(Json(result).into_response())
}
pub async fn get_channel_e2e_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
) -> Result<Response, E2eError> {
    replication::refresh_for_request(state.clone(), &identity, &channel, None)
        .await
        .map_err(authority_error)?;
    Ok(Json(transaction(state,identity,move|conn,state,pid|{
        let moderator:bool=conn.query_row("SELECT can_moderate FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2 AND active=1",params![state.server_id,pid],|r|r.get(0))?;
        if !moderator{e2e::require_member(conn,state,&channel,pid)?;}
        Ok(json!({"e2e_enabled":e2e::read_state(conn,state,&channel)?.e2e_enabled}))
    }).await?).into_response())
}

#[derive(Deserialize)]
pub struct ManifestPageQuery {
    pub after_seq: Option<i64>,
    pub limit: Option<u32>,
}
pub async fn get_channel_manifest_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    Query(q): Query<ManifestPageQuery>,
) -> Result<Response, E2eError> {
    replication::refresh_for_request(state.clone(), &identity, &channel, None)
        .await
        .map_err(authority_error)?;
    Ok(Json(
        transaction(state, identity, move |conn, state, pid| {
            crate::services::channel_manifest::history(
                conn,
                state,
                &channel,
                pid,
                q.after_seq.unwrap_or(0),
                q.limit.unwrap_or(50),
            )
        })
        .await?,
    )
    .into_response())
}
pub async fn post_channel_manifest_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    Json(event): Json<crate::services::channel_manifest::SignedManifestEvent>,
) -> Result<Response, E2eError> {
    if let Some(result) = replication::maybe_forward(
        state.clone(),
        &identity,
        &channel,
        Some(&event.signer_device_id),
        Some(&event.event_hash),
        json!({"kind":"manifest","event":event}),
    )
    .await
    .map_err(authority_error)?
    {
        return Ok(Json(
            replication::result_value(&state, &channel, &result).map_err(authority_error)?,
        )
        .into_response());
    }
    let id = channel.clone();
    let result = transaction(state.clone(), identity, move |conn, state, pid| {
        let result = crate::services::channel_manifest::append(conn, state, &id, pid, &event)?;
        crate::services::channel_authority::record_local_manifest(
            conn, state, &id, pid, &event, &result,
        )
        .map_err(authority_error)?;
        Ok(result)
    })
    .await?;
    if let Ok(event) = serde_json::from_value::<
        crate::services::channel_manifest::SignedManifestEvent,
    >(result["event"].clone())
    {
        if let Err(error) =
            crate::services::channel_manifest::notify_departure(state.clone(), &channel, &event)
                .await
        {
            tracing::warn!(%error,"committed departure notification deferred");
        }
    }
    if let Err(error) = e2e::notify_channel(state, channel).await {
        tracing::warn!(%error,"manifest notification deferred");
    }
    Ok(Json(result).into_response())
}

fn authority_error(error: crate::services::channel_authority::AuthorityError) -> E2eError {
    use crate::services::channel_authority::AuthorityError as E;
    match error {
        E::BadRequest(s) => E2eError::BadRequest(s),
        E::Forbidden(s) => E2eError::Forbidden(s),
        E::NotFound(s) => E2eError::NotFound(s),
        E::Conflict(s) => E2eError::Conflict {
            message: s,
            state: None,
        },
        E::Remote(error) => E2eError::Remote(error),
        E::Unavailable(message) => E2eError::Remote(
            annex_federation::transport::ApplicationError::new(503, message),
        ),
        e => E2eError::Internal(e.to_string()),
    }
}
