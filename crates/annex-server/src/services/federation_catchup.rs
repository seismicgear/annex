//! Authenticated, restart-safe recovery of the durable federation outbox.
//!
//! Every batch signs its request nonce, peer, channel, sequence numbers and full
//! payload. Existing envelopes retain their own signatures and admission checks.
//! A tombstone replaces a deleted body; retention never becomes a history source.

use std::sync::Arc;

use ed25519_dalek::Signer;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::federation_repository as repo;
use super::federation_service::{
    check_freshness, message_signing_input, outbound_message_timestamp, verify_ed25519,
    DeliveryMode, FederationError, FederationService,
};
use crate::{api_ws::OutgoingMessage, AppState};

const MAX_BATCH: u32 = 64;
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatchupRequest {
    pub requesting_server: String,
    pub channel_id: String,
    pub after_seq: i64,
    pub max_envelopes: u32,
    pub nonce: String,
    pub created_at: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatchupEntry {
    pub seq: i64,
    pub message_id: String,
    /// Present only while the original message remains available.
    pub envelope: Option<serde_json::Value>,
    /// `deleted` or `retention`; mutually exclusive with `envelope`.
    pub removed_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CatchupResponse {
    pub originating_server: String,
    pub requesting_server: String,
    pub channel_id: String,
    pub nonce: String,
    pub after_seq: i64,
    pub next_after_seq: i64,
    pub status: String,
    pub entries: Vec<CatchupEntry>,
    pub signature: String,
}

pub fn request_signing_input(request: &CatchupRequest) -> Vec<u8> {
    serde_json::to_vec(&(
        "annex-catchup-request-v1",
        &request.requesting_server,
        &request.channel_id,
        request.after_seq,
        request.max_envelopes,
        &request.nonce,
        &request.created_at,
    ))
    .expect("serializing a fixed catch-up request cannot fail")
}

pub fn response_signing_input(response: &CatchupResponse) -> Vec<u8> {
    serde_json::to_vec(&(
        "annex-catchup-response-v1",
        &response.originating_server,
        &response.requesting_server,
        &response.channel_id,
        &response.nonce,
        response.after_seq,
        response.next_after_seq,
        &response.status,
        &response.entries,
    ))
    .expect("serializing an already parsed catch-up response cannot fail")
}

fn internal(error: impl std::fmt::Display) -> FederationError {
    FederationError::RemoteServer(error.to_string())
}

fn allowed_peer(
    conn: &rusqlite::Connection,
    state: &AppState,
    peer_url: &str,
) -> Result<repo::RemoteInstance, FederationError> {
    let peer = repo::find_instance_by_base_url(conn, peer_url)?
        .ok_or_else(|| FederationError::UnknownRemote(peer_url.to_owned()))?;
    let scope = repo::active_agreement_transfer_scope(conn, state.server_id, peer.id)?;
    if peer.status != "ACTIVE" || scope.is_none() || scope.as_deref() == Some("NO_TRANSFER") {
        return Err(FederationError::Forbidden(
            "federation agreement is not active for transfer".into(),
        ));
    }
    Ok(peer)
}

fn require_channel(
    conn: &rusqlite::Connection,
    state: &AppState,
    channel: &str,
) -> Result<(), FederationError> {
    let row = annex_channels::get_channel(conn, channel)?;
    let allowed = row.server_id == state.server_id
        && matches!(
            row.federation_scope,
            annex_types::FederationScope::Federated
        );
    if !allowed {
        return Err(FederationError::Forbidden(
            "channel is not an active federated channel".into(),
        ));
    }
    Ok(())
}

/// Backfill metadata and repair locally signed pre-fix SQLite timestamps before
/// workers start. Batch reads bound memory even for a large retained outbox.
/// The original instant is preserved; repair never resets freshness or retries.
pub fn backfill_outbox(state: &AppState) -> Result<(), FederationError> {
    let mut conn = state.pool.get().map_err(internal)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut after_id = 0;
    loop {
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, envelope_json, source_message_id IS NULL FROM federation_outbox WHERE id > ?1 AND envelope_kind <> 'rtx' AND envelope_json <> '' AND (source_message_id IS NULL OR source_message_id = message_id) ORDER BY id LIMIT 256",
            )?;
            let rows = stmt.query_map([after_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if rows.is_empty() {
            break;
        }
        for (id, stored, needs_metadata) in rows {
            after_id = id;
            let value: serde_json::Value =
                serde_json::from_str(&state.message_cipher().decrypt(&stored)?)?;
            let message_id = value
                .get("message_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| internal("legacy outbox envelope missing message_id"))?
                .to_owned();
            let channel = value
                .get("channel_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| internal("legacy outbox envelope missing channel_id"))?
                .to_owned();
            let reason = removal_reason(&tx, &message_id)?;
            let mut stored = stored;
            let mut repaired = false;
            // Only a verified locally authored message may be re-signed. Editing
            // a signature's input without checking it would bless tampered data.
            if reason.is_none()
                && value.get("envelope_kind").is_none()
                && value
                    .get("created_at")
                    .and_then(|v| v.as_str())
                    .is_some_and(|v| chrono::DateTime::parse_from_rfc3339(v).is_err())
            {
                let mut envelope: annex_federation::FederatedMessageEnvelope =
                    serde_json::from_value(value.clone())?;
                verify_ed25519(
                    &hex::encode(state.signing_key.verifying_key().to_bytes()),
                    &envelope.signature,
                    message_signing_input(&envelope).as_bytes(),
                )?;
                envelope.created_at =
                    outbound_message_timestamp(&envelope.created_at).map_err(internal)?;
                envelope.signature = hex::encode(
                    state
                        .signing_key
                        .sign(message_signing_input(&envelope).as_bytes())
                        .to_bytes(),
                );
                stored = state
                    .message_cipher()
                    .encrypt(&serde_json::to_string(&envelope)?)?;
                repaired = true;
            }
            if !needs_metadata && !repaired && reason.is_none() {
                continue;
            }
            tx.execute("UPDATE federation_outbox SET source_message_id = ?2, channel_id = ?3, removed_reason = ?4, envelope_json = CASE WHEN ?4 IS NULL THEN ?5 ELSE '' END, status = CASE WHEN ?4 IS NULL THEN status ELSE 'delivered' END WHERE id = ?1", params![id, message_id, channel, reason, stored])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn removal_reason(
    conn: &rusqlite::Connection,
    message: &str,
) -> rusqlite::Result<Option<&'static str>> {
    let source: Option<(Option<String>, bool)> = conn
        .query_row(
            "SELECT deleted_at,expires_at IS NOT NULL AND (datetime(expires_at) IS NULL OR datetime(expires_at)<=datetime('now')) FROM messages WHERE message_id = ?1",
            [message],
            |r| Ok((r.get(0)?,r.get(1)?)),
        )
        .optional()?;
    Ok(match source {
        None => Some("retention"),
        Some((Some(_), _)) => Some("deleted"),
        Some((None, true)) => Some("retention"),
        Some((None, false)) => None,
    })
}

/// Recheck retention at the point of delivery, independently of the periodic
/// physical purge. A queued body cannot outlive its source's read entitlement.
pub(crate) fn live_outbox_body(
    conn: &mut rusqlite::Connection,
    id: i64,
) -> rusqlite::Result<Option<String>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("UPDATE federation_outbox SET envelope_json='',removed_reason='retention',status='delivered',updated_at=datetime('now') WHERE id=?1 AND envelope_kind<>'rtx' AND source_message_id IN (SELECT message_id FROM messages WHERE expires_at IS NOT NULL AND (datetime(expires_at) IS NULL OR datetime(expires_at)<=datetime('now')))",[id])?;
    let body=tx.query_row("SELECT envelope_json FROM federation_outbox WHERE id=?1 AND status='pending' AND (removed_reason IS NULL OR message_id LIKE 'redaction:%')",[id],|r|r.get(0)).optional()?;
    tx.commit()?;
    Ok(body)
}

/// Insert payload and cleartext routing metadata atomically. A late edit or
/// redaction enqueue after deletion can never put the removed body back.
pub fn enqueue_outbox(
    conn: &rusqlite::Connection,
    peer_id: i64,
    ledger_key: &str,
    message_id: &str,
    channel_id: &str,
    stored_envelope: &str,
) -> rusqlite::Result<usize> {
    let reason = removal_reason(conn, message_id)?;
    // Redaction envelopes contain no message body and remain eligible for the
    // live worker; catch-up uses their deletion marker instead of replaying them.
    let scrub = reason.is_some() && !ledger_key.starts_with("redaction:");
    conn.execute(
        "INSERT OR IGNORE INTO federation_outbox (peer_instance_id, message_id, source_message_id, channel_id, envelope_json, removed_reason, status, attempts, next_retry_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, datetime('now'))",
        params![peer_id, ledger_key, message_id, channel_id, if scrub { "" } else { stored_envelope }, reason, if scrub { "delivered" } else { "pending" }],
    )
}

pub async fn serve(
    state: Arc<AppState>,
    request: CatchupRequest,
) -> Result<CatchupResponse, FederationError> {
    if request.after_seq < 0
        || !(1..=MAX_BATCH).contains(&request.max_envelopes)
        || request.nonce.len() > 128
        || request.nonce.is_empty()
    {
        return Err(FederationError::Forbidden(
            "invalid catch-up request bounds".into(),
        ));
    }
    check_freshness(
        &request.created_at,
        chrono::Utc::now(),
        120,
        30,
        DeliveryMode::Live,
    )
    .map_err(|_| FederationError::Forbidden("catch-up request timestamp is not fresh".into()))?;
    tokio::task::spawn_blocking(move || {
        let conn = state.pool.get().map_err(internal)?;
        // One read transaction prevents deletion from racing payload selection.
        let tx = annex_db::read_transaction(&conn)?;
        let peer = allowed_peer(&tx, &state, &request.requesting_server)?;
        verify_ed25519(&peer.public_key_hex, &request.signature, &request_signing_input(&request))?;
        super::federation_service::require_legacy_channel(&tx,state.server_id,&request.channel_id,None)?;
        let channel_exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM channels WHERE channel_id = ?1 AND server_id = ?2)", params![request.channel_id, state.server_id], |r| r.get(0))?;
        if channel_exists {
            require_channel(&tx, &state, &request.channel_id)?;
            let member: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM channel_members cm JOIN federated_identities fi ON fi.pseudonym_id = cm.pseudonym_id AND fi.server_id = cm.server_id JOIN platform_identities pi ON pi.pseudonym_id = cm.pseudonym_id AND pi.server_id = cm.server_id WHERE cm.channel_id = ?1 AND cm.server_id = ?2 AND fi.remote_instance_id = ?3 AND pi.active = 1)",
            params![request.channel_id, state.server_id, peer.id], |r| r.get(0),
            )?;
            if !member { return Err(FederationError::Forbidden("requesting peer has no active member of this channel".into())); }
        } else {
            // Channel deletion removes its membership rows. The only remaining
            // authority is this peer's already addressed outbox; it may recover
            // deletion markers, never any former channel content.
            let addressed: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM federation_outbox WHERE peer_instance_id = ?1 AND channel_id = ?2)", params![peer.id, request.channel_id], |r| r.get(0))?;
            if !addressed { return Err(FederationError::Forbidden("unknown catch-up channel".into())); }
        }
        let mut stmt = tx.prepare("SELECT id, source_message_id, envelope_json, removed_reason FROM federation_outbox WHERE peer_instance_id = ?1 AND channel_id = ?2 AND id > ?3 ORDER BY id LIMIT ?4")?;
        let rows = stmt.query_map(params![peer.id, request.channel_id, request.after_seq, request.max_envelopes], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))?;
        let mut entries = Vec::new();
        for row in rows {
            let (seq, message_id, stored, recorded_reason) = row?;
            // Defence for a pre-migration row or an operator-restored database.
            let reason = recorded_reason.or(removal_reason(&tx, &message_id)?.map(str::to_owned));
            let envelope = if reason.is_none() { Some(serde_json::from_str(&state.message_cipher().decrypt(&stored)?)?) } else { None };
            entries.push(CatchupEntry { seq, message_id, envelope, removed_reason: reason });
        }
        let next = entries.last().map_or(request.after_seq, |e| e.seq);
        let status = if entries.iter().any(|e| e.removed_reason.as_deref() == Some("retention")) { "history_pruned" } else { "ok" };
        let mut response = CatchupResponse { originating_server: state.get_public_url(), requesting_server: request.requesting_server, channel_id: request.channel_id, nonce: request.nonce, after_seq: request.after_seq, next_after_seq: next, status: status.into(), entries, signature: String::new() };
        response.signature = hex::encode(state.signing_key.sign(&response_signing_input(&response)).to_bytes());
        Ok(response)
    }).await.map_err(internal)?
}

/// Verify the whole batch before applying any entry; a response from another
/// request, peer or channel cannot advance this cursor, even with a valid key.
pub fn verify_response(
    request: &CatchupRequest,
    response: &CatchupResponse,
    peer_url: &str,
    key: &str,
) -> Result<(), FederationError> {
    verify_ed25519(key, &response.signature, &response_signing_input(response))?;
    if response.originating_server != peer_url
        || response.requesting_server != request.requesting_server
        || response.channel_id != request.channel_id
        || response.nonce != request.nonce
        || response.after_seq != request.after_seq
        || response.entries.len() > request.max_envelopes as usize
    {
        return Err(FederationError::Forbidden(
            "catch-up response does not match request".into(),
        ));
    }
    let mut prev = request.after_seq;
    let mut pruned = false;
    for entry in &response.entries {
        if entry.seq <= prev || entry.message_id.is_empty() {
            return Err(FederationError::Forbidden(
                "invalid catch-up sequence".into(),
            ));
        }
        prev = entry.seq;
        match (&entry.envelope, entry.removed_reason.as_deref()) {
            (Some(envelope), None)
                if envelope.get("originating_server").and_then(|v| v.as_str())
                    == Some(peer_url)
                    && envelope.get("channel_id").and_then(|v| v.as_str())
                        == Some(request.channel_id.as_str())
                    && envelope.get("message_id").and_then(|v| v.as_str())
                        == Some(entry.message_id.as_str()) => {}
            (None, Some("deleted")) => {}
            (None, Some("retention")) => pruned = true,
            _ => return Err(FederationError::Forbidden("invalid catch-up entry".into())),
        }
    }
    if prev != response.next_after_seq
        || response.status != if pruned { "history_pruned" } else { "ok" }
    {
        return Err(FederationError::Forbidden(
            "invalid catch-up result cursor or status".into(),
        ));
    }
    Ok(())
}

async fn apply_removed(
    state: Arc<AppState>,
    peer_url: String,
    channel: String,
    entry: CatchupEntry,
) -> Result<(), FederationError> {
    let state_db = state.clone();
    let channel_db = channel.clone();
    let updated = tokio::task::spawn_blocking(move || {
        let mut conn = state_db.pool.get().map_err(internal)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let peer = allowed_peer(&tx, &state_db, &peer_url)?;
        require_channel(&tx, &state_db, &channel_db)?;
        let existing_channel: Option<String> = tx.query_row(
            "SELECT channel_id FROM messages WHERE message_id = ?1", [&entry.message_id], |r| r.get(0),
        ).optional()?;
        if existing_channel.as_ref().is_some_and(|actual| actual != &channel_db) {
            return Err(FederationError::Forbidden("removal marker does not match the original message channel".into()));
        }
        tx.execute("INSERT OR IGNORE INTO federation_catchup_tombstones (remote_instance_id, message_id, channel_id, reason) VALUES (?1, ?2, ?3, ?4)", params![peer.id, entry.message_id, channel_db, entry.removed_reason])?;
        // A peer cannot delete a locally authored message or another peer's copy.
        let owns: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM federation_message_receipts WHERE remote_instance_id = ?1 AND message_id = ?2)", params![peer.id, entry.message_id], |r| r.get(0))?;
        let updated = if owns && existing_channel.is_some() {
            tx.execute("UPDATE messages SET content = '', deleted_at = COALESCE(deleted_at, datetime('now')) WHERE message_id = ?1 AND channel_id = ?2", params![entry.message_id, channel_db])?;
            tx.execute("DELETE FROM message_edits WHERE message_id = ?1", [&entry.message_id])?;
            Some(annex_channels::get_message(&tx, &entry.message_id)?)
        } else { None };
        tx.commit()?;
        Ok::<_, FederationError>(updated)
    }).await.map_err(internal)??;
    if let Some(message) = updated {
        state
            .connection_manager
            .broadcast(
                &channel,
                serde_json::to_string(&OutgoingMessage::MessageDeleted(message.into()))?,
            )
            .await;
    }
    Ok(())
}

