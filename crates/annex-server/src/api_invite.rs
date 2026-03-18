//! Invite link generation and protocol handler parsing for monolithannex.com.
//!
//! Invite links are shareable URLs that encode server connection details as
//! base64url JSON payloads. The monolithannex.com site renders social media
//! previews (OG images, Discord embeds) and provides a "Open in Annex" button
//! that launches the desktop app via the `annex://` protocol.

use crate::{api::ApiError, middleware::IdentityContext, AppState};
use axum::extract::{Extension, Json, Path};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

/// Base URL for monolithannex.com invite links.
const DEFAULT_INVITE_BASE_URL: &str = "https://monolithannex.com/invite";

/// Invite payload for monolithannex.com link sharing.
///
/// Serializes to camelCase JSON for compatibility with the
/// monolithannex.com invite resolver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvitePayload {
    /// Public HTTPS URL of the Annex server. Required.
    pub server: String,

    /// Invite code. Required. Max 100 characters.
    pub code: String,

    /// Human-readable server display name. Optional.
    /// Controls the title in social media previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,

    /// Server description. Optional.
    /// Controls the description in social media previews.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Errors that occur during invite link generation.
#[derive(Debug, Error)]
pub enum InviteLinkError {
    #[error("invalid server URL: {0}")]
    InvalidServerUrl(String),
    #[error("invalid invite code: {0}")]
    InvalidCode(String),
    #[error("invalid server name: {0}")]
    InvalidServerName(String),
    #[error("invalid description: {0}")]
    InvalidDescription(String),
    #[error("serialization failed: {0}")]
    SerializationFailed(String),
}

impl InvitePayload {
    /// Create a new invite payload.
    pub fn new(
        server_url: impl Into<String>,
        invite_code: impl Into<String>,
        server_name: Option<impl Into<String>>,
        description: Option<impl Into<String>>,
    ) -> Self {
        Self {
            server: server_url.into(),
            code: invite_code.into(),
            server_name: server_name.map(Into::into),
            description: description.map(Into::into),
        }
    }

    /// Encode this payload as a monolithannex.com invite URL.
    pub fn to_invite_url(&self) -> Result<String, InviteLinkError> {
        self.to_invite_url_with_base(DEFAULT_INVITE_BASE_URL)
    }

    /// Encode this payload as an invite URL with a custom base URL.
    pub fn to_invite_url_with_base(&self, base_url: &str) -> Result<String, InviteLinkError> {
        self.validate()?;
        let json = serde_json::to_string(self)
            .map_err(|e| InviteLinkError::SerializationFailed(e.to_string()))?;
        let encoded = URL_SAFE_NO_PAD.encode(json.as_bytes());
        Ok(format!("{}/{}", base_url.trim_end_matches('/'), encoded))
    }

    /// Validate the payload before encoding.
    fn validate(&self) -> Result<(), InviteLinkError> {
        if !self.server.starts_with("https://") {
            return Err(InviteLinkError::InvalidServerUrl(
                "server URL must use HTTPS".to_string(),
            ));
        }

        if url::Url::parse(&self.server).is_err() {
            return Err(InviteLinkError::InvalidServerUrl(
                "server URL is not a valid URL".to_string(),
            ));
        }

        if self.code.is_empty() {
            return Err(InviteLinkError::InvalidCode(
                "invite code cannot be empty".to_string(),
            ));
        }

        if self.code.len() > 100 {
            return Err(InviteLinkError::InvalidCode(
                "invite code exceeds 100 character limit".to_string(),
            ));
        }

        if self.code.chars().any(|c| c.is_control()) {
            return Err(InviteLinkError::InvalidCode(
                "invite code contains control characters".to_string(),
            ));
        }

        if let Some(ref name) = self.server_name {
            if name.len() > 100 {
                return Err(InviteLinkError::InvalidServerName(
                    "server name exceeds 100 character limit".to_string(),
                ));
            }
        }

        if let Some(ref desc) = self.description {
            if desc.len() > 300 {
                return Err(InviteLinkError::InvalidDescription(
                    "description exceeds 300 character limit".to_string(),
                ));
            }
        }

        Ok(())
    }
}

