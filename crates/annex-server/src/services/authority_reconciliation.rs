//! Explicit, restartable adoption of a populated legacy replica. No local row
//! is changed until the signed chain proves every retained body, edit, key and
//! attachment. Remote Join is a separately acknowledged distributed side effect.
use super::{
    authority_bootstrap, channel_authority as a, channel_replication as replica,
    federation_delivery, human_federation,
};
use crate::{middleware::IdentityContext, AppState};
use a::{
    AuthorityBinding, AuthorityError as Error, CatchupBatch, EventKind, EventRecord, SignedRequest,
};
use axum::{extract::Path, Extension, Json};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const MAX_PAGES: usize = 16;
const MAX_ACTIVE: i64 = 16;
const MAX_STAGED_BYTES: i64 = 512 * 1024 * 1024;
const MAX_CONFLICTS: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconcileRequest {
    pub peer_id: i64,
    pub authority_identity: String,
    pub device_id: String,
    pub operation_id: String,
    pub expected_snapshot_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub channel_id: String,
    pub snapshot_sha256: String,
    pub messages: i64,
    pub edits: i64,
    pub attachments: i64,
    pub key_records: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub kind: String,
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub operation_id: String,
    pub status: String,
    pub remote_join_accepted: bool,
    pub after_seq: i64,
    pub head_seq: i64,
    pub head_hash: String,
    pub retained_messages: i64,
    pub removed_messages: i64,
    pub conflicts: Vec<Conflict>,
}
fn internal(e: impl std::fmt::Display) -> Error {
    Error::Internal(e.to_string())
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn operation_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn operator(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    reader: &replica::Reader,
) -> Result<(), Error> {
    replica::authorize_reader(conn, state, reader)?;
    moderator(
        conn,
        state,
        channel,
        &reader.pseudonym_id,
        reader.token_epoch,
    )
}
fn moderator(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    pid: &str,
    epoch: i64,
) -> Result<(), Error> {
    let who = human_federation::current_local_identity(conn, state, pid, epoch)?;
    if !who.can_moderate {
        return Err(Error::Forbidden(
            "current moderator permission is required".into(),
        ));
    }
    super::e2e_service::require_member(conn, state, channel, pid)?;
    let row =
        annex_channels::get_channel(conn, channel).map_err(|e| Error::NotFound(e.to_string()))?;
    if row.server_id != state.server_id
        || row.federation_scope != annex_types::FederationScope::Federated
    {
        return Err(Error::BadRequest(
            "reconciliation requires an existing federated channel".into(),
        ));
    }
    Ok(())
}

/// Fixed SQL only; values are framed and hashed, so neither concatenation nor
/// at-rest nonce differences can disguise a changed local snapshot.
fn hash_query(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    sql: &str,
    hash: &mut Sha256,
) -> Result<i64, Error> {
    hash.update((sql.len() as u64).to_be_bytes());
    hash.update(sql.as_bytes());
    let mut stmt = conn.prepare(sql)?;
    let names = stmt
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let mut rows = stmt.query(params![state.server_id, channel])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let mut value = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let raw: rusqlite::types::Value = row.get(index)?;
            let item = match raw {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(n) => json!(n),
                rusqlite::types::Value::Real(n) => json!(n),
                rusqlite::types::Value::Blob(b) => json!(hex::encode(b)),
                rusqlite::types::Value::Text(s) => json!(if matches!(
                    name.as_str(),
                    "content" | "old_content" | "envelope_json"
                ) && !s.is_empty()
                {
                    state.message_cipher().decrypt(&s)?
                } else {
                    s
                }),
            };
            value.push(item);
        }
        let raw = a::canonical_json(&json!(value));
        hash.update((raw.len() as u64).to_be_bytes());
        hash.update(raw.as_bytes());
        count += 1;
    }
    Ok(count)
}
pub fn preview(conn: &Connection, state: &AppState, channel: &str) -> Result<Preview, Error> {
    let mut hash = Sha256::new();
    hash.update(b"annex-reconciliation-snapshot-v1");
    for sql in ["SELECT * FROM channels WHERE server_id=?1 AND channel_id=?2 ORDER BY channel_id",
        "SELECT * FROM channel_members WHERE server_id=?1 AND channel_id=?2 ORDER BY pseudonym_id",
        "SELECT * FROM channel_key_state WHERE server_id=?1 AND channel_id=?2",
        "SELECT * FROM channel_manifest_state WHERE server_id=?1 AND channel_id=?2",
        "SELECT event_seq,event_hash,event_json FROM channel_manifest_events WHERE server_id=?1 AND channel_id=?2 ORDER BY event_seq",
        "SELECT peer_instance_id,message_id,source_message_id,envelope_kind,envelope_json,removed_reason FROM federation_outbox WHERE channel_id=?2 AND EXISTS(SELECT 1 FROM channels WHERE server_id=?1 AND channel_id=?2) ORDER BY peer_instance_id,message_id"]{hash_query(conn,state,channel,sql,&mut hash)?;}
    let messages = hash_query(
        conn,
        state,
        channel,
        "SELECT * FROM messages WHERE server_id=?1 AND channel_id=?2 ORDER BY message_id",
        &mut hash,
    )?;
    let edits=hash_query(conn,state,channel,"SELECT e.message_id,e.old_content,e.edited_at FROM message_edits e JOIN messages m ON m.message_id=e.message_id WHERE m.server_id=?1 AND m.channel_id=?2 ORDER BY e.message_id,e.id",&mut hash)?;
    let attachments=hash_query(conn,state,channel,"SELECT u.*,m.message_id FROM message_uploads m JOIN uploads u ON u.server_id=m.server_id AND u.upload_id=m.upload_id WHERE u.server_id=?1 AND u.channel_id=?2 ORDER BY u.upload_id,m.message_id",&mut hash)?;
    let mut key_records = 0;
    for (table, cols) in authority_bootstrap::KEY_TABLES {
        key_records += hash_query(
            conn,
            state,
            channel,
            &format!(
                "SELECT {} FROM {table} WHERE server_id=?1 AND channel_id=?2 ORDER BY {}",
                cols.join(","),
                cols.join(",")
            ),
            &mut hash,
        )?;
    }
    Ok(Preview {
        channel_id: channel.into(),
        snapshot_sha256: hex::encode(hash.finalize()),
        messages,
        edits,
        attachments,
        key_records,
    })
}
fn selected(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    input: &ReconcileRequest,
) -> Result<AuthorityBinding, Error> {
    let (url,key):(String,String)=conn.query_row("SELECT i.base_url,i.public_key FROM instances i JOIN federation_agreements g ON g.remote_instance_id=i.id WHERE i.id=?1 AND i.status='ACTIVE' AND g.local_server_id=?2 AND g.active=1 AND g.transfer_scope<>'NO_TRANSFER'",params![input.peer_id,state.server_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or_else(||Error::Forbidden("selected authority has no active agreement".into()))?;
    if a::server_identity(&key)? != input.authority_identity {
        return Err(Error::Forbidden(
            "confirmed authority fingerprint does not match the pinned peer".into(),
        ));
    }
    let bound = AuthorityBinding {
        channel_id: channel.into(),
        authority_identity: input.authority_identity.clone(),
        authority_key_hex: key,
        authority_url: url.trim_end_matches('/').into(),
        authority_peer_id: Some(input.peer_id),
        local_authority: false,
        head_seq: 0,
        head_hash: String::new(),
        deleted_seq: None,
    };
    replica::current_peer(conn, state, &bound)?;
    Ok(bound)
}
fn saved(conn: &Connection, state: &AppState, channel: &str, id: &str) -> Result<Outcome, Error> {
    conn.query_row("SELECT status,remote_join_accepted,after_seq,head_seq,head_hash,result_json FROM channel_reconciliations WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?))).map_err(Error::from).and_then(|(status,joined,after,head,hash,result)|{
        if let Some(raw)=result {
            let mut saved:Outcome=serde_json::from_str(&raw).map_err(internal)?;
            if saved.operation_id!=id {return Err(Error::Internal("stored reconciliation outcome has the wrong operation ID".into()));}
            // The durable lifecycle columns remain authoritative when expiry or
            // an in-flight accepted Join updates them after a prior result.
            saved.status=status; saved.remote_join_accepted=joined;
            saved.after_seq=after; saved.head_seq=head; saved.head_hash=hash;
            return Ok(saved);
        }
        Ok(Outcome{operation_id:id.into(),status,remote_join_accepted:joined,after_seq:after,head_seq:head,head_hash:hash,retained_messages:0,removed_messages:0,conflicts:vec![]})
    })
}
fn persist_result(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    result: &Outcome,
) -> Result<(), Error> {
    conn.execute("UPDATE channel_reconciliations SET status=?4,result_json=?5,updated_at=?6 WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,result.operation_id,result.status,serde_json::to_string(result).map_err(internal)?,now()])?;
    Ok(())
}
fn verify_batch(
    bound: &AuthorityBinding,
    request: &SignedRequest,
    batch: &CatchupBatch,
) -> Result<(), Error> {
    if batch.version != 1
        || batch.authority_identity != bound.authority_identity
        || batch.channel_id != bound.channel_id
        || batch.request_hash != a::digest(&request.signing_value())
        || batch.after_seq != bound.head_seq
        || batch.records.len() > 64
        || batch.head_seq < bound.head_seq
        || batch.head_seq > 9_007_199_254_740_991
    {
        return Err(Error::Forbidden(
            "reconciliation response does not match its fresh request and pin".into(),
        ));
    }
    a::verify(
        &bound.authority_key_hex,
        &batch.signature,
        &batch.signing_value(),
    )?;
    if batch.access_revoked {
        return Err(Error::Forbidden(
            "authority revoked reconciliation reader".into(),
        ));
    }
    let mut seq = bound.head_seq;
    let mut previous = bound.head_hash.as_str();
    for record in &batch.records {
        a::verify_record(bound, record)?;
        if record.header().seq != seq + 1 || record.header().previous_hash != previous {
            return Err(Error::Forbidden(
                "reconciliation chain has a gap or fork".into(),
            ));
        }
        seq += 1;
        previous = record.hash();
    }
    if seq > batch.head_seq
        || batch.more != (seq < batch.head_seq)
        || (seq == batch.head_seq && previous != batch.head_hash)
    {
        return Err(Error::Forbidden(
            "invalid reconciliation signed head".into(),
        ));
    }
    Ok(())
}
async fn pull(
    state: Arc<AppState>,
    bound: &AuthorityBinding,
    reader: &replica::Reader,
    limit: usize,
) -> Result<(SignedRequest, CatchupBatch), Error> {
    replica::attest_human(state.clone(), bound, reader).await?;
    let request = replica::signed_request(
        &state,
        bound,
        reader,
        &uuid::Uuid::new_v4().to_string(),
        json!({"type":"catch_up","after_seq":bound.head_seq,"limit":limit}),
    );
    let raw = federation_delivery::request_typed(
        state.clone(),
        bound.authority_peer_id.unwrap(),
        "/api/federation/channel-authority/catch-up",
        &serde_json::to_string(&request).map_err(internal)?,
    )
    .await
    .map_err(Error::from)?;
    let batch: CatchupBatch = serde_json::from_str(&raw).map_err(internal)?;
    verify_batch(bound, &request, &batch)?;
    Ok((request, batch))
}
fn record_message(record: &EventRecord) -> Option<&str> {
    match record {
        EventRecord::Removed(e) => e.message_id.as_deref(),
        EventRecord::Present(e) => e.payload["message_id"]
            .as_str()
            .or_else(|| e.payload["message"]["message_id"].as_str()),
    }
}
fn body_record(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::MessageAccepted
            | EventKind::MessageEdited
            | EventKind::HistoryImported
            | EventKind::HistoryEditImported
    )
}
fn stage(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
    id: &str,
    batch: &CatchupBatch,
) -> Result<(), Error> {
    require_active(tx, state, channel, id)?;
    for record in &batch.records {
        let raw = serde_json::to_string(record).map_err(internal)?;
        let ciphertext = state.message_cipher().encrypt(&raw)?;
        let expiry = match record {
            EventRecord::Present(e) => e.payload["message"]["expires_at"].as_str(),
            _ => None,
        };
        let body = matches!(record,EventRecord::Present(e) if body_record(&e.header.event_kind));
        tx.execute("INSERT INTO channel_reconciliation_records(server_id,channel_id,operation_id,event_seq,event_hash,header_json,message_id,record_ciphertext,body_record,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![state.server_id,channel,id,record.header().seq,record.hash(),serde_json::to_string(record.header()).map_err(internal)?,record_message(record),ciphertext,body,expiry])?;
        if matches!(record, EventRecord::Removed(_))
            || matches!(record,EventRecord::Present(e) if matches!(e.header.event_kind,EventKind::MessageDeleted|EventKind::HistoryPruned))
        {
            if let Some(message) = record_message(record) {
                tx.execute("UPDATE channel_reconciliation_records SET record_ciphertext=NULL WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3 AND message_id=?4 AND body_record=1",params![state.server_id,channel,id,message])?;
            }
        }
        if matches!(record,EventRecord::Present(e) if e.header.event_kind==EventKind::ChannelDeleted)
        {
            tx.execute("UPDATE channel_reconciliation_records SET record_ciphertext=NULL WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3 AND body_record=1",params![state.server_id,channel,id])?;
        }
    }
    let used:i64=tx.query_row("SELECT COALESCE(SUM(length(record_ciphertext)),0) FROM channel_reconciliation_records WHERE server_id=?1",[state.server_id],|r|r.get(0))?;
    if used > MAX_STAGED_BYTES {
        return Err(Error::Unavailable(
            "reconciliation staging capacity is full; cancel an unused operation before continuing"
                .into(),
        ));
    }
    let last = batch.records.last();
    tx.execute("UPDATE channel_reconciliations SET after_seq=?4,after_hash=?5,head_seq=?6,head_hash=?7,updated_at=?8,result_json=NULL,status='pending' WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,id,last.map_or(batch.after_seq,|r|r.header().seq),last.map_or(batch.head_hash.as_str(),|r|r.hash()),batch.head_seq,batch.head_hash,now()])?;
    Ok(())
}
/// Payload retention applies to temporary durable copies too. A cleared record
/// can only be repaired with the authority's signed removal certificate.
pub(crate) fn expire_staging(conn: &Connection, state: &AppState) -> Result<(), Error> {
    if conn.is_autocommit() {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        expire_staging_for_server(&tx, state.server_id)?;
        tx.commit()?;
        Ok(())
    } else {
        expire_staging_for_server(conn, state.server_id)
    }
}
fn require_active(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    id: &str,
) -> Result<(), Error> {
    let active:bool=conn.query_row("SELECT status IN('pending','conflict') FROM channel_reconciliations WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,id],|r|r.get(0))?;
    if !active {
        return Err(Error::Conflict(
            "reconciliation staging was cancelled; review a new operation".into(),
        ));
    }
    Ok(())
}
fn expire_staging_for_server(conn: &Connection, server_id: i64) -> Result<(), Error> {
    conn.execute("UPDATE channel_reconciliation_records SET record_ciphertext=NULL WHERE server_id=?1 AND body_record=1 AND EXISTS(SELECT 1 FROM channel_reconciliation_records original WHERE original.server_id=channel_reconciliation_records.server_id AND original.channel_id=channel_reconciliation_records.channel_id AND original.operation_id=channel_reconciliation_records.operation_id AND original.message_id=channel_reconciliation_records.message_id AND original.expires_at IS NOT NULL AND (datetime(original.expires_at) IS NULL OR datetime(original.expires_at)<=datetime('now')))",[server_id])?;
    conn.execute("DELETE FROM channel_reconciliation_records WHERE server_id=?1 AND EXISTS(SELECT 1 FROM channel_reconciliations r WHERE r.server_id=channel_reconciliation_records.server_id AND r.channel_id=channel_reconciliation_records.channel_id AND r.operation_id=channel_reconciliation_records.operation_id AND r.updated_at<?2)",params![server_id,now()-86400])?;
    conn.execute("UPDATE channel_reconciliations SET status='cancelled',join_receipt_ciphertext=NULL,result_json=NULL WHERE server_id=?1 AND status IN('pending','conflict') AND updated_at<?2",params![server_id,now()-86400])?;
    Ok(())
}

pub async fn reconcile(
    state: Arc<AppState>,
    identity: &annex_identity::PlatformIdentity,
    channel: &str,
    input: ReconcileRequest,
) -> Result<Outcome, Error> {
    if !operation_id(&input.operation_id)
        || !super::e2e_service::lower_hex(&input.expected_snapshot_sha256)
    {
        return Err(Error::BadRequest(
            "invalid operation ID or confirmed snapshot digest".into(),
        ));
    }
    let _guard = replica::channel_guard(&state, channel).await?;
    let reader = replica::Reader {
        pseudonym_id: identity.pseudonym_id.clone(),
        device_id: input.device_id.clone(),
        token_epoch: identity.token_epoch,
    };
    let mut bound = {
        let mut conn = state.pool.get().map_err(internal)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        operator(&tx, &state, channel, &reader)?;
        let bound = selected(&tx, &state, channel, &input)?;
        expire_staging(&tx, &state)?;
        let prior:Option<(String,String,String,String,i64,String,String)>=tx.query_row("SELECT pseudonym_id,device_id,snapshot_sha256,authority_identity,peer_id,authority_key_hex,authority_url FROM channel_reconciliations WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,input.operation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
        if let Some(prior) = prior {
            if prior
                != (
                    reader.pseudonym_id.clone(),
                    reader.device_id.clone(),
                    input.expected_snapshot_sha256.clone(),
                    input.authority_identity.clone(),
                    input.peer_id,
                    bound.authority_key_hex.clone(),
                    bound.authority_url.clone(),
                )
            {
                return Err(Error::Conflict(
                    "operation ID belongs to a different confirmed reconciliation".into(),
                ));
            }
            let result = saved(&tx, &state, channel, &input.operation_id)?;
            if result.status == "complete" || result.status == "cancelled" {
                tx.commit()?;
                return Ok(result);
            }
            tx.execute("UPDATE channel_reconciliations SET token_epoch=?4 WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,input.operation_id,reader.token_epoch])?;
        } else {
            if a::binding(&tx, &state, channel).is_ok() {
                return Err(Error::Conflict(
                    "channel already has an authority; ownership cannot be reassigned".into(),
                ));
            }
            if preview(&tx, &state, channel)?.snapshot_sha256 != input.expected_snapshot_sha256 {
                return Err(Error::Conflict(
                    "local snapshot changed; review a new preview".into(),
                ));
            }
            let active:i64=tx.query_row("SELECT COUNT(*) FROM channel_reconciliations WHERE server_id=?1 AND status IN('pending','conflict')",[state.server_id],|r|r.get(0))?;
            if active >= MAX_ACTIVE {
                return Err(Error::Unavailable(
                    "too many pending reconciliations".into(),
                ));
            }
            tx.execute("INSERT INTO channel_reconciliations(server_id,channel_id,operation_id,pseudonym_id,device_id,token_epoch,peer_id,authority_identity,authority_key_hex,authority_url,snapshot_sha256,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)",params![state.server_id,channel,input.operation_id,reader.pseudonym_id,reader.device_id,reader.token_epoch,input.peer_id,bound.authority_identity,bound.authority_key_hex,bound.authority_url,input.expected_snapshot_sha256,now()])?;
        }
        tx.commit()?;
        bound
    };
    let joined = {
        let conn = state.pool.get().map_err(internal)?;
        saved(&conn, &state, channel, &input.operation_id)?.remote_join_accepted
    };
    if !joined {
        replica::attest_human(state.clone(), &bound, &reader).await?;
        let join_id = a::digest(&json!([
            "annex-reconciliation-join-v1",
            input.operation_id,
            reader.pseudonym_id,
            input.expected_snapshot_sha256
        ]));
        let request =
            replica::signed_request(&state, &bound, &reader, &join_id, json!({"kind":"join"}));
        let raw = federation_delivery::request_typed(
            state.clone(),
            input.peer_id,
            "/api/federation/channel-authority/operations",
            &serde_json::to_string(&request).map_err(internal)?,
        )
        .await
        .map_err(Error::from)?;
        let receipt: a::OperationReceipt = serde_json::from_str(&raw).map_err(internal)?;
        receipt.verify(&bound, &request)?;
        let mut conn = state.pool.get().map_err(internal)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        operator(&tx, &state, channel, &reader)?;
        replica::current_peer(&tx, &state, &bound)?;
        tx.execute("UPDATE channel_reconciliations SET join_receipt_ciphertext=?4,remote_join_accepted=1,updated_at=?5 WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,input.operation_id,state.message_cipher().encrypt(&serde_json::to_string(&json!({"request":request,"receipt":receipt})).map_err(internal)?)?,now()])?;
        tx.commit()?;
    }
    for _ in 0..MAX_PAGES {
        {
            let conn = state.pool.get().map_err(internal)?;
            operator(&conn, &state, channel, &reader)?;
            replica::current_peer(&conn, &state, &bound)?;
            expire_staging(&conn, &state)?;
            let current = saved(&conn, &state, channel, &input.operation_id)?;
            if current.status == "cancelled" || current.status == "complete" {
                return Ok(current);
            }
            let cursor:(i64,String)=conn.query_row("SELECT after_seq,after_hash FROM channel_reconciliations WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,input.operation_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
            bound.head_seq = cursor.0;
            bound.head_hash = cursor.1;
        }
        let (request, batch) = pull(state.clone(), &bound, &reader, 64).await?;
        if batch.records.is_empty() && !batch.more {
            // Repair bodyless staged history before finalizing. These are real
            // fresh reads of the original sequence, never synthetic tombstones.
            let missing: Option<(i64, String, String)> = {
                let conn = state.pool.get().map_err(internal)?;
                conn.query_row("SELECT event_seq,event_hash,header_json FROM channel_reconciliation_records WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3 AND record_ciphertext IS NULL ORDER BY event_seq LIMIT 1",params![state.server_id,channel,input.operation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?
            };
            if let Some((seq, hash, header)) = missing {
                let header: a::EventHeader = serde_json::from_str(&header).map_err(internal)?;
                let mut old = bound.clone();
                old.head_seq = seq - 1;
                old.head_hash = header.previous_hash;
                let (_, repair) = pull(state.clone(), &old, &reader, 1).await?;
                let record = repair
                    .records
                    .first()
                    .filter(|r| r.hash() == hash && matches!(r, EventRecord::Removed(_)))
                    .ok_or_else(|| {
                        Error::Forbidden(
                            "authority did not provide the required signed history removal".into(),
                        )
                    })?;
                let mut conn = state.pool.get().map_err(internal)?;
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                operator(&tx, &state, channel, &reader)?;
                replica::current_peer(&tx, &state, &bound)?;
                expire_staging(&tx, &state)?;
                require_active(&tx, &state, channel, &input.operation_id)?;
                tx.execute("UPDATE channel_reconciliation_records SET record_ciphertext=?5,body_record=0,expires_at=NULL WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3 AND event_seq=?4 AND event_hash=?6",params![state.server_id,channel,input.operation_id,seq,state.message_cipher().encrypt(&serde_json::to_string(record).map_err(internal)?)?,hash])?;
                tx.commit()?;
                continue;
            }
            let finish_state = state.clone();
            let finish_channel = channel.to_owned();
            let result = run_owned_writer(_guard, move || {
                finish(
                    &finish_state,
                    &finish_channel,
                    &reader,
                    &input,
                    &bound,
                    &request,
                    &batch,
                )
            })
            .await?;
            if result.status == "complete" {
                drain_departures(state.clone()).await?;
            }
            return Ok(result);
        }
        let mut conn = state.pool.get().map_err(internal)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        operator(&tx, &state, channel, &reader)?;
        replica::current_peer(&tx, &state, &bound)?;
        stage(&tx, &state, channel, &input.operation_id, &batch)?;
        tx.commit()?;
    }
    let conn = state.pool.get().map_err(internal)?;
    saved(&conn, &state, channel, &input.operation_id)
}

// Final projection/comparison is below. Helpers use a private temporary table
// scoped to this connection and writer transaction, containing hashes only.
/// Non-cancellable replay retains channel ownership even if its HTTP waiter
/// disappears. Database and signature work never runs on an async I/O worker.
async fn run_owned_writer<T: Send + 'static>(
    guard: tokio::sync::OwnedMutexGuard<()>,
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(move || {
        let _channel_guard = guard;
        work()
    })
    .await
    .map_err(internal)?
}

fn reference(
    conn: &Connection,
    table: &str,
    kind: &str,
    id: &str,
    digest: &str,
    message: Option<&str>,
) -> Result<(), Error> {
    debug_assert!(matches!(table, "reconcile_local" | "reconcile_known"));
    conn.execute(
        &format!("INSERT OR IGNORE INTO {table}(kind,id,digest,message_id) VALUES(?1,?2,?3,?4)"),
        params![kind, id, digest, message],
    )?;
    Ok(())
}
fn message_identity(message: &annex_channels::Message) -> String {
    a::digest(&json!([
        message.channel_id,
        message.message_id,
        message.sender_pseudonym,
        message.created_at,
        message.reply_to_message_id
    ]))
}
fn body_digest(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))
}
fn copy_keys(
    conn: &Connection,
    state: &AppState,
    channel: &str,
    target: &str,
) -> Result<(), Error> {
    for (table, columns) in authority_bootstrap::KEY_TABLES {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM {table} WHERE server_id=?1 AND channel_id=?2",
            columns.join(",")
        ))?;
        let mut rows = stmt.query(params![state.server_id, channel])?;
        while let Some(row) = rows.next()? {
            let mut values = Vec::new();
            for index in 0..columns.len() {
                values.push(match row.get::<_, rusqlite::types::Value>(index)? {
                    rusqlite::types::Value::Null => Value::Null,
                    rusqlite::types::Value::Integer(n) => json!(n),
                    rusqlite::types::Value::Text(s) => json!(s),
                    _ => {
                        return Err(Error::Forbidden(
                            "invalid immutable historical key record".into(),
                        ))
                    }
                });
            }
            let digest = a::digest(&json!(values));
            reference(
                conn,
                target,
                "key_namespace",
                &format!("{table}:{digest}"),
                &digest,
                None,
            )?;
        }
    }
    let mut stmt=conn.prepare("SELECT event_seq,event_hash FROM channel_manifest_events WHERE server_id=?1 AND channel_id=?2")?;
    let mut rows = stmt.query(params![state.server_id, channel])?;
    while let Some(row) = rows.next()? {
        reference(
            conn,
            target,
            "signed_manifest",
            &row.get::<_, i64>(0)?.to_string(),
            &row.get::<_, String>(1)?,
            None,
        )?;
    }
    Ok(())
}
fn collect_local(tx: &Transaction<'_>, state: &AppState, channel: &str) -> Result<i64, Error> {
    let mut stmt=tx.prepare("SELECT message_id,deleted_at IS NOT NULL OR (expires_at IS NOT NULL AND (datetime(expires_at) IS NULL OR datetime(expires_at)<=datetime('now'))) FROM messages WHERE server_id=?1 AND channel_id=?2 ORDER BY message_id")?;
    let mut rows = stmt.query(params![state.server_id, channel])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let inaccessible: bool = row.get(1)?;
        count += 1;
        let message =
            annex_channels::get_message(tx, &id).map_err(|e| Error::Internal(e.to_string()))?;
        if inaccessible {
            reference(tx, "reconcile_local", "removed", &id, "removed", Some(&id))?;
            continue;
        }
        reference(
            tx,
            "reconcile_local",
            "message_identity",
            &id,
            &message_identity(&message),
            Some(&id),
        )?;
        reference(
            tx,
            "reconcile_local",
            "message_body",
            &id,
            &body_digest(&state.message_cipher().decrypt(&message.content)?),
            Some(&id),
        )?;
        if let Some(expiry) = &message.expires_at {
            reference(
                tx,
                "reconcile_local",
                "retention_deadline",
                &id,
                expiry,
                Some(&id),
            )?;
        }
        let mut edits = tx.prepare(
            "SELECT old_content,edited_at FROM message_edits WHERE message_id=?1 ORDER BY id",
        )?;
        let mut edit_rows = edits.query([&id])?;
        while let Some(edit) = edit_rows.next()? {
            let at: String = edit.get(1)?;
            let body = state.message_cipher().decrypt(&edit.get::<_, String>(0)?)?;
            reference(
                tx,
                "reconcile_local",
                "message_edit",
                &format!("{id}@{at}"),
                &body_digest(&body),
                Some(&id),
            )?;
        }
        let descriptors = super::federated_upload_service::descriptors_for_bootstrap_message(
            tx, state, channel, &id,
        )
        .map_err(a::transfer_error)?;
        for descriptor in descriptors {
            reference(
                tx,
                "reconcile_local",
                "attachment",
                &descriptor.attachment_id,
                &a::digest(&serde_json::to_value(&descriptor).map_err(internal)?),
                Some(&id),
            )?;
        }
    }
    copy_keys(tx, state, channel, "reconcile_local")?;
    Ok(count)
}
fn collect_record(tx: &Transaction<'_>, record: &EventRecord) -> Result<(), Error> {
    if let EventRecord::Present(event) = record {
        if matches!(
            event.header.event_kind,
            EventKind::MessageAccepted | EventKind::MessageEdited | EventKind::HistoryImported
        ) {
            let message: annex_channels::Message =
                serde_json::from_value(event.payload["message"].clone()).map_err(internal)?;
            let id = &message.message_id;
            reference(
                tx,
                "reconcile_known",
                "message_identity",
                id,
                &message_identity(&message),
                Some(id),
            )?;
            reference(
                tx,
                "reconcile_known",
                "message_body",
                id,
                &body_digest(&message.content),
                Some(id),
            )?;
            if let Some(body) = event.payload["old_content"].as_str() {
                reference(
                    tx,
                    "reconcile_known",
                    "message_body",
                    id,
                    &body_digest(body),
                    Some(id),
                )?;
            }
        }
    }
    if matches!(record, EventRecord::Removed(_))
        || matches!(record,EventRecord::Present(e) if matches!(e.header.event_kind,EventKind::MessageDeleted|EventKind::HistoryPruned))
    {
        if let Some(message) = record_message(record) {
            reference(
                tx,
                "reconcile_known",
                "removed",
                message,
                "removed",
                Some(message),
            )?;
        }
    }
    Ok(())
}
fn finish(
    state: &AppState,
    channel: &str,
    reader: &replica::Reader,
    input: &ReconcileRequest,
    bound: &AuthorityBinding,
    request: &SignedRequest,
    batch: &CatchupBatch,
) -> Result<Outcome, Error> {
    verify_batch(bound, request, batch)?;
    let received = chrono::DateTime::parse_from_rfc3339(&request.created_at)
        .map_err(internal)?
        .timestamp();
    if !batch.records.is_empty() || batch.more || now() - received > 30 || received > now() + 5 {
        return Err(Error::Forbidden(
            "reconciliation requires a fresh complete authority head".into(),
        ));
    }
    let mut conn = state.pool.get().map_err(internal)?;
    // The tables contain only one-way digests/IDs, never body or key plaintext.
    conn.execute_batch("DROP TABLE IF EXISTS temp.reconcile_local;DROP TABLE IF EXISTS temp.reconcile_known;DROP TABLE IF EXISTS temp.reconcile_uploads;DROP TABLE IF EXISTS temp.reconcile_serving;CREATE TEMP TABLE reconcile_local(kind TEXT,id TEXT,digest TEXT,message_id TEXT,UNIQUE(kind,id,digest));CREATE TEMP TABLE reconcile_known(kind TEXT,id TEXT,digest TEXT,message_id TEXT,UNIQUE(kind,id,digest));CREATE TEMP TABLE reconcile_uploads(upload_id TEXT PRIMARY KEY);CREATE TEMP TABLE reconcile_serving(pseudonym_id TEXT PRIMARY KEY,weight REAL NOT NULL,created_at TEXT NOT NULL);")?;
    let attempt = (|| -> Result<Outcome, Error> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        operator(&tx, state, channel, reader)?;
        replica::current_peer(&tx, state, bound)?;
        expire_staging(&tx, state)?;
        let mut result = saved(&tx, state, channel, &input.operation_id)?;
        if result.status == "cancelled" || result.status == "complete" {
            tx.commit()?;
            return Ok(result);
        }
        if preview(&tx, state, channel)?.snapshot_sha256 != input.expected_snapshot_sha256 {
            result.status = "conflict".into();
            result.conflicts = vec![Conflict {
                kind: "local_snapshot_changed".into(),
                id: channel.into(),
            }];
            tx.rollback()?;
            persist_result(&conn, state, channel, &result)?;
            return Ok(result);
        }
        if a::binding(&tx, state, channel).is_ok() {
            return Err(Error::Conflict(
                "channel acquired an authority while reconciliation was pending".into(),
            ));
        }
        expire_staging(&tx, state)?;
        let missing:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM channel_reconciliation_records WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3 AND record_ciphertext IS NULL)",params![state.server_id,channel,input.operation_id],|r|r.get(0))?;
        if missing {
            tx.commit()?;
            return Err(Error::Conflict(
                "retention changed during reconciliation; resume to verify signed removals".into(),
            ));
        }
        let proof:String=tx.query_row("SELECT join_receipt_ciphertext FROM channel_reconciliations WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,input.operation_id],|r|r.get(0))?;
        let proof: Value =
            serde_json::from_str(&state.message_cipher().decrypt(&proof)?).map_err(internal)?;
        let join_request: SignedRequest =
            serde_json::from_value(proof["request"].clone()).map_err(internal)?;
        let join_receipt: a::OperationReceipt =
            serde_json::from_value(proof["receipt"].clone()).map_err(internal)?;
        join_receipt.verify(bound, &join_request)?;
        if join_request.pseudonym_id != reader.pseudonym_id
            || join_request.device_id.as_deref() != Some(reader.device_id.as_str())
            || join_request.operation != json!({"kind":"join"})
            || join_receipt.accepted_seq > batch.head_seq
        {
            return Err(Error::Forbidden(
                "reconciliation Join receipt differs from the initiating reader or head".into(),
            ));
        }
        let accepted_hash:String=tx.query_row("SELECT event_hash FROM channel_reconciliation_records WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3 AND event_seq=?4",params![state.server_id,channel,input.operation_id,join_receipt.accepted_seq],|r|r.get(0))?;
        if accepted_hash != join_receipt.accepted_hash {
            return Err(Error::Forbidden(
                "reconciliation Join receipt differs from staged ledger".into(),
            ));
        }
        let original_count = collect_local(&tx, state, channel)?;
        tx.execute("INSERT INTO reconcile_uploads(upload_id) SELECT u.upload_id FROM uploads u WHERE u.server_id=?1 AND u.channel_id=?2 AND EXISTS(SELECT 1 FROM message_uploads m WHERE m.server_id=u.server_id AND m.upload_id=u.upload_id)",params![state.server_id,channel])?;
        tx.execute(
            "INSERT INTO channel_reconciliation_guards(server_id,channel_id) VALUES(?1,?2)",
            params![state.server_id, channel],
        )?;
        tx.execute("INSERT INTO channel_authorities(server_id,channel_id,authority_identity,authority_key_hex,authority_url,authority_peer_id,local_authority) VALUES(?1,?2,?3,?4,?5,?6,0)",params![state.server_id,channel,bound.authority_identity,bound.authority_key_hex,bound.authority_url,bound.authority_peer_id])?;
        // All deletions remain rollbackable. The guard retains upload metadata
        // and bytes; replay restores exact accepted associations before cleanup.
        tx.execute("DELETE FROM message_edits WHERE message_id IN(SELECT message_id FROM messages WHERE server_id=?1 AND channel_id=?2)",params![state.server_id,channel])?;
        tx.execute(
            "DELETE FROM messages WHERE server_id=?1 AND channel_id=?2",
            params![state.server_id, channel],
        )?;
        tx.execute("INSERT INTO reconcile_serving(pseudonym_id,weight,created_at) SELECT e.from_node,e.weight,e.created_at FROM graph_edges e JOIN channel_members m ON m.server_id=e.server_id AND m.pseudonym_id=e.from_node WHERE e.server_id=?1 AND m.channel_id=?2 AND e.to_node=?3 AND e.kind='AGENT_SERVING'",params![state.server_id,channel,annex_graph::channel_vertex(state.server_id,channel)])?;
        tx.execute("INSERT OR IGNORE INTO channel_reconciliation_departures(server_id,channel_id,pseudonym_id) SELECT server_id,channel_id,pseudonym_id FROM channel_members WHERE server_id=?1 AND channel_id=?2",params![state.server_id,channel])?;
        tx.execute(
            "DELETE FROM channel_members WHERE server_id=?1 AND channel_id=?2",
            params![state.server_id, channel],
        )?;
        for table in [
            "channel_device_key_wraps",
            "channel_legacy_device_wraps",
            "channel_key_epochs",
            "channel_legacy_key_candidates",
            "channel_key_wraps",
            "channel_key_authorizations",
            "channel_manifest_events",
            "channel_manifest_state",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE server_id=?1 AND channel_id=?2"),
                params![state.server_id, channel],
            )?;
        }
        {
            let mut stmt=tx.prepare("SELECT record_ciphertext FROM channel_reconciliation_records WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3 ORDER BY event_seq")?;
            let mut rows = stmt.query(params![state.server_id, channel, input.operation_id])?;
            while let Some(row) = rows.next()? {
                let record: EventRecord = serde_json::from_str(
                    &state.message_cipher().decrypt(&row.get::<_, String>(0)?)?,
                )
                .map_err(internal)?;
                if matches!(&record,EventRecord::Present(e) if e.header.event_kind==EventKind::ChannelDeleted)
                {
                    return Err(Error::Conflict(
                        "selected authority has deleted this channel; local history was preserved"
                            .into(),
                    ));
                }
                collect_record(&tx, &record)?;
                a::apply_verified_record(
                    &tx,
                    state,
                    channel,
                    &record,
                    &reader.pseudonym_id,
                    &reader.device_id,
                )?;
            }
        }
        let applied = a::binding(&tx, state, channel)?;
        if applied.head_seq != batch.head_seq || applied.head_hash != batch.head_hash {
            return Err(Error::Forbidden(
                "staged ledger differs from the fresh signed authority head".into(),
            ));
        }
        copy_keys(&tx, state, channel, "reconcile_known")?;
        {
            let mut stmt=tx.prepare("SELECT e.message_id,e.old_content,e.edited_at FROM message_edits e JOIN messages m ON m.message_id=e.message_id WHERE m.server_id=?1 AND m.channel_id=?2")?;
            let mut rows = stmt.query(params![state.server_id, channel])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let body = state.message_cipher().decrypt(&row.get::<_, String>(1)?)?;
                let at: String = row.get(2)?;
                reference(
                    &tx,
                    "reconcile_known",
                    "message_edit",
                    &format!("{id}@{at}"),
                    &body_digest(&body),
                    Some(&id),
                )?;
            }
            let mut stmt=tx.prepare("SELECT attachment_id,descriptor_json,message_id FROM channel_authority_attachment_bindings WHERE server_id=?1 AND channel_id=?2")?;
            let mut rows = stmt.query(params![state.server_id, channel])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let descriptor: Value =
                    serde_json::from_str(&row.get::<_, String>(1)?).map_err(internal)?;
                reference(
                    &tx,
                    "reconcile_known",
                    "attachment",
                    &id,
                    &a::digest(&descriptor),
                    Some(&row.get::<_, String>(2)?),
                )?;
            }
        }
        tx.execute("INSERT OR IGNORE INTO reconcile_known(kind,id,digest,message_id) SELECT l.kind,l.id,l.digest,l.message_id FROM reconcile_local l JOIN messages m ON m.message_id=l.id WHERE l.kind='retention_deadline' AND julianday(m.expires_at)<=julianday(l.digest)",[])?;
        // Removal certificates can intentionally suppress old content. Otherwise
        // each local revision must be present in authenticated authority history.
        let conflicts = {
            let mut stmt=tx.prepare("SELECT DISTINCT l.kind,l.id FROM reconcile_local l WHERE NOT EXISTS(SELECT 1 FROM reconcile_known k WHERE k.kind=l.kind AND k.id=l.id AND k.digest=l.digest) AND NOT EXISTS(SELECT 1 FROM reconcile_known r WHERE r.kind='removed' AND r.id=l.message_id) ORDER BY l.kind,l.id LIMIT ?1")?;
            let rows = stmt
                .query_map([MAX_CONFLICTS as i64], |r| {
                    Ok(Conflict {
                        kind: r.get(0)?,
                        id: r.get(1)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        // A locally deleted/expired message must never be resurrected, even if
        // an old signed authority event has its exact pre-deletion bytes.
        let mut conflicts = conflicts;
        if conflicts.len() < MAX_CONFLICTS {
            let mut stmt=tx.prepare("SELECT l.id FROM reconcile_local l JOIN messages m ON m.message_id=l.id WHERE l.kind='removed' AND m.deleted_at IS NULL AND (m.expires_at IS NULL OR datetime(m.expires_at)>datetime('now')) LIMIT ?1")?;
            conflicts.extend(
                stmt.query_map([(MAX_CONFLICTS - conflicts.len()) as i64], |r| {
                    Ok(Conflict {
                        kind: "would_resurrect_removed_message".into(),
                        id: r.get(0)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?,
            );
        }
        if !conflicts.is_empty() {
            result.status = "conflict".into();
            result.conflicts = conflicts;
            tx.rollback()?;
            persist_result(&conn, state, channel, &result)?;
            return Ok(result);
        }
        tx.execute("DELETE FROM channel_reconciliation_departures WHERE server_id=?1 AND channel_id=?2 AND EXISTS(SELECT 1 FROM channel_members m WHERE m.server_id=channel_reconciliation_departures.server_id AND m.channel_id=channel_reconciliation_departures.channel_id AND m.pseudonym_id=channel_reconciliation_departures.pseudonym_id)",params![state.server_id,channel])?;
        // Replaying membership recreates MEMBER_OF through its trigger.
        // Preserve only actual prior serving claims for retained active local
        // agents, including their original weight and creation time.
        tx.execute("INSERT INTO graph_edges(server_id,from_node,to_node,kind,weight,created_at) SELECT ?1,s.pseudonym_id,?3,'AGENT_SERVING',s.weight,s.created_at FROM reconcile_serving s JOIN channel_members m ON m.server_id=?1 AND m.channel_id=?2 AND m.pseudonym_id=s.pseudonym_id JOIN platform_identities p ON p.server_id=m.server_id AND p.pseudonym_id=m.pseudonym_id WHERE p.active=1 AND p.participant_type='AI_AGENT' ON CONFLICT(server_id,from_node,to_node,kind) DO UPDATE SET weight=excluded.weight,created_at=excluded.created_at",params![state.server_id,channel,annex_graph::channel_vertex(state.server_id,channel)])?;
        result.status = "complete".into();
        result.conflicts.clear();
        result.head_seq = applied.head_seq;
        result.head_hash = applied.head_hash;
        result.after_seq = applied.head_seq;
        result.removed_messages=tx.query_row("SELECT COUNT(DISTINCT l.message_id) FROM reconcile_local l WHERE l.message_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM messages m WHERE m.message_id=l.message_id AND m.deleted_at IS NULL)",[],|r|r.get(0))?;
        result.retained_messages = original_count - result.removed_messages;
        tx.execute(
            "DELETE FROM channel_reconciliation_guards WHERE server_id=?1 AND channel_id=?2",
            params![state.server_id, channel],
        )?;
        tx.execute("DELETE FROM uploads WHERE server_id=?1 AND channel_id=?2 AND upload_id IN(SELECT upload_id FROM reconcile_uploads) AND NOT EXISTS(SELECT 1 FROM message_uploads m WHERE m.server_id=uploads.server_id AND m.upload_id=uploads.upload_id)",params![state.server_id,channel])?;
        tx.execute("UPDATE federation_outbox SET status='failed',last_error='channel reconciled to ordered authority; use signed catch-up' WHERE channel_id=?1 AND envelope_kind<>'rtx' AND status IN('pending','failed')",[channel])?;
        tx.execute("INSERT INTO channel_replica_readers(server_id,channel_id,pseudonym_id,device_id,token_epoch,updated_at) VALUES(?1,?2,?3,?4,?5,?6)",params![state.server_id,channel,reader.pseudonym_id,reader.device_id,reader.token_epoch,now()])?;
        tx.execute("DELETE FROM channel_reconciliation_records WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,input.operation_id])?;
        persist_result(&tx, state, channel, &result)?;
        tx.commit()?;
        Ok(result)
    })();
    let cleanup=conn.execute_batch("DROP TABLE IF EXISTS temp.reconcile_local;DROP TABLE IF EXISTS temp.reconcile_known;DROP TABLE IF EXISTS temp.reconcile_uploads;DROP TABLE IF EXISTS temp.reconcile_serving;");
    match attempt {
        Err(error) => Err(error),
        Ok(result) => {
            cleanup?;
            Ok(result)
        }
    }
}

pub async fn http_preview(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
) -> Result<Json<Preview>, Error> {
    let conn = state.pool.get().map_err(internal)?;
    let tx = annex_db::read_transaction(&conn)?;
    moderator(
        &tx,
        &state,
        &channel,
        &identity.pseudonym_id,
        identity.token_epoch,
    )?;
    if a::binding(&tx, &state, &channel).is_ok() {
        return Err(Error::Conflict("channel already has an authority".into()));
    }
    let value = preview(&tx, &state, &channel)?;
    tx.commit()?;
    Ok(Json(value))
}
pub async fn http_reconcile(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
    Json(request): Json<ReconcileRequest>,
) -> Result<Json<Outcome>, Error> {
    reconcile(state, &identity, &channel, request)
        .await
        .map(Json)
}
pub async fn http_list(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
) -> Result<Json<Value>, Error> {
    let conn = state.pool.get().map_err(internal)?;
    moderator(
        &conn,
        &state,
        &channel,
        &identity.pseudonym_id,
        identity.token_epoch,
    )?;
    let mut stmt=conn.prepare("SELECT peer_id,authority_identity,device_id,operation_id,snapshot_sha256 FROM channel_reconciliations WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3 ORDER BY created_at DESC LIMIT 64")?;
    let rows = stmt
        .query_map(
            params![state.server_id, channel, identity.pseudonym_id],
            |r| {
                Ok(ReconcileRequest {
                    peer_id: r.get(0)?,
                    authority_identity: r.get(1)?,
                    device_id: r.get(2)?,
                    operation_id: r.get(3)?,
                    expected_snapshot_sha256: r.get(4)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut operations = Vec::new();
    for request in rows {
        let result = saved(&conn, &state, &channel, &request.operation_id)?;
        operations.push(json!({"request":request,"result":result}));
    }
    Ok(Json(json!({"operations":operations})))
}
pub async fn http_cancel(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path((channel, id)): Path<(String, String)>,
) -> Result<Json<Outcome>, Error> {
    let _guard = replica::channel_guard(&state, &channel).await?;
    let mut conn = state.pool.get().map_err(internal)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    moderator(
        &tx,
        &state,
        &channel,
        &identity.pseudonym_id,
        identity.token_epoch,
    )?;
    let owner:String=tx.query_row("SELECT pseudonym_id FROM channel_reconciliations WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,id],|r|r.get(0)).optional()?.ok_or_else(||Error::NotFound("reconciliation operation not found".into()))?;
    if owner != identity.pseudonym_id {
        return Err(Error::Forbidden(
            "only the initiating moderator can cancel staging".into(),
        ));
    }
    let mut result = saved(&tx, &state, &channel, &id)?;
    if result.status == "complete" {
        return Err(Error::Conflict(
            "committed reconciliation cannot be cancelled".into(),
        ));
    }
    result.status = "cancelled".into();
    result.conflicts.clear();
    tx.execute("DELETE FROM channel_reconciliation_records WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,id])?;
    tx.execute("UPDATE channel_reconciliations SET join_receipt_ciphertext=NULL WHERE server_id=?1 AND channel_id=?2 AND operation_id=?3",params![state.server_id,channel,id])?;
    persist_result(&tx, &state, &channel, &result)?;
    tx.commit()?;
    Ok(Json(result))
}

/// Notifications/media cleanup survive a process exit after the adoption commit.
pub(crate) async fn drain_departures(state: Arc<AppState>) -> Result<(), Error> {
    let rows = {
        let conn = state.pool.get().map_err(internal)?;
        let mut stmt=conn.prepare("SELECT channel_id,pseudonym_id FROM channel_reconciliation_departures WHERE server_id=?1 ORDER BY channel_id,pseudonym_id LIMIT 32")?;
        let rows = stmt
            .query_map([state.server_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (channel, pid) in rows {
        // Replica Join/Leave use this same guard, including the final awaited
        // notifications/unsubscribe, so a fresh rejoin cannot be cleaned up.
        let _channel_guard = replica::channel_guard(&state, &channel).await?;
        let departed = super::ChannelService::new(state.clone())
            .end_reconciled_departure(&pid, &channel)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        if departed {
            let still_absent = {
                let conn = state.pool.get().map_err(internal)?;
                conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM channel_members WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3)",params![state.server_id,channel,pid],|r|r.get::<_,bool>(0))?
            };
            if still_absent {
                let frame=json!({"type":"channel_membership_changed","channelId":channel,"pseudonymId":pid,"member":false}).to_string();
                state.connection_manager.send(&pid, frame.clone()).await;
                state.connection_manager.broadcast(&channel, frame).await;
                state.connection_manager.unsubscribe(&channel, &pid).await;
            }
        }
        state.pool.get().map_err(internal)?.execute("DELETE FROM channel_reconciliation_departures WHERE server_id=?1 AND channel_id=?2 AND pseudonym_id=?3",params![state.server_id,channel,pid])?;
    }
    Ok(())
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    #[tokio::test(flavor = "current_thread")]
    async fn dropped_waiter_does_not_release_replay_guard_or_block_async_progress() {
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let guard = lock.clone().lock_owned().await;
        let (entered, observed) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::spawn(run_owned_writer(guard, move || {
            entered.send(()).unwrap();
            wait.recv().unwrap();
            Ok(())
        }));
        observed.await.unwrap();
        // This current-thread runtime still advances while the real blocking
        // writer is parked, and cancellation retains serialization until exit.
        task.abort();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(25),
            lock.clone().lock_owned()
        )
        .await
        .is_err());
        release.send(()).unwrap();
        let acquired = tokio::time::timeout(std::time::Duration::from_secs(2), lock.lock_owned())
            .await
            .unwrap();
        drop(acquired);
    }

    #[test]
    fn staged_expiry_scrubs_every_revision_before_sweep_and_scopes_operation_identity() {
        let conn = Connection::open_in_memory().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'s','S','{}')",
            [],
        )
        .unwrap();
        for (channel, created) in [("expired", now()), ("other", now()), ("old", now() - 90000)] {
            conn.execute("INSERT INTO channel_reconciliations(server_id,channel_id,operation_id,pseudonym_id,device_id,token_epoch,peer_id,authority_identity,authority_key_hex,authority_url,snapshot_sha256,created_at,updated_at) VALUES(1,?1,'same-id','owner','device',0,1,'authority','key','https://authority.test','hash',?2,?2)",params![channel,created]).unwrap();
            for (seq, expiry) in [
                (
                    1,
                    Some(if channel == "expired" {
                        "2000-01-01 00:00:00"
                    } else {
                        "2099-01-01 00:00:00"
                    }),
                ),
                (2, None),
            ] {
                conn.execute("INSERT INTO channel_reconciliation_records(server_id,channel_id,operation_id,event_seq,event_hash,header_json,message_id,record_ciphertext,body_record,expires_at) VALUES(1,?1,'same-id',?2,'hash','{}','message','opaque encrypted staged record',1,?3)",params![channel,seq,expiry]).unwrap();
            }
        }
        expire_staging_for_server(&conn, 1).unwrap();
        let count = |channel: &str| {
            conn.query_row("SELECT COUNT(*) FROM channel_reconciliation_records WHERE channel_id=?1 AND record_ciphertext IS NOT NULL",[channel],|r|r.get::<_,i64>(0)).unwrap()
        };
        assert_eq!(
            count("expired"),
            0,
            "old edit has no own expiry but must be purged with its containing message"
        );
        assert_eq!(count("other"),2,"same operation ID in another live channel must not be expired by a different operation");
        assert_eq!(count("old"), 0);
        assert_eq!(
            conn.query_row(
                "SELECT status FROM channel_reconciliations WHERE channel_id='old'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "cancelled"
        );
    }
}
