//! Actual HTTP recovery between two running Annex routers. Admission rows are
//! fixtures here; cryptographic handshake/proof acceptance has its own suite.
mod common;

use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_federation::{FederatedEditEnvelope, FederatedMessageEnvelope};
use annex_identity::MerkleTree;
use annex_server::services::{
    federation_catchup::*,
    federation_service::{edit_signing_input, message_signing_input, FederationService},
};
use annex_server::{app, AppState};
use annex_types::ServerPolicy;
use ed25519_dalek::Signer;
use rusqlite::params;
use std::{net::SocketAddr, sync::Arc, time::Duration};

const CHANNEL: &str = "shared";
const COMMITMENT: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const OLD: &str = "2020-01-01T00:00:00Z";

struct Server {
    state: Arc<AppState>,
    task: tokio::task::JoinHandle<()>,
    directory: tempfile::TempDir,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.shutdown.cancel();
        self.task.abort();
    }
}

async fn server() -> Server {
    let directory = tempfile::tempdir().unwrap();
    let pool = create_pool(
        directory.path().join("annex.db").to_str().unwrap(),
        DbRuntimeSettings::default(),
    )
    .unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (slug,label,policy_json) VALUES ('test','Test',?1)",
            [serde_json::to_string(&ServerPolicy::default()).unwrap()],
        )
        .unwrap();
        conn.execute("INSERT INTO platform_identities (server_id,pseudonym_id,participant_type,active) VALUES (1,'author','HUMAN',1)", []).unwrap();
        conn.execute("INSERT INTO channels (server_id,channel_id,name,channel_type,federation_scope) VALUES (1,?1,'Shared','\"Text\"','\"Federated\"')", [CHANNEL]).unwrap();
        conn.execute("INSERT INTO channel_members (server_id,channel_id,pseudonym_id,role) VALUES (1,?1,'author','MEMBER')", [CHANNEL]).unwrap();
    }
    let mut state =
        common::build_app_state(pool, MerkleTree::new(20).unwrap(), ServerPolicy::default());
    state.federation_config.allow_private_peer_addresses = true;
    state.federation_config.outbox_interval_seconds = 1;
    state.federation_config.default_outbound_envelope_version = "v2".into();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    *state.public_url.write().unwrap() = format!("http://{}", listener.local_addr().unwrap());
    let router = app(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Server {
        state: Arc::new(state),
        task,
        directory,
    }
}

