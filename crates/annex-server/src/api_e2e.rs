//! End-to-end encrypted channel key distribution — the content-blind backend.
//!
//! The server is a dumb, blind store here. It holds:
//!   * a per-member **public** key directory (`member_keys`), and
//!   * **opaque** sealed channel-key blobs (`channel_key_wraps`).
//!
//! It never sees a device secret, a channel content key (CEK), or — for E2E
//! channels — any plaintext message body. The actual sealing/opening happens in
//! the clients via the cross-language sealed box
//! (`crates/annex-federation/src/seal.rs::seal_x25519` ⇄ `client/src/lib/e2e.ts`).
//!
//! AI agents work unchanged: an agent is just another member that advertises an
//! X25519 key and receives the CEK sealed to it, so it can read/produce content
//! exactly like a human client — without the server ever holding the key.

use crate::{api::ApiError, middleware::IdentityContext, AppState};
use axum::{
    extract::{Extension, Path},
    response::{IntoResponse, Response},
    Json as AxumJson,
};
use serde::Deserialize;
use std::sync::Arc;

/// Length of a hex-encoded X25519 public key (32 bytes).
const X25519_PUB_HEX_LEN: usize = 64;
/// Defensive ceiling on a single sealed-key blob (base64). A seal of a 32-byte
/// CEK is ~88 bytes; allow generous headroom but reject obvious abuse.
const MAX_WRAP_B64_LEN: usize = 512;
/// Cap the number of wraps uploaded in one request (one per channel member).
const MAX_WRAPS_PER_REQUEST: usize = 10_000;

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_base64ish(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_WRAP_B64_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

// ── Member public-key directory ───────────────────────────────────────────

#[derive(Deserialize)]
pub struct PutMyKeyRequest {
    pub x25519_pub_hex: String,
}

/// `PUT /api/keys/me` — publish (upsert) the caller's own X25519 public key.
///
/// A member may only set *their own* key: it is bound to the authenticated
/// pseudonym, never to a value supplied in the body, so no one can impersonate
/// another member's key.
pub async fn put_my_key_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    AxumJson(req): AxumJson<PutMyKeyRequest>,
) -> Result<Response, ApiError> {
    let pub_hex = req.x25519_pub_hex.trim().to_lowercase();
    if !is_lower_hex(&pub_hex, X25519_PUB_HEX_LEN) {
        return Err(ApiError::BadRequest(
            "x25519_pub_hex must be 64 lowercase hex characters".into(),
        ));
    }
    let server_id = state.server_id;
    let pseudonym = identity.pseudonym_id.clone();
    let state_clone = state.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;
        conn.execute(
            "INSERT INTO member_keys (server_id, pseudonym_id, x25519_pub_hex, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(server_id, pseudonym_id)
             DO UPDATE SET x25519_pub_hex = excluded.x25519_pub_hex, updated_at = datetime('now')",
            rusqlite::params![server_id, pseudonym, pub_hex],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to store key: {e}")))?;
        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({ "status": "ok" })).into_response())
}

/// `GET /api/keys/{pseudonymId}` — fetch a member's advertised public key so the
/// caller can seal the channel key to them. Public keys are not secret.
pub async fn get_member_key_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(_identity)): Extension<IdentityContext>,
    Path(pseudonym_id): Path<String>,
) -> Result<Response, ApiError> {
    let server_id = state.server_id;
    let state_clone = state.clone();
    let lookup_id = pseudonym_id.clone();

    let pub_hex = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;
        conn.query_row(
            "SELECT x25519_pub_hex FROM member_keys WHERE server_id = ?1 AND pseudonym_id = ?2",
            rusqlite::params![server_id, lookup_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                ApiError::NotFound(format!("no key published for {lookup_id}"))
            }
            _ => ApiError::InternalServerError(e.to_string()),
        })
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({
        "pseudonym_id": pseudonym_id,
        "x25519_pub_hex": pub_hex
    }))
    .into_response())
}

