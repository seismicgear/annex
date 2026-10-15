//! Transactional adoption of retained local channel history. Imported records
//! state their provenance explicitly; they are not forged historical author
//! requests, and expired/deleted content is never reconstructed.
use super::channel_authority::{
    self as authority, AuthorityError as Error, AuthorityEvent, EventKind,
};
use crate::AppState;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// Fixed identifiers only. Each key record keeps its original ciphertext
// namespace; importing never re-seals or relabels an existing key.
pub(crate) const KEY_TABLES: &[(&str, &[&str])] = &[
    (
        "channel_key_authorizations",
        &["authorization_sha256", "receipt_json"],
    ),
    (
        "channel_key_epochs",
        &[
            "epoch",
            "key_id",
            "committed_revision",
            "committed_by_pseudonym",
            "committed_by_device",
            "created_at",
            "manifest_hash",
            "authority_namespace",
        ],
    ),
    (
        "channel_legacy_key_candidates",
        &["epoch", "key_id", "authority_namespace"],
    ),
    (
        "channel_device_key_wraps",
        &[
            "recipient_pseudonym_id",
            "recipient_device_id",
            "epoch",
            "key_id",
            "sender_pseudonym_id",
            "sender_device_id",
            "wrapped_key_b64",
            "created_at",
            "authorization_sha256",
        ],
    ),
    (
        "channel_legacy_device_wraps",
        &[
            "epoch",
            "key_id",
            "recipient_pseudonym_id",
            "recipient_device_id",
            "sender_pseudonym_id",
            "sender_device_id",
            "wrapped_key_b64",
            "authorization_sha256",
        ],
    ),
    (
        "channel_key_wraps",
        &[
            "recipient_pseudonym_id",
            "sender_pseudonym_id",
            "key_epoch",
            "wrapped_key_b64",
            "created_at",
            "authority_namespace",
        ],
    ),
];