fn admit(local: &Server, remote: &Server) {
    let conn = local.state.pool.get().unwrap();
    conn.execute(
        "INSERT INTO instances (base_url,public_key,label,status) VALUES (?1,?2,'Peer','ACTIVE')",
        params![
            remote.state.get_public_url(),
            hex::encode(remote.state.signing_key.verifying_key().to_bytes())
        ],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    conn.execute("INSERT INTO federation_agreements (local_server_id,remote_instance_id,alignment_status,transfer_scope,agreement_json,active) VALUES (1,?1,'ALIGNED','REFLECTION_SUMMARIES_ONLY','{}',1)", [id]).unwrap();
    conn.execute("INSERT INTO platform_identities (server_id,pseudonym_id,participant_type,active) VALUES (1,'remote-author','HUMAN',1)", []).unwrap();
    conn.execute("INSERT INTO federated_identities (server_id,remote_instance_id,commitment_hex,pseudonym_id,vrp_topic) VALUES (1,?1,?2,'remote-author','annex:server:v1')", params![id,COMMITMENT]).unwrap();
    conn.execute("INSERT INTO channel_members (server_id,channel_id,pseudonym_id,role) VALUES (1,?1,'remote-author','MEMBER')", [CHANNEL]).unwrap();
}

async fn pair() -> (Server, Server) {
    let a = server().await;
    let b = server().await;
    admit(&a, &b);
    admit(&b, &a);
    (a, b)
}

#[tokio::test]
async fn authority_adoption_refuses_legacy_http_and_data_channel_mutations_atomically() {
    use annex_server::services::federation_delivery::receive;
    use annex_server::services::federation_service::{
        enqueue_edit_envelope, enqueue_redaction_envelope, redaction_signing_input,
    };
    let (origin, receiver) = pair().await;
    let client = reqwest::Client::new();
    let key = hex::encode(origin.state.signing_key.verifying_key().to_bytes());
    let mut original = enqueue(&origin, "before-authority", "preserved original");
    original.created_at = chrono::Utc::now().to_rfc3339();
    original.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(message_signing_input(&original).as_bytes())
            .to_bytes(),
    );
    let response = client
        .post(format!(
            "{}/api/federation/messages",
            receiver.state.get_public_url()
        ))
        .json(&original)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    {
        let conn = receiver.state.pool.get().unwrap();
        conn.execute("INSERT INTO channel_authorities(server_id,channel_id,authority_identity,authority_key_hex,authority_url,authority_peer_id,local_authority) VALUES(1,?1,?2,?3,?4,1,0)",
            params![CHANNEL, format!("annex-server:{}", hex::encode(<sha2::Sha256 as sha2::Digest>::digest(origin.state.signing_key.verifying_key().to_bytes()))), key, origin.state.get_public_url()]).unwrap();
    }
    for (id, version) in [
        ("legacy-v1-after", None),
        ("legacy-v2-after", Some("v2".into())),
    ] {
        let mut message = original.clone();
        message.message_id = id.into();
        message.envelope_version = version;
        message.signature = hex::encode(
            origin
                .state
                .signing_key
                .sign(message_signing_input(&message).as_bytes())
                .to_bytes(),
        );
        let response = client
            .post(format!(
                "{}/api/federation/messages",
                receiver.state.get_public_url()
            ))
            .json(&message)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("ordered authority protocol"));
        let refusal = receive(
            receiver.state.clone(),
            &key,
            "/api/federation/messages",
            &serde_json::to_string(&message).unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(refusal.status, 403);
        assert!(refusal.message.contains("ordered authority protocol"));
    }
    let mut edit = FederatedEditEnvelope {
        envelope_kind: "edit".into(),
        envelope_version: "v1".into(),
        edit_id: uuid::Uuid::new_v4().to_string(),
        message_id: original.message_id.clone(),
        channel_id: CHANNEL.into(),
        content: "unauthorized bypass".into(),
        originating_server: origin.state.get_public_url(),
        edited_by: "remote-author".into(),
        attestation_ref: original.attestation_ref.clone(),
        signature: String::new(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    edit.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(edit_signing_input(&edit).as_bytes())
            .to_bytes(),
    );
    let mut deletion = annex_federation::FederatedRedactionEnvelope {
        envelope_kind: "redaction".into(),
        envelope_version: "v1".into(),
        message_id: original.message_id.clone(),
        channel_id: CHANNEL.into(),
        originating_server: origin.state.get_public_url(),
        redacted_by: "remote-author".into(),
        redaction_reason: "deleted".into(),
        attestation_ref: original.attestation_ref.clone(),
        signature: String::new(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    deletion.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(redaction_signing_input(&deletion).as_bytes())
            .to_bytes(),
    );
    for (path, body) in [
        (
            "/api/federation/edits",
            serde_json::to_string(&edit).unwrap(),
        ),
        (
            "/api/federation/redactions",
            serde_json::to_string(&deletion).unwrap(),
        ),
    ] {
        let response = client
            .post(format!("{}{path}", receiver.state.get_public_url()))
            .header("content-type", "application/json")
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("ordered authority protocol"));
        let refusal = receive(receiver.state.clone(), &key, path, &body)
            .await
            .unwrap_err();
        assert_eq!(refusal.status, 403);
        assert!(refusal.message.contains("ordered authority protocol"));
    }
    assert_eq!(
        content(&receiver, &original.message_id).unwrap(),
        ("preserved original".into(), None)
    );
    let conn = receiver.state.pool.get().unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM federation_message_receipts",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM message_edits", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(enqueue_edit_envelope(
        &tx,
        &receiver.state,
        CHANNEL,
        &original.message_id,
        "remote-author",
        "no bypass"
    )
    .unwrap_err()
    .contains("ordered authority protocol"));
    assert!(enqueue_redaction_envelope(
        &tx,
        &receiver.state,
        CHANNEL,
        &original.message_id,
        "remote-author",
        "deleted"
    )
    .unwrap_err()
    .contains("ordered authority protocol"));
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM federation_outbox", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// Use the actual local persistence -> signed outbox -> HTTP receiver path.
/// Constructing a fixture envelope with an RFC3339 timestamp misses SQLite's
/// default creation format, which otherwise makes every real v2 send fail.
#[tokio::test]
async fn local_send_federates_sqlite_timestamp_through_the_live_outbox() {
    let (origin, receiver) = pair().await;
    // Production attestation derives the same pseudonym from the sender's
    // topic/nullifier. The generic catch-up fixture uses different readable
    // aliases because it authors its envelopes directly; an actual local edit
    // must retain the originating identity at the receiver as real proofs do.
    receiver
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federated_identities SET pseudonym_id='author' WHERE commitment_hex=?1",
            [COMMITMENT],
        )
        .unwrap();
    origin.state.pool.get().unwrap().execute(
        "INSERT INTO zk_nullifiers (topic,nullifier_hex,pseudonym_id,commitment_hex) VALUES ('annex:server:v1','test-nullifier','author',?1)",
        [COMMITMENT],
    ).unwrap();
    let (message, _, _) = annex_server::services::ChannelService::new(origin.state.clone())
        .send_message("author", CHANNEL, "actual local send".into(), None, None)
        .await
        .unwrap();
    assert!(
        message.created_at.contains(' '),
        "exercise SQLite's actual timestamp format"
    );
    annex_server::background::drain_outbox_batch(origin.state.clone(), 32)
        .await
        .unwrap();
    let conn = origin.state.pool.get().unwrap();
    let (status, error): (String, Option<String>) = conn
        .query_row(
            "SELECT status,last_error FROM federation_outbox WHERE source_message_id=?1",
            [&message.message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        status, "delivered",
        "receiver refused a production send: {error:?}"
    );
    assert_eq!(
        content(&receiver, &message.message_id).unwrap().0,
        "actual local send"
    );
    let mode: String = receiver
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT delivery_mode FROM federation_message_receipts WHERE message_id=?1",
            [&message.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mode, "live");

    drop(conn);
    let (edited, _) = annex_server::services::ChannelService::new(origin.state.clone())
        .edit_message(
            "author",
            CHANNEL,
            &message.message_id,
            "actual atomic revision",
        )
        .await
        .unwrap();
    assert_eq!(edited.content, "actual atomic revision");
    // No detached relay, sleep or polling: the obligation must exist before
    // the edit reports success, and the ordinary worker must deliver it.
    let edit_count: i64 = origin.state.pool.get().unwrap().query_row(
        "SELECT COUNT(*) FROM federation_outbox WHERE source_message_id=?1 AND message_id LIKE 'edit:%'",
        [&message.message_id], |r| r.get(0),
    ).unwrap();
    assert_eq!(edit_count, 1);
    annex_server::background::drain_outbox_batch(origin.state.clone(), 32)
        .await
        .unwrap();
    let (edit_status, edit_error): (String, Option<String>) = origin.state.pool.get().unwrap().query_row(
        "SELECT status,last_error FROM federation_outbox WHERE source_message_id=?1 AND message_id LIKE 'edit:%'",
        [&message.message_id], |r|Ok((r.get(0)?,r.get(1)?)),
    ).unwrap();
    assert_eq!(
        edit_status, "delivered",
        "actual local edit failed: {edit_error:?}"
    );
    assert_eq!(
        content(&receiver, &message.message_id).unwrap().0,
        "actual atomic revision"
    );
    let old_content: String = receiver
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT old_content FROM message_edits WHERE message_id=?1",
            [&message.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        receiver
            .state
            .message_cipher()
            .decrypt(&old_content)
            .unwrap(),
        "actual local send"
    );
    let (deleted, _) = annex_server::services::ChannelService::new(origin.state.clone())
        .delete_message("author", CHANNEL, &message.message_id)
        .await
        .unwrap();
    assert!(deleted.deleted_at.is_some());
    let queued:i64=origin.state.pool.get().unwrap().query_row("SELECT COUNT(*) FROM federation_outbox WHERE source_message_id=?1 AND message_id LIKE 'redaction:%'",[&message.message_id],|r|r.get(0)).unwrap();
    assert_eq!(
        queued, 1,
        "delete returns only after durable tombstone enqueue"
    );
    annex_server::background::drain_outbox_batch(origin.state.clone(), 32)
        .await
        .unwrap();
    let (body, deleted) = content(&receiver, &message.message_id).unwrap();
    assert!(body.is_empty() && deleted.is_some());
    let history: i64 = receiver
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM message_edits WHERE message_id=?1",
            [&message.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history, 0);
}

#[tokio::test]
async fn data_channel_dispatch_preserves_message_versions_edit_redaction_and_current_authority() {
    use annex_server::services::federation_delivery::receive;
    use annex_server::services::federation_service::redaction_signing_input;
    let (origin, receiver) = pair().await;
    let key = hex::encode(origin.state.signing_key.verifying_key().to_bytes());
    for (id, version) in [("dc-v1", None), ("dc-v2", Some("v2".to_string()))] {
        let mut envelope = enqueue(&origin, id, "initial content");
        envelope.envelope_version = version;
        envelope.created_at = chrono::Utc::now().to_rfc3339();
        envelope.signature = hex::encode(
            origin
                .state
                .signing_key
                .sign(message_signing_input(&envelope).as_bytes())
                .to_bytes(),
        );
        let body = serde_json::to_string(&envelope).unwrap();
        let ack = receive(
            receiver.state.clone(),
            &key,
            "/api/federation/messages",
            &body,
        )
        .await
        .unwrap();
        assert!(ack.contains("webrtc"));
        assert_eq!(content(&receiver, id).unwrap().0, "initial content");
        let mut forged = envelope.clone();
        forged.originating_server = "https://different-origin.invalid".into();
        let refusal = receive(
            receiver.state.clone(),
            &key,
            "/api/federation/messages",
            &serde_json::to_string(&forged).unwrap(),
        )
        .await
        .unwrap_err();
        assert_eq!(refusal.status, 403);
        assert!(refusal.message.contains("origin does not match"));
    }
    let mut edit = FederatedEditEnvelope {
        envelope_kind: "edit".into(),
        envelope_version: "v1".into(),
        edit_id: uuid::Uuid::new_v4().to_string(),
        message_id: "dc-v2".into(),
        channel_id: CHANNEL.into(),
        content: "committed DC edit".into(),
        originating_server: origin.state.get_public_url(),
        edited_by: "remote-author".into(),
        attestation_ref: format!("annex:server:v1:{COMMITMENT}"),
        signature: String::new(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    edit.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(edit_signing_input(&edit).as_bytes())
            .to_bytes(),
    );
    let stored_before: String = receiver
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT content FROM messages WHERE message_id='dc-v2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let damaged = "\u{1}ar1:invalid-base64!";
    receiver
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET content=?1 WHERE message_id='dc-v2'",
            [damaged],
        )
        .unwrap();
    assert!(receive(
        receiver.state.clone(),
        &key,
        "/api/federation/edits",
        &serde_json::to_string(&edit).unwrap()
    )
    .await
    .is_err());
    {
        let conn = receiver.state.pool.get().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT content FROM messages WHERE message_id='dc-v2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let history: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM message_edits WHERE message_id='dc-v2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let receipts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM federation_message_receipts WHERE message_id=?1",
                [format!("edit:{}", edit.edit_id)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, damaged);
        assert_eq!((history, receipts), (0, 0));
        conn.execute(
            "UPDATE messages SET content=?1 WHERE message_id='dc-v2'",
            [stored_before],
        )
        .unwrap();
    }
    receiver.state.pool.get().unwrap().execute_batch("CREATE TRIGGER refuse_dc_edit BEFORE INSERT ON message_edits BEGIN SELECT RAISE(ABORT,'storage rejected edit'); END;").unwrap();
    let body = serde_json::to_string(&edit).unwrap();
    assert!(
        receive(receiver.state.clone(), &key, "/api/federation/edits", &body)
            .await
            .is_err()
    );
    assert_eq!(content(&receiver, "dc-v2").unwrap().0, "initial content");
    receiver
        .state
        .pool
        .get()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_dc_edit;")
        .unwrap();
    receive(receiver.state.clone(), &key, "/api/federation/edits", &body)
        .await
        .unwrap();
    assert_eq!(content(&receiver, "dc-v2").unwrap().0, "committed DC edit");
    let mut redaction = annex_federation::FederatedRedactionEnvelope {
        envelope_kind: "redaction".into(),
        envelope_version: "v1".into(),
        message_id: "dc-v2".into(),
        channel_id: CHANNEL.into(),
        originating_server: origin.state.get_public_url(),
        redacted_by: "remote-author".into(),
        redaction_reason: "deleted".into(),
        attestation_ref: format!("annex:server:v1:{COMMITMENT}"),
        signature: String::new(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    redaction.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(redaction_signing_input(&redaction).as_bytes())
            .to_bytes(),
    );
    receive(
        receiver.state.clone(),
        &key,
        "/api/federation/redactions",
        &serde_json::to_string(&redaction).unwrap(),
    )
    .await
    .unwrap();
    let (body, deleted) = content(&receiver, "dc-v2").unwrap();
    assert!(body.is_empty() && deleted.is_some());
    let history: i64 = receiver
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM message_edits WHERE message_id='dc-v2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history, 0);
    receiver
        .state
        .pool
        .get()
        .unwrap()
        .execute("UPDATE federation_agreements SET active=0", [])
        .unwrap();
    let refusal = receive(
        receiver.state.clone(),
        &key,
        "/api/federation/edits",
        &serde_json::to_string(&edit).unwrap(),
    )
    .await
    .unwrap_err();
    assert_eq!(refusal.status, 403);
    assert!(refusal.message.contains("no active agreement"));
}

#[tokio::test]
async fn configured_data_channel_failure_never_falls_back_to_available_http_receiver() {
    let (origin, receiver) = pair().await;
    let transport = Arc::new(annex_federation::transport::FederationTransport::new(
        "origin",
        hex::encode(origin.state.signing_key.verifying_key().to_bytes()),
        origin.state.signing_key.clone(),
        annex_federation::signal::SignalClient::with_base_url("http://127.0.0.1:1").unwrap(),
        Arc::new(|_, _, _| Box::pin(async { Err("unexpected ingress".into()) })),
        Arc::new(|_| Err("configured data-channel peer refused".into())),
    ));
    assert!(origin.state.federation_transport.set(transport).is_ok());
    let mut envelope = enqueue(&origin, "dc-no-fallback", "must not use available HTTP");
    envelope.created_at = chrono::Utc::now().to_rfc3339();
    envelope.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(message_signing_input(&envelope).as_bytes())
            .to_bytes(),
    );
    let conn = origin.state.pool.get().unwrap();
    conn.execute("UPDATE federation_outbox SET envelope_json=?1,status='pending',attempts=0,next_retry_at=datetime('now') WHERE source_message_id='dc-no-fallback'",
        [origin.state.message_cipher().encrypt(&serde_json::to_string(&envelope).unwrap()).unwrap()]).unwrap();
    drop(conn);
    annex_server::background::drain_outbox_batch(origin.state.clone(), 32)
        .await
        .unwrap();
    assert!(content(&receiver, "dc-no-fallback").is_none());
    let (status,error):(String,String)=origin.state.pool.get().unwrap().query_row("SELECT status,last_error FROM federation_outbox WHERE source_message_id='dc-no-fallback'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(status, "pending");
    assert!(error.contains("configured data-channel peer refused"));
}

fn enqueue(source: &Server, id: &str, body: &str) -> FederatedMessageEnvelope {
    let conn = source.state.pool.get().unwrap();
    conn.execute("INSERT INTO messages (server_id,channel_id,message_id,sender_pseudonym,content,created_at) VALUES (1,?1,?2,'author',?3,?4)", params![CHANNEL,id,source.state.message_cipher().encrypt(body).unwrap(),OLD]).unwrap();
    let mut envelope = FederatedMessageEnvelope {
        envelope_version: Some("v2".into()),
        message_id: id.into(),
        channel_id: CHANNEL.into(),
        content: body.into(),
        sender_pseudonym: "author".into(),
        originating_server: source.state.get_public_url(),
        attestation_ref: format!("annex:server:v1:{COMMITMENT}"),
        signature: String::new(),
        created_at: OLD.into(),
    };
    envelope.signature = hex::encode(
        source
            .state
            .signing_key
            .sign(message_signing_input(&envelope).as_bytes())
            .to_bytes(),
    );
    enqueue_outbox(
        &conn,
        1,
        id,
        id,
        CHANNEL,
        &source
            .state
            .message_cipher()
            .encrypt(&serde_json::to_string(&envelope).unwrap())
            .unwrap(),
    )
    .unwrap();
    conn.execute(
        "UPDATE federation_outbox SET status='failed',attempts=12",
        [],
    )
    .unwrap();
    envelope
}

fn request(source: &Server, receiver: &Server) -> CatchupRequest {
    let mut request = CatchupRequest {
        requesting_server: receiver.state.get_public_url(),
        channel_id: CHANNEL.into(),
        after_seq: 0,
        max_envelopes: 64,
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    request.signature = hex::encode(
        receiver
            .state
            .signing_key
            .sign(&request_signing_input(&request))
            .to_bytes(),
    );
    assert_ne!(source.state.get_public_url(), request.requesting_server);
    request
}

#[tokio::test]
async fn historical_sqlite_timestamps_are_repaired_without_refreshing_age_or_blessing_tampering() {
    let (origin, receiver) = pair().await;
    let mut old = enqueue(&origin, "legacy-time", "historical body");
    old.created_at = "2020-01-01 00:00:00".into();
    old.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(message_signing_input(&old).as_bytes())
            .to_bytes(),
    );
    let legacy_stored = origin
        .state
        .message_cipher()
        .encrypt(&serde_json::to_string(&old).unwrap())
        .unwrap();
    origin
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federation_outbox SET envelope_json=?1 WHERE source_message_id='legacy-time'",
            [&legacy_stored],
        )
        .unwrap();
    backfill_outbox(&origin.state).unwrap();
    let (stored, status, attempts): (String,String,i64) = origin.state.pool.get().unwrap().query_row(
        "SELECT envelope_json,status,attempts FROM federation_outbox WHERE source_message_id='legacy-time'", [],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).unwrap();
    let repaired: FederatedMessageEnvelope =
        serde_json::from_str(&origin.state.message_cipher().decrypt(&stored).unwrap()).unwrap();
    assert_eq!(repaired.created_at, OLD);
    assert_eq!(status, "failed");
    assert_eq!(attempts, 12);
    assert!(
        FederationService::new(receiver.state.clone())
            .receive_federated_message(repaired)
            .await
            .is_err(),
        "repair must not make historical traffic freshly live"
    );
    assert_eq!(
        recover_channel(
            receiver.state.clone(),
            &origin.state.get_public_url(),
            CHANNEL
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        content(&receiver, "legacy-time").unwrap().0,
        "historical body"
    );

    old.content = "tampered without a valid origin signature".into();
    let tampered = origin
        .state
        .message_cipher()
        .encrypt(&serde_json::to_string(&old).unwrap())
        .unwrap();
    origin
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federation_outbox SET envelope_json=?1 WHERE source_message_id='legacy-time'",
            [&tampered],
        )
        .unwrap();
    assert!(
        backfill_outbox(&origin.state).is_err(),
        "migration must not re-sign tampered payloads"
    );
    let unchanged: String = origin
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT envelope_json FROM federation_outbox WHERE source_message_id='legacy-time'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, tampered);
}

fn content(server: &Server, id: &str) -> Option<(String, Option<String>)> {
    use rusqlite::OptionalExtension;
    server
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT content,deleted_at FROM messages WHERE message_id=?1",
            [id],
            |r| {
                Ok((
                    server
                        .state
                        .message_cipher()
                        .decrypt(&r.get::<_, String>(0)?)
                        .unwrap(),
                    r.get(1)?,
                ))
            },
        )
        .optional()
        .unwrap()
}

