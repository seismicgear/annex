//! Platform Identity Registry.
//!
//! Manages `platform_identities` table, linking pseudonyms to participants
//! and storing their capability flags.

use crate::IdentityError;
pub use annex_types::Capabilities;
use annex_types::RoleCode;
use rusqlite::{params, Connection};

/// A platform identity record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformIdentity {
    pub id: i64,
    pub server_id: i64,
    pub pseudonym_id: String,
    pub participant_type: RoleCode,
    pub can_voice: bool,
    pub can_moderate: bool,
    pub can_invite: bool,
    pub can_federate: bool,
    pub can_bridge: bool,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Converts a string label to a RoleCode.
pub fn role_from_str(s: &str) -> Result<RoleCode, IdentityError> {
    match s {
        "HUMAN" => Ok(RoleCode::Human),
        "AI_AGENT" => Ok(RoleCode::AiAgent),
        "COLLECTIVE" => Ok(RoleCode::Collective),
        "BRIDGE" => Ok(RoleCode::Bridge),
        "SERVICE" => Ok(RoleCode::Service),
        _ => Err(IdentityError::InvalidRoleLabel(s.to_string())),
    }
}

/// Creates a new platform identity.
///
/// # Errors
///
/// Returns `IdentityError::DatabaseError` if the insertion fails (e.g. duplicate constraint).
pub fn create_platform_identity(
    conn: &Connection,
    server_id: i64,
    pseudonym_id: &str,
    participant_type: RoleCode,
) -> Result<PlatformIdentity, IdentityError> {
    // The first identity on a server becomes the founder and gets core capabilities
    // (voice, moderate, invite, federate). The founder check and insert are combined
    // into a single SQL statement to eliminate the TOCTOU race between SELECT COUNT(*)
    // and INSERT that would allow concurrent registrations to both become founders.
    conn.execute(
        "INSERT INTO platform_identities (
            server_id, pseudonym_id, participant_type,
            can_voice, can_moderate, can_invite, can_federate
        ) VALUES (?1, ?2, ?3,
            (SELECT CASE WHEN COUNT(*) = 0 THEN 1 ELSE 0 END FROM platform_identities WHERE server_id = ?1),
            (SELECT CASE WHEN COUNT(*) = 0 THEN 1 ELSE 0 END FROM platform_identities WHERE server_id = ?1),
            (SELECT CASE WHEN COUNT(*) = 0 THEN 1 ELSE 0 END FROM platform_identities WHERE server_id = ?1),
            (SELECT CASE WHEN COUNT(*) = 0 THEN 1 ELSE 0 END FROM platform_identities WHERE server_id = ?1)
        )",
        params![
            server_id,
            pseudonym_id,
            participant_type.label(),
        ],
    )?;

    get_platform_identity(conn, server_id, pseudonym_id)
}

/// Retrieves a platform identity by server ID and pseudonym ID.
///
/// # Errors
///
/// Returns `IdentityError::DatabaseError` if the query fails.
/// Returns error if not found (rusqlite::Error::QueryReturnedNoRows wrapped).
pub fn get_platform_identity(
    conn: &Connection,
    server_id: i64,
    pseudonym_id: &str,
) -> Result<PlatformIdentity, IdentityError> {
    conn.query_row(
        "SELECT
            id, server_id, pseudonym_id, participant_type,
            can_voice, can_moderate, can_invite, can_federate, can_bridge,
            active, created_at, updated_at
        FROM platform_identities
        WHERE server_id = ?1 AND pseudonym_id = ?2",
        params![server_id, pseudonym_id],
        |row| {
            let role_str: String = row.get(3)?;
            let participant_type = role_from_str(&role_str).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;

            Ok(PlatformIdentity {
                id: row.get(0)?,
                server_id: row.get(1)?,
                pseudonym_id: row.get(2)?,
                participant_type,
                can_voice: row.get(4)?,
                can_moderate: row.get(5)?,
                can_invite: row.get(6)?,
                can_federate: row.get(7)?,
                can_bridge: row.get(8)?,
                active: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        },
    )
    .map_err(IdentityError::DatabaseError)
}

/// Updates the capability flags for a platform identity.
///
/// # Errors
///
/// Returns `IdentityError::DatabaseError` if the update fails.
pub fn update_capabilities(
    conn: &Connection,
    server_id: i64,
    pseudonym_id: &str,
    caps: Capabilities,
) -> Result<(), IdentityError> {
    let changed = conn.execute(
        "UPDATE platform_identities SET
            can_voice = ?1,
            can_moderate = ?2,
            can_invite = ?3,
            can_federate = ?4,
            can_bridge = ?5,
            updated_at = datetime('now')
        WHERE server_id = ?6 AND pseudonym_id = ?7",
        params![
            caps.can_voice,
            caps.can_moderate,
            caps.can_invite,
            caps.can_federate,
            caps.can_bridge,
            server_id,
            pseudonym_id
        ],
    )?;

    if changed == 0 {
        return Err(IdentityError::DatabaseError(
            rusqlite::Error::QueryReturnedNoRows,
        ));
    }

    Ok(())
}

/// Deactivates a platform identity (sets active = 0).
///
/// # Errors
///
/// Returns `IdentityError::DatabaseError` if the update fails.
pub fn deactivate_platform_identity(
    conn: &Connection,
    server_id: i64,
    pseudonym_id: &str,
) -> Result<(), IdentityError> {
    let changed = conn.execute(
        "UPDATE platform_identities SET
            active = 0,
            updated_at = datetime('now')
        WHERE server_id = ?1 AND pseudonym_id = ?2",
        params![server_id, pseudonym_id],
    )?;

    if changed == 0 {
        return Err(IdentityError::DatabaseError(
            rusqlite::Error::QueryReturnedNoRows,
        ));
    }

    Ok(())
}
