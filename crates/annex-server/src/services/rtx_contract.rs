//! Current RTX authorization and durable retention. All body mutation uses the
//! caller's transaction; only digest tombstones survive expiry.
use crate::AppState;
use annex_rtx::ReflectionSummaryBundle;
use annex_vrp::{NegotiatedRtxContract, VrpCapabilitySharingContract, VrpFederationHandshake};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Arc;

pub(crate) fn now_ms() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system clock precedes epoch".to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "system clock exceeds supported range".to_string())
}

pub(crate) fn policy_contract(state: &AppState) -> Result<VrpCapabilitySharingContract, String> {
    let policy = state
        .policy
        .read()
        .map_err(|_| "RTX policy lock poisoned")?;
    let contract = VrpCapabilitySharingContract::from_policy(&policy);
    contract.rtx_restrictions()?;
    Ok(contract)
}

pub(crate) fn agent_contract(
    conn: &Connection,
    state: &AppState,
    pseudonym: &str,
) -> Result<NegotiatedRtxContract, String> {
    let raw: String = conn
        .query_row(
            "SELECT capability_contract_json FROM agent_registrations
        WHERE server_id=?1 AND pseudonym_id=?2 AND active=1",
            params![state.server_id, pseudonym],
            |r| r.get(0),
        )
        .map_err(|_| "RTX participant has no active contract".to_string())?;
    let remote = serde_json::from_str(&raw)
        .map_err(|_| "RTX participant contract is invalid".to_string())?;
    annex_vrp::negotiate_rtx_contract(&policy_contract(state)?, &remote)
}

pub(crate) fn peer_contract(
    conn: &Connection,
    state: &AppState,
    peer: i64,
) -> Result<NegotiatedRtxContract, String> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT remote_handshake_json FROM federation_agreements
        WHERE local_server_id=?1 AND remote_instance_id=?2 AND active=1",
            params![state.server_id, peer],
            |r| r.get(0),
        )
        .map_err(|_| "RTX peer has no active contract".to_string())?;
    let remote = match raw {
        Some(raw) => {
            serde_json::from_str::<VrpFederationHandshake>(&raw)
                .map_err(|_| {
                    "RTX peer stored handshake is invalid; re-handshake required".to_string()
                })?
                .capability_contract
        }
        None => VrpCapabilitySharingContract::default(),
    };
    annex_vrp::negotiate_rtx_contract(&policy_contract(state)?, &remote)
}

pub(crate) fn require_agent_support(
    conn: &Connection,
    state: &AppState,
    pseudonym: &str,
    bundle: &ReflectionSummaryBundle,
    contract: &NegotiatedRtxContract,
) -> Result<(), String> {
    let raw: String = conn.query_row("SELECT capability_contract_json FROM agent_registrations WHERE server_id=?1 AND pseudonym_id=?2 AND active=1",
        params![state.server_id,pseudonym],|r|r.get(0)).map_err(|e|e.to_string())?;
    let declared: VrpCapabilitySharingContract =
        serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if declared.version < 2 && !contract.is_legacy_compatible() {
        return Err("RTX recipient must acknowledge restricted contract version 2".into());
    }
    annex_rtx::check_redacted_topics(bundle, &declared.redacted_topics).map_err(|e| e.to_string())
}

pub(crate) fn require_peer_support(
    conn: &Connection,
    state: &AppState,
    peer: i64,
    contract: &NegotiatedRtxContract,
) -> Result<(), String> {
    if contract.is_legacy_compatible() {
        return Ok(());
    }
    let raw: Option<String> = conn.query_row("SELECT remote_handshake_json FROM federation_agreements WHERE local_server_id=?1 AND remote_instance_id=?2 AND active=1",
        params![state.server_id,peer],|r|r.get(0)).map_err(|e|e.to_string())?;
    let handshake: VrpFederationHandshake =
        serde_json::from_str(&raw.ok_or("restricted RTX peer requires a new handshake")?)
            .map_err(|e| e.to_string())?;
    if handshake.capability_contract.version < 2 {
        return Err("RTX peer must acknowledge restricted contract version 2".into());
    }
    Ok(())
}

pub(crate) fn require_live(
    conn: &Connection,
    state: &AppState,
    bundle_id: &str,
    now: u64,
) -> Result<(), String> {
    let expired: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM rtx_retention_tombstones WHERE server_id=?1 AND bundle_id=?2)
        OR EXISTS(SELECT 1 FROM rtx_bundles WHERE server_id=?1 AND bundle_id=?2 AND expires_at_ms IS NOT NULL AND expires_at_ms<=?3)",
        params![state.server_id,bundle_id,now],|r|r.get(0)).map_err(|e|e.to_string())?;
    if expired {
        return Err(
            "RTX bundle has expired; retained replay protection forbids resurrection".into(),
        );
    }
    Ok(())
}

