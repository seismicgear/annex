//! Username API handlers for server-scoped encrypted usernames.
//!
//! Provides endpoints for setting/removing a username, granting/revoking
//! username visibility to other users, and fetching visible usernames.
//!
//! Usernames are encrypted at rest using a key derived from the server's
//! Ed25519 signing key. This means:
//! - The server admin can theoretically decrypt (they control the key).
//! - Federation peers and external API consumers never see plaintext usernames.
//! - Database dumps are not directly readable.
//!
//! The encryption uses HMAC-SHA256 to derive a per-pseudonym keystream,
//! then XORs with the username bytes before hex-encoding.

use crate::{api::ApiError, middleware::IdentityContext, AppState};
use axum::{
    extract::{Extension, Path},
    response::{IntoResponse, Response},
    Json as AxumJson,
};
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Maximum username length in characters.
const MAX_USERNAME_LEN: usize = 32;

// ── Encryption ──

/// Derives a per-pseudonym keystream for username encryption.
///
/// Uses HMAC-like construction: SHA256("annex-username-key-v1" || server_key || pseudonym_id)
/// to produce a unique keystream per user per server.
fn derive_keystream(signing_key: &SigningKey, pseudonym_id: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"annex-username-key-v1");
    hasher.update(signing_key.as_bytes());
    hasher.update(pseudonym_id.as_bytes());
    hasher.finalize().to_vec()
}

/// Encrypts a username using XOR with a per-pseudonym keystream, then hex-encodes.
fn encrypt_username(signing_key: &SigningKey, pseudonym_id: &str, username: &str) -> String {
    let keystream = derive_keystream(signing_key, pseudonym_id);
    let encrypted: Vec<u8> = username
        .as_bytes()
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ keystream[i % keystream.len()])
        .collect();
    hex::encode(encrypted)
}

/// Decrypts a hex-encoded encrypted username.
fn decrypt_username(signing_key: &SigningKey, pseudonym_id: &str, encrypted_hex: &str) -> Option<String> {
    let encrypted = hex::decode(encrypted_hex).ok()?;
    let keystream = derive_keystream(signing_key, pseudonym_id);
    let decrypted: Vec<u8> = encrypted
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ keystream[i % keystream.len()])
        .collect();
    String::from_utf8(decrypted).ok()
}

/// Validates a username: non-empty, max length, no control characters.
fn validate_username(username: &str) -> Result<(), String> {
    let trimmed = username.trim();
    if trimmed.is_empty() {
        return Err("username cannot be empty".to_string());
    }
    if trimmed.len() > MAX_USERNAME_LEN {
        return Err(format!("username too long (max {} chars)", MAX_USERNAME_LEN));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("username cannot contain control characters".to_string());
    }
    Ok(())
}

// ── Handlers ──

/// Request body for setting a username.
#[derive(serde::Deserialize)]
pub struct SetUsernameRequest {
    username: String,
}

/// Request body for granting username visibility.
#[derive(serde::Deserialize)]
pub struct GrantUsernameRequest {
    grantee_pseudonym: String,
}