#[tokio::test]
async fn corrupted_outbox_is_retained_and_never_delivered_or_signed_for_catchup() {
    let (origin, receiver) = pair().await;
    enqueue(&origin, "corrupt-storage", "must not leave damaged storage");
    let corrupted = "\u{1}ar1:not-valid-base64!";
    origin.state.pool.get().unwrap().execute(
        "UPDATE federation_outbox SET envelope_json=?1,status='pending',attempts=0,next_retry_at=datetime('now') WHERE source_message_id='corrupt-storage'",
        [corrupted],
    ).unwrap();
    assert!(backfill_outbox(&origin.state).is_err());
    assert!(recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL
    )
    .await
    .is_err());
    assert!(content(&receiver, "corrupt-storage").is_none());

    annex_server::background::drain_outbox_batch(origin.state.clone(), 32)
        .await
        .unwrap();
    let (stored, status, attempts, error): (String, String, i64, String) = origin.state.pool.get().unwrap().query_row(
        "SELECT envelope_json,status,attempts,last_error FROM federation_outbox WHERE source_message_id='corrupt-storage'", [],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
    ).unwrap();
    assert_eq!(stored, corrupted);
    assert_eq!(status, "failed");
    assert_eq!(attempts, 1);
    assert_eq!(error, "stored envelope integrity check failed");
    assert!(content(&receiver, "corrupt-storage").is_none());
    assert_eq!(receiver.state.pool.get().unwrap().query_row(
        "SELECT COUNT(*) FROM federation_message_receipts WHERE message_id='corrupt-storage'", [], |row| row.get::<_, i64>(0),
    ).unwrap(), 0);
}

