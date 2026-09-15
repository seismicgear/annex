//! What an AI agent may still do after its alignment changes.
//!
//! Alignment was enforced at exactly one moment: the join. `check_join_policy`
//! reads `agent_registrations` and refuses a `Conflict` agent, a
//! `Partial` agent in a non-text channel, and an agent that does not meet the
//! channel's stated minimum — and then never looks again. So the sweep in
//! `policy.rs::recalculate_agent_alignments`, whose whole purpose is to cut off
//! an agent whose principles no longer match the server's, did two things:
//! it set `agent_registrations.active = 0` and it dropped the agent's
//! WebSocket. It did not touch `channel_members`, and it did not touch
//! `platform_identities.active`, so the agent's session token still verified,
//! it reconnected, and it kept sending, editing, deleting, speaking and
//! creating channels in every channel it had already joined. The sweep emitted
//! `AgentDisconnected` into the audit log while the agent carried on working.
//!
//! This is the gate at action time. It is deliberately the twin of
//! `channel_policy` in shape — `&Connection` in, a refusal enum out that each
//! caller maps into its own error type — because a second copy of a gate is how
//! the join path and the federation path drifted apart, and the failure mode
//! was silent on both sides.
//!
//! ## What is NOT enforced here, and why
//!
//! `agent_registrations.capability_contract_json` looks like an action
//! permission set and is not one. Its `required_capabilities` and
//! `offered_capabilities` arrays have exactly one consumer,
//! `annex_vrp::contracts_mutually_accepted`, and that is a handshake-time
//! compatibility test between two peers' declarations. Treating them as a
//! grant list would start from "grants nothing" on every agent already
//! registered: `ServerPolicy::agent_required_capabilities` defaults to empty,
//! and the stored contract is `'{}'` in every fixture in this repo and almost
//! certainly in real deployments. The one contract field that does name an
//! action — `redacted_topics` — is already enforced at action time, at RTX
//! publish and at federation ingest.
//!
//! Worse, `'{}'` does not even deserialize: `VrpCapabilitySharingContract`
//! gives a serde default only to `redacted_topics`, so
//! `from_str::<VrpCapabilitySharingContract>("{}")` fails. That failure is a
//! refusal in the RTX path, which is right for RTX and would be catastrophic
//! on `send_message`. The contract is therefore carried through this module as
//! an unparsed string and is never deserialized here.

use annex_types::AlignmentStatus;
use rusqlite::{params, Connection, OptionalExtension};

/// An action an agent can take that alignment governs.
///
/// Reads are absent on purpose. Alignment decides whether this server accepts
/// what an agent *does*; removing an existing member's ability to read back a
/// channel it is still a member of is a retention decision, not an alignment
/// one, and nothing in the ROADMAP or AGENTS.md asks for it. A `Conflict`
/// agent loses its sessions anyway (see `revoke_agent_sessions` in
/// `policy.rs`), so in practice it reads nothing either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentAction {
    SendText,
    EditText,
    DeleteText,
    /// Publishing or receiving call media — the HTTP voice join and the
    /// WebSocket `webrtc_offer` frame both land here.
    VoiceMedia,
    /// The `voice_intent` frame: text an agent asks the server to speak.
    VoiceIntent,
    CreateChannel,
}

impl AgentAction {
    fn describe(self) -> &'static str {
        match self {
            AgentAction::SendText => "send messages",
            AgentAction::EditText => "edit messages",
            AgentAction::DeleteText => "delete messages",
            AgentAction::VoiceMedia => "use voice",
            AgentAction::VoiceIntent => "speak",
            AgentAction::CreateChannel => "create channels",
        }
    }

    /// Whether a `Partial`-aligned agent may take this action.
    ///
    /// ROADMAP 6.2: a partially-aligned agent is "TEXT only — no VOICE, no
    /// HYBRID voice features". The same table in AGENTS.md says the same
    /// thing. Channel creation is grouped with voice rather than with text:
    /// creating a channel sets policy for other participants, which is not a
    /// text message.
    fn allowed_when_partial(self) -> bool {
        matches!(
            self,
            AgentAction::SendText | AgentAction::EditText | AgentAction::DeleteText
        )
    }
}