/// One bounded recovery page. Useful to operators/tests; the production worker
/// repeats it automatically, with the same persisted cursor after a restart.
pub async fn recover_channel(
    state: Arc<AppState>,
    peer_url: &str,
    channel: &str,
) -> Result<usize, FederationError> {
    if crate::api_link_preview::is_peer_url_disallowed(
        peer_url,
        state.federation_config.allow_private_peer_addresses,
    ) {
        return Err(FederationError::Forbidden(
            "peer URL is not permitted".into(),
        ));
    }
    let (peer_id, key, after_seq) = {
        let conn = state.pool.get().map_err(internal)?;
        let peer = allowed_peer(&conn, &state, peer_url)?;
        require_channel(&conn, &state, channel)?;
        let seq = conn.query_row("SELECT after_seq FROM federation_catchup_cursors WHERE local_server_id = ?1 AND remote_instance_id = ?2 AND channel_id = ?3", params![state.server_id, peer.id, channel], |r| r.get::<_, i64>(0)).optional()?.unwrap_or(0);
        (peer.id, peer.public_key_hex, seq)
    };
    let mut request = CatchupRequest {
        requesting_server: state.get_public_url(),
        channel_id: channel.into(),
        after_seq,
        max_envelopes: MAX_BATCH,
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    request.signature = hex::encode(
        state
            .signing_key
            .sign(&request_signing_input(&request))
            .to_bytes(),
    );
    let body = serde_json::to_string(&request)?;
    let body = super::federation_delivery::request(
        state.clone(),
        peer_id,
        "/api/federation/catch-up",
        &body,
    )
    .await
    .map_err(FederationError::RemoteServer)?;
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(FederationError::Forbidden(
            "catch-up response exceeds size limit".into(),
        ));
    }
    let response: CatchupResponse = serde_json::from_str(&body)?;
    verify_response(&request, &response, peer_url, &key)?;
    let count = response.entries.len();
    let service = FederationService::new(state.clone());
    for entry in response.entries {
        // Revocation/deletion during a page takes effect before its next entry.
        {
            let conn = state.pool.get().map_err(internal)?;
            allowed_peer(&conn, &state, peer_url)?;
            require_channel(&conn, &state, channel)?;
        }
        let seq = entry.seq;
        let pruned = entry.removed_reason.as_deref() == Some("retention");
        if let Some(envelope) = entry.envelope.clone() {
            match envelope.get("envelopeKind").and_then(|v| v.as_str()) {
                Some("redaction") => {
                    service
                        .receive_federated_redaction_with_mode(
                            serde_json::from_value(envelope)?,
                            DeliveryMode::Catchup,
                        )
                        .await?;
                }
                Some("edit") => {
                    service
                        .receive_federated_edit_with_mode(
                            serde_json::from_value(envelope)?,
                            DeliveryMode::Catchup,
                        )
                        .await?;
                }
                None => {
                    service
                        .receive_federated_message_with_mode(
                            serde_json::from_value(envelope)?,
                            DeliveryMode::Catchup,
                        )
                        .await?
                }
                _ => {
                    return Err(FederationError::Forbidden(
                        "unsupported catch-up envelope kind".into(),
                    ))
                }
            }
        } else {
            apply_removed(state.clone(), peer_url.into(), channel.into(), entry).await?;
        }
        // Cursor advancement follows successful receipt/tombstone commit. A crash
        // between them repeats an idempotent operation, never skips an event.
        {
            let conn = state.pool.get().map_err(internal)?;
            conn.execute("INSERT INTO federation_catchup_cursors (local_server_id, remote_instance_id, channel_id, after_seq, pruned_through_seq) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(local_server_id, remote_instance_id, channel_id) DO UPDATE SET after_seq = MAX(after_seq, excluded.after_seq), pruned_through_seq = CASE WHEN excluded.pruned_through_seq IS NULL THEN pruned_through_seq ELSE MAX(COALESCE(pruned_through_seq, 0), excluded.pruned_through_seq) END, updated_at = datetime('now')", params![state.server_id, peer_id, channel, seq, if pruned { Some(seq) } else { None }])?;
        }
        if pruned {
            let notice = serde_json::json!({"type":"federation_history_pruned", "channelId":channel, "originatingServer":peer_url, "throughSeq":seq});
            state
                .connection_manager
                .broadcast(channel, notice.to_string())
                .await;
        }
    }
    Ok(count)
}