pub fn append_retained_keys(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
) -> Result<(), Error> {
    // Past authorized recipients may have left the ordinary roster. Preserve
    // their certified directory records without granting current membership.
    let devices = {
        let mut stmt=tx.prepare("SELECT d.pseudonym_id,d.device_id,d.x25519_pub_hex,d.device_certificate,d.revoked_at,p.participant_type FROM member_device_keys d JOIN platform_identities p ON p.server_id=d.server_id AND p.pseudonym_id=d.pseudonym_id WHERE d.server_id=?1 AND (EXISTS(SELECT 1 FROM channel_device_key_wraps w WHERE w.server_id=d.server_id AND w.channel_id=?2 AND w.recipient_pseudonym_id=d.pseudonym_id AND w.recipient_device_id=d.device_id) OR EXISTS(SELECT 1 FROM channel_legacy_device_wraps w WHERE w.server_id=d.server_id AND w.channel_id=?2 AND w.recipient_pseudonym_id=d.pseudonym_id AND w.recipient_device_id=d.device_id)) ORDER BY d.pseudonym_id,d.device_id")?;
        let rows=stmt.query_map(params![state.server_id,channel],|r|Ok(json!({"pseudonym_id":r.get::<_,String>(0)?,"device_id":r.get::<_,String>(1)?,"x25519_pub_hex":r.get::<_,String>(2)?,"device_certificate":r.get::<_,Option<String>>(3)?,"revoked_at":r.get::<_,Option<String>>(4)?,"participant_type":r.get::<_,String>(5)?})))?.collect::<Result<Vec<_>,_>>()?;
        rows
    };
    for device in devices {
        authority::append_event(
            tx,
            state,
            channel,
            EventKind::DeviceHistoryImported,
            json!({"device":device}),
        )?;
    }
    let mut manifest_stmt=tx.prepare("SELECT event_json FROM channel_manifest_events WHERE server_id=?1 AND channel_id=?2 ORDER BY event_seq")?;
    let manifests =
        manifest_stmt.query_map(params![state.server_id, channel], |r| r.get::<_, String>(0))?;
    for raw in manifests {
        let signed: Value =
            serde_json::from_str(&raw?).map_err(|e| Error::Internal(e.to_string()))?;
        authority::append_event(
            tx,
            state,
            channel,
            EventKind::ManifestHistoryImported,
            json!({"manifest_event":signed}),
        )?;
    }
    for (table, columns) in KEY_TABLES {
        let sql = format!(
            "SELECT {} FROM {table} WHERE server_id=?1 AND channel_id=?2 ORDER BY {}",
            columns.join(","),
            columns.join(",")
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![state.server_id, channel], |r| {
            let mut values = serde_json::Map::new();
            for (i, column) in columns.iter().enumerate() {
                let value = match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(v) => json!(v),
                    rusqlite::types::ValueRef::Text(v) => Value::String(
                        std::str::from_utf8(v)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    i,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .into(),
                    ),
                    _ => {
                        return Err(rusqlite::Error::InvalidColumnType(
                            i,
                            column.to_string(),
                            r.get_ref(i)?.data_type(),
                        ))
                    }
                };
                values.insert(column.to_string(), value);
            }
            Ok(Value::Object(values))
        })?;
        for row in rows {
            authority::append_event(
                tx,
                state,
                channel,
                EventKind::KeyHistoryImported,
                json!({"table":table,"row":row?}),
            )?;
        }
    }
    Ok(())
}
pub fn materialize_device(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
) -> Result<(), Error> {
    let d = &event.payload["device"];
    let field = |name: &str| {
        d[name]
            .as_str()
            .ok_or_else(|| Error::BadRequest(format!("historical device missing {name}")))
    };
    let pid = field("pseudonym_id")?;
    let id = field("device_id")?;
    let key = field("x25519_pub_hex")?;
    let role = field("participant_type")?;
    if !crate::middleware::is_valid_pseudonym_format(pid)
        || !super::e2e_service::lower_hex(id)
        || !super::e2e_service::lower_hex(key)
        || super::e2e_service::device_id(key)? != id
        || !matches!(
            role,
            "HUMAN" | "AI_AGENT" | "COLLECTIVE" | "BRIDGE" | "SERVICE"
        )
    {
        return Err(Error::Forbidden(
            "invalid historical device identity".into(),
        ));
    }
    tx.execute("INSERT OR IGNORE INTO platform_identities(server_id,pseudonym_id,participant_type,active,can_voice,can_moderate,can_invite,can_federate,can_bridge) VALUES(?1,?2,?3,1,0,0,0,0,0)",params![state.server_id,pid,role])?;
    let revoked = d["revoked_at"].as_str();
    if let Some(raw) = d["device_certificate"].as_str() {
        let cert: crate::device_certificates::DeviceCertificate =
            serde_json::from_str(raw).map_err(|e| Error::Forbidden(e.to_string()))?;
        if cert.pseudonym_id != pid || cert.device_id != id || cert.x25519_pub_hex != key {
            return Err(Error::Forbidden(
                "historical certificate directory mismatch".into(),
            ));
        }
        authority::import_verified_certificate(tx, state, &cert, revoked)?;
    } else {
        let prior:Option<String>=tx.query_row("SELECT x25519_pub_hex FROM member_device_keys WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3",params![state.server_id,pid,id],|r|r.get(0)).optional()?;
        if prior.as_deref().is_some_and(|old| old != key) {
            return Err(Error::Forbidden("historical device key collision".into()));
        }
        tx.execute("INSERT OR IGNORE INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex,revoked_at) VALUES(?1,?2,?3,?4,?5)",params![state.server_id,pid,id,key,revoked])?;
        if let Some(at) = revoked {
            tx.execute("UPDATE member_device_keys SET revoked_at=COALESCE(revoked_at,?4) WHERE server_id=?1 AND pseudonym_id=?2 AND device_id=?3",params![state.server_id,pid,id,at])?;
        }
    }
    Ok(())
}
pub fn materialize_key(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
) -> Result<(), Error> {
    let table = event.payload["table"]
        .as_str()
        .ok_or_else(|| Error::BadRequest("historical key table missing".into()))?;
    let (_, columns) = KEY_TABLES
        .iter()
        .find(|(name, _)| *name == table)
        .ok_or_else(|| Error::Forbidden("unsupported historical key record".into()))?;
    let row = event.payload["row"]
        .as_object()
        .ok_or_else(|| Error::BadRequest("historical key row missing".into()))?;
    if row.len() != columns.len() || columns.iter().any(|name| !row.contains_key(*name)) {
        return Err(Error::Forbidden(
            "historical key fields differ from schema".into(),
        ));
    }
    if table == "channel_key_authorizations" {
        let receipt: super::channel_manifest::WrapReceipt = serde_json::from_str(
            row["receipt_json"]
                .as_str()
                .ok_or_else(|| Error::BadRequest("historical receipt is not text".into()))?,
        )
        .map_err(|e| Error::Forbidden(e.to_string()))?;
        let mut projection = None;
        let mut stmt=tx.prepare("SELECT event_json FROM channel_manifest_events WHERE server_id=?1 AND channel_id=?2 AND event_seq<=?3 ORDER BY event_seq")?;
        let events = stmt
            .query_map(
                params![
                    state.server_id,
                    event.header.channel_id,
                    receipt.manifest_seq
                ],
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let vkey = state
            .membership_vkey_v2
            .as_ref()
            .ok_or_else(|| Error::Forbidden("membership verifier unavailable".into()))?;
        for raw in events {
            let e = serde_json::from_str(&raw).map_err(|e| Error::Forbidden(e.to_string()))?;
            projection = Some(super::channel_manifest::verify_transition(
                projection.as_ref(),
                &e,
                vkey,
            )?);
        }
        let p = projection.ok_or_else(|| {
            Error::Forbidden("historical key authorization lacks manifest".into())
        })?;
        super::channel_manifest::verify_wrap_receipt(&p, &receipt)?;
        if receipt.channel_id != event.header.channel_id
            || receipt.authority_identity != event.header.authority_identity
            || row["authorization_sha256"] != receipt.authorization_sha256
        {
            return Err(Error::Forbidden(
                "historical key authorization context mismatch".into(),
            ));
        }
    }
    if matches!(
        table,
        "channel_device_key_wraps" | "channel_legacy_device_wraps"
    ) {
        if let Some(hash) = row["authorization_sha256"].as_str() {
            use base64::Engine;
            use sha2::Digest;
            let raw:String=tx.query_row("SELECT receipt_json FROM channel_key_authorizations WHERE server_id=?1 AND channel_id=?2 AND authorization_sha256=?3",params![state.server_id,event.header.channel_id,hash],|r|r.get(0))?;
            let receipt: super::channel_manifest::WrapReceipt =
                serde_json::from_str(&raw).map_err(|e| Error::Forbidden(e.to_string()))?;
            let encoded = row["wrapped_key_b64"]
                .as_str()
                .ok_or_else(|| Error::Forbidden("historical wrap is not text".into()))?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| Error::Forbidden("historical wrap is not base64".into()))?;
            let hash = hex::encode(sha2::Sha256::digest(&bytes));
            if base64::engine::general_purpose::STANDARD.encode(&bytes) != encoded
                || row["epoch"] != receipt.epoch
                || row["key_id"] != receipt.key_id
                || row["sender_pseudonym_id"] != receipt.sender_pseudonym_id
                || row["sender_device_id"] != receipt.sender_device_id
                || (table == "channel_legacy_device_wraps")
                    != (receipt.operation == "admit_legacy_devices")
                || !receipt.wrap_digests.iter().any(|(pid, device, _, digest)| {
                    row["recipient_pseudonym_id"] == *pid
                        && row["recipient_device_id"] == *device
                        && *digest == hash
                })
            {
                return Err(Error::Forbidden(
                    "historical wrap differs from signed authorization".into(),
                ));
            }
        }
    }
    let mut values = vec![
        rusqlite::types::Value::Integer(state.server_id),
        rusqlite::types::Value::Text(event.header.channel_id.clone()),
    ];
    for column in *columns {
        values.push(match &row[*column] {
            Value::Null => rusqlite::types::Value::Null,
            Value::String(s) => rusqlite::types::Value::Text(s.clone()),
            Value::Number(n) => rusqlite::types::Value::Integer(
                n.as_i64()
                    .ok_or_else(|| Error::Forbidden("invalid historical integer".into()))?,
            ),
            _ => return Err(Error::Forbidden("invalid historical key value".into())),
        });
    }
    let placeholders = (1..=values.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    tx.execute(
        &format!(
            "INSERT INTO {table}(server_id,channel_id,{}) VALUES({placeholders})",
            columns.join(",")
        ),
        rusqlite::params_from_iter(values),
    )?;
    Ok(())
}

pub fn materialize_manifest(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
) -> Result<(), Error> {
    let signed: super::channel_manifest::SignedManifestEvent =
        serde_json::from_value(event.payload["manifest_event"].clone())
            .map_err(|e| Error::Forbidden(e.to_string()))?;
    if signed.channel_id != event.header.channel_id
        || signed.authority_identity != event.header.authority_identity
    {
        return Err(Error::Forbidden(
            "historical manifest binding mismatch".into(),
        ));
    }
    let previous = super::channel_manifest::projection(tx, state, &event.header.channel_id)?;
    let vkey = state
        .membership_vkey_v2
        .as_ref()
        .ok_or_else(|| Error::Forbidden("membership verifier unavailable".into()))?;
    let next = super::channel_manifest::verify_transition(previous.as_ref(), &signed, vkey)?;
    // This is the verified past lineage. The genesis roster and final state
    // describe membership now; replaying past departures must not remove a
    // subsequently rejoined current member.
    super::channel_manifest::persist(tx, state, &event.header.channel_id, &signed, &next)?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportedMessage {
    pub provenance: String,
    pub message: annex_channels::Message,
    pub legacy_receipts: Vec<Value>,
    pub attachments: Vec<super::federated_upload_types::AttachmentDescriptor>,
}
pub fn append_retained_messages(
    tx: &Transaction<'_>,
    state: &AppState,
    channel: &str,
) -> Result<(u64, u64), Error> {
    let mut message_stmt=tx.prepare("SELECT message_id FROM messages WHERE server_id=?1 AND channel_id=?2 AND deleted_at IS NULL AND (expires_at IS NULL OR datetime(expires_at)>datetime('now')) ORDER BY id")?;
    let ids =
        message_stmt.query_map(params![state.server_id, channel], |r| r.get::<_, String>(0))?;
    let mut messages = 0;
    let mut edits = 0;
    for id in ids {
        let id = id?;
        let mut message =
            annex_channels::get_message(tx, &id).map_err(|e| Error::Internal(e.to_string()))?;
        state
            .message_cipher()
            .decrypt_in_place(&mut message.content)?;
        let receipts = {
            let mut stmt=tx.prepare("SELECT i.base_url,i.public_key,r.envelope_hash,r.envelope_created_at,r.delivery_mode FROM federation_message_receipts r JOIN instances i ON i.id=r.remote_instance_id WHERE r.message_id=?1 ORDER BY i.id")?;
            let rows=stmt.query_map([&id],|r|Ok(json!({"originating_server":r.get::<_,String>(0)?,"origin_public_key":r.get::<_,String>(1)?,"envelope_hash":r.get::<_,String>(2)?,"envelope_created_at":r.get::<_,String>(3)?,"delivery_mode":r.get::<_,String>(4)?})))?.collect::<Result<Vec<_>,_>>()?;
            rows
        };
        let imported = ImportedMessage {
            provenance: "retained_local_store".into(),
            message,
            legacy_receipts: receipts,
            attachments: super::federated_upload_service::descriptors_for_bootstrap_message(
                tx, state, channel, &id,
            )
            .map_err(authority::transfer_error)?,
        };
        // Message's ordinary HTTP shape omits the internal expiry. The signed
        // authority history must retain it so adoption cannot extend retention.
        let mut payload =
            serde_json::to_value(&imported).map_err(|e| Error::Internal(e.to_string()))?;
        payload["message"]["expires_at"] = json!(imported.message.expires_at);
        let event =
            authority::append_event(tx, state, channel, EventKind::HistoryImported, payload)?;
        authority::bind_accepted_attachments(tx, state, &event, &imported.attachments)?;
        authority::materialize_accepted_attachments(tx, state, &event, &imported.attachments)?;
        messages += 1;
        let mut history_stmt = tx.prepare(
            "SELECT old_content,edited_at FROM message_edits WHERE message_id=?1 ORDER BY id",
        )?;
        let history = history_stmt.query_map([&id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for edit in history {
            let (body, edited_at) = edit?;
            let body = state.message_cipher().decrypt(&body)?;
            authority::append_event(
                tx,
                state,
                channel,
                EventKind::HistoryEditImported,
                json!({"provenance":"retained_local_store","message":{"message_id":id},"old_content":body,"edited_at":edited_at}),
            )?;
            edits += 1;
        }
    }
    Ok((messages, edits))
}
pub fn materialize_message(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
) -> Result<(), Error> {
    let imported: ImportedMessage = serde_json::from_value(event.payload.clone())
        .map_err(|e| Error::BadRequest(e.to_string()))?;
    let message = imported.message;
    if imported.provenance != "retained_local_store"
        || message.channel_id != event.header.channel_id
        || message.deleted_at.is_some()
    {
        return Err(Error::Forbidden("invalid retained history import".into()));
    }
    chrono::NaiveDateTime::parse_from_str(&message.created_at, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| Error::Forbidden("invalid imported creation timestamp".into()))?;
    annex_channels::create_message(
        tx,
        &annex_channels::CreateMessageParams {
            channel_id: event.header.channel_id.clone(),
            message_id: message.message_id.clone(),
            sender_pseudonym: message.sender_pseudonym,
            content: state.message_cipher().encrypt(&message.content)?,
            reply_to_message_id: message.reply_to_message_id,
        },
    )
    .map_err(|e| Error::Conflict(format!("imported message collision: {e}")))?;
    tx.execute("UPDATE messages SET created_at=?2,edited_at=?3,expires_at=?4 WHERE server_id=?5 AND message_id=?1",params![message.message_id,message.created_at,message.edited_at,message.expires_at,state.server_id])?;
    authority::bind_accepted_attachments(tx, state, event, &imported.attachments)?;
    authority::materialize_accepted_attachments(tx, state, event, &imported.attachments)?;
    Ok(())
}

pub fn materialize_edit(
    tx: &Transaction<'_>,
    state: &AppState,
    event: &AuthorityEvent,
) -> Result<(), Error> {
    let value = &event.payload;
    let id = value["message"]["message_id"]
        .as_str()
        .ok_or_else(|| Error::BadRequest("history edit lacks message".into()))?;
    let body = value["old_content"]
        .as_str()
        .ok_or_else(|| Error::BadRequest("history edit lacks body".into()))?;
    let at = value["edited_at"]
        .as_str()
        .ok_or_else(|| Error::BadRequest("history edit lacks timestamp".into()))?;
    let message =
        annex_channels::get_message(tx, id).map_err(|e| Error::Conflict(e.to_string()))?;
    if value["provenance"] != "retained_local_store"
        || message.channel_id != event.header.channel_id
        || message.deleted_at.is_some()
    {
        return Err(Error::Forbidden("invalid retained edit import".into()));
    }
    let parse = |s: &str| {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .map_err(|_| Error::Forbidden("invalid retained edit time".into()))
    };
    let time = parse(at)?;
    if time < parse(&message.created_at)?
        || message
            .edited_at
            .as_deref()
            .map(parse)
            .transpose()?
            .is_none_or(|last| time > last)
    {
        return Err(Error::Forbidden(
            "retained edit is outside message history".into(),
        ));
    }
    tx.execute(
        "INSERT INTO message_edits(message_id,old_content,edited_at) VALUES(?1,?2,?3)",
        params![id, state.message_cipher().encrypt(body)?, at],
    )?;
    Ok(())
}
