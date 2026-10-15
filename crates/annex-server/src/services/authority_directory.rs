//! Pinned peer discovery. A channel label from a remote URL is not authority:
//! the selected agreement, request nonce and peer signature bind every page.
use super::{
    channel_authority as authority, channel_replication, federation_delivery, human_federation,
};
use crate::{middleware::IdentityContext, AppState};
use authority::AuthorityError as Error;
use axum::{
    extract::{Path, Query},
    Extension, Json,
};
use ed25519_dalek::Signer;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

pub const ENDPOINT: &str = "/api/federation/channel-authority/directory";
const MAX_PAGE: usize = 64;
const FRESH_SECONDS: i64 = 120;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub peer_id: i64,
    pub label: String,
    pub base_url: String,
    pub authority_identity: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub channel_id: String,
    pub name: String,
    pub channel_type: annex_types::ChannelType,
    pub e2e_enabled: bool,
    pub initialized: bool,
    pub head_seq: i64,
    pub head_hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryRequest {
    pub version: u8,
    pub originating_server: String,
    pub requesting_identity: String,
    pub authority_identity: String,
    pub after_channel_id: Option<String>,
    pub limit: usize,
    pub nonce: String,
    pub created_at: i64,
    pub signature: String,
}
impl DirectoryRequest {
    pub fn signing_value(&self) -> Value {
        json!([
            "annex-authority-directory-request-v1",
            self.version,
            self.originating_server,
            self.requesting_identity,
            self.authority_identity,
            self.after_channel_id,
            self.limit,
            self.nonce,
            self.created_at
        ])
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryResponse {
    pub version: u8,
    pub authority_identity: String,
    pub request_sha256: String,
    pub channels: Vec<Descriptor>,
    pub next_after_channel_id: Option<String>,
    pub signature: String,
}
impl DirectoryResponse {
    pub fn signing_value(&self) -> Value {
        json!([
            "annex-authority-directory-response-v1",
            self.version,
            self.authority_identity,
            self.request_sha256,
            self.channels,
            self.next_after_channel_id
        ])
    }
    fn verify(&self, request: &DirectoryRequest, key: &str) -> Result<(), Error> {
        if self.version != 1
            || self.authority_identity != request.authority_identity
            || authority::server_identity(key)? != self.authority_identity
            || self.request_sha256 != authority::digest(&request.signing_value())
            || self.channels.len() > request.limit
        {
            return Err(Error::Forbidden(
                "authority directory response binding mismatch".into(),
            ));
        }
        authority::verify(key, &self.signature, &self.signing_value())?;
        let mut after = request.after_channel_id.as_deref().unwrap_or("");
        for channel in &self.channels {
            if !identifier(&channel.channel_id)
                || channel.channel_id.as_str() <= after
                || channel.name.len() > 4096
                || channel.head_seq < 0
                || channel.initialized != (channel.head_seq > 0)
                || (channel.head_seq == 0 && !channel.head_hash.is_empty())
                || (channel.head_seq > 0 && !super::e2e_service::lower_hex(&channel.head_hash))
            {
                return Err(Error::Forbidden(
                    "invalid signed authority channel descriptor".into(),
                ));
            }
            after = &channel.channel_id;
        }
        if self.next_after_channel_id.as_deref().is_some_and(|next| {
            self.channels
                .last()
                .is_none_or(|last| last.channel_id != next)
        }) {
            return Err(Error::Forbidden(
                "authority directory cursor is not its final entry".into(),
            ));
        }
        Ok(())
    }
}
fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
fn internal(e: impl std::fmt::Display) -> Error {
    Error::Internal(e.to_string())
}
fn sign(state: &AppState, value: &Value) -> String {
    hex::encode(
        state
            .signing_key
            .sign(authority::canonical_json(value).as_bytes())
            .to_bytes(),
    )
}
fn enabled(conn: &Connection, state: &AppState) -> Result<(), Error> {
    let raw: String = conn.query_row(
        "SELECT policy_json FROM servers WHERE id=?1",
        [state.server_id],
        |r| r.get(0),
    )?;
    let policy: annex_types::ServerPolicy = serde_json::from_str(&raw).map_err(internal)?;
    if !policy.federation_enabled {
        return Err(Error::Forbidden(
            "federation is disabled by current policy".into(),
        ));
    }
    Ok(())
}
fn peers(conn: &Connection, state: &AppState) -> Result<Vec<(Peer, String)>, Error> {
    enabled(conn, state)?;
    let mut stmt=conn.prepare("SELECT i.id,i.label,i.base_url,i.public_key FROM instances i JOIN federation_agreements a ON a.remote_instance_id=i.id WHERE a.local_server_id=?1 AND a.active=1 AND a.transfer_scope<>'NO_TRANSFER' AND i.status='ACTIVE' ORDER BY i.label,i.id")?;
    let rows = stmt
        .query_map([state.server_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, label, url, key)| {
            Ok((
                Peer {
                    peer_id: id,
                    label,
                    base_url: url,
                    authority_identity: authority::server_identity(&key)?,
                },
                key,
            ))
        })
        .collect()
}
pub async fn receive(
    state: Arc<AppState>,
    request: DirectoryRequest,
) -> Result<DirectoryResponse, Error> {
    tokio::task::spawn_blocking(move||{
        let now=chrono::Utc::now().timestamp();
        if request.version!=1 || request.limit==0 || request.limit>MAX_PAGE
            || request.nonce.len()!=64 || !super::e2e_service::lower_hex(&request.nonce)
            || request.created_at>now+30 || request.created_at<now-FRESH_SECONDS
            || request.after_channel_id.as_deref().is_some_and(|id|!identifier(id))
            || request.authority_identity!=crate::device_certificates::server_identity(&state) {
            return Err(Error::BadRequest("invalid authority directory request".into()));
        }
        let mut conn=state.pool.get().map_err(internal)?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (_,key)=peers(&tx,&state)?.into_iter().find(|(p,_)|p.base_url==request.originating_server&&p.authority_identity==request.requesting_identity)
            .ok_or_else(||Error::Forbidden("directory requester has no pinned active agreement".into()))?;
        authority::verify(&key,&request.signature,&request.signing_value())?;
        tx.execute("DELETE FROM channel_authority_nonces WHERE expires_at<?1",[now])?;
        let seen:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_authority_nonces WHERE server_id=?1 AND requesting_identity=?2 AND nonce=?3)",params![state.server_id,request.requesting_identity,request.nonce],|r|r.get(0))?;
        if seen{return Err(Error::Forbidden("authority directory nonce was already used".into()));}
        let hash=authority::digest(&request.signing_value());
        tx.execute("INSERT INTO channel_authority_nonces(server_id,requesting_identity,nonce,request_hash,expires_at) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,request.requesting_identity,request.nonce,hash,now+FRESH_SECONDS])?;
        let mut channels={
            let mut stmt=tx.prepare("SELECT c.channel_id,c.name,c.channel_type,c.e2e_enabled,COALESCE(a.head_seq,0),COALESCE(a.head_hash,'') FROM channels c LEFT JOIN channel_authorities a ON a.server_id=c.server_id AND a.channel_id=c.channel_id WHERE c.server_id=?1 AND c.federation_scope=json_quote('Federated') AND c.channel_id>?2 AND (a.channel_id IS NULL OR (a.local_authority=1 AND a.deleted_seq IS NULL)) ORDER BY c.channel_id LIMIT ?3")?;
            let raw=stmt.query_map(params![state.server_id,request.after_channel_id.as_deref().unwrap_or(""),request.limit as i64+1],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?)))?.collect::<Result<Vec<_>,_>>()?;
            raw.into_iter().map(|(channel_id,name,kind,e2e_enabled,head_seq,head_hash)|Ok(Descriptor{channel_id,name,channel_type:serde_json::from_str(&kind).map_err(internal)?,e2e_enabled,initialized:head_seq>0,head_seq,head_hash})).collect::<Result<Vec<_>,Error>>()?
        };
        let more=channels.len()>request.limit;channels.truncate(request.limit);
        let next=if more{channels.last().map(|c|c.channel_id.clone())}else{None};
        let mut reply=DirectoryResponse{version:1,authority_identity:request.authority_identity.clone(),request_sha256:hash,channels,next_after_channel_id:next,signature:String::new()};
        reply.signature=sign(&state,&reply.signing_value());tx.commit()?;Ok(reply)
    }).await.map_err(internal)?
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    pub after_channel_id: Option<String>,
    pub limit: Option<usize>,
}
pub async fn http_peers(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<Value>, Error> {
    let conn = state.pool.get().map_err(internal)?;
    human_federation::current_local_identity(
        &conn,
        &state,
        &identity.pseudonym_id,
        identity.token_epoch,
    )?;
    Ok(Json(
        json!({"peers":peers(&conn,&state)?.into_iter().map(|(peer,_)|peer).collect::<Vec<_>>()}),
    ))
}
pub async fn http_channels(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(peer_id): Path<i64>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Value>, Error> {
    let (peer, key) = {
        let conn = state.pool.get().map_err(internal)?;
        human_federation::current_local_identity(
            &conn,
            &state,
            &identity.pseudonym_id,
            identity.token_epoch,
        )?;
        peers(&conn, &state)?
            .into_iter()
            .find(|(p, _)| p.peer_id == peer_id)
            .ok_or_else(|| Error::Forbidden("selected authority agreement is unavailable".into()))?
    };
    let mut request = DirectoryRequest {
        version: 1,
        originating_server: state.get_public_url().trim_end_matches('/').into(),
        requesting_identity: crate::device_certificates::server_identity(&state),
        authority_identity: peer.authority_identity.clone(),
        after_channel_id: page.after_channel_id,
        limit: page.limit.unwrap_or(16),
        nonce: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        created_at: chrono::Utc::now().timestamp(),
        signature: String::new(),
    };
    if request.limit == 0
        || request.limit > MAX_PAGE
        || request
            .after_channel_id
            .as_deref()
            .is_some_and(|s| !identifier(s))
    {
        return Err(Error::BadRequest("invalid authority directory page".into()));
    }
    request.signature = sign(&state, &request.signing_value());
    let raw = federation_delivery::request_typed(
        state.clone(),
        peer_id,
        ENDPOINT,
        &serde_json::to_string(&request).map_err(internal)?,
    )
    .await
    .map_err(Error::from)?;
    let reply: DirectoryResponse = serde_json::from_str(&raw)
        .map_err(|_| Error::Forbidden("invalid signed directory response".into()))?;
    reply.verify(&request, &key)?;
    let conn = state.pool.get().map_err(internal)?;
    human_federation::current_local_identity(
        &conn,
        &state,
        &identity.pseudonym_id,
        identity.token_epoch,
    )?;
    if !peers(&conn, &state)?
        .iter()
        .any(|(p, k)| p == &peer && k == &key)
    {
        return Err(Error::Forbidden(
            "authority agreement changed while reading directory".into(),
        ));
    }
    Ok(Json(
        json!({"peer":peer,"channels":reply.channels,"next_after_channel_id":reply.next_after_channel_id}),
    ))
}
pub async fn http_receive(
    Extension(state): Extension<Arc<AppState>>,
    Json(request): Json<DirectoryRequest>,
) -> Result<Json<DirectoryResponse>, Error> {
    receive(state, request).await.map(Json)
}
pub async fn http_binding(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
) -> Result<Json<Value>, Error> {
    channel_replication::refresh_for_request(state.clone(), &identity, &channel, None).await?;
    let conn = state.pool.get().map_err(internal)?;
    human_federation::current_local_identity(
        &conn,
        &state,
        &identity.pseudonym_id,
        identity.token_epoch,
    )?;
    super::e2e_service::require_member(&conn, &state, &channel, &identity.pseudonym_id)?;
    let row =
        annex_channels::get_channel(&conn, &channel).map_err(|e| Error::NotFound(e.to_string()))?;
    if row.server_id != state.server_id
        || row.federation_scope != annex_types::FederationScope::Federated
    {
        return Err(Error::BadRequest(
            "channel is not a local federated channel".into(),
        ));
    }
    let value = match authority::binding(&conn, &state, &channel) {
        Ok(bound) => {
            json!({"channel_id":channel,"authority_identity":bound.authority_identity,"authority_server":bound.authority_url,"local_authority":bound.local_authority,"initialized":bound.head_seq>0,"head_seq":bound.head_seq,"head_hash":bound.head_hash})
        }
        Err(Error::NotFound(_)) => {
            json!({"channel_id":channel,"authority_identity":crate::device_certificates::server_identity(&state),"authority_server":state.get_public_url().trim_end_matches('/'),"local_authority":true,"initialized":false,"head_seq":0,"head_hash":""})
        }
        Err(error) => return Err(error),
    };
    Ok(Json(value))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeRequest {
    pub expected_authority_identity: String,
}
pub async fn http_initialize(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    Json(request): Json<InitializeRequest>,
) -> Result<Json<Value>, Error> {
    let result=tokio::task::spawn_blocking(move||{
        let mut conn=state.pool.get().map_err(internal)?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        human_federation::current_local_identity(&tx,&state,&identity.pseudonym_id,identity.token_epoch)?;
        enabled(&tx,&state)?;
        if request.expected_authority_identity!=crate::device_certificates::server_identity(&state){return Err(Error::Conflict("local signing identity differs from the confirmed authority".into()));}
        authority::initialize_local(&tx,&state,&channel,&identity.pseudonym_id)?;
        let bound=authority::binding(&tx,&state,&channel)?;
        let messages:i64=tx.query_row("SELECT COUNT(*) FROM channel_authority_events WHERE server_id=?1 AND channel_id=?2 AND event_kind=json_quote('history_imported')",params![state.server_id,channel],|r|r.get(0))?;
        let edits:i64=tx.query_row("SELECT COUNT(*) FROM channel_authority_events WHERE server_id=?1 AND channel_id=?2 AND event_kind=json_quote('history_edit_imported')",params![state.server_id,channel],|r|r.get(0))?;
        let response=json!({"status":"ok","authority_identity":bound.authority_identity,"channel_id":channel,"head_seq":bound.head_seq,"head_hash":bound.head_hash,"imported_messages":messages,"imported_edits":edits});
        tx.commit()?;Ok(response)
    }).await.map_err(internal)??;
    Ok(Json(result))
}