#[tokio::test]
async fn corrupt_later_legacy_outbox_rolls_back_earlier_metadata_backfill() {
    let (origin, _receiver) = pair().await;
    enqueue(&origin, "valid-earlier", "valid legacy message");
    enqueue(&origin, "corrupt-later", "damaged legacy message");
    let conn = origin.state.pool.get().unwrap();
    conn.execute(
        "UPDATE federation_outbox SET source_message_id=NULL,channel_id=NULL",
        [],
    )
    .unwrap();
    let corrupted = "\u{1}ar1:!!!";
    conn.execute(
        "UPDATE federation_outbox SET envelope_json=?1 WHERE message_id='corrupt-later'",
        [corrupted],
    )
    .unwrap();
    assert!(backfill_outbox(&origin.state).is_err());
    let untouched: i64 = conn.query_row(
        "SELECT COUNT(*) FROM federation_outbox WHERE source_message_id IS NULL AND channel_id IS NULL AND status='failed' AND attempts=12", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(untouched, 2, "backfill must be atomic across damaged rows");
    let retained: String = conn
        .query_row(
            "SELECT envelope_json FROM federation_outbox WHERE message_id='corrupt-later'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, corrupted);
}

#[tokio::test]
async fn expired_failed_envelopes_recover_over_http_and_restart_uses_persisted_cursor() {
    let (origin, receiver) = pair().await;
    let old = enqueue(&origin, "missed", "missed while offline");
    assert!(
        FederationService::new(receiver.state.clone())
            .receive_federated_message(old)
            .await
            .is_err(),
        "live freshness gate must stay strict"
    );
    assert_eq!(
        recover_channel(
            receiver.state.clone(),
            &origin.state.get_public_url(),
            CHANNEL
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        content(&receiver, "missed").unwrap().0,
        "missed while offline"
    );
    let conn = receiver.state.pool.get().unwrap();
    let mode: String = conn
        .query_row(
            "SELECT delivery_mode FROM federation_message_receipts WHERE message_id='missed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mode, "catchup");
    drop(conn);
    // Reopen the on-disk database with a fresh runtime/worker, not its old pool.
    let mut restarted = (*receiver.state).clone();
    restarted.pool = create_pool(
        receiver.directory.path().join("annex.db").to_str().unwrap(),
        DbRuntimeSettings::default(),
    )
    .unwrap();
    restarted.shutdown = Default::default();
    let restarted = Arc::new(restarted);
    assert_eq!(
        recover_channel(restarted.clone(), &origin.state.get_public_url(), CHANNEL)
            .await
            .unwrap(),
        0
    );
    enqueue(&origin, "after-restart", "automatic startup recovery");
    let worker = tokio::spawn(annex_server::background::start_federation_catchup_task(
        restarted.clone(),
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if content(&receiver, "after-restart").is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    restarted.shutdown.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn deleted_content_is_scrubbed_never_restored_and_later_tombstone_advances_cursor() {
    let (origin, receiver) = pair().await;
    let original = enqueue(&origin, "gone", "never restore this secret");
    let conn = origin.state.pool.get().unwrap();
    conn.execute(
        "UPDATE messages SET content='',deleted_at=datetime('now') WHERE message_id='gone'",
        [],
    )
    .unwrap();
    let retained:i64=conn.query_row("SELECT COUNT(*) FROM federation_outbox WHERE source_message_id='gone' AND envelope_json!=''",[],|r|r.get(0)).unwrap();
    assert_eq!(retained, 0);
    drop(conn);
    recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL,
    )
    .await
    .unwrap();
    assert!(content(&receiver, "gone").is_none());
    // Even an original message held in another delivery lane before deletion
    // cannot arrive later and resurrect it. Re-sign a fresh legitimate retry.
    let mut delayed = original;
    delayed.created_at = chrono::Utc::now().to_rfc3339();
    delayed.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(message_signing_input(&delayed).as_bytes())
            .to_bytes(),
    );
    FederationService::new(receiver.state.clone())
        .receive_federated_message(delayed)
        .await
        .unwrap();
    assert!(content(&receiver, "gone").is_none());
    enqueue(&origin, "delivered-first", "visible before delete");
    recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL,
    )
    .await
    .unwrap();
    assert!(content(&receiver, "delivered-first").unwrap().1.is_none());
    origin.state.pool.get().unwrap().execute("UPDATE messages SET content='',deleted_at=datetime('now') WHERE message_id='delivered-first'",[]).unwrap();
    recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL,
    )
    .await
    .unwrap();
    let after = content(&receiver, "delivered-first").unwrap();
    assert!(after.0.is_empty());
    assert!(after.1.is_some());
}

#[tokio::test]
async fn retention_reports_persistent_gap_and_channel_deletion_transmits_only_removals() {
    let (origin, receiver) = pair().await;
    enqueue(&origin, "pruned", "retention removes every durable payload");
    origin
        .state
        .pool
        .get()
        .unwrap()
        .execute("DELETE FROM messages WHERE message_id='pruned'", [])
        .unwrap();
    let response = serve(origin.state.clone(), request(&origin, &receiver))
        .await
        .unwrap();
    assert_eq!(response.status, "history_pruned");
    assert!(response.entries.iter().all(|e| e.envelope.is_none()));
    recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL,
    )
    .await
    .unwrap();
    assert!(content(&receiver, "pruned").is_none());
    let gap: Option<i64> = receiver
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT pruned_through_seq FROM federation_catchup_cursors",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(gap.is_some());
    enqueue(&origin, "channel-gone", "channel deletion copy");
    recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL,
    )
    .await
    .unwrap();
    annex_channels::delete_channel(&origin.state.pool.get().unwrap(), CHANNEL).unwrap();
    recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL,
    )
    .await
    .unwrap();
    assert!(content(&receiver, "channel-gone").unwrap().0.is_empty());
    let retained: i64 = origin
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM federation_outbox WHERE envelope_json!=''",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(retained, 0);
}