/// Parsed invite from an `annex://` protocol handler URL.
#[derive(Debug, Clone)]
pub struct ProtocolInvite {
    /// The Annex server's public HTTPS URL (percent-decoded).
    pub server: String,
    /// The invite code (percent-decoded).
    pub code: String,
}

/// Parse an `annex://` protocol handler URL.
///
/// Expected format: `annex://invite?server={percent_encoded}&code={percent_encoded}`
///
/// Returns `None` if the URL is not a valid `annex://invite` URL.
pub fn parse_protocol_invite(raw_url: &str) -> Option<ProtocolInvite> {
    let parsed = url::Url::parse(raw_url).ok()?;

    if parsed.scheme() != "annex" {
        return None;
    }

    // The host portion of annex://invite is "invite"
    if parsed.host_str() != Some("invite") {
        return None;
    }

    let mut server = None;
    let mut code = None;

    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "server" => server = Some(value.into_owned()),
            "code" => code = Some(value.into_owned()),
            _ => {}
        }
    }

    let server = server?;
    let code = code?;

    if !server.starts_with("https://") {
        return None;
    }

    Some(ProtocolInvite { server, code })
}

// ── API types ──

/// Request body for `POST /api/invites`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInviteRequest {
    /// Maximum number of uses. Omit for unlimited.
    pub max_uses: Option<i64>,
    /// Hours until the invite expires. Omit for no expiry.
    pub expires_in_hours: Option<i64>,
}

/// Response body for `POST /api/invites`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInviteResponse {
    /// The generated invite code.
    pub code: String,
    /// The full monolithannex.com shareable URL.
    pub url: String,
    /// When the invite expires (ISO 8601), if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// A stored invite code returned by `GET /api/invites`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteCodeEntry {
    pub code: String,
    pub url: String,
    pub created_by: String,
    pub max_uses: Option<i64>,
    pub use_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub created_at: String,
}