/// Why an action was refused.
#[derive(Debug)]
pub(crate) enum ActionRefusal {
    /// The agent may not do this, with a reason safe to return to it.
    Forbidden(String),
    /// Something this server could not read or compute.
    Internal(String),
}

/// One agent's registration row, as the action gate sees it.
///
/// Two fields the row also carries are deliberately absent. `transfer_scope`
/// and `capability_contract_json` are read by RTX, which has its own parsers
/// for them (`rtx_service::extract_redacted_topics`,
/// `rtx_repository::agent_active_transfer_scope`) and its own semantics — a
/// contract it cannot parse is a hard refusal there, which is right for a
/// knowledge transfer and would be catastrophic on `send_message`. Carrying
/// them here unread, in anticipation of unifying the three readers, would be
/// two fields that rot; the unification belongs in the RTX work, with RTX's
/// tests beside it.
#[derive(Debug, Clone)]
pub(crate) struct AgentGrant {
    pub alignment: AlignmentStatus,
    pub active: bool,
}

/// Read the agent registration for `pseudonym`, if there is one.
///
/// Keyed on the pseudonym rather than on a `PlatformIdentity`, so the hot
/// paths do not have to thread an identity through. That is sound because
/// `agent_registrations` is written by exactly one thing — the local VRP
/// handshake — and only for AI agents, so a human never has a row and a
/// missing row is indistinguishable from "not an agent". The cases are
/// treated identically either way: both pass.
pub(crate) fn load_agent_grant(
    conn: &Connection,
    server_id: i64,
    pseudonym: &str,
) -> Result<Option<AgentGrant>, ActionRefusal> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT alignment_status, active \
             FROM agent_registrations WHERE server_id = ?1 AND pseudonym_id = ?2",
            params![server_id, pseudonym],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| ActionRefusal::Internal(format!("agent_registrations query: {e}")))?;

    let Some((alignment_str, active)) = row else {
        return Ok(None);
    };

    Ok(Some(AgentGrant {
        alignment: parse_alignment(&alignment_str)?,
        active: active != 0,
    }))
}

/// Two spellings reach this column and both have to parse.
///
/// `api_vrp.rs` writes `'CONFLICT'` on one path and `'Conflict'` on another,
/// and `policy.rs` writes `'CONFLICT'`. A match on one spelling reads the
/// other as unparseable, and then — depending on which way the error maps —
/// either 500s every send or fails open. The tolerant two-step is copied from
/// `channel_policy::check_join_policy` for exactly this reason.
fn parse_alignment(raw: &str) -> Result<AlignmentStatus, ActionRefusal> {
    serde_json::from_str(raw)
        .or_else(|_| serde_json::from_str(&format!("\"{raw}\"")))
        .map_err(|e| ActionRefusal::Internal(format!("alignment parse ({raw:?}): {e}")))
}

/// The gate. Refuse `action` if this agent's registration says so.
///
/// Passes for anything with no registration row. That is the same judgement
/// `channel_policy` documents at its own missing-row branch: a federated agent
/// has no local registration, and refusing here would silently cut off every
/// agent from every other server — a different defect from the one this
/// closes. Only an EXISTING row that says `Conflict`, or `active = 0`, refuses.
pub(crate) fn check_agent_action(
    conn: &Connection,
    server_id: i64,
    pseudonym: &str,
    action: AgentAction,
) -> Result<(), ActionRefusal> {
    let Some(grant) = load_agent_grant(conn, server_id, pseudonym)? else {
        return Ok(());
    };
    check_grant(&grant, action)
}

