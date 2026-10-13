//! Member-signed membership lineage. A server directory never grants key access.
use super::e2e_service::{self as e2e, E2eError};
use crate::{
    device_certificates::{self, DeviceCertificate},
    AppState,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JoinPolicy {
    EligibleSelfJoin,
    ModeratorAdmission,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Member,
    Moderator,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Genesis {
        join_policy: JoinPolicy,
        history_policy: String,
        certificate: DeviceCertificate,
    },
    Join {
        certificate: DeviceCertificate,
    },
    AdmitMember {
        certificate: DeviceCertificate,
    },
    Leave,
    RemoveMember {
        pseudonym_id: String,
    },
    SetRole {
        pseudonym_id: String,
        role: Role,
    },
    AdmitDevice {
        certificate: DeviceCertificate,
    },
    RecoverDevice {
        certificate: DeviceCertificate,
    },
    RevokeDevice {
        device_id: String,
    },
    SetPolicy {
        join_policy: JoinPolicy,
    },
    SetEncryption {
        enabled: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedManifestEvent {
    pub version: u8,
    pub authority_identity: String,
    pub channel_id: String,
    pub seq: i64,
    pub previous_hash: String,
    pub operation: Operation,
    pub signer_pseudonym_id: String,
    pub signer_device_id: String,
    pub signer_certificate_sha256: String,
    pub event_hash: String,
    pub signature_hex: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDevice {
    pub device_id: String,
    pub certificate_sha256: String,
    pub certificate: DeviceCertificate,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestMember {
    pub pseudonym_id: String,
    pub role: Role,
    pub devices: Vec<ManifestDevice>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestProjection {
    pub authority_identity: String,
    pub channel_id: String,
    pub head_seq: i64,
    pub head_hash: String,
    pub genesis_hash: String,
    pub join_policy: JoinPolicy,
    pub history_policy: String,
    pub e2e_enabled: bool,
    pub members: Vec<ManifestMember>,
    pub revoked_devices: Vec<(String, String)>,
}
fn bad(message: &str) -> E2eError {
    E2eError::BadRequest(message.into())
}
fn forbidden(message: &str) -> E2eError {
    E2eError::Forbidden(message.into())
}
fn conflict(message: &str) -> E2eError {
    E2eError::Conflict {
        message: message.into(),
        state: None,
    }
}
fn canonical(value: &Value) -> String {
    super::channel_authority::canonical_json(value)
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn signing_input(event: &SignedManifestEvent) -> Vec<u8> {
    canonical(&json!([
        "annex-channel-manifest-v1",
        event.version,
        event.authority_identity,
        event.channel_id,
        event.seq,
        event.previous_hash,
        event.operation,
        event.signer_pseudonym_id,
        event.signer_device_id,
        event.signer_certificate_sha256
    ]))
    .into_bytes()
}
fn verify_signature(key: &str, signature: &str, input: &[u8]) -> Result<(), E2eError> {
    if !e2e::lower_hex(key)
        || signature.len() != 128
        || signature
            .bytes()
            .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
    {
        return Err(forbidden("invalid member signature encoding"));
    }
    let bytes: [u8; 32] = hex::decode(key)
        .map_err(|_| bad("invalid signing key"))?
        .try_into()
        .map_err(|_| bad("invalid signing key length"))?;
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|_| bad("invalid signing key"))?;
    if key.is_weak() {
        return Err(forbidden("weak member signing key"));
    }
    let signature = ed25519_dalek::Signature::from_slice(
        &hex::decode(signature).map_err(|_| bad("invalid signature"))?,
    )
    .map_err(|_| bad("invalid signature"))?;
    key.verify_strict(input, &signature)
        .map_err(|_| forbidden("member signature does not verify"))
}
fn operation_certificate(op: &Operation) -> Option<&DeviceCertificate> {
    match op {
        Operation::Genesis { certificate, .. }
        | Operation::Join { certificate }
        | Operation::AdmitMember { certificate }
        | Operation::AdmitDevice { certificate }
        | Operation::RecoverDevice { certificate } => Some(certificate),
        _ => None,
    }
}
fn device(certificate: &DeviceCertificate) -> ManifestDevice {
    ManifestDevice {
        device_id: certificate.device_id.clone(),
        certificate_sha256: device_certificates::certificate_digest(certificate),
        certificate: certificate.clone(),
    }
}
fn member(certificate: &DeviceCertificate, role: Role) -> ManifestMember {
    ManifestMember {
        pseudonym_id: certificate.pseudonym_id.clone(),
        role,
        devices: vec![device(certificate)],
    }
}
impl ManifestProjection {
    pub fn admitted(&self, pid: &str, device: &str) -> Option<&ManifestDevice> {
        self.members
            .iter()
            .find(|m| m.pseudonym_id == pid)?
            .devices
            .iter()
            .find(|d| d.device_id == device)
    }
    fn moderator(&self, pid: &str) -> bool {
        self.members
            .iter()
            .any(|m| m.pseudonym_id == pid && m.role == Role::Moderator)
    }
    fn normalize(&mut self) {
        self.members
            .sort_by(|a, b| a.pseudonym_id.cmp(&b.pseudonym_id));
        for m in &mut self.members {
            m.devices.sort_by(|a, b| a.device_id.cmp(&b.device_id));
        }
        self.revoked_devices.sort();
        self.revoked_devices.dedup();
    }
}
/// Independently usable verifier: all authority comes from the previous signed
/// projection, except the explicit first-use genesis trust decision.
pub fn verify_transition(
    previous: Option<&ManifestProjection>,
    event: &SignedManifestEvent,
    vkey: &annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>,
) -> Result<ManifestProjection, E2eError> {
    if event.version != 1
        || event.seq < 1
        || event.seq > e2e::MAX_SAFE_INTEGER
        || event.channel_id.is_empty()
        || event.channel_id.len() > 128
        || event.channel_id.chars().any(char::is_control)
        || !event.authority_identity.starts_with("annex-server:")
        || !e2e::lower_hex(event.authority_identity.trim_start_matches("annex-server:"))
        || !e2e::lower_hex(&event.signer_certificate_sha256)
    {
        return Err(bad("invalid manifest event fields"));
    }
    let input = signing_input(event);
    if input.len() > 256 * 1024 {
        return Err(bad("manifest event exceeds size limit"));
    }
    if event.event_hash != hash(&input) {
        return Err(forbidden("manifest event digest mismatch"));
    }
    if let Some(p) = previous {
        if event.seq != p.head_seq + 1
            || event.previous_hash != p.head_hash
            || event.authority_identity != p.authority_identity
            || event.channel_id != p.channel_id
        {
            return Err(conflict("manifest ancestry changed"));
        }
    } else if event.seq != 1 || !event.previous_hash.is_empty() {
        return Err(conflict("manifest must begin at genesis"));
    }
    if let Some(cert) = operation_certificate(&event.operation) {
        device_certificates::verify_certificate(
            cert,
            &cert.server_identity,
            &cert.pseudonym_id,
            vkey,
        )?;
    }
    let self_cert = match &event.operation {
        Operation::Genesis { certificate, .. }
        | Operation::Join { certificate }
        | Operation::RecoverDevice { certificate } => Some(certificate),
        _ => None,
    };
    let signer = if let Some(cert) = self_cert {
        if cert.pseudonym_id != event.signer_pseudonym_id
            || cert.device_id != event.signer_device_id
        {
            return Err(forbidden("self-authorized operation certificate mismatch"));
        }
        device(cert)
    } else {
        previous
            .and_then(|p| p.admitted(&event.signer_pseudonym_id, &event.signer_device_id))
            .cloned()
            .ok_or_else(|| forbidden("signing device is not admitted by the previous manifest"))?
    };
    if signer.certificate_sha256 != event.signer_certificate_sha256 {
        return Err(forbidden("signing certificate digest mismatch"));
    }
    device_certificates::verify_certificate(
        &signer.certificate,
        &signer.certificate.server_identity,
        &event.signer_pseudonym_id,
        vkey,
    )?;
    verify_signature(
        &signer.certificate.ed25519_pub_hex,
        &event.signature_hex,
        &input,
    )?;
    let pid = &event.signer_pseudonym_id;
    let mut next = if let Some(p) = previous {
        p.clone()
    } else {
        let Operation::Genesis {
            join_policy,
            history_policy,
            certificate,
        } = &event.operation
        else {
            return Err(forbidden("first manifest event must be genesis"));
        };
        if history_policy != "all_members" {
            return Err(bad("unsupported history policy"));
        }
        ManifestProjection {
            authority_identity: event.authority_identity.clone(),
            channel_id: event.channel_id.clone(),
            head_seq: 0,
            head_hash: String::new(),
            genesis_hash: event.event_hash.clone(),
            join_policy: join_policy.clone(),
            history_policy: history_policy.clone(),
            e2e_enabled: true,
            members: vec![member(certificate, Role::Moderator)],
            revoked_devices: vec![],
        }
    };
    if previous.is_some() {
        match &event.operation {
            Operation::Genesis { .. } => {
                return Err(forbidden("manifest genesis cannot be replaced"))
            }
            Operation::Join { certificate } => {
                if next.join_policy != JoinPolicy::EligibleSelfJoin
                    || next.members.iter().any(|m| &m.pseudonym_id == pid)
                {
                    return Err(forbidden("self-join is not permitted by this manifest"));
                }
                let role = if next.members.is_empty() {
                    Role::Moderator
                } else {
                    Role::Member
                };
                next.members.push(member(certificate, role));
            }
            Operation::AdmitMember { certificate } => {
                if !next.moderator(pid)
                    || next
                        .members
                        .iter()
                        .any(|m| m.pseudonym_id == certificate.pseudonym_id)
                {
                    return Err(forbidden(
                        "member admission requires an existing moderator and absent target",
                    ));
                }
                next.members.push(member(certificate, Role::Member));
            }
            Operation::Leave => {
                next.members.retain(|m| &m.pseudonym_id != pid);
            }
            Operation::RemoveMember { pseudonym_id } => {
                if !next.moderator(pid) || pseudonym_id == pid {
                    return Err(forbidden(
                        "use self-leave, or moderator removal of another member",
                    ));
                }
                if !next.members.iter().any(|m| &m.pseudonym_id == pseudonym_id) {
                    return Err(bad("member is absent"));
                }
                next.members.retain(|m| &m.pseudonym_id != pseudonym_id);
            }
            Operation::SetRole { pseudonym_id, role } => {
                if !next.moderator(pid) {
                    return Err(forbidden("only a manifest moderator changes roles"));
                }
                next.members
                    .iter_mut()
                    .find(|m| &m.pseudonym_id == pseudonym_id)
                    .ok_or_else(|| bad("member is absent"))?
                    .role = role.clone();
            }
            Operation::AdmitDevice { certificate } => {
                if &certificate.pseudonym_id != pid {
                    return Err(forbidden(
                        "device admission is restricted to the signing identity",
                    ));
                }
                let m = next
                    .members
                    .iter_mut()
                    .find(|m| &m.pseudonym_id == pid)
                    .ok_or_else(|| forbidden("member is absent"))?;
                if m.devices
                    .iter()
                    .any(|d| d.device_id == certificate.device_id)
                {
                    return Err(conflict("device is already admitted"));
                }
                m.devices.push(device(certificate));
            }
            Operation::RecoverDevice { certificate } => {
                let m = next
                    .members
                    .iter_mut()
                    .find(|m| &m.pseudonym_id == pid)
                    .ok_or_else(|| forbidden("recovery cannot grant channel membership"))?;
                if m.devices
                    .iter()
                    .any(|d| d.device_id == certificate.device_id)
                {
                    return Err(forbidden("recovery must use a fresh unadmitted device"));
                }
                for d in &m.devices {
                    next.revoked_devices
                        .push((pid.clone(), d.device_id.clone()));
                }
                m.devices = vec![device(certificate)];
            }
            Operation::RevokeDevice { device_id } => {
                let m = next
                    .members
                    .iter_mut()
                    .find(|m| &m.pseudonym_id == pid)
                    .ok_or_else(|| forbidden("member is absent"))?;
                if !m.devices.iter().any(|d| &d.device_id == device_id) {
                    return Err(bad("device is absent"));
                }
                m.devices.retain(|d| &d.device_id != device_id);
                next.revoked_devices.push((pid.clone(), device_id.clone()));
            }
            Operation::SetEncryption { enabled } => {
                if !next.moderator(pid) {
                    return Err(forbidden(
                        "only a manifest moderator changes encryption policy",
                    ));
                }
                next.e2e_enabled = *enabled;
            }
            Operation::SetPolicy { join_policy } => {
                if !next.moderator(pid) {
                    return Err(forbidden(
                        "only a manifest moderator changes admission policy",
                    ));
                }
                next.join_policy = join_policy.clone();
            }
        }
    }
    if !next.members.is_empty() && !next.members.iter().any(|m| m.role == Role::Moderator) {
        return Err(forbidden(
            "transfer the moderator role before removing the last moderator",
        ));
    }
    if next.members.is_empty() && next.join_policy == JoinPolicy::ModeratorAdmission {
        return Err(forbidden("change to public admission or appoint another moderator before leaving the final private membership"));
    }
    for m in &next.members {
        if m.devices.len() > 64 {
            return Err(bad("member device limit exceeded"));
        }
        for d in &m.devices {
            if next
                .revoked_devices
                .contains(&(m.pseudonym_id.clone(), d.device_id.clone()))
            {
                return Err(forbidden("a revoked device cannot be readmitted"));
            }
        }
    }
    if next.members.len() > 10000 {
        return Err(bad("manifest member limit exceeded"));
    }
    next.head_seq = event.seq;
    next.head_hash = event.event_hash.clone();
    next.normalize();
    Ok(next)
}

pub fn authority_identity(
    conn: &Connection,
    state: &AppState,
    channel: &str,
) -> Result<String, E2eError> {
    let bound:Option<String>=conn.query_row("SELECT authority_identity FROM channel_authorities WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel],|r|r.get(0)).optional()?;
    Ok(bound.unwrap_or_else(|| device_certificates::server_identity(state)))
}
pub fn projection(
    conn: &Connection,
    state: &AppState,
    channel: &str,
) -> Result<Option<ManifestProjection>, E2eError> {
    let raw:Option<String>=conn.query_row("SELECT projection_json FROM channel_manifest_state WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel],|r|r.get(0)).optional()?;
    let parsed: Option<ManifestProjection> = raw
        .map(|s| {
            serde_json::from_str(&s)
                .map_err(|e| E2eError::Internal(format!("invalid stored manifest: {e}")))
        })
        .transpose()?;
    if parsed.as_ref().is_some_and(|p| p.channel_id != channel)
        || parsed.as_ref().is_some_and(|p| {
            authority_identity(conn, state, channel).map_or(true, |a| a != p.authority_identity)
        })
    {
        return Err(forbidden(
            "stored manifest context does not match its pinned channel authority",
        ));
    }
    Ok(parsed)
}
pub fn require_certified_device(
    conn: &Connection,
    state: &AppState,
    pid: &str,
    id: &str,
) -> Result<DeviceCertificate, E2eError> {
    e2e::require_device(conn, state.server_id, pid, id)?;
    let stored:Option<(String,String)>=conn.query_row("SELECT device_certificate,certificate_sha256 FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3 AND device_certificate IS NOT NULL AND certificate_sha256 IS NOT NULL AND revoked_at IS NULL",params![state.server_id,pid,id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (raw, digest) =
        stored.ok_or_else(|| forbidden("a verified device certificate is required"))?;
    let cert: DeviceCertificate = serde_json::from_str(&raw)
        .map_err(|_| forbidden("stored device certificate is malformed"))?;
    if cert.device_id != id
        || cert.pseudonym_id != pid
        || device_certificates::certificate_digest(&cert) != digest
    {
        return Err(forbidden("stored device certificate binding mismatch"));
    }
    let vkey = state
        .membership_vkey_v2
        .as_ref()
        .ok_or_else(|| forbidden("membership_v2 verifier unavailable"))?;
    device_certificates::verify_certificate(&cert, &cert.server_identity, pid, vkey)?;
    Ok(cert)
}
pub fn require_admitted_device(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    id: &str,
) -> Result<(), E2eError> {
    e2e::require_member(conn, state, channel, pid)?;
    let cert = require_certified_device(conn, state, pid, id)?;
    let p = projection(conn, state, channel)?
        .ok_or_else(|| forbidden("channel has no verified member manifest"))?;
    if p.channel_id != channel || p.authority_identity != authority_identity(conn, state, channel)?
    {
        return Err(forbidden(
            "stored manifest differs from the pinned channel authority",
        ));
    }
    if p.admitted(pid, id)
        .is_none_or(|d| d.certificate_sha256 != device_certificates::certificate_digest(&cert))
    {
        return Err(forbidden("device is not admitted by the channel manifest"));
    }
    Ok(())
}
/// Caller holds the authenticated writer transaction. No detached signature or
/// roster write can race a wrap commitment.
pub fn append(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    event: &SignedManifestEvent,
) -> Result<Value, E2eError> {
    if event.channel_id != channel
        || event.signer_pseudonym_id != pid
        || event.authority_identity != authority_identity(conn, state, channel)?
    {
        return Err(forbidden(
            "manifest context does not match channel and authenticated signer",
        ));
    }
    let cert = require_certified_device(conn, state, pid, &event.signer_device_id)?;
    if device_certificates::certificate_digest(&cert) != event.signer_certificate_sha256 {
        return Err(forbidden("manifest signer certificate mismatch"));
    }
    if let Some(cert) = operation_certificate(&event.operation) {
        let published = require_certified_device(conn, state, &cert.pseudonym_id, &cert.device_id)?;
        e2e::require_member(conn, state, channel, &cert.pseudonym_id)?;
        if device_certificates::certificate_digest(&published)
            != device_certificates::certificate_digest(cert)
        {
            return Err(forbidden("operation names an unpublished certificate"));
        }
    }
    let prior:Option<(String,Option<String>)>=conn.query_row("SELECT event_json,response_json FROM channel_manifest_events WHERE server_id=?1 AND channel_id=?2 AND event_seq=?3",params![state.server_id,channel,event.seq],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((raw, response)) = prior {
        let prior: SignedManifestEvent = serde_json::from_str(&raw)
            .map_err(|_| E2eError::Internal("invalid manifest event storage".into()))?;
        if prior.event_hash != event.event_hash || prior.signature_hex != event.signature_hex {
            return Err(conflict(
                "manifest sequence is occupied by a different signed event",
            ));
        }
        if let Some(response) = response {
            let mut response: Value = serde_json::from_str(&response)
                .map_err(|_| E2eError::Internal("invalid manifest receipt storage".into()))?;
            response["applied"] = json!(false);
            return Ok(response);
        }
        e2e::require_member(conn, state, channel, pid)?;
        return Ok(
            json!({"status":"ok","applied":false,"event":prior,"manifest":projection(conn,state,channel)?,"state":e2e::read_state(conn,state,channel)?}),
        );
    }
    // An exact signed receipt stays retryable after that very operation removes
    // its signer. New operations still require current ordinary membership.
    e2e::require_member(conn, state, channel, pid)?;
    let previous = projection(conn, state, channel)?;
    if previous.is_none() {
        let moderator:bool=conn.query_row("SELECT can_moderate FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2 AND active=1",params![state.server_id,pid],|r|r.get(0))?;
        if !moderator {
            return Err(forbidden(
                "only a channel moderator may establish the genesis trust anchor",
            ));
        }
    }
    let vkey = state
        .membership_vkey_v2
        .as_ref()
        .ok_or_else(|| forbidden("membership_v2 verifier unavailable"))?;
    let next = verify_transition(previous.as_ref(), event, vkey)?;
    persist(conn, state, channel, event, &next)?;
    if let Some(removed) = removed_member(event) {
        annex_channels::remove_member(conn, state.server_id, channel, removed)
            .map_err(|e| E2eError::Internal(e.to_string()))?;
        conn.execute("DELETE FROM graph_edges WHERE server_id=?1 AND from_node=?2 AND to_node=?3 AND kind IN('MEMBER_OF','MODERATES','AGENT_SERVING')",params![state.server_id,removed,annex_graph::channel_vertex(state.server_id,channel)])?;
    }
    if previous.is_none() || matches!(event.operation, Operation::SetEncryption { .. }) {
        conn.execute(
            "UPDATE channels SET e2e_enabled=?3 WHERE server_id=?1 AND channel_id=?2",
            params![state.server_id, channel, next.e2e_enabled],
        )?;
    }
    let rotate = previous.is_none()
        || matches!(
            event.operation,
            Operation::Leave
                | Operation::RemoveMember { .. }
                | Operation::RevokeDevice { .. }
                | Operation::RecoverDevice { .. }
        );
    conn.execute("UPDATE channel_key_state SET revision=revision+1,required_epoch=CASE WHEN ?3 THEN MAX(required_epoch,current_epoch+1) ELSE required_epoch END WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel,rotate])?;
    let response = json!({"status":"ok","applied":true,"event":event,"manifest":next,"state":e2e::read_state(conn,state,channel)?});
    conn.execute("UPDATE channel_manifest_events SET response_json=?4 WHERE server_id=?1 AND channel_id=?2 AND event_seq=?3",params![state.server_id,channel,event.seq,serde_json::to_string(&response).map_err(|e|E2eError::Internal(e.to_string()))?])?;
    Ok(response)
}
pub fn persist(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    event: &SignedManifestEvent,
    next: &ManifestProjection,
) -> Result<(), E2eError> {
    conn.execute("INSERT INTO channel_manifest_events(server_id,channel_id,event_seq,event_hash,event_json) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,channel,event.seq,event.event_hash,serde_json::to_string(event).map_err(|e|E2eError::Internal(e.to_string()))?])?;
    conn.execute("INSERT INTO channel_manifest_state(server_id,channel_id,authority_identity,head_seq,head_hash,genesis_hash,projection_json) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(server_id,channel_id) DO UPDATE SET head_seq=excluded.head_seq,head_hash=excluded.head_hash,projection_json=excluded.projection_json",params![state.server_id,channel,next.authority_identity,next.head_seq,next.head_hash,next.genesis_hash,serde_json::to_string(next).map_err(|e|E2eError::Internal(e.to_string()))?])?;
    Ok(())
}

pub fn removed_member(event: &SignedManifestEvent) -> Option<&str> {
    match &event.operation {
        Operation::Leave => Some(&event.signer_pseudonym_id),
        Operation::RemoveMember { pseudonym_id } => Some(pseudonym_id),
        _ => None,
    }
}
pub fn require_signed_departure(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
) -> Result<(), E2eError> {
    if projection(conn, state, channel)?
        .as_ref()
        .is_some_and(|p| p.members.iter().any(|m| m.pseudonym_id == pid))
    {
        return Err(forbidden(
            "leave the signed channel manifest before removing ordinary membership",
        ));
    }
    Ok(())
}
pub fn require_signed_device_departure(
    conn: &Connection,
    state: &AppState,
    pid: &str,
    device: &str,
) -> Result<(), E2eError> {
    let mut stmt =
        conn.prepare("SELECT channel_id FROM channel_manifest_state WHERE server_id=?1")?;
    let channels = stmt
        .query_map([state.server_id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for channel in channels {
        if projection(conn, state, &channel)?
            .as_ref()
            .is_some_and(|p| p.admitted(pid, device).is_some())
        {
            return Err(forbidden(
                "remove the device from every signed channel manifest before global revocation",
            ));
        }
    }
    Ok(())
}
pub async fn notify_departure(
    state: Arc<AppState>,
    channel: &str,
    event: &SignedManifestEvent,
) -> Result<(), E2eError> {
    let Some(pid) = removed_member(event) else {
        return Ok(());
    };
    let message=json!({"type":"channel_membership_changed","channelId":channel,"pseudonymId":pid,"member":false}).to_string();
    state
        .connection_manager
        .broadcast(channel, message.clone())
        .await;
    state.connection_manager.send(pid, message).await;
    state.connection_manager.unsubscribe(channel, pid).await;
    super::ChannelService::new(state)
        .end_channel_participation(pid, channel)
        .await;
    Ok(())
}
pub fn history(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    after: i64,
    limit: u32,
) -> Result<Value, E2eError> {
    e2e::require_member(conn, state, channel, pid)?;
    if !(0..=e2e::MAX_SAFE_INTEGER).contains(&after) || !(1..=200).contains(&limit) {
        return Err(bad("invalid manifest pagination"));
    }
    let p = projection(conn, state, channel)?;
    let mut stmt=conn.prepare("SELECT event_json FROM channel_manifest_events WHERE server_id=?1 AND channel_id=?2 AND event_seq>?3 ORDER BY event_seq LIMIT ?4")?;
    let rows = stmt
        .query_map(
            params![state.server_id, channel, after, limit as i64 + 1],
            |r| r.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut events: Vec<SignedManifestEvent> = rows
        .into_iter()
        .map(|s| {
            serde_json::from_str(&s)
                .map_err(|_| E2eError::Internal("invalid stored manifest event".into()))
        })
        .collect::<Result<_, _>>()?;
    let next = if events.len() > limit as usize {
        events.pop();
        events.last().map(|e| e.seq)
    } else {
        None
    };
    Ok(
        json!({"version":1,"authority_identity":authority_identity(conn,state,channel)?,"channel_id":channel,"head_seq":p.as_ref().map_or(0,|p|p.head_seq),"head_hash":p.as_ref().map_or("",|p|p.head_hash.as_str()),"genesis_hash":p.as_ref().map(|p|&p.genesis_hash),"events":events,"next_after_seq":next}),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationProof {
    pub manifest_seq: i64,
    pub manifest_hash: String,
    pub signature_hex: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WrapReceipt {
    pub version: u8,
    pub authority_identity: String,
    pub channel_id: String,
    pub sender_pseudonym_id: String,
    pub sender_device_id: String,
    pub sender_certificate_sha256: String,
    pub manifest_seq: i64,
    pub manifest_hash: String,
    pub operation: String,
    pub expected_revision: i64,
    pub epoch: i64,
    pub key_id: String,
    pub wrap_digests: Vec<(String, String, String, String)>,
    pub signature_hex: String,
    pub authorization_sha256: String,
}
pub fn wrap_signing_input(receipt: &WrapReceipt) -> Vec<u8> {
    canonical(&json!([
        "annex-channel-key-authorization-v1",
        receipt.authority_identity,
        receipt.channel_id,
        receipt.sender_pseudonym_id,
        receipt.sender_device_id,
        receipt.sender_certificate_sha256,
        receipt.manifest_seq,
        receipt.manifest_hash,
        receipt.operation,
        receipt.expected_revision,
        receipt.epoch,
        receipt.key_id,
        receipt.wrap_digests
    ]))
    .into_bytes()
}
pub fn verify_wrap_receipt(p: &ManifestProjection, r: &WrapReceipt) -> Result<(), E2eError> {
    if r.version != 1
        || r.authority_identity != p.authority_identity
        || r.channel_id != p.channel_id
        || r.manifest_seq != p.head_seq
        || r.manifest_hash != p.head_hash
        || !e2e::lower_hex(&r.key_id)
        || !(1..=e2e::MAX_SAFE_INTEGER).contains(&r.epoch)
        || !(0..=e2e::MAX_SAFE_INTEGER).contains(&r.expected_revision)
        || !matches!(
            r.operation.as_str(),
            "commit_epoch" | "admit_devices" | "admit_history_devices" | "admit_legacy_devices"
        )
        || r.wrap_digests.is_empty()
        || r.wrap_digests.len() > 10000
    {
        return Err(forbidden("key authorization context is invalid"));
    }
    let sender = p
        .admitted(&r.sender_pseudonym_id, &r.sender_device_id)
        .ok_or_else(|| forbidden("key signer is not admitted by the named manifest"))?;
    if sender.certificate_sha256 != r.sender_certificate_sha256 {
        return Err(forbidden("key signer certificate mismatch"));
    }
    let mut last: Option<(&str, &str)> = None;
    for (pid, id, cert, digest) in &r.wrap_digests {
        let tuple = (pid.as_str(), id.as_str());
        if last.is_some_and(|p| p >= tuple) {
            return Err(bad("wrap digests must be strictly sorted and unique"));
        }
        last = Some(tuple);
        if !e2e::lower_hex(digest)
            || p.admitted(pid, id)
                .is_none_or(|d| &d.certificate_sha256 != cert)
        {
            return Err(forbidden("key receipt includes an unauthorized recipient"));
        }
    }
    let input = wrap_signing_input(r);
    if hash(&input) != r.authorization_sha256 {
        return Err(forbidden("key authorization digest mismatch"));
    }
    verify_signature(
        &sender.certificate.ed25519_pub_hex,
        &r.signature_hex,
        &input,
    )
}
pub fn authorize_wraps(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    req: &e2e::PostWrapsRequest,
    proof: &AuthorizationProof,
) -> Result<WrapReceipt, E2eError> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    require_admitted_device(conn, state, channel, pid, &req.sender_device_id)?;
    let p = projection(conn, state, channel)?.ok_or_else(|| forbidden("manifest is missing"))?;
    if p.head_seq != proof.manifest_seq || p.head_hash != proof.manifest_hash {
        return Err(conflict("channel manifest changed"));
    }
    let sender = p
        .admitted(pid, &req.sender_device_id)
        .ok_or_else(|| forbidden("sender is not admitted"))?;
    let mut wrap_digests = Vec::with_capacity(req.wraps.len());
    let mut unique = BTreeSet::new();
    for w in &req.wraps {
        if !unique.insert((&w.recipient_pseudonym_id, &w.recipient_device_id)) {
            return Err(bad("duplicate key recipient"));
        }
        let recipient = p
            .admitted(&w.recipient_pseudonym_id, &w.recipient_device_id)
            .ok_or_else(|| forbidden("key recipient is not admitted by the manifest"))?;
        require_admitted_device(
            conn,
            state,
            channel,
            &w.recipient_pseudonym_id,
            &w.recipient_device_id,
        )?;
        let bytes = STANDARD
            .decode(&w.wrapped_key_b64)
            .map_err(|_| bad("invalid wrapped key base64"))?;
        if STANDARD.encode(&bytes) != w.wrapped_key_b64 {
            return Err(bad("noncanonical wrapped key base64"));
        }
        wrap_digests.push((
            w.recipient_pseudonym_id.clone(),
            w.recipient_device_id.clone(),
            recipient.certificate_sha256.clone(),
            hash(&bytes),
        ));
    }
    wrap_digests.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let mut r = WrapReceipt {
        version: 1,
        authority_identity: p.authority_identity.clone(),
        channel_id: channel.into(),
        sender_pseudonym_id: pid.into(),
        sender_device_id: req.sender_device_id.clone(),
        sender_certificate_sha256: sender.certificate_sha256.clone(),
        manifest_seq: proof.manifest_seq,
        manifest_hash: proof.manifest_hash.clone(),
        operation: req.operation.clone(),
        expected_revision: req.expected_revision,
        epoch: req.epoch,
        key_id: req.key_id.clone(),
        wrap_digests,
        signature_hex: proof.signature_hex.clone(),
        authorization_sha256: String::new(),
    };
    r.authorization_sha256 = hash(&wrap_signing_input(&r));
    verify_wrap_receipt(&p, &r)?;
    conn.execute("INSERT OR IGNORE INTO channel_key_authorizations(server_id,channel_id,authorization_sha256,receipt_json) VALUES(?1,?2,?3,?4)",params![state.server_id,channel,r.authorization_sha256,serde_json::to_string(&r).map_err(|e|E2eError::Internal(e.to_string()))?])?;
    Ok(r)
}
pub fn receipt(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    id: Option<&str>,
) -> Result<Option<WrapReceipt>, E2eError> {
    let Some(id) = id else { return Ok(None) };
    let raw:Option<String>=conn.query_row("SELECT receipt_json FROM channel_key_authorizations WHERE server_id=?1 AND channel_id=?2 AND authorization_sha256=?3",params![state.server_id,channel,id],|r|r.get(0)).optional()?;
    let raw =
        raw.ok_or_else(|| E2eError::Internal("key authorization receipt is missing".into()))?;
    let r: WrapReceipt = serde_json::from_str(&raw)
        .map_err(|_| E2eError::Internal("invalid key authorization receipt".into()))?;
    if r.authorization_sha256 != id || hash(&wrap_signing_input(&r)) != id {
        return Err(E2eError::Internal(
            "stored key authorization digest mismatch".into(),
        ));
    }
    Ok(Some(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (
        Value,
        annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>,
    ) {
        let f: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/signed-manifest-public.json"
        ))
        .unwrap();
        let c: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/device-certificate-public.json"
        ))
        .unwrap();
        (
            f,
            annex_identity::zk::parse_verification_key(&c["vkey"].to_string()).unwrap(),
        )
    }
    fn history(
        f: &Value,
        key: &annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>,
    ) -> Vec<ManifestProjection> {
        let mut p = Vec::new();
        for event in f["events"].as_array().unwrap() {
            let event: SignedManifestEvent = serde_json::from_value(event.clone()).unwrap();
            p.push(verify_transition(p.last(), &event, key).unwrap());
        }
        p
    }
    #[test]
    fn independently_generated_genuine_proofs_and_member_signatures_form_one_verified_lineage() {
        let (f, key) = fixture();
        let p = history(&f, &key);
        assert_eq!(p.len(), 9);
        assert_eq!(p[0].members.len(), 1);
        assert_eq!(p[1].members.len(), 2);
        assert_eq!(p[3].members.len(), 1);
        assert_eq!(p[4].members.len(), 2);
        assert_eq!(p[8].revoked_devices.len(), 1);
    }
    #[test]
    fn valid_device_signatures_cannot_grant_unauthorized_membership_roles_or_restore_revoked_devices(
    ) {
        let (f, key) = fixture();
        let p = history(&f, &key);
        for name in [
            "non_moderator_removal",
            "private_self_join",
            "revoked_device_recovery",
        ] {
            let e: SignedManifestEvent =
                serde_json::from_value(f["invalid_events"][name].clone()).unwrap();
            assert!(
                verify_transition(Some(&p[e.seq as usize - 2]), &e, &key).is_err(),
                "signed invalid operation {name}"
            );
        }
    }
    #[test]
    fn tampering_certificate_context_or_manifest_ancestry_fails_closed() {
        let (f, key) = fixture();
        let p = history(&f, &key);
        let e: SignedManifestEvent = serde_json::from_value(f["events"][1].clone()).unwrap();
        for field in [
            "channel_id",
            "authority_identity",
            "signer_device_id",
            "signer_certificate_sha256",
            "previous_hash",
            "event_hash",
        ] {
            let mut changed = serde_json::to_value(&e).unwrap();
            changed[field] = json!("0".repeat(64));
            let changed = serde_json::from_value(changed).unwrap();
            assert!(
                verify_transition(Some(&p[0]), &changed, &key).is_err(),
                "{field}"
            );
        }
        let mut changed = e.clone();
        if let Operation::Join { certificate } = &mut changed.operation {
            certificate.ed25519_pub_hex = "42".repeat(32);
        }
        changed.event_hash = hash(&signing_input(&changed));
        assert!(verify_transition(Some(&p[0]), &changed, &key).is_err());
    }
    #[test]
    fn real_sealed_wrap_receipts_verify_only_at_their_named_member_signed_head() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let (f, key) = fixture();
        let p = history(&f, &key);
        for bundle in f["authorizations"].as_array().unwrap() {
            let r: WrapReceipt = serde_json::from_value(bundle["receipt"].clone()).unwrap();
            let head = &p[r.manifest_seq as usize - 1];
            verify_wrap_receipt(head, &r).unwrap();
            for wrap in bundle["wraps"].as_array().unwrap() {
                let blob = STANDARD
                    .decode(wrap["wrapped_key_b64"].as_str().unwrap())
                    .unwrap();
                assert!(r
                    .wrap_digests
                    .iter()
                    .any(|t| t.0 == wrap["recipient_pseudonym_id"]
                        && t.1 == wrap["recipient_device_id"]
                        && t.3 == hash(&blob)));
            }
            assert!(verify_wrap_receipt(&p[8], &r).is_err());
            let mut changed = r.clone();
            changed.key_id = "42".repeat(32);
            changed.authorization_sha256 = hash(&wrap_signing_input(&changed));
            assert!(verify_wrap_receipt(head, &changed).is_err());
            let mut changed = r.clone();
            changed.wrap_digests[0].2 = "42".repeat(32);
            changed.authorization_sha256 = hash(&wrap_signing_input(&changed));
            assert!(verify_wrap_receipt(head, &changed).is_err());
        }
    }
}