#[tokio::test]
async fn expired_message_is_not_delivered_or_recovered_before_the_retention_sweep() {
    let (origin, receiver) = pair().await;
    enqueue(
        &origin,
        "expired-unswept",
        "expired payload must never leave the outbox",
    );
    {
        let conn = origin.state.pool.get().unwrap();
        conn.execute(
            "UPDATE messages SET expires_at=datetime('now') WHERE message_id='expired-unswept'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE federation_outbox SET status='pending',attempts=0,next_retry_at=datetime('now') WHERE source_message_id='expired-unswept'",[]).unwrap();
    }
    assert!(
        content(&origin, "expired-unswept").is_some(),
        "physical sweep has not run"
    );
    let response = serve(origin.state.clone(), request(&origin, &receiver))
        .await
        .unwrap();
    assert_eq!(response.status, "history_pruned");
    assert_eq!(response.entries.len(), 1);
    assert!(response.entries[0].envelope.is_none());
    assert_eq!(
        response.entries[0].removed_reason.as_deref(),
        Some("retention")
    );
    annex_server::background::drain_outbox_batch(origin.state.clone(), 32)
        .await
        .unwrap();
    assert!(content(&receiver, "expired-unswept").is_none());
    let (body,status,reason):(String,String,String)=origin.state.pool.get().unwrap().query_row("SELECT envelope_json,status,removed_reason FROM federation_outbox WHERE source_message_id='expired-unswept'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(
        (body, status, reason),
        (String::new(), "delivered".into(), "retention".into())
    );
    recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL,
    )
    .await
    .unwrap();
    assert!(content(&receiver, "expired-unswept").is_none());
}

