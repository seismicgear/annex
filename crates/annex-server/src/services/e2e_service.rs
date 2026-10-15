//! Device-scoped content keys. Every authorization/CAS read runs inside the
//! caller's IMMEDIATE transaction, together with the resulting write.
use crate::{services::channel_service::ChannelServiceError, AppState};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const MAX_WRAPS: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum E2eError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{message}")]
    Conflict {
        message: String,
        state: Option<Box<KeyState>>,
    },
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Remote(annex_federation::transport::ApplicationError),
}
impl E2eError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Remote(error) => {
                StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY)
            }
        }
    }
}
impl From<rusqlite::Error> for E2eError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Internal(format!("key database: {error}"))
    }
}
impl IntoResponse for E2eError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let state = match &self {
            Self::Conflict { state, .. } => state.clone(),
            _ => None,
        };
        (
            status,
            Json(json!({"error":self.to_string(),"state":state})),
        )
            .into_response()
    }
}
impl From<E2eError> for ChannelServiceError {
    fn from(error: E2eError) -> Self {
        match error {
            E2eError::BadRequest(s) => Self::BadRequest(s),
            E2eError::Forbidden(s) => Self::Forbidden(s),
            E2eError::NotFound(s) => Self::NotFound(s),
            E2eError::Conflict { message, .. } => Self::Conflict(message),
            E2eError::Internal(s) => Self::Internal(s),
            E2eError::Remote(error) => Self::Remote(error),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyState {
    pub version: u8,
    pub server_identity: String,
    pub authority_server: String,
    pub authority_identity: String,
    pub revision: i64,
    pub current_epoch: i64,
    pub required_epoch: i64,
    pub current_key_id: Option<String>,
    pub history_policy: String,
    pub e2e_enabled: bool,
    pub has_key: bool,
    pub rotation_required: bool,
    pub legacy_history_available: bool,
    pub max_epoch: i64,
}
#[derive(Clone, Debug, Serialize)]
pub struct MemberDeviceKey {
    pub pseudonym_id: String,
    pub device_id: String,
    pub x25519_pub_hex: String,
    pub device_certificate: Option<Value>,
    pub certificate_sha256: Option<String>,
    pub has_current_wrap: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyWrap {
    pub recipient_pseudonym_id: String,
    pub recipient_device_id: String,
    pub wrapped_key_b64: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostWrapsRequest {
    pub version: u8,
    pub operation: String,
    pub expected_revision: i64,
    pub epoch: i64,
    pub key_id: String,
    pub sender_device_id: String,
    pub wraps: Vec<KeyWrap>,
    pub authorization: Option<super::channel_manifest::AuthorizationProof>,
}
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PageQuery {
    pub device_id: Option<String>,
    pub after_epoch: Option<i64>,
    pub history_after_epoch: Option<i64>,
    pub legacy_after_epoch: Option<i64>,
    pub legacy_wrap_after_epoch: Option<i64>,
    pub legacy_after_key_id: Option<String>,
    pub limit: Option<u32>,
}
impl PageQuery {
    fn limit(&self) -> Result<i64, E2eError> {
        let limit = self.limit.unwrap_or(50);
        if limit == 0 || limit > 200 {
            return Err(E2eError::BadRequest("limit must be 1..200".into()));
        }
        for n in [
            self.after_epoch,
            self.history_after_epoch,
            self.legacy_after_epoch,
            self.legacy_wrap_after_epoch,
        ]
        .into_iter()
        .flatten()
        {
            if !(0..=MAX_SAFE_INTEGER).contains(&n) {
                return Err(E2eError::BadRequest("invalid epoch cursor".into()));
            }
        }
        if self
            .legacy_after_key_id
            .as_ref()
            .is_some_and(|s| !lower_hex(s))
        {
            return Err(E2eError::BadRequest("invalid legacy key cursor".into()));
        }
        Ok(limit as i64)
    }
}
pub fn lower_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn device_id(pub_hex: &str) -> Result<String, E2eError> {
    if !lower_hex(pub_hex) {
        return Err(E2eError::BadRequest(
            "x25519_pub_hex must be canonical 64 lowercase hex characters".into(),
        ));
    }
    let bytes: [u8; 32] = hex::decode(pub_hex)
        .map_err(|_| E2eError::BadRequest("invalid device public key".into()))?
        .try_into()
        .unwrap();
    // Reject low-order/noncontributory inputs. The fixed scalar is only a
    // public validation probe; it is never used to seal or derive user keys.
    let secret = x25519_dalek::StaticSecret::from([0x42u8; 32]);
    if !secret
        .diffie_hellman(&x25519_dalek::PublicKey::from(bytes))
        .was_contributory()
    {
        return Err(E2eError::BadRequest(
            "noncontributory X25519 public key".into(),
        ));
    }
    Ok(hex::encode(Sha256::digest(bytes)))
}
pub fn require_active(conn: &Connection, server_id: i64, pseudonym: &str) -> Result<(), E2eError> {
    let active:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2 AND active=1)",params![server_id,pseudonym],|r|r.get(0))?;
    if !active {
        return Err(E2eError::Forbidden("identity is inactive".into()));
    }
    Ok(())
}
pub fn require_member(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pseudonym: &str,
) -> Result<(), E2eError> {
    require_active(conn, state.server_id, pseudonym)?;
    let member:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_members WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3)",params![state.server_id,channel,pseudonym],|r|r.get(0))?;
    if !member {
        return Err(E2eError::Forbidden("not a member of this channel".into()));
    }
    Ok(())
}
pub fn require_device(
    conn: &Connection,
    server_id: i64,
    pseudonym: &str,
    device: &str,
) -> Result<(), E2eError> {
    if !lower_hex(device) {
        return Err(E2eError::BadRequest("invalid device_id".into()));
    }
    let active:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3 AND revoked_at IS NULL)",params![server_id,pseudonym,device],|r|r.get(0))?;
    if !active {
        return Err(E2eError::Forbidden(
            "device key is missing or revoked".into(),
        ));
    }
    Ok(())
}
/// Backfill valid legacy advertised keys once, without assigning ambiguous old
/// wraps to them. INSERT OR IGNORE preserves immutable revocation records.
pub fn import_legacy_devices(conn: &Connection, server_id: i64) -> Result<(), E2eError> {
    let mut stmt=conn.prepare("SELECT k.pseudonym_id,k.x25519_pub_hex FROM member_keys k JOIN platform_identities p ON p.server_id=k.server_id AND p.pseudonym_id=k.pseudonym_id WHERE k.server_id=?1 AND NOT EXISTS(SELECT 1 FROM member_device_keys d WHERE d.server_id=k.server_id AND d.pseudonym_id=k.pseudonym_id AND d.x25519_pub_hex=k.x25519_pub_hex)")?;
    let rows = stmt
        .query_map([server_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for (pid, key) in rows {
        if let Ok(id) = device_id(&key) {
            conn.execute("INSERT OR IGNORE INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex) VALUES(?1,?2,?3,?4)",params![server_id,pid,id,key])?;
        }
    }
    Ok(())
}
pub fn read_state(
    conn: &Connection,
    state: &AppState,
    channel: &str,
) -> Result<KeyState, E2eError> {
    let server_identity = format!(
        "annex-server:{}",
        hex::encode(Sha256::digest(state.signing_key.verifying_key().to_bytes()))
    );
    let mut value=conn.query_row("SELECT s.authority_server,s.revision,s.current_epoch,s.required_epoch,s.current_key_id,s.history_policy,c.e2e_enabled,EXISTS(SELECT 1 FROM channel_key_wraps w WHERE w.server_id=s.server_id AND w.channel_id=s.channel_id) FROM channel_key_state s JOIN channels c ON c.channel_id=s.channel_id AND c.server_id=s.server_id WHERE s.server_id=?1 AND s.channel_id=?2",params![state.server_id,channel],|r| {
        let current:i64=r.get(2)?; let required:i64=r.get(3)?;
        Ok(KeyState{version:2,server_identity:server_identity.clone(),authority_server:r.get(0)?,authority_identity:String::new(),revision:r.get(1)?,current_epoch:current,required_epoch:required,current_key_id:r.get(4)?,history_policy:r.get(5)?,e2e_enabled:r.get::<_,i64>(6)?!=0,has_key:current>0,rotation_required:current<required,legacy_history_available:r.get(7)?,max_epoch:current})
    }).optional()?.ok_or_else(||E2eError::NotFound("channel not found".into()))?;
    if value.authority_server.is_empty() {
        let url = state
            .public_url
            .read()
            .map_err(|_| E2eError::Internal("public URL lock".into()))?
            .clone();
        value.authority_server = if url.is_empty() {
            server_identity
        } else {
            url.trim_end_matches('/').into()
        };
        conn.execute("UPDATE channel_key_state SET authority_server=?3 WHERE server_id=?1 AND channel_id=?2 AND authority_server=''",params![state.server_id,channel,value.authority_server])?;
    }
    value.authority_identity = super::channel_manifest::authority_identity(conn, state, channel)?;
    Ok(value)
}
fn conflict(message: &str, current: &KeyState) -> E2eError {
    E2eError::Conflict {
        message: message.into(),
        state: Some(Box::new(current.clone())),
    }
}
pub fn eligible_devices(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    epoch: i64,
) -> Result<Vec<MemberDeviceKey>, E2eError> {
    let server_id = state.server_id;
    let manifest = super::channel_manifest::projection(conn, state, channel)?;
    let mut stmt=conn.prepare("SELECT d.pseudonym_id,d.device_id,d.x25519_pub_hex,d.device_certificate,d.certificate_sha256,EXISTS(SELECT 1 FROM channel_device_key_wraps w WHERE w.server_id=d.server_id AND w.channel_id=?2 AND w.recipient_pseudonym_id=d.pseudonym_id AND w.recipient_device_id=d.device_id AND w.epoch=?3) FROM member_device_keys d JOIN channel_members m ON m.server_id=d.server_id AND m.pseudonym_id=d.pseudonym_id JOIN platform_identities p ON p.server_id=d.server_id AND p.pseudonym_id=d.pseudonym_id WHERE d.server_id=?1 AND m.channel_id=?2 AND d.revoked_at IS NULL AND d.device_certificate IS NOT NULL AND d.certificate_sha256 IS NOT NULL AND p.active=1 ORDER BY d.pseudonym_id,d.device_id LIMIT 10001")?;
    let result = stmt
        .query_map(params![server_id, channel, epoch], |r| {
            Ok(MemberDeviceKey {
                pseudonym_id: r.get(0)?,
                device_id: r.get(1)?,
                x25519_pub_hex: r.get(2)?,
                device_certificate: r
                    .get::<_, Option<String>>(3)?
                    .map(|s| {
                        serde_json::from_str(&s).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                3,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })
                    })
                    .transpose()?,
                certificate_sha256: r.get(4)?,
                has_current_wrap: r.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if result.len() > MAX_WRAPS {
        return Err(E2eError::BadRequest("channel device limit exceeded".into()));
    }
    Ok(result
        .into_iter()
        .filter(|d| {
            manifest.as_ref().is_some_and(|p| {
                p.admitted(&d.pseudonym_id, &d.device_id)
                    .is_some_and(|m| Some(&m.certificate_sha256) == d.certificate_sha256.as_ref())
            })
        })
        .collect())
}
pub fn directory(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    q: &PageQuery,
) -> Result<Value, E2eError> {
    let limit = q.limit()?;
    require_member(conn, state, channel, pid)?;
    import_legacy_devices(conn, state.server_id)?;
    let current = read_state(conn, state, channel)?;
    let devices = eligible_devices(conn, state, channel, current.current_epoch)?;
    let mut stmt=conn.prepare("SELECT epoch,key_id,authority_namespace FROM channel_key_epochs WHERE server_id=?1 AND channel_id=?2 AND epoch>?3 ORDER BY epoch LIMIT ?4")?;
    let mut epochs = stmt
        .query_map(
            params![
                state.server_id,
                channel,
                q.history_after_epoch.unwrap_or(0),
                limit + 1
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let next = if epochs.len() > limit as usize {
        epochs.pop();
        epochs.last().map(|v| v.0)
    } else {
        None
    };
    let mut history = Vec::new();
    for (epoch, key_id, namespace) in epochs {
        let missing = eligible_devices(conn, state, channel, epoch)?
            .into_iter()
            .filter(|d| !d.has_current_wrap)
            .collect::<Vec<_>>();
        history.push(json!({"epoch":epoch,"key_id":key_id,"authority_namespace":namespace,"missing_devices":missing}));
    }
    let (legacy, next_legacy) = legacy_candidates(conn, state, channel, &devices, q)?;
    let mut result =
        serde_json::to_value(&current).map_err(|e| E2eError::Internal(e.to_string()))?;
    result["missing_current_devices"] = json!(devices
        .iter()
        .filter(|d| !d.has_current_wrap)
        .collect::<Vec<_>>());
    result["member_keys"] = json!(devices);
    result["manifest"] = json!(super::channel_manifest::projection(conn, state, channel)?);
    result["historical_epochs"] = json!(history);
    result["next_history_after_epoch"] = json!(next);
    result["legacy_candidates"] = json!(legacy);
    result["next_legacy_cursor"] = next_legacy;
    Ok(result)
}
fn legacy_candidates(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    devices: &[MemberDeviceKey],
    q: &PageQuery,
) -> Result<(Vec<Value>, Value), E2eError> {
    let limit = q.limit()?;
    let mut stmt=conn.prepare("SELECT epoch,key_id,authority_namespace FROM channel_legacy_key_candidates WHERE server_id=?1 AND channel_id=?2 AND (epoch>?3 OR (epoch=?3 AND key_id>?4)) ORDER BY epoch,key_id LIMIT ?5")?;
    let mut rows = stmt
        .query_map(
            params![
                state.server_id,
                channel,
                q.legacy_after_epoch.unwrap_or(0),
                q.legacy_after_key_id.as_deref().unwrap_or(""),
                limit + 1
            ],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let next = if rows.len() > limit as usize {
        rows.pop();
        let last = rows.last().unwrap();
        json!({"epoch":last.0,"key_id":last.1})
    } else {
        Value::Null
    };
    let mut result = Vec::new();
    for (epoch, key, namespace) in rows {
        let mut missing = Vec::new();
        for d in devices {
            let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_legacy_device_wraps WHERE server_id=?1 AND channel_id=?2 AND epoch=?3 AND key_id=?4 AND recipient_pseudonym_id=?5 AND recipient_device_id=?6)",params![state.server_id,channel,epoch,key,d.pseudonym_id,d.device_id],|r|r.get(0))?;
            if !exists {
                missing.push(d.clone());
            }
        }
        result.push(json!({"epoch":epoch,"key_id":key,"authority_namespace":namespace,"missing_devices":missing}));
    }
    Ok((result, next))
}

pub fn store_wraps(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    req: &PostWrapsRequest,
) -> Result<Value, E2eError> {
    require_member(conn, state, channel, pid)?;
    require_device(conn, state.server_id, pid, &req.sender_device_id)?;
    if req.version != 2
        || !(1..=MAX_SAFE_INTEGER).contains(&req.epoch)
        || !lower_hex(&req.key_id)
        || !(0..=MAX_SAFE_INTEGER).contains(&req.expected_revision)
        || req.wraps.is_empty()
        || req.wraps.len() > MAX_WRAPS
    {
        return Err(E2eError::BadRequest(
            "invalid version, epoch, key_id, revision or wrap count".into(),
        ));
    }
    let current = read_state(conn, state, channel)?;
    if req.expected_revision != current.revision {
        return Err(conflict("channel key revision changed", &current));
    }
    let proof = req.authorization.as_ref().ok_or_else(|| {
        E2eError::Forbidden("a member-signed key authorization is required".into())
    })?;
    let authorization =
        super::channel_manifest::authorize_wraps(conn, state, channel, pid, req, proof)?;
    let eligible = eligible_devices(conn, state, channel, current.current_epoch)?;
    let expected: BTreeSet<_> = eligible
        .iter()
        .map(|d| (d.pseudonym_id.clone(), d.device_id.clone()))
        .collect();
    let mut supplied = BTreeSet::new();
    for w in &req.wraps {
        let tuple = (
            w.recipient_pseudonym_id.clone(),
            w.recipient_device_id.clone(),
        );
        if !expected.contains(&tuple) || !supplied.insert(tuple) {
            return Err(E2eError::BadRequest(
                "duplicate, revoked or ineligible wrap recipient".into(),
            ));
        }
        if w.wrapped_key_b64.len() > 4096 {
            return Err(E2eError::BadRequest(
                "wrapped key exceeds 4096 bytes of base64".into(),
            ));
        }
        let decoded = STANDARD
            .decode(&w.wrapped_key_b64)
            .map_err(|_| E2eError::BadRequest("wrapped key must be canonical base64".into()))?;
        if decoded.len() < 60 || STANDARD.encode(&decoded) != w.wrapped_key_b64 {
            return Err(E2eError::BadRequest("invalid sealed key envelope".into()));
        }
    }
    let legacy = req.operation == "admit_legacy_devices";
    match req.operation.as_str() {
        "commit_epoch" => {
            if !current.e2e_enabled {
                return Err(conflict(
                    "enable channel encryption before committing an epoch",
                    &current,
                ));
            }
            if req.epoch != current.required_epoch
                || current.current_epoch >= current.required_epoch
            {
                return Err(conflict(
                    "a different channel epoch is required or already committed",
                    &current,
                ));
            }
            if supplied != expected {
                return Err(E2eError::BadRequest(
                    "commit must wrap exactly every eligible device".into(),
                ));
            }
            let reused:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_key_epochs WHERE server_id=?1 AND channel_id=?2 AND key_id=?3) OR EXISTS(SELECT 1 FROM channel_legacy_key_candidates WHERE server_id=?1 AND channel_id=?2 AND key_id=?3)",params![state.server_id,channel,req.key_id],|r|r.get(0))?;
            if reused {
                return Err(E2eError::BadRequest(
                    "rotation requires fresh key material".into(),
                ));
            }
            conn.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device,manifest_hash,authority_namespace) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![state.server_id,channel,req.epoch,req.key_id,current.revision,pid,req.sender_device_id,authorization.manifest_hash,authorization.authority_identity])?;
        }
        "admit_devices" | "admit_history_devices" => {
            if req.operation == "admit_devices"
                && (req.epoch != current.current_epoch
                    || current.current_epoch != current.required_epoch)
            {
                return Err(conflict(
                    "current key is pending rotation or epoch changed",
                    &current,
                ));
            }
            let known:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_key_epochs WHERE server_id=?1 AND channel_id=?2 AND epoch=?3 AND key_id=?4)",params![state.server_id,channel,req.epoch,req.key_id],|r|r.get(0))?;
            if !known {
                return Err(conflict("unknown committed key epoch", &current));
            }
            let entitled:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_device_key_wraps WHERE server_id=?1 AND channel_id=?2 AND epoch=?3 AND key_id=?4 AND recipient_pseudonym_id=?5 AND recipient_device_id=?6)",params![state.server_id,channel,req.epoch,req.key_id,pid,req.sender_device_id],|r|r.get(0))?;
            if !entitled {
                return Err(E2eError::Forbidden(
                    "sender device has no wrap for this epoch".into(),
                ));
            }
        }
        "admit_legacy_devices" => {
            let entitled:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_key_wraps WHERE server_id=?1 AND channel_id=?2 AND key_epoch=?3 AND recipient_pseudonym_id=?4) OR EXISTS(SELECT 1 FROM channel_legacy_device_wraps WHERE server_id=?1 AND channel_id=?2 AND epoch=?3 AND key_id=?6 AND recipient_pseudonym_id=?4 AND recipient_device_id=?5)",params![state.server_id,channel,req.epoch,pid,req.sender_device_id,req.key_id],|r|r.get(0))?;
            if !entitled {
                return Err(E2eError::Forbidden(
                    "sender has no legacy history entitlement".into(),
                ));
            }
            conn.execute("INSERT OR IGNORE INTO channel_legacy_key_candidates(server_id,channel_id,epoch,key_id,authority_namespace) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,channel,req.epoch,req.key_id,current.authority_server])?;
        }
        _ => return Err(E2eError::BadRequest("unknown key operation".into())),
    }
    let table = if legacy {
        "channel_legacy_device_wraps"
    } else {
        "channel_device_key_wraps"
    };
    let mut inserted = 0;
    for w in &req.wraps {
        let existing:Option<String>=conn.query_row(&format!("SELECT wrapped_key_b64 FROM {table} WHERE server_id=?1 AND channel_id=?2 AND recipient_pseudonym_id=?3 AND recipient_device_id=?4 AND epoch=?5 AND key_id=?6"),params![state.server_id,channel,w.recipient_pseudonym_id,w.recipient_device_id,req.epoch,req.key_id],|r|r.get(0)).optional()?;
        if let Some(blob) = existing {
            if blob != w.wrapped_key_b64 {
                return Err(conflict(
                    "occupied device wrap cannot be replaced",
                    &current,
                ));
            }
            continue;
        }
        inserted+=conn.execute(&format!("INSERT INTO {table}(server_id,channel_id,recipient_pseudonym_id,recipient_device_id,epoch,key_id,sender_pseudonym_id,sender_device_id,wrapped_key_b64,authorization_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"),params![state.server_id,channel,w.recipient_pseudonym_id,w.recipient_device_id,req.epoch,req.key_id,pid,req.sender_device_id,w.wrapped_key_b64,authorization.authorization_sha256])?;
    }
    if req.operation == "commit_epoch" {
        conn.execute("UPDATE channel_key_state SET current_epoch=?3,current_key_id=?4,revision=revision+1 WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel,req.epoch,req.key_id])?;
    } else if inserted > 0 {
        conn.execute(
            "UPDATE channel_key_state SET revision=revision+1 WHERE server_id=?1 AND channel_id=?2",
            params![state.server_id, channel],
        )?;
    }
    Ok(
        json!({"status":"ok","inserted":inserted,"authorization":authorization,"state":read_state(conn,state,channel)?}),
    )
}

pub fn get_wraps(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    q: &PageQuery,
) -> Result<Value, E2eError> {
    let limit = q.limit()?;
    require_member(conn, state, channel, pid)?;
    let device = q
        .device_id
        .as_deref()
        .ok_or_else(|| E2eError::BadRequest("device_id is required".into()))?;
    require_device(conn, state.server_id, pid, device)?;
    let current = read_state(conn, state, channel)?;
    let mut stmt=conn.prepare("SELECT epoch,key_id,sender_pseudonym_id,sender_device_id,wrapped_key_b64,authorization_sha256,(SELECT e.authority_namespace FROM channel_key_epochs e WHERE e.server_id=channel_device_key_wraps.server_id AND e.channel_id=channel_device_key_wraps.channel_id AND e.epoch=channel_device_key_wraps.epoch) FROM channel_device_key_wraps WHERE server_id=?1 AND channel_id=?2 AND recipient_pseudonym_id=?3 AND recipient_device_id=?4 AND epoch>?5 ORDER BY epoch LIMIT ?6")?;
    let mut wraps=stmt.query_map(params![state.server_id,channel,pid,device,q.after_epoch.unwrap_or(0),limit+1],|r|Ok(json!({"epoch":r.get::<_,i64>(0)?,"key_id":r.get::<_,String>(1)?,"recipient_device_id":device,"sender_pseudonym_id":r.get::<_,String>(2)?,"sender_device_id":r.get::<_,String>(3)?,"wrapped_key_b64":r.get::<_,String>(4)?,"authorization_sha256":r.get::<_,Option<String>>(5)?,"authority_namespace":r.get::<_,String>(6)?})))?.collect::<Result<Vec<_>,_>>()?;
    for wrap in &mut wraps {
        wrap["authorization"] = json!(super::channel_manifest::receipt(
            conn,
            state,
            channel,
            wrap["authorization_sha256"].as_str()
        )?);
    }
    let next = if wraps.len() > limit as usize {
        wraps.pop();
        wraps.last().unwrap()["epoch"].clone()
    } else {
        Value::Null
    };
    let mut stmt=conn.prepare("SELECT epoch,key_id,sender_pseudonym_id,sender_device_id,wrapped_key_b64,authorization_sha256,(SELECT e.authority_namespace FROM channel_legacy_key_candidates e WHERE e.server_id=channel_legacy_device_wraps.server_id AND e.channel_id=channel_legacy_device_wraps.channel_id AND e.epoch=channel_legacy_device_wraps.epoch AND e.key_id=channel_legacy_device_wraps.key_id) FROM channel_legacy_device_wraps WHERE server_id=?1 AND channel_id=?2 AND recipient_pseudonym_id=?3 AND recipient_device_id=?4 AND (epoch>?5 OR(epoch=?5 AND key_id>?6)) ORDER BY epoch,key_id LIMIT ?7")?;
    let mut legacy_devices=stmt.query_map(params![state.server_id,channel,pid,device,q.legacy_after_epoch.unwrap_or(0),q.legacy_after_key_id.as_deref().unwrap_or(""),limit+1],|r|Ok(json!({"epoch":r.get::<_,i64>(0)?,"key_id":r.get::<_,String>(1)?,"recipient_device_id":device,"sender_pseudonym_id":r.get::<_,String>(2)?,"sender_device_id":r.get::<_,String>(3)?,"wrapped_key_b64":r.get::<_,String>(4)?,"authorization_sha256":r.get::<_,Option<String>>(5)?,"authority_namespace":r.get::<_,String>(6)?})))?.collect::<Result<Vec<_>,_>>()?;
    for wrap in &mut legacy_devices {
        wrap["authorization"] = json!(super::channel_manifest::receipt(
            conn,
            state,
            channel,
            wrap["authorization_sha256"].as_str()
        )?);
    }
    let next_legacy = if legacy_devices.len() > limit as usize {
        legacy_devices.pop();
        let last = legacy_devices.last().unwrap();
        json!({"epoch":last["epoch"],"key_id":last["key_id"]})
    } else {
        Value::Null
    };
    let mut legacy = Vec::new();
    let mut next_legacy_wrap = Value::Null;
    if q.after_epoch.unwrap_or(0) == 0 || q.legacy_wrap_after_epoch.is_some() {
        let mut stmt=conn.prepare("SELECT key_epoch,wrapped_key_b64,sender_pseudonym_id,authority_namespace FROM channel_key_wraps WHERE server_id=?1 AND channel_id=?2 AND recipient_pseudonym_id=?3 AND key_epoch>?4 ORDER BY key_epoch LIMIT ?5")?;
        legacy=stmt.query_map(params![state.server_id,channel,pid,q.legacy_wrap_after_epoch.unwrap_or(0),limit+1],|r|Ok(json!({"epoch":r.get::<_,i64>(0)?,"key_epoch":r.get::<_,i64>(0)?,"wrapped_key_b64":r.get::<_,String>(1)?,"sender_pseudonym_id":r.get::<_,String>(2)?,"authority_namespace":r.get::<_,String>(3)?})))?.collect::<Result<Vec<_>,_>>()?;
        if legacy.len() > limit as usize {
            legacy.pop();
            next_legacy_wrap = legacy.last().unwrap()["epoch"].clone();
        }
    }
    Ok(
        json!({"state":current,"wraps":wraps,"next_after_epoch":next,"legacy_device_wraps":legacy_devices,"next_legacy_cursor":next_legacy,"legacy_wraps":legacy,"next_legacy_wrap_after_epoch":next_legacy_wrap}),
    )
}

/// Shared upload/send gate; caller owns the IMMEDIATE transaction.
pub fn require_current_encrypted_epoch(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    epoch: i64,
    key_id: &str,
) -> Result<(), E2eError> {
    require_member(conn, state, channel, pid)?;
    let current = read_state(conn, state, channel)?;
    if !current.e2e_enabled {
        return Err(conflict("channel encryption is disabled", &current));
    }
    if current.current_epoch == 0 || current.current_epoch != current.required_epoch {
        return Err(conflict("channel key rotation is required", &current));
    }
    let manifest = super::channel_manifest::projection(conn, state, channel)?.ok_or_else(|| {
        conflict(
            "a member-signed manifest and fresh key epoch are required",
            &current,
        )
    })?;
    if !manifest
        .members
        .iter()
        .any(|member| member.pseudonym_id == pid && !member.devices.is_empty())
    {
        return Err(E2eError::Forbidden(
            "sender has no admitted device in the channel manifest".into(),
        ));
    }
    let signed_epoch:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM channel_key_epochs WHERE server_id=?1 AND channel_id=?2 AND epoch=?3 AND key_id=?4 AND manifest_hash IS NOT NULL)",params![state.server_id,channel,epoch,key_id],|r|r.get(0))?;
    if !manifest.e2e_enabled || !signed_epoch {
        return Err(conflict(
            "a signed encryption policy and fresh authorized epoch are required",
            &current,
        ));
    }
    if epoch != current.current_epoch || current.current_key_id.as_deref() != Some(key_id) {
        return Err(conflict("message key epoch is stale or unknown", &current));
    }
    Ok(())
}
/// Validate canonical wire encoding independently of channel state. Returns the
/// clear epoch header only; the blind server cannot authenticate the AEAD tag.
pub fn parse_message_body(body: &str) -> Result<(i64, String), E2eError> {
    let parts: Vec<_> = body.split(':').collect();
    if parts.len() != 4
        || parts[0] != "e2e2"
        || parts[1].is_empty()
        || parts[1].starts_with('0')
        || !parts[1].bytes().all(|b| b.is_ascii_digit())
        || !lower_hex(parts[2])
    {
        return Err(E2eError::BadRequest(
            "encrypted channels require canonical e2e2 message bodies".into(),
        ));
    }
    let epoch = parts[1]
        .parse::<i64>()
        .map_err(|_| E2eError::BadRequest("invalid encrypted epoch".into()))?;
    if !(1..=MAX_SAFE_INTEGER).contains(&epoch) {
        return Err(E2eError::BadRequest("invalid encrypted epoch".into()));
    }
    let bytes = STANDARD
        .decode(parts[3])
        .map_err(|_| E2eError::BadRequest("invalid encrypted body base64".into()))?;
    if bytes.len() < 28 || STANDARD.encode(&bytes) != parts[3] {
        return Err(E2eError::BadRequest(
            "invalid encrypted body envelope".into(),
        ));
    }
    Ok((epoch, parts[2].into()))
}
pub fn validate_message_body(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    body: &str,
) -> Result<Option<(i64, String)>, E2eError> {
    let current = read_state(conn, state, channel)?;
    if let Some(manifest) = super::channel_manifest::projection(conn, state, channel)? {
        if manifest.e2e_enabled != current.e2e_enabled {
            return Err(E2eError::Forbidden(
                "channel encryption mode differs from its signed policy".into(),
            ));
        }
    }
    if !current.e2e_enabled {
        return Ok(None);
    }
    let (epoch, key) = parse_message_body(body)?;
    require_current_encrypted_epoch(conn, state, channel, pid, epoch, &key)?;
    Ok(Some((epoch, key)))
}

pub async fn notify_channel(state: Arc<AppState>, channel: String) -> Result<(), E2eError> {
    let (current,members)=tokio::task::spawn_blocking({let state=state.clone();let channel=channel.clone();move||{
        let mut conn=state.pool.get().map_err(|e|E2eError::Internal(e.to_string()))?;
        let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current=read_state(&tx,&state,&channel)?;
        let members={let mut stmt=tx.prepare("SELECT m.pseudonym_id FROM channel_members m JOIN platform_identities p ON p.server_id=m.server_id AND p.pseudonym_id=m.pseudonym_id WHERE m.server_id=?1 AND m.channel_id=?2 AND p.active=1")?;
            let rows=stmt.query_map(params![state.server_id,channel],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;rows};
        tx.commit()?;Ok::<_,E2eError>((current,members))
    }}).await.map_err(|e|E2eError::Internal(e.to_string()))??;
    let message=json!({"type":"channel_key_state_changed","channelId":channel,"revision":current.revision,"currentEpoch":current.current_epoch,"requiredEpoch":current.required_epoch,"keyId":current.current_key_id}).to_string();
    for member in members {
        state
            .connection_manager
            .send(&member, message.clone())
            .await;
    }
    let conn = state
        .pool
        .get()
        .map_err(|e| E2eError::Internal(e.to_string()))?;
    conn.execute("DELETE FROM channel_key_notifications WHERE server_id=?1 AND channel_id=?2 AND revision=?3",params![state.server_id,channel,current.revision])?;
    Ok(())
}
/// Durable local notification delivery. Lost WebSocket hints are harmless:
/// write gates always read authoritative state, and clients fetch on reconnect.
pub async fn notification_worker(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {_ = state.shutdown.cancelled()=>break, _=tick.tick()=>{}}
        let channels=tokio::task::spawn_blocking({let pool=state.pool.clone();let sid=state.server_id;move||->Result<Vec<String>,String>{
            let conn=pool.get().map_err(|e|e.to_string())?;
            let mut stmt=conn.prepare("SELECT channel_id FROM channel_key_notifications WHERE server_id=?1 ORDER BY channel_id LIMIT 100").map_err(|e|e.to_string())?;
            let rows=stmt.query_map([sid],|r|r.get(0)).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;Ok(rows)
        }}).await;
        if let Ok(Ok(channels)) = channels {
            for channel in channels {
                if let Err(error) = notify_channel(state.clone(), channel).await {
                    tracing::warn!(%error,"key state notification deferred");
                }
            }
        }
    }
}
