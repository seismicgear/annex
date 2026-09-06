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
    // The first identity on a server becomes the founder and gets the three
    // PRIVILEGES — moderate, invite, federate. The founder check and insert are
    // combined into a single SQL statement to eliminate the TOCTOU race between
    // SELECT COUNT(*) and INSERT that would let concurrent registrations both
    // become founders.
    //
    // `can_voice` is NOT one of them. It used to be, which meant every member
    // except the very first silently could not join any call: the button
    // rendered disabled reading "Voice is disabled by server policy for your
    // identity", on a server whose `ServerPolicy::voice_enabled` defaults to
    // true. The operator's switch said voice was on and nobody but the owner
    // could use it — an internal contradiction, and one that hid every other
    // voice defect behind it, because two ordinary members could never get into
    // a call together to find them.
    //
    // Speaking is participation, not privilege. The server-level
    // `voice_enabled` is the operator's control; the per-identity flag exists
    // so a moderator can revoke voice from a *specific* person via
    // `PATCH /api/admin/members/{id}/capabilities`.
    conn.execute(
        "INSERT INTO platform_identities (
            server_id, pseudonym_id, participant_type,
            can_voice, can_moderate, can_invite, can_federate
        ) VALUES (?1, ?2, ?3,
            1,
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

/// Ensures at least one active identity on the server has moderator capabilities.
///
/// If no active identity has `can_moderate = 1`, the earliest active identity
/// is promoted to founder with core capabilities (voice, moderate, invite,
/// federate). This self-heals scenarios where stale identities prevented the
/// normal founder bootstrap in [`create_platform_identity`].
///
/// Returns `true` if a promotion was performed.
///
/// # Errors
///
/// Returns `IdentityError::DatabaseError` on query/update failure.
pub fn ensure_founder(conn: &Connection, server_id: i64) -> Result<bool, IdentityError> {
    let has_moderator: bool = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM platform_identities
            WHERE server_id = ?1 AND can_moderate = 1 AND active = 1
        )",
        params![server_id],
        |row| row.get(0),
    )?;

    if has_moderator {
        return Ok(false);
    }

    let promoted = conn.execute(
        "UPDATE platform_identities SET
            can_voice = 1,
            can_moderate = 1,
            can_invite = 1,
            can_federate = 1,
            updated_at = datetime('now')
        WHERE id = (
            SELECT MIN(id) FROM platform_identities
            WHERE server_id = ?1 AND active = 1
        )",
        params![server_id],
    )?;

    Ok(promoted > 0)
}

/// Returns `true` if applying `new_caps` to `pseudonym_id` would remove the
/// **last** active moderator from the server.
///
/// This is the case when all of the following hold:
///   * `new_caps.can_moderate` is `false` (the update drops moderation), and
///   * the target is currently an active moderator, and
///   * no *other* active moderator exists.
///
/// Callers must refuse such an update: a server with zero moderators is locked
/// out of all administrative control and drops into the no-moderator self-heal
/// path in [`ensure_founder`], where the next identity read re-promotes the
/// lowest-id active account. A moderator may still demote other moderators (or
/// themselves) as long as at least one active moderator remains.
///
/// # Errors
///
/// Returns `IdentityError::DatabaseError` on query failure.
pub fn would_remove_last_moderator(
    conn: &Connection,
    server_id: i64,
    pseudonym_id: &str,
    new_caps: Capabilities,
) -> Result<bool, IdentityError> {
    // Granting or retaining moderation can never drop the moderator count.
    if new_caps.can_moderate {
        return Ok(false);
    }

    let target_is_active_moderator: bool = conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM platform_identities
            WHERE server_id = ?1 AND pseudonym_id = ?2 AND can_moderate = 1 AND active = 1
        )",
        params![server_id, pseudonym_id],
        |row| row.get(0),
    )?;
    if !target_is_active_moderator {
        return Ok(false);
    }

    let active_moderators: i64 = conn.query_row(
        "SELECT COUNT(*) FROM platform_identities
         WHERE server_id = ?1 AND can_moderate = 1 AND active = 1",
        params![server_id],
        |row| row.get(0),
    )?;

    Ok(active_moderators <= 1)
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