/// Handler for `PUT /api/profile/username`.
///
/// Sets the authenticated user's username. Encrypts it with the server key.
pub async fn set_username_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    AxumJson(body): AxumJson<SetUsernameRequest>,
) -> Result<Response, ApiError> {
    // Check if usernames are enabled
    let policy = state
        .policy
        .read()
        .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?
        .clone();

    if !policy.usernames_enabled {
        return Err(ApiError::BadRequest(
            "usernames are not enabled on this server".to_string(),
        ));
    }

    let username = body.username.trim().to_string();
    validate_username(&username).map_err(ApiError::BadRequest)?;

    let pseudonym = identity.pseudonym_id.clone();
    let encrypted = encrypt_username(&state.signing_key, &pseudonym, &username);
    let server_id = state.server_id;

    let state_clone = state.clone();
    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        conn.execute(
            "INSERT INTO user_profiles (server_id, pseudonym_id, encrypted_username, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))
             ON CONFLICT(server_id, pseudonym_id) DO UPDATE SET
                encrypted_username = excluded.encrypted_username,
                updated_at = datetime('now')",
            rusqlite::params![server_id, pseudonym, encrypted],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to set username: {}", e)))?;

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    tracing::info!(
        pseudonym = %identity.pseudonym_id,
        "username set"
    );

    Ok(AxumJson(serde_json::json!({ "status": "ok" })).into_response())
}

/// Handler for `DELETE /api/profile/username`.
///
/// Removes the authenticated user's username and all their grants.
pub async fn delete_username_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    let pseudonym = identity.pseudonym_id.clone();
    let server_id = state.server_id;
    let state_clone = state.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        conn.execute(
            "DELETE FROM user_profiles WHERE server_id = ?1 AND pseudonym_id = ?2",
            rusqlite::params![server_id, pseudonym],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to delete username: {}", e)))?;

        conn.execute(
            "DELETE FROM username_grants WHERE server_id = ?1 AND granter_pseudonym = ?2",
            rusqlite::params![server_id, pseudonym],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to delete grants: {}", e)))?;

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(AxumJson(serde_json::json!({ "status": "ok" })).into_response())
}

/// Handler for `POST /api/profile/username/grant`.
///
/// Grants username visibility to another user.
pub async fn grant_username_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    AxumJson(body): AxumJson<GrantUsernameRequest>,
) -> Result<Response, ApiError> {
    let granter = identity.pseudonym_id.clone();
    let grantee = body.grantee_pseudonym.trim().to_string();
    let server_id = state.server_id;

    if grantee.is_empty() {
        return Err(ApiError::BadRequest(
            "grantee_pseudonym is required".to_string(),
        ));
    }

    if granter == grantee {
        return Err(ApiError::BadRequest(
            "cannot grant visibility to yourself".to_string(),
        ));
    }

    let state_clone = state.clone();
    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        // Verify granter has a username set
        let has_username: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM user_profiles WHERE server_id = ?1 AND pseudonym_id = ?2",
                rusqlite::params![server_id, granter],
                |row| row.get(0),
            )
            .map_err(|e| ApiError::InternalServerError(format!("db query failed: {}", e)))?;

        if !has_username {
            return Err(ApiError::BadRequest(
                "set a username before granting visibility".to_string(),
            ));
        }

        conn.execute(
            "INSERT OR IGNORE INTO username_grants (server_id, granter_pseudonym, grantee_pseudonym)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![server_id, granter, grantee],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to create grant: {}", e)))?;

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(AxumJson(serde_json::json!({ "status": "ok" })).into_response())
}

/// Handler for `DELETE /api/profile/username/grant/{granteePseudonym}`.
///
/// Revokes username visibility from a specific user.
pub async fn revoke_grant_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(grantee_pseudonym): Path<String>,
) -> Result<Response, ApiError> {
    let granter = identity.pseudonym_id.clone();
    let server_id = state.server_id;
    let state_clone = state.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        conn.execute(
            "DELETE FROM username_grants WHERE server_id = ?1 AND granter_pseudonym = ?2 AND grantee_pseudonym = ?3",
            rusqlite::params![server_id, granter, grantee_pseudonym],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to revoke grant: {}", e)))?;

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(AxumJson(serde_json::json!({ "status": "ok" })).into_response())
}

/// Handler for `GET /api/profile/username/grants`.
///
/// Lists all users the authenticated user has granted visibility to.
pub async fn list_grants_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    let granter = identity.pseudonym_id.clone();
    let server_id = state.server_id;
    let state_clone = state.clone();

    let grantees = tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT grantee_pseudonym FROM username_grants
                 WHERE server_id = ?1 AND granter_pseudonym = ?2
                 ORDER BY created_at",
            )
            .map_err(|e| ApiError::InternalServerError(format!("query prepare failed: {}", e)))?;

        let rows = stmt
            .query_map(rusqlite::params![server_id, granter], |row| row.get::<_, String>(0))
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {}", e)))?;

        let mut grantees = Vec::new();
        for row in rows {
            grantees.push(
                row.map_err(|e| ApiError::InternalServerError(format!("row read failed: {}", e)))?,
            );
        }

        Ok::<Vec<String>, ApiError>(grantees)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(AxumJson(serde_json::json!({ "grantees": grantees })).into_response())
}

