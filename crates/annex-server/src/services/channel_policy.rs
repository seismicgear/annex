//! Who may join a channel.
//!
//! One function, called by both paths that insert into `channel_members`,
//! because they had drifted apart and the drift was total rather than partial.
//! `ChannelService::join_channel` ran five gates — required capabilities, the
//! agent-channel restriction, the conflict refusal, the partial-alignment
//! text-only restriction, and the channel's own minimum alignment.
//! `FederationService::join_federated_channel` ran one: is the channel
//! federated. Everything else — including whether the identity is active at
//! all — was checked on the local path and nowhere else, and the two callers
//! reached the same `annex_channels::add_member`, which carries no policy of
//! its own.
//!
//! So a remote peer could put a member into a channel that the local operator
//! had marked agents-only, or capability-gated, or restricted to
//! well-aligned agents, by asking through federation instead of the front
//! door. The channel's stated policy applied to local members and to nobody
//! else.
//!
//! This is extracted rather than copied on purpose: a second copy of a gate is
//! how the first divergence happened, and the failure mode is silent on both
//! sides — the local path keeps working and the federated one keeps admitting.

use annex_channels::Channel;
use annex_identity::PlatformIdentity;
use annex_types::{AlignmentStatus, ChannelType, RoleCode};
use rusqlite::{params, Connection, OptionalExtension};

/// Why a join was refused. The caller maps it into its own error type, because
/// the two callers answer to different protocols.
#[derive(Debug)]
pub(crate) enum JoinRefusal {
    /// The identity may not join, with a reason safe to show the requester.
    Forbidden(String),
    /// Something this server could not read or compute.
    Internal(String),
}

/// Evaluate every join gate for `identity` against `channel`.
///
/// Blocking: it reads `agent_registrations` for an AI agent. Both callers hold
/// a `Connection` already, which is why this takes one rather than a pool.
///
/// # The agent case, which is the one judgement call
///
/// An agent's alignment lives in `agent_registrations`, and the only thing
/// that writes that table is the local VRP handshake. A federated agent has no
/// row. Mirroring the local path verbatim would therefore refuse every
/// federated agent everywhere, which is a different defect from the one being
/// fixed — a policy that says "no agents from other servers, ever" was not
/// what any operator configured.
///
/// The rule here: a missing registration is fatal exactly where alignment
/// actually governs — an `Agent` channel, or a channel that states an
/// `agent_min_alignment` — and permissive elsewhere. An unknown alignment
/// never satisfies a stated minimum, and never passes for `Aligned`. So an
/// operator who has expressed a requirement gets it enforced, and one who has
/// not is not handed a restriction they never asked for.
pub(crate) fn check_join_policy(
    conn: &Connection,
    server_id: i64,
    channel: &Channel,
    identity: &PlatformIdentity,
) -> Result<(), JoinRefusal> {
    // A deactivated identity does not join anything. On the local path
    // `auth_middleware` refuses the request long before this point; the
    // federated path has no such middleware, which is exactly why this lives
    // in the shared gate rather than at one call site.
    if !identity.active {
        return Err(JoinRefusal::Forbidden(
            "identity is deactivated on this server".to_string(),
        ));
    }

    if let Some(caps_json) = &channel.required_capabilities_json {
        let required: Vec<String> = serde_json::from_str(caps_json).map_err(|e| {
            JoinRefusal::Internal(format!("malformed required_capabilities_json: {e}"))
        })?;

        for req in required {
            let has_cap = match req.as_str() {
                "can_voice" => identity.can_voice,
                "can_moderate" => identity.can_moderate,
                "can_invite" => identity.can_invite,
                "can_federate" => identity.can_federate,
                "can_bridge" => identity.can_bridge,
                _ => false, // Unknown capability required -> deny
            };
            if !has_cap {
                return Err(JoinRefusal::Forbidden(
                    "missing required capability".to_string(),
                ));
            }
        }
    }

    // Agent channels are restricted to AI agents only. Allowing humans
    // would let them bypass agent-specific policy controls (alignment,
    // VRP handshake, transfer scope).
    if channel.channel_type == ChannelType::Agent && identity.participant_type != RoleCode::AiAgent
    {
        return Err(JoinRefusal::Forbidden(
            "agent channel requires AiAgent participant".to_string(),
        ));
    }

    if identity.participant_type != RoleCode::AiAgent {
        return Ok(());
    }

    let alignment_status: Option<String> = conn
        .query_row(
            "SELECT alignment_status FROM agent_registrations \
             WHERE server_id = ?1 AND pseudonym_id = ?2",
            params![server_id, identity.pseudonym_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| JoinRefusal::Internal(format!("alignment query: {e}")))?;

    let Some(status_str) = alignment_status else {
        // See the doc comment: fatal only where alignment governs.
        if channel.channel_type == ChannelType::Agent || channel.agent_min_alignment.is_some() {
            return Err(JoinRefusal::Forbidden("agent not registered".to_string()));
        }
        return Ok(());
    };

    let status: AlignmentStatus = serde_json::from_str(&status_str)
        .or_else(|_| serde_json::from_str(&format!("\"{status_str}\"")))
        .map_err(|e| JoinRefusal::Internal(format!("alignment parse: {e}")))?;

    if status == AlignmentStatus::Conflict {
        return Err(JoinRefusal::Forbidden(
            "conflict-aligned agents may not join channels".to_string(),
        ));
    }

    if status == AlignmentStatus::Partial && channel.channel_type != ChannelType::Text {
        return Err(JoinRefusal::Forbidden(
            "partial-aligned agents are restricted to text channels".to_string(),
        ));
    }

    if let Some(min_alignment) = channel.agent_min_alignment {
        let allowed = match min_alignment {
            AlignmentStatus::Conflict => true,
            AlignmentStatus::Partial => status != AlignmentStatus::Conflict,
            AlignmentStatus::Aligned => status == AlignmentStatus::Aligned,
        };
        if !allowed {
            return Err(JoinRefusal::Forbidden(
                "channel min alignment not met".to_string(),
            ));
        }
    }

    Ok(())
}