/// The rules, separated from the query so they can be tested without a
/// database and reused by a caller that already holds the row.
pub(crate) fn check_grant(grant: &AgentGrant, action: AgentAction) -> Result<(), ActionRefusal> {
    if !grant.active {
        return Err(ActionRefusal::Forbidden(format!(
            "this agent's registration is not active on this server, so it may not {}",
            action.describe()
        )));
    }

    match grant.alignment {
        AlignmentStatus::Conflict => Err(ActionRefusal::Forbidden(format!(
            "this agent's alignment is in conflict with the server's principles, so it may not {}",
            action.describe()
        ))),
        AlignmentStatus::Partial if !action.allowed_when_partial() => {
            Err(ActionRefusal::Forbidden(format!(
                "partially-aligned agents are restricted to text and may not {}",
                action.describe()
            )))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(alignment: AlignmentStatus, active: bool) -> AgentGrant {
        AgentGrant { alignment, active }
    }

    const EVERY_ACTION: [AgentAction; 6] = [
        AgentAction::SendText,
        AgentAction::EditText,
        AgentAction::DeleteText,
        AgentAction::VoiceMedia,
        AgentAction::VoiceIntent,
        AgentAction::CreateChannel,
    ];

    #[test]
    fn a_conflict_agent_may_do_nothing() {
        for action in EVERY_ACTION {
            assert!(
                matches!(
                    check_grant(&grant(AlignmentStatus::Conflict, true), action),
                    Err(ActionRefusal::Forbidden(_))
                ),
                "{action:?} should be refused for a conflict-aligned agent",
            );
        }
    }

    #[test]
    fn an_inactive_registration_refuses_even_when_aligned() {
        // The state the sweep actually writes: it sets `active = 0` and
        // leaves `alignment_status` at whatever it computed.
        for action in EVERY_ACTION {
            assert!(matches!(
                check_grant(&grant(AlignmentStatus::Aligned, false), action),
                Err(ActionRefusal::Forbidden(_))
            ));
        }
    }

    #[test]
    fn a_partial_agent_keeps_text_and_loses_voice() {
        let g = grant(AlignmentStatus::Partial, true);
        for action in [
            AgentAction::SendText,
            AgentAction::EditText,
            AgentAction::DeleteText,
        ] {
            assert!(check_grant(&g, action).is_ok(), "{action:?} should pass");
        }
        for action in [
            AgentAction::VoiceMedia,
            AgentAction::VoiceIntent,
            AgentAction::CreateChannel,
        ] {
            assert!(
                matches!(check_grant(&g, action), Err(ActionRefusal::Forbidden(_))),
                "{action:?} should be refused",
            );
        }
    }

    #[test]
    fn an_aligned_active_agent_may_do_everything() {
        let g = grant(AlignmentStatus::Aligned, true);
        for action in EVERY_ACTION {
            assert!(check_grant(&g, action).is_ok(), "{action:?} should pass");
        }
    }

    #[test]
    fn both_spellings_of_every_status_parse() {
        // api_vrp.rs writes 'CONFLICT' on one path and 'Conflict' on another.
        for (raw, expected) in [
            ("CONFLICT", AlignmentStatus::Conflict),
            ("Conflict", AlignmentStatus::Conflict),
            ("ALIGNED", AlignmentStatus::Aligned),
            ("Aligned", AlignmentStatus::Aligned),
            ("PARTIAL", AlignmentStatus::Partial),
            ("Partial", AlignmentStatus::Partial),
        ] {
            assert_eq!(
                parse_alignment(raw).expect(raw),
                expected,
                "{raw} should parse",
            );
        }
    }

    #[test]
    fn an_unreadable_status_is_internal_not_a_silent_pass() {
        // Failing open here would be the whole defect again, one layer down.
        assert!(matches!(
            parse_alignment("not-a-status"),
            Err(ActionRefusal::Internal(_))
        ));
    }
}