/// Handler for `POST /api/invites`.
///
/// Creates a new invite code and returns the monolithannex.com shareable URL.
/// Requires `can_invite` capability.
pub async fn create_invite_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(payload): Json<CreateInviteRequest>,
) -> Result<Json<CreateInviteResponse>, ApiError> {
    if !identity.can_invite {
        return Err(ApiError::Forbidden(
            "insufficient permissions to create invites".to_string(),
        ));
    }

    let code = Uuid::new_v4().to_string();
    let invite_base_url = state.invite_base_url.clone();
    let pseudonym_id = identity.pseudonym_id.clone();

    let expires_at = payload.expires_in_hours.map(|hours| {
        let now = chrono::Utc::now();
        let expires = now + chrono::Duration::hours(hours);
        expires.format("%Y-%m-%d %H:%M:%S").to_string()
    });

    let code_clone = code.clone();
    let expires_at_clone = expires_at.clone();
    let state_clone = state.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        conn.execute(
            "INSERT INTO invite_codes (server_id, code, created_by, max_uses, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                state_clone.server_id,
                code_clone,
                pseudonym_id,
                payload.max_uses,
                expires_at_clone,
            ],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to insert invite code: {}", e)))?;

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    // Build the shareable URL
    let public_url = state
        .public_url
        .read()
        .map_err(|_| ApiError::InternalServerError("public_url lock poisoned".to_string()))?
        .clone();

    if public_url.is_empty() {
        return Err(ApiError::BadRequest(
            "server public URL is not configured; set ANNEX_PUBLIC_URL first".to_string(),
        ));
    }

    // Fetch server label and description for social preview
    let (server_label, server_description) = {
        let state_clone = state.clone();
        tokio::task::spawn_blocking(move || {
            let conn = state_clone.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;
            let (label, description): (String, String) = conn
                .query_row(
                    "SELECT label, description FROM servers WHERE id = ?1",
                    [state_clone.server_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to fetch server metadata: {}", e))
                })?;
            Ok::<(String, String), ApiError>((label, description))
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??
    };

    let desc = if server_description.is_empty() {
        None
    } else {
        Some(server_description.as_str())
    };

    let invite = InvitePayload::new(
        &public_url,
        &code,
        Some(&server_label),
        desc,
    );

    let url = invite.to_invite_url_with_base(&invite_base_url).map_err(|e| {
        ApiError::InternalServerError(format!("failed to generate invite URL: {}", e))
    })?;

    Ok(Json(CreateInviteResponse {
        code,
        url,
        expires_at,
    }))
}

/// Handler for `GET /api/invites`.
///
/// Lists all invite codes for the server. Requires `can_moderate` capability.
pub async fn list_invites_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<Vec<InviteCodeEntry>>, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to list invites".to_string(),
        ));
    }

    let invite_base_url = state.invite_base_url.clone();
    let public_url = state
        .public_url
        .read()
        .map_err(|_| ApiError::InternalServerError("public_url lock poisoned".to_string()))?
        .clone();

    let state_clone = state.clone();
    let entries = tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        // Fetch server label and description once
        let (server_label, server_description): (String, String) = conn
            .query_row(
                "SELECT label, description FROM servers WHERE id = ?1",
                [state_clone.server_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| {
                ApiError::InternalServerError(format!("failed to fetch server metadata: {}", e))
            })?;
        let desc_for_payload: Option<&str> = if server_description.is_empty() {
            None
        } else {
            Some(&server_description)
        };

        let mut stmt = conn
            .prepare(
                "SELECT code, created_by, max_uses, use_count, expires_at, created_at \
                 FROM invite_codes WHERE server_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| {
                ApiError::InternalServerError(format!("failed to prepare query: {}", e))
            })?;

        let rows = stmt
            .query_map([state_clone.server_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| {
                ApiError::InternalServerError(format!("failed to query invite codes: {}", e))
            })?;

        let mut entries = Vec::new();
        for row in rows {
            let (code, created_by, max_uses, use_count, expires_at, created_at) = row.map_err(
                |e| ApiError::InternalServerError(format!("failed to read row: {}", e)),
            )?;

            let url = if !public_url.is_empty() {
                let invite = InvitePayload::new(
                    &public_url,
                    &code,
                    Some(&server_label),
                    desc_for_payload,
                );
                invite
                    .to_invite_url_with_base(&invite_base_url)
                    .unwrap_or_default()
            } else {
                String::new()
            };

            entries.push(InviteCodeEntry {
                code,
                url,
                created_by,
                max_uses,
                use_count,
                expires_at,
                created_at,
            });
        }

        Ok::<Vec<InviteCodeEntry>, ApiError>(entries)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(entries))
}

/// Request body for `POST /api/invites/redeem`.
#[derive(Debug, Deserialize)]
pub struct RedeemInviteRequest {
    /// The invite code to validate and consume.
    pub code: String,
}

/// Response body for `POST /api/invites/redeem`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemInviteResponse {
    pub valid: bool,
    pub server_name: String,
    pub server_slug: String,
}

/// Handler for `POST /api/invites/redeem`.
///
/// Public (no auth) endpoint that validates an invite code during registration.
/// Checks expiration and max_uses, then increments use_count.
pub async fn redeem_invite_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<RedeemInviteRequest>,
) -> Result<Json<RedeemInviteResponse>, ApiError> {
    let code = payload.code.trim().to_string();
    if code.is_empty() {
        return Err(ApiError::BadRequest("invite code is required".to_string()));
    }

    let server_id = state.server_id;
    let state_clone = state.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        // Look up the invite code for this server
        let row: Result<(i64, Option<i64>, i64, Option<String>), _> = conn.query_row(
            "SELECT id, max_uses, use_count, expires_at FROM invite_codes WHERE server_id = ?1 AND code = ?2",
            rusqlite::params![server_id, code],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        );

        let (invite_id, max_uses, use_count, expires_at) = row.map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                ApiError::BadRequest("Invalid or expired invite code".to_string())
            }
            _ => ApiError::InternalServerError(format!("failed to query invite: {}", e)),
        })?;

        // Check expiration
        if let Some(ref exp) = expires_at {
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            if *exp < now {
                return Err(ApiError::BadRequest("Invalid or expired invite code".to_string()));
            }
        }

        // Check max uses
        if let Some(max) = max_uses {
            if use_count >= max {
                return Err(ApiError::BadRequest("Invalid or expired invite code".to_string()));
            }
        }

        // Increment use_count
        conn.execute(
            "UPDATE invite_codes SET use_count = use_count + 1 WHERE id = ?1",
            [invite_id],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to update invite: {}", e)))?;

        // Fetch server slug and label
        let (slug, label): (String, String) = conn
            .query_row(
                "SELECT slug, label FROM servers WHERE id = ?1",
                [server_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| {
                ApiError::InternalServerError(format!("failed to fetch server info: {}", e))
            })?;

        Ok(RedeemInviteResponse {
            valid: true,
            server_name: label,
            server_slug: slug,
        })
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(result))
}