/// `GET /api/channels/{channelId}/member-keys` — the directory of
/// `(pseudonym, public key)` for members of this channel who have published a
/// key, so a member can seal the CEK to everyone. Caller must be a member.
pub async fn list_channel_member_keys_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Response, ApiError> {
    let server_id = state.server_id;
    let pseudonym = identity.pseudonym_id.clone();
    let state_clone = state.clone();

    let keys = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        if !annex_channels::is_member(&conn, server_id, &channel_id, &pseudonym)
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        {
            return Err(ApiError::Forbidden("not a member of this channel".into()));
        }

        let mut stmt = conn
            .prepare(
                "SELECT cm.pseudonym_id, mk.x25519_pub_hex
                 FROM channel_members cm
                 JOIN member_keys mk
                   ON mk.server_id = cm.server_id AND mk.pseudonym_id = cm.pseudonym_id
                 WHERE cm.server_id = ?1 AND cm.channel_id = ?2
                 ORDER BY cm.joined_at ASC",
            )
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![server_id, channel_id], |row| {
                Ok(serde_json::json!({
                    "pseudonym_id": row.get::<_, String>(0)?,
                    "x25519_pub_hex": row.get::<_, String>(1)?,
                }))
            })
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| ApiError::InternalServerError(e.to_string()))?);
        }
        Ok::<Vec<serde_json::Value>, ApiError>(out)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({ "member_keys": keys })).into_response())
}

// ── Sealed channel-key wraps ──────────────────────────────────────────────

#[derive(Deserialize)]
pub struct KeyWrap {
    pub recipient_pseudonym_id: String,
    pub wrapped_key_b64: String,
}

#[derive(Deserialize)]
pub struct PostWrapsRequest {
    #[serde(default = "default_epoch")]
    pub epoch: i64,
    pub wraps: Vec<KeyWrap>,
}

fn default_epoch() -> i64 {
    1
}

/// `POST /api/channels/{channelId}/key-wraps` — upload sealed CEK blobs for
/// members. Caller must be a member. The first wrap for a
/// `(channel, recipient, epoch)` wins (INSERT OR IGNORE), so no member can
/// clobber another's key material; re-keying uses a fresh epoch. Blobs are
/// opaque to the server.
pub async fn post_channel_key_wraps_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
    AxumJson(req): AxumJson<PostWrapsRequest>,
) -> Result<Response, ApiError> {
    if req.epoch < 1 {
        return Err(ApiError::BadRequest("epoch must be >= 1".into()));
    }
    if req.wraps.is_empty() {
        return Err(ApiError::BadRequest("wraps must not be empty".into()));
    }
    if req.wraps.len() > MAX_WRAPS_PER_REQUEST {
        return Err(ApiError::BadRequest("too many wraps in one request".into()));
    }
    for w in &req.wraps {
        if w.recipient_pseudonym_id.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "recipient_pseudonym_id required".into(),
            ));
        }
        if !is_base64ish(&w.wrapped_key_b64) {
            return Err(ApiError::BadRequest(
                "wrapped_key_b64 is not valid base64".into(),
            ));
        }
    }

    let server_id = state.server_id;
    let sender = identity.pseudonym_id.clone();
    let state_clone = state.clone();
    let epoch = req.epoch;
    let wraps = req.wraps;

    let inserted = tokio::task::spawn_blocking(move || {
        let mut conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        if !annex_channels::is_member(&conn, server_id, &channel_id, &sender)
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        {
            return Err(ApiError::Forbidden("not a member of this channel".into()));
        }

        let tx = conn
            .transaction()
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        let mut inserted = 0usize;
        for w in &wraps {
            inserted += tx
                .execute(
                    "INSERT OR IGNORE INTO channel_key_wraps
                       (server_id, channel_id, recipient_pseudonym_id, sender_pseudonym_id,
                        key_epoch, wrapped_key_b64)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        server_id,
                        channel_id,
                        w.recipient_pseudonym_id,
                        sender,
                        epoch,
                        w.wrapped_key_b64
                    ],
                )
                .map_err(|e| ApiError::InternalServerError(format!("failed to store wrap: {e}")))?;
        }
        tx.commit()
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        Ok::<usize, ApiError>(inserted)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({ "status": "ok", "inserted": inserted })).into_response())
}