#[tokio::test]
async fn request_and_response_tampering_replay_membership_and_revocation_fail_closed() {
    let (origin, receiver) = pair().await;
    enqueue(&origin, "secured", "signed data");
    let request = request(&origin, &receiver);
    let response = serve(origin.state.clone(), request.clone()).await.unwrap();
    let key = hex::encode(origin.state.signing_key.verifying_key().to_bytes());
    verify_response(&request, &response, &origin.state.get_public_url(), &key).unwrap();
    let mut tampered = response.clone();
    tampered.entries[0].seq += 1;
    assert!(verify_response(&request, &tampered, &origin.state.get_public_url(), &key).is_err());
    let mut later = request.clone();
    later.nonce = uuid::Uuid::new_v4().to_string();
    assert!(verify_response(&later, &response, &origin.state.get_public_url(), &key).is_err());
    let mut forged = request.clone();
    forged.after_seq = 5;
    assert!(serve(origin.state.clone(), forged).await.is_err());
    origin
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "DELETE FROM channel_members WHERE pseudonym_id='remote-author'",
            [],
        )
        .unwrap();
    assert!(serve(origin.state.clone(), request.clone()).await.is_err());
    origin.state.pool.get().unwrap().execute("INSERT INTO channel_members (server_id,channel_id,pseudonym_id,role) VALUES (1,?1,'remote-author','MEMBER')",[CHANNEL]).unwrap();
    origin
        .state
        .pool
        .get()
        .unwrap()
        .execute("UPDATE federation_agreements SET active=0", [])
        .unwrap();
    assert!(serve(origin.state.clone(), request).await.is_err());
    receiver
        .state
        .pool
        .get()
        .unwrap()
        .execute("UPDATE federation_agreements SET active=0", [])
        .unwrap();
    assert!(recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL
    )
    .await
    .is_err());
    assert!(content(&receiver, "secured").is_none());
}