/// Handler for `DELETE /api/invites/{code}`.
///
/// Deletes an invite code. Requires `can_moderate` capability.
pub async fn delete_invite_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(code): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to delete invites".to_string(),
        ));
    }

    let state_clone = state.clone();
    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        let deleted = conn
            .execute(
                "DELETE FROM invite_codes WHERE server_id = ?1 AND code = ?2",
                rusqlite::params![state_clone.server_id, code],
            )
            .map_err(|e| {
                ApiError::InternalServerError(format!("failed to delete invite code: {}", e))
            })?;

        if deleted == 0 {
            return Err(ApiError::NotFound("invite code not found".to_string()));
        }

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_uses_camel_case() {
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            Some("My Server"),
            Some("A cool server"),
        );
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"serverName\""));
        assert!(!json.contains("\"server_name\""));
        assert!(json.contains("\"description\""));
        assert!(json.contains("\"server\""));
        assert!(json.contains("\"code\""));
    }

    #[test]
    fn optional_none_fields_omitted() {
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            None::<String>,
            None::<String>,
        );
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("serverName"));
        assert!(!json.contains("description"));
    }

    #[test]
    fn base64url_encoding_no_padding_no_plus_no_slash() {
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            Some("My Server"),
            Some("A cool sovereign server"),
        );
        let url = payload.to_invite_url().unwrap();
        let encoded_part = url.strip_prefix("https://monolithannex.com/invite/").unwrap();
        assert!(!encoded_part.contains('+'));
        assert!(!encoded_part.contains('/'));
        assert!(!encoded_part.contains('='));
    }

    #[test]
    fn url_starts_with_correct_base() {
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            None::<String>,
            None::<String>,
        );
        let url = payload.to_invite_url().unwrap();
        assert!(url.starts_with("https://monolithannex.com/invite/"));
    }

    #[test]
    fn custom_base_url() {
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            None::<String>,
            None::<String>,
        );
        let url = payload
            .to_invite_url_with_base("https://staging.monolithannex.com/invite")
            .unwrap();
        assert!(url.starts_with("https://staging.monolithannex.com/invite/"));
    }

    #[test]
    fn roundtrip_encode_decode() {
        let original = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            Some("My Server"),
            Some("A cool sovereign server"),
        );
        let url = original.to_invite_url().unwrap();
        let encoded_part = url.strip_prefix("https://monolithannex.com/invite/").unwrap();

        let decoded_bytes = URL_SAFE_NO_PAD.decode(encoded_part).unwrap();
        let decoded: InvitePayload = serde_json::from_slice(&decoded_bytes).unwrap();

        assert_eq!(decoded.server, original.server);
        assert_eq!(decoded.code, original.code);
        assert_eq!(decoded.server_name, original.server_name);
        assert_eq!(decoded.description, original.description);
    }

    #[test]
    fn rejects_http_server_url() {
        let payload = InvitePayload::new(
            "http://annex.example.com",
            "abc123",
            None::<String>,
            None::<String>,
        );
        let err = payload.to_invite_url().unwrap_err();
        assert!(matches!(err, InviteLinkError::InvalidServerUrl(_)));
    }

    #[test]
    fn rejects_empty_code() {
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "",
            None::<String>,
            None::<String>,
        );
        let err = payload.to_invite_url().unwrap_err();
        assert!(matches!(err, InviteLinkError::InvalidCode(_)));
    }

    #[test]
    fn rejects_code_over_100_chars() {
        let long_code = "a".repeat(101);
        let payload = InvitePayload::new(
            "https://annex.example.com",
            long_code,
            None::<String>,
            None::<String>,
        );
        let err = payload.to_invite_url().unwrap_err();
        assert!(matches!(err, InviteLinkError::InvalidCode(_)));
    }

    #[test]
    fn rejects_server_name_over_100_chars() {
        let long_name = "a".repeat(101);
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            Some(long_name),
            None::<String>,
        );
        let err = payload.to_invite_url().unwrap_err();
        assert!(matches!(err, InviteLinkError::InvalidServerName(_)));
    }

    #[test]
    fn rejects_description_over_300_chars() {
        let long_desc = "a".repeat(301);
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            None::<String>,
            Some(long_desc),
        );
        let err = payload.to_invite_url().unwrap_err();
        assert!(matches!(err, InviteLinkError::InvalidDescription(_)));
    }

    #[test]
    fn accepts_valid_https_url() {
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            Some("Test Server"),
            Some("A test server"),
        );
        assert!(payload.to_invite_url().is_ok());
    }

    #[test]
    fn url_under_2048_chars_for_reasonable_input() {
        let payload = InvitePayload::new(
            "https://annex.example.com",
            "abc123",
            Some("My Awesome Annex Server"),
            Some("The best sovereign communication platform around"),
        );
        let url = payload.to_invite_url().unwrap();
        assert!(url.len() < 2048);
    }

    #[test]
    fn parse_valid_protocol_invite() {
        let url = "annex://invite?server=https%3A%2F%2Fannex.example.com&code=abc123";
        let invite = parse_protocol_invite(url).unwrap();
        assert_eq!(invite.server, "https://annex.example.com");
        assert_eq!(invite.code, "abc123");
    }

    #[test]
    fn parse_protocol_invite_missing_server() {
        let url = "annex://invite?code=abc123";
        assert!(parse_protocol_invite(url).is_none());
    }

    #[test]
    fn parse_protocol_invite_missing_code() {
        let url = "annex://invite?server=https%3A%2F%2Fannex.example.com";
        assert!(parse_protocol_invite(url).is_none());
    }

    #[test]
    fn parse_protocol_invite_wrong_path() {
        let url = "annex://something-else?server=https%3A%2F%2Fannex.example.com&code=abc123";
        assert!(parse_protocol_invite(url).is_none());
    }

    #[test]
    fn parse_protocol_invite_rejects_http() {
        let url = "annex://invite?server=http%3A%2F%2Fannex.example.com&code=abc123";
        assert!(parse_protocol_invite(url).is_none());
    }

    #[test]
    fn parse_protocol_invite_wrong_scheme() {
        let url = "https://invite?server=https%3A%2F%2Fannex.example.com&code=abc123";
        assert!(parse_protocol_invite(url).is_none());
    }

    #[test]
    fn exact_encoding_example_from_spec() {
        // Verify against the exact example from the spec
        let payload = InvitePayload {
            server: "https://annex.example.com".to_string(),
            code: "abc123".to_string(),
            server_name: Some("My Server".to_string()),
            description: Some("A cool sovereign server".to_string()),
        };
        let json = serde_json::to_string(&payload).unwrap();

        // Verify JSON structure
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["server"], "https://annex.example.com");
        assert_eq!(parsed["code"], "abc123");
        assert_eq!(parsed["serverName"], "My Server");
        assert_eq!(parsed["description"], "A cool sovereign server");

        // Verify the URL can be generated
        let url = payload.to_invite_url().unwrap();
        assert!(url.starts_with("https://monolithannex.com/invite/"));
    }
}