pub(crate) fn stored_contract(
    conn: &Connection,
    state: &AppState,
    bundle_id: &str,
) -> Result<NegotiatedRtxContract, String> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT contract_json FROM rtx_bundles WHERE server_id=?1 AND bundle_id=?2",
            params![state.server_id, bundle_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let value = match raw {
        Some(raw) => serde_json::from_str(&raw).map_err(|_| "stored RTX contract is invalid")?,
        None => NegotiatedRtxContract::default(),
    };
    value.validate()?;
    Ok(value)
}

/// After insert, in the same transaction as receipts/fanout/outbox. Intersecting
/// preserves an earlier shorter deadline even after policy is loosened.
pub(crate) fn retain(
    conn: &Connection,
    state: &AppState,
    bundle: &ReflectionSummaryBundle,
    incoming: &NegotiatedRtxContract,
) -> Result<(), String> {
    let now = now_ms()?;
    require_live(conn, state, &bundle.bundle_id, now)?;
    let contract = stored_contract(conn, state, &bundle.bundle_id)?.intersect(incoming);
    let expiry = annex_rtx::check_bundle_contract(bundle, &contract, now)?;
    let raw = serde_json::to_string(&contract).map_err(|e| e.to_string())?;
    conn.execute("UPDATE rtx_bundles SET contract_json=?3, expires_at_ms=CASE
        WHEN expires_at_ms IS NULL THEN ?4 WHEN ?4 IS NULL THEN expires_at_ms ELSE MIN(expires_at_ms,?4) END
        WHERE server_id=?1 AND bundle_id=?2",params![state.server_id,bundle.bundle_id,raw,expiry]).map_err(|e|e.to_string())?;
    Ok(())
}

/// Atomic erasure intentionally preserves no content-bearing governance or
/// origin-attestation JSON. Receipt digests and a tombstone can reject replay
/// without retaining the reflection itself.
pub(crate) fn erase_expired(conn: &Connection, server_id: i64, now: u64) -> Result<usize, String> {
    let ids: Vec<(String, String)> = {
        let mut stmt=conn.prepare("SELECT bundle_id, signature FROM rtx_bundles
            WHERE server_id=?1 AND expires_at_ms IS NOT NULL AND expires_at_ms<=?2 ORDER BY expires_at_ms LIMIT 500").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map(params![server_id, now], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    for (id, signature) in &ids {
        use sha2::{Digest, Sha256};
        let digest = hex::encode(Sha256::digest(signature.as_bytes()));
        conn.execute(
            "INSERT INTO rtx_retention_tombstones(server_id,bundle_id,content_digest,expired_at_ms)
            VALUES(?1,?2,?3,?4) ON CONFLICT(server_id,bundle_id) DO NOTHING",
            params![server_id, id, digest, now],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM federation_outbox WHERE envelope_kind='rtx' AND message_id=?1",
            [format!("rtx:{id}")],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM rtx_transfer_log WHERE server_id=?1 AND bundle_id=?2",
            params![server_id, id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("UPDATE rtx_federation_receipts SET origin_attestation_json=NULL WHERE server_id=?1 AND bundle_id=?2",params![server_id,id]).map_err(|e|e.to_string())?;
        conn.execute(
            "DELETE FROM rtx_bundles WHERE server_id=?1 AND bundle_id=?2",
            params![server_id, id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(ids.len())
}

pub(crate) fn sweep(state: &AppState) -> Result<usize, String> {
    let now = now_ms()?;
    let current = policy_contract(state)?.rtx_restrictions()?;
    let mut conn = state.pool.get().map_err(|e| e.to_string())?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if let Some(seconds) = current.retention_seconds {
        // Signed creation times are already bounded before admission. MIN keeps
        // retained history from gaining time when an operator loosens policy.
        tx.execute(
            "UPDATE rtx_bundles SET contract_json=json_set(contract_json,'$.retention_seconds',
            CASE WHEN json_extract(contract_json,'$.retention_seconds') IS NULL THEN ?3
            ELSE MIN(json_extract(contract_json,'$.retention_seconds'),?3) END), expires_at_ms=CASE WHEN expires_at_ms IS NULL
            THEN created_at_ms+?2 ELSE MIN(expires_at_ms,created_at_ms+?2) END
            WHERE server_id=?1",
            params![state.server_id, seconds * 1000, seconds],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.execute("UPDATE federation_outbox SET rtx_expires_at_ms=CASE WHEN rtx_expires_at_ms IS NULL
        THEN (SELECT expires_at_ms FROM rtx_bundles b WHERE b.server_id=?1 AND 'rtx:'||b.bundle_id=message_id)
        ELSE MIN(rtx_expires_at_ms,(SELECT expires_at_ms FROM rtx_bundles b WHERE b.server_id=?1 AND 'rtx:'||b.bundle_id=message_id)) END
        WHERE envelope_kind='rtx' AND EXISTS(SELECT 1 FROM rtx_bundles b WHERE b.server_id=?1 AND 'rtx:'||b.bundle_id=message_id AND b.expires_at_ms IS NOT NULL)",
        [state.server_id]).map_err(|e|e.to_string())?;
    tx.execute("DELETE FROM federation_outbox WHERE envelope_kind='rtx' AND rtx_expires_at_ms IS NOT NULL AND rtx_expires_at_ms<=?1",[now]).map_err(|e|e.to_string())?;
    let count = erase_expired(&tx, state.server_id, now)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(count)
}

pub(crate) async fn start_retention_task(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        tokio::select! {
            _=state.shutdown.cancelled()=>break,
            _=interval.tick()=>{
                let state=state.clone();
                match tokio::task::spawn_blocking(move||sweep(&state)).await {
                    Ok(Ok(_))=>{}, Ok(Err(error))=>tracing::error!(%error,"RTX retention sweep failed"),
                    Err(error)=>tracing::error!(%error,"RTX retention task failed"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seed(conn: &Connection) {
        annex_db::run_migrations(conn).unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'retention','Retention','{}')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO instances(id,base_url,public_key,label) VALUES(1,'https://peer.example','aa','Peer')",[]).unwrap();
        conn.execute("INSERT INTO rtx_bundles(server_id,bundle_id,source_pseudonym,source_server,domain_tags_json,summary,reasoning_chain,caveats_json,created_at_ms,signature,vrp_handshake_ref,provenance_json,expires_at_ms)
            VALUES(1,'expired','agent','https://origin.example','[\"private-domain\"]','private reflection','private reasoning','[\"private caveat\"]',1000,'signature','vrp:1:1','private provenance',2000)",[]).unwrap();
        conn.execute("INSERT INTO rtx_transfer_log(server_id,bundle_id,source_pseudonym,transfer_scope_applied,redactions_applied) VALUES(1,'expired','agent','FULL_KNOWLEDGE_BUNDLE','private annotation')",[]).unwrap();
        conn.execute("INSERT INTO rtx_federation_receipts(server_id,bundle_id,relaying_server,envelope_digest,origin_content_digest,origin_attestation_json)
            VALUES(1,'expired','https://peer.example','authenticated-envelope-digest','authenticated-origin-digest','private-domain-attestation')",[]).unwrap();
        conn.execute("INSERT INTO federation_outbox(peer_instance_id,message_id,envelope_json,envelope_kind,status,rtx_expires_at_ms)
            VALUES(1,'rtx:expired','encrypted-private-payload','rtx','failed',2000)",[]).unwrap();
        conn.execute("INSERT INTO federation_outbox(peer_instance_id,message_id,envelope_json,envelope_kind,status)
            VALUES(1,'unrelated-message','ordinary-body','message','pending')",[]).unwrap();
    }
    #[test]
    fn retention_erases_all_content_atomically_and_replay_state_survives_restart() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("retention.sqlite");
        let mut conn = Connection::open(&path).unwrap();
        seed(&conn);
        {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            assert_eq!(erase_expired(&tx, 1, 1999).unwrap(), 0);
            assert_eq!(erase_expired(&tx, 1, 2000).unwrap(), 1);
            tx.rollback().unwrap();
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM rtx_bundles", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            assert_eq!(erase_expired(&tx, 1, 2000).unwrap(), 1);
            tx.commit().unwrap();
        }
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        for table in ["rtx_bundles", "rtx_transfer_log"] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM federation_outbox WHERE envelope_kind='rtx'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM federation_outbox WHERE envelope_kind='message'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM rtx_retention_tombstones WHERE bundle_id='expired'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let receipt: (String, Option<String>) = conn
            .query_row(
                "SELECT envelope_digest,origin_attestation_json FROM rtx_federation_receipts",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(receipt, ("authenticated-envelope-digest".into(), None));
        assert_eq!(erase_expired(&conn, 1, 3000).unwrap(), 0);
    }
    #[test]
    fn retention_failure_does_not_partially_delete_bodies_or_queue() {
        let mut conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        conn.execute_batch("CREATE TRIGGER refuse_rtx_delete BEFORE DELETE ON rtx_bundles BEGIN SELECT RAISE(ABORT,'controlled storage fault'); END;").unwrap();
        {
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .unwrap();
            assert!(erase_expired(&tx, 1, 2000).is_err());
        }
        for table in ["rtx_bundles", "rtx_transfer_log"] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM federation_outbox WHERE envelope_kind='rtx'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM rtx_retention_tombstones", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
    }
}