#[tokio::test]
async fn missed_edit_uses_same_verified_pipeline_and_catchup_is_idempotent() {
    let (origin, receiver) = pair().await;
    enqueue(&origin, "edited", "first version");
    let mut edit = FederatedEditEnvelope {
        envelope_kind: "edit".into(),
        envelope_version: "v1".into(),
        edit_id: uuid::Uuid::new_v4().to_string(),
        message_id: "edited".into(),
        channel_id: CHANNEL.into(),
        content: "changed while disconnected".into(),
        originating_server: origin.state.get_public_url(),
        edited_by: "remote-author".into(),
        attestation_ref: format!("annex:server:v1:{COMMITMENT}"),
        signature: String::new(),
        created_at: "2021-01-01T00:00:00Z".into(),
    };
    edit.signature = hex::encode(
        origin
            .state
            .signing_key
            .sign(edit_signing_input(&edit).as_bytes())
            .to_bytes(),
    );
    enqueue_outbox(
        &origin.state.pool.get().unwrap(),
        1,
        &format!("edit:{}", edit.edit_id),
        "edited",
        CHANNEL,
        &origin
            .state
            .message_cipher()
            .encrypt(&serde_json::to_string(&edit).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        recover_channel(
            receiver.state.clone(),
            &origin.state.get_public_url(),
            CHANNEL
        )
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        content(&receiver, "edited").unwrap().0,
        "changed while disconnected"
    );
    assert_eq!(
        recover_channel(
            receiver.state.clone(),
            &origin.state.get_public_url(),
            CHANNEL
        )
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn signed_removal_cannot_target_or_broadcast_another_channels_message() {
    let (origin, receiver) = pair().await;
    enqueue(
        &origin,
        "channel-secret",
        "keep this in the original channel",
    );
    recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        CHANNEL,
    )
    .await
    .unwrap();
    for server in [&origin, &receiver] {
        let conn = server.state.pool.get().unwrap();
        conn.execute("INSERT INTO channels (server_id,channel_id,name,channel_type,federation_scope) VALUES (1,'other','Other','\"Text\"','\"Federated\"')", []).unwrap();
        conn.execute("INSERT INTO channel_members (server_id,channel_id,pseudonym_id,role) VALUES (1,'other','remote-author','MEMBER')", []).unwrap();
    }
    // A hostile origin can sign any response. Seed its incorrect routing
    // metadata so its real handler produces such a correctly signed response.
    origin.state.pool.get().unwrap().execute("INSERT INTO federation_outbox (peer_instance_id,message_id,source_message_id,channel_id,envelope_json,removed_reason,status) VALUES (1,'malicious-removal','channel-secret','other','','deleted','delivered')", []).unwrap();
    let result = recover_channel(
        receiver.state.clone(),
        &origin.state.get_public_url(),
        "other",
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("original message channel"));
    assert_eq!(
        content(&receiver, "channel-secret").unwrap(),
        ("keep this in the original channel".into(), None)
    );
    let conn = receiver.state.pool.get().unwrap();
    let markers: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM federation_catchup_tombstones WHERE message_id='channel-secret'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        markers, 0,
        "a rejected removal must not commit a suppression marker"
    );
    let cursors: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM federation_catchup_cursors WHERE channel_id='other'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cursors, 0, "a rejected batch must not advance its cursor");
}

#[tokio::test]
async fn another_tenant_cannot_supply_channel_membership_or_active_identity() {
    let (origin, receiver) = pair().await;
    enqueue(
        &origin,
        "tenant-secret",
        "only the admitting tenant grants access",
    );
    let conn = origin.state.pool.get().unwrap();
    conn.execute(
        "INSERT INTO servers (id,slug,label,policy_json) VALUES (2,'other','Other','{}')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO platform_identities (server_id,pseudonym_id,participant_type,active) VALUES (2,'remote-author','HUMAN',1)", []).unwrap();
    conn.execute("UPDATE platform_identities SET active=0 WHERE server_id=1 AND pseudonym_id='remote-author'", []).unwrap();
    drop(conn);
    assert!(serve(origin.state.clone(), request(&origin, &receiver))
        .await
        .is_err());
    let conn = origin.state.pool.get().unwrap();
    conn.execute("UPDATE platform_identities SET active=1 WHERE server_id=1 AND pseudonym_id='remote-author'", []).unwrap();
    conn.execute(
        "UPDATE federated_identities SET server_id=2 WHERE pseudonym_id='remote-author'",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(serve(origin.state.clone(), request(&origin, &receiver))
        .await
        .is_err());
}