/// Handler for `GET /api/usernames/visible`.
///
/// Returns all usernames visible to the authenticated user: their own username
/// (if set) plus usernames of users who have granted them visibility.
/// Decrypts usernames server-side before returning.
pub async fn get_visible_usernames_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    // Check if usernames are enabled
    let policy = state
        .policy
        .read()
        .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?
        .clone();

    if !policy.usernames_enabled {
        return Ok(
            AxumJson(serde_json::json!({ "usernames": {} })).into_response()
        );
    }

    let grantee = identity.pseudonym_id.clone();
    let server_id = state.server_id;
    let signing_key = state.signing_key.clone();
    let state_clone = state.clone();

    let usernames = tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        let mut usernames = serde_json::Map::new();

        // Include the user's own username if they have one set
        let own_username: Option<String> = conn
            .query_row(
                "SELECT encrypted_username FROM user_profiles WHERE server_id = ?1 AND pseudonym_id = ?2",
                rusqlite::params![server_id, grantee],
                |row| row.get(0),
            )
            .ok();
        if let Some(encrypted) = own_username {
            if let Some(decrypted) = decrypt_username(&signing_key, &grantee, &encrypted) {
                usernames.insert(grantee.clone(), serde_json::Value::String(decrypted));
            }
        }

        // Include usernames of users who granted visibility to us
        let mut stmt = conn
            .prepare(
                "SELECT up.pseudonym_id, up.encrypted_username
                 FROM username_grants ug
                 JOIN user_profiles up ON up.server_id = ug.server_id AND up.pseudonym_id = ug.granter_pseudonym
                 WHERE ug.server_id = ?1 AND ug.grantee_pseudonym = ?2",
            )
            .map_err(|e| ApiError::InternalServerError(format!("query prepare failed: {}", e)))?;

        let rows = stmt
            .query_map(rusqlite::params![server_id, grantee], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {}", e)))?;

        for row in rows {
            let (pseudonym_id, encrypted) =
                row.map_err(|e| ApiError::InternalServerError(format!("row read failed: {}", e)))?;
            if let Some(decrypted) = decrypt_username(&signing_key, &pseudonym_id, &encrypted) {
                usernames.insert(pseudonym_id, serde_json::Value::String(decrypted));
            }
        }

        Ok::<serde_json::Map<String, serde_json::Value>, ApiError>(usernames)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(AxumJson(serde_json::json!({ "usernames": usernames })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&[42u8; 32])
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = test_signing_key();
        let pseudonym = "test-pseudo-123";
        let username = "Alice";

        let encrypted = encrypt_username(&key, pseudonym, username);
        let decrypted = decrypt_username(&key, pseudonym, &encrypted).unwrap();
        assert_eq!(decrypted, username);
    }

    #[test]
    fn different_pseudonyms_produce_different_ciphertext() {
        let key = test_signing_key();
        let e1 = encrypt_username(&key, "user-a", "Alice");
        let e2 = encrypt_username(&key, "user-b", "Alice");
        assert_ne!(e1, e2);
    }

    #[test]
    fn validate_username_rejects_empty() {
        assert!(validate_username("").is_err());
        assert!(validate_username("   ").is_err());
    }

    #[test]
    fn validate_username_rejects_too_long() {
        let long = "a".repeat(MAX_USERNAME_LEN + 1);
        assert!(validate_username(&long).is_err());
    }

    #[test]
    fn validate_username_rejects_control_chars() {
        assert!(validate_username("hello\x00world").is_err());
        assert!(validate_username("hello\nworld").is_err());
    }

    #[test]
    fn validate_username_accepts_valid() {
        assert!(validate_username("Alice").is_ok());
        assert!(validate_username("seismicgear").is_ok());
        assert!(validate_username("Jane Doe").is_ok());
        let max = "a".repeat(MAX_USERNAME_LEN);
        assert!(validate_username(&max).is_ok());
    }
}
