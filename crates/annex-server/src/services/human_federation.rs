//! Origin-authenticated human membership. This protocol does not reinterpret a
//! human as an agent, grant channel admission, or issue a local bearer token.
use super::{channel_authority as authority, channel_manifest, federation_repository};
use crate::{
    device_certificates::{self, DeviceCertificate},
    AppState,
};
use authority::AuthorityError as Error;
use ed25519_dalek::Signer;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

const LIFETIME: i64 = 120;

/// A general federated identity row is not a current human membership proof.
/// Human operations, history and attachment permits share this exact check;
/// other participant types retain their separately verified attestation path.
pub(crate) fn current_remote_membership(
    conn: &Connection,
    state: &AppState,
    peer: i64,
    channel: &str,
    pseudonym: &str,
    device: Option<&str>,
    now: i64,
) -> Result<bool, Error> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM platform_identities p JOIN federated_identities f ON f.server_id=p.server_id AND f.pseudonym_id=p.pseudonym_id WHERE p.server_id=?1 AND f.remote_instance_id=?2 AND p.pseudonym_id=?4 AND p.active=1 AND (p.participant_type<>'HUMAN' OR EXISTS(SELECT 1 FROM federated_human_memberships h JOIN member_device_keys d ON d.server_id=h.server_id AND d.pseudonym_id=h.pseudonym_id AND d.device_id=h.device_id WHERE h.server_id=p.server_id AND h.peer_instance_id=f.remote_instance_id AND h.channel_id=?3 AND h.pseudonym_id=p.pseudonym_id AND h.device_id=?5 AND h.expires_at>?6 AND d.revoked_at IS NULL AND d.certificate_sha256=h.certificate_sha256)))",
        params![state.server_id,peer,channel,pseudonym,device,now], |r| r.get(0),
    )?)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanMembership {
    pub version: u8,
    pub participant_type: String,
    pub originating_server: String,
    pub originating_identity: String,
    pub authority_identity: String,
    pub channel_id: String,
    pub certificate: DeviceCertificate,
    pub token_epoch: i64,
    pub nonce: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub signature: String,
}
impl HumanMembership {
    pub fn signing_value(&self) -> Value {
        json!([
            "annex-human-membership-v1",
            self.version,
            self.participant_type,
            self.originating_server,
            self.originating_identity,
            self.authority_identity,
            self.channel_id,
            self.certificate,
            self.token_epoch,
            self.nonce,
            self.issued_at,
            self.expires_at
        ])
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MembershipReceipt {
    pub version: u8,
    pub authority_identity: String,
    pub request_hash: String,
    pub expires_at: i64,
    pub signature: String,
}
impl MembershipReceipt {
    fn signing_value(&self) -> Value {
        json!([
            "annex-human-membership-receipt-v1",
            self.version,
            self.authority_identity,
            self.request_hash,
            self.expires_at
        ])
    }
    pub fn verify(&self, request: &HumanMembership, key: &str) -> Result<(), Error> {
        if self.version != 1
            || self.authority_identity != request.authority_identity
            || self.request_hash != authority::digest(&request.signing_value())
            || self.expires_at != request.expires_at
            || self.expires_at <= chrono::Utc::now().timestamp()
            || authority::server_identity(key)? != self.authority_identity
        {
            return Err(Error::Forbidden(
                "human membership receipt context mismatch".into(),
            ));
        }
        authority::verify(key, &self.signature, &self.signing_value())
    }
}

pub fn current_local_identity(
    conn: &Connection,
    state: &AppState,
    pid: &str,
    epoch: i64,
) -> Result<annex_identity::PlatformIdentity, Error> {
    let identity = annex_identity::get_platform_identity(conn, state.server_id, pid)
        .map_err(|_| Error::Forbidden("local membership is unavailable".into()))?;
    if !identity.active || identity.token_epoch != epoch {
        return Err(Error::Forbidden("local session was revoked".into()));
    }
    Ok(identity)
}

/// Construct only after checking the actual local registration, current session
/// epoch, published certificate and policy in the caller's database snapshot.
pub fn build(
    conn: &Connection,
    state: &AppState,
    pid: &str,
    epoch: i64,
    device: &str,
    channel: &str,
    authority_identity: &str,
) -> Result<HumanMembership, Error> {
    let identity = current_local_identity(conn, state, pid, epoch)?;
    if identity.participant_type != annex_types::RoleCode::Human {
        return Err(Error::Forbidden(
            "human attestation requires a human identity".into(),
        ));
    }
    let certificate = channel_manifest::require_certified_device(conn, state, pid, device)?;
    device_certificates::validate_publication(conn, state, pid, &certificate)?;
    let now = chrono::Utc::now().timestamp();
    let mut request = HumanMembership {
        version: 1,
        participant_type: "HUMAN".into(),
        originating_server: state.get_public_url().trim_end_matches('/').into(),
        originating_identity: device_certificates::server_identity(state),
        authority_identity: authority_identity.into(),
        channel_id: channel.into(),
        certificate,
        token_epoch: epoch,
        nonce: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        issued_at: now,
        expires_at: now + LIFETIME,
        signature: String::new(),
    };
    request.signature = hex::encode(
        state
            .signing_key
            .sign(authority::canonical_json(&request.signing_value()).as_bytes())
            .to_bytes(),
    );
    Ok(request)
}

pub fn accept(
    tx: &Transaction<'_>,
    state: &AppState,
    request: &HumanMembership,
) -> Result<MembershipReceipt, Error> {
    let now = chrono::Utc::now().timestamp();
    if request.version != 1
        || request.participant_type != "HUMAN"
        || request.channel_id.is_empty()
        || request.channel_id.len() > 128
        || request.channel_id.chars().any(char::is_control)
        || !super::e2e_service::lower_hex(&request.nonce)
        || request.issued_at > now + 30
        || request.issued_at < now - LIFETIME
        || request.expires_at <= now
        || request.expires_at - request.issued_at != LIFETIME
        || request.token_epoch < 0
    {
        return Err(Error::BadRequest(
            "invalid or expired human membership attestation".into(),
        ));
    }
    let policy: String = tx.query_row(
        "SELECT policy_json FROM servers WHERE id=?1",
        [state.server_id],
        |r| r.get(0),
    )?;
    let policy: annex_types::ServerPolicy =
        serde_json::from_str(&policy).map_err(|e| Error::Internal(e.to_string()))?;
    if !policy.federation_enabled {
        return Err(Error::Forbidden("federation is disabled".into()));
    }
    let bound = authority::binding(tx, state, &request.channel_id)?;
    if !bound.local_authority || bound.authority_identity != request.authority_identity {
        return Err(Error::Forbidden(
            "human attestation targets another or deleted authority".into(),
        ));
    }
    let peer:Option<(i64,String)>=tx.query_row("SELECT i.id,i.public_key FROM instances i JOIN federation_agreements a ON a.remote_instance_id=i.id WHERE i.base_url=?1 AND i.status='ACTIVE' AND a.local_server_id=?2 AND a.active=1 AND a.transfer_scope<>'NO_TRANSFER'",
        params![request.originating_server,state.server_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let (peer, key) = peer.ok_or_else(|| {
        Error::Forbidden("human origin has no current federation agreement".into())
    })?;
    if authority::server_identity(&key)? != request.originating_identity {
        return Err(Error::Forbidden(
            "human origin differs from pinned peer".into(),
        ));
    }
    authority::verify(&key, &request.signature, &request.signing_value())?;
    if bound.deleted_seq.is_some() {
        let prior_replica:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_authority_replicas WHERE server_id=?1 AND channel_id=?2 AND peer_instance_id=?3 AND revoked_at IS NULL)",params![state.server_id,request.channel_id,peer],|r|r.get(0))?;
        if !prior_replica {
            return Err(Error::Forbidden(
                "deleted authority only admits cleanup for a previously admitted replica".into(),
            ));
        }
    }
    let cert = &request.certificate;
    let verifier = state
        .membership_vkey_v2
        .as_ref()
        .ok_or_else(|| Error::Forbidden("membership verifier is unavailable".into()))?;
    let certificate_hash = device_certificates::verify_certificate(
        cert,
        &request.originating_identity,
        &cert.pseudonym_id,
        verifier,
    )?;
    let local: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM zk_nullifiers WHERE pseudonym_id=?1)",
        [&cert.pseudonym_id],
        |r| r.get(0),
    )?;
    let other:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM federated_identities WHERE server_id=?1 AND pseudonym_id=?2 AND (remote_instance_id<>?3 OR commitment_hex<>?4 OR vrp_topic<>?5))",
        params![state.server_id,cert.pseudonym_id,peer,cert.commitment_hex,cert.topic],|r|r.get(0))?;
    if local || other {
        return Err(Error::Forbidden(
            "human attestation would replace an existing identity binding".into(),
        ));
    }
    let prior:Option<(String,bool)>=tx.query_row("SELECT participant_type,active FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2",params![state.server_id,cert.pseudonym_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if prior
        .as_ref()
        .is_some_and(|(role, active)| role != "HUMAN" || !*active)
    {
        return Err(Error::Forbidden(
            "human identity is locally deactivated or has another type".into(),
        ));
    }
    let previously_bound:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM federated_identities WHERE server_id=?1 AND remote_instance_id=?2 AND pseudonym_id=?3)",params![state.server_id,peer,cert.pseudonym_id],|r|r.get(0))?;
    if prior.is_some() && !previously_bound {
        return Err(Error::Forbidden(
            "human attestation cannot claim an existing unbound local identity".into(),
        ));
    }
    let prior_epoch:Option<i64>=tx.query_row("SELECT MAX(origin_token_epoch) FROM federated_human_memberships WHERE server_id=?1 AND peer_instance_id=?2 AND pseudonym_id=?3",params![state.server_id,peer,cert.pseudonym_id],|r|r.get(0))?;
    if prior_epoch.is_some_and(|epoch| epoch > request.token_epoch) {
        return Err(Error::Forbidden(
            "human attestation predates a known origin session revocation".into(),
        ));
    }
    tx.execute(
        "DELETE FROM channel_authority_nonces WHERE expires_at<?1",
        [now],
    )?;
    let seen:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_authority_nonces WHERE server_id=?1 AND requesting_identity=?2 AND nonce=?3)",params![state.server_id,request.originating_identity,request.nonce],|r|r.get(0))?;
    if seen {
        return Err(Error::Forbidden(
            "human attestation nonce was already used".into(),
        ));
    }
    let request_hash = authority::digest(&request.signing_value());
    tx.execute("INSERT INTO channel_authority_nonces(server_id,requesting_identity,nonce,request_hash,expires_at) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,request.originating_identity,request.nonce,request_hash,request.expires_at])?;
    federation_repository::upsert_federated_identity(
        tx,
        state.server_id,
        peer,
        &cert.commitment_hex,
        &cert.pseudonym_id,
        &cert.topic,
        &cert.root_hex,
    )?;
    // Never overwrite role/capabilities or reactivate a locally banned person.
    tx.execute("INSERT OR IGNORE INTO platform_identities(server_id,pseudonym_id,participant_type,active) VALUES(?1,?2,'HUMAN',1)",params![state.server_id,cert.pseudonym_id])?;
    authority::import_verified_certificate(tx, state, cert, None)?;
    tx.execute("UPDATE federated_human_memberships SET expires_at=0 WHERE server_id=?1 AND peer_instance_id=?2 AND pseudonym_id=?3 AND origin_token_epoch<?4",params![state.server_id,peer,cert.pseudonym_id,request.token_epoch])?;
    tx.execute("INSERT INTO federated_human_memberships(server_id,peer_instance_id,channel_id,pseudonym_id,device_id,certificate_sha256,origin_token_epoch,expires_at,request_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(server_id,peer_instance_id,channel_id,pseudonym_id,device_id) DO UPDATE SET expires_at=excluded.expires_at,origin_token_epoch=excluded.origin_token_epoch,request_hash=excluded.request_hash,certificate_sha256=excluded.certificate_sha256",
        params![state.server_id,peer,request.channel_id,cert.pseudonym_id,cert.device_id,certificate_hash,request.token_epoch,request.expires_at,request_hash])?;
    let mut receipt = MembershipReceipt {
        version: 1,
        authority_identity: bound.authority_identity,
        request_hash,
        expires_at: request.expires_at,
        signature: String::new(),
    };
    receipt.signature = hex::encode(
        state
            .signing_key
            .sign(authority::canonical_json(&receipt.signing_value()).as_bytes())
            .to_bytes(),
    );
    Ok(receipt)
}

pub async fn receive(
    state: Arc<AppState>,
    request: HumanMembership,
) -> Result<MembershipReceipt, Error> {
    tokio::task::spawn_blocking(move || {
        let mut conn = state
            .pool
            .get()
            .map_err(|e| Error::Internal(e.to_string()))?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let receipt = accept(&tx, &state, &request)?;
        tx.commit()?;
        Ok(receipt)
    })
    .await
    .map_err(|e| Error::Internal(e.to_string()))?
}
pub async fn http_attest(
    axum::Extension(state): axum::Extension<Arc<AppState>>,
    axum::Json(request): axum::Json<HumanMembership>,
) -> Result<axum::Json<MembershipReceipt>, Error> {
    receive(state, request).await.map(axum::Json)
}