/// `GET /api/channels/{channelId}/key-wraps` — the sealed CEK blobs addressed to
/// the caller (only their own), so the client can open the channel key. Caller
/// must be a member.
pub async fn get_channel_key_wraps_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Response, ApiError> {
    let server_id = state.server_id;
    let recipient = identity.pseudonym_id.clone();
    let state_clone = state.clone();

    let wraps = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        if !annex_channels::is_member(&conn, server_id, &channel_id, &recipient)
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        {
            return Err(ApiError::Forbidden("not a member of this channel".into()));
        }

        let mut stmt = conn
            .prepare(
                "SELECT key_epoch, sender_pseudonym_id, wrapped_key_b64
                 FROM channel_key_wraps
                 WHERE server_id = ?1 AND channel_id = ?2 AND recipient_pseudonym_id = ?3
                 ORDER BY key_epoch DESC",
            )
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![server_id, channel_id, recipient], |row| {
                Ok(serde_json::json!({
                    "epoch": row.get::<_, i64>(0)?,
                    "sender_pseudonym_id": row.get::<_, String>(1)?,
                    "wrapped_key_b64": row.get::<_, String>(2)?,
                }))
            })
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| ApiError::InternalServerError(e.to_string()))?);
        }
        Ok::<Vec<serde_json::Value>, ApiError>(out)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({ "wraps": wraps })).into_response())
}

/// `GET /api/channels/{channelId}/key-status` — whether the channel already has
/// any sealed key material and the highest epoch present. Lets a client decide
/// whether to provision a fresh content key (none exists yet) or wait to be
/// admitted by an existing member (one already exists) — avoiding two members
/// minting rival keys for the same channel. Reveals only counts, never key
/// bytes. Caller must be a member.
pub async fn get_channel_key_status_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Response, ApiError> {
    let server_id = state.server_id;
    let pseudonym = identity.pseudonym_id.clone();
    let state_clone = state.clone();

    let (count, max_epoch) = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        if !annex_channels::is_member(&conn, server_id, &channel_id, &pseudonym)
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?
        {
            return Err(ApiError::Forbidden("not a member of this channel".into()));
        }

        conn.query_row(
            "SELECT COUNT(*), COALESCE(MAX(key_epoch), 0)
             FROM channel_key_wraps WHERE server_id = ?1 AND channel_id = ?2",
            rusqlite::params![server_id, channel_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|e| ApiError::InternalServerError(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({
        "has_key": count > 0,
        "max_epoch": max_epoch
    }))
    .into_response())
}

// ── E2E channel flag ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SetE2eRequest {
    pub enabled: bool,
}

/// `PUT /api/channels/{channelId}/e2e` — enable/disable end-to-end encryption on
/// a channel. Requires moderation capability. Once enabled, clients encrypt
/// message bodies with the channel key before sending.
pub async fn set_channel_e2e_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
    AxumJson(req): AxumJson<SetE2eRequest>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "moderation capability required to change E2E setting".into(),
        ));
    }
    let server_id = state.server_id;
    let state_clone = state.clone();
    let enabled = req.enabled;

    let updated = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;
        conn.execute(
            "UPDATE channels SET e2e_enabled = ?1 WHERE channel_id = ?2 AND server_id = ?3",
            rusqlite::params![enabled as i64, channel_id, server_id],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to update channel: {e}")))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    if updated == 0 {
        return Err(ApiError::NotFound("channel not found".into()));
    }
    Ok(AxumJson(serde_json::json!({ "status": "ok", "e2e_enabled": enabled })).into_response())
}

/// `GET /api/channels/{channelId}/e2e` — read the E2E flag so the client knows
/// whether to encrypt outgoing bodies and expect ciphertext on inbound ones.
pub async fn get_channel_e2e_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(_identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Response, ApiError> {
    let server_id = state.server_id;
    let state_clone = state.clone();

    let enabled = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;
        conn.query_row(
            "SELECT e2e_enabled FROM channels WHERE channel_id = ?1 AND server_id = ?2",
            rusqlite::params![channel_id, server_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => ApiError::NotFound("channel not found".into()),
            _ => ApiError::InternalServerError(e.to_string()),
        })
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({ "e2e_enabled": enabled != 0 })).into_response())
}