pub async fn recover_all(state: Arc<AppState>) -> Result<(), FederationError> {
    let pairs = {
        let conn = state.pool.get().map_err(internal)?;
        let peers = repo::list_active_peers(&conn, state.server_id)?;
        let mut channels = annex_channels::list_federated_channels(&conn, state.server_id)?;
        let managed = {
            let mut stmt =
                conn.prepare("SELECT channel_id FROM channel_authorities WHERE server_id=?1")?;
            let rows = stmt
                .query_map([state.server_id], |r| r.get::<_, String>(0))?
                .collect::<Result<std::collections::HashSet<_>, _>>()?;
            rows
        };
        channels.retain(|channel| !managed.contains(&channel.channel_id));
        peers
            .into_iter()
            .filter(|p| p.transfer_scope != "NO_TRANSFER")
            .flat_map(|p| {
                channels
                    .iter()
                    .map(move |c| (p.base_url.clone(), c.channel_id.clone()))
            })
            .collect::<Vec<_>>()
    };
    for (peer, channel) in pairs {
        if state.shutdown.is_cancelled() {
            break;
        }
        if let Err(error) = recover_channel(state.clone(), &peer, &channel).await {
            tracing::debug!(%peer, %channel, %error, "federation catch-up will retry");
        }
    }
    Ok(())
}
