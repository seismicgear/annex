//! Production signed discovery and atomic initialization components. Peer rows
//! are controlled fixtures; these tests do not claim a live handshake UI run.
mod common;
use annex_server::{
    middleware::IdentityContext,
    services::{authority_directory as directory, channel_authority as authority},
    AppState,
};
use axum::{extract::Path, Extension, Json};
use ed25519_dalek::Signer;
use std::sync::Arc;

fn fixture() -> (Arc<AppState>, ed25519_dalek::SigningKey) {
    let pool = annex_db::create_pool(":memory:", Default::default()).unwrap();
    let policy = annex_types::ServerPolicy {
        federation_enabled: true,
        ..Default::default()
    };
    let mut state = common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        policy.clone(),
    );
    state.signing_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0x55; 32]));
    *state.public_url.write().unwrap() = "https://authority.test".into();
    let peer = ed25519_dalek::SigningKey::from_bytes(&[0x66; 32]);
    {
        let conn = state.pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'test','Test',?1)",
            [serde_json::to_string(&policy).unwrap()],
        )
        .unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_moderate) VALUES(1,'operator','HUMAN',1)",[]).unwrap();
        conn.execute("INSERT INTO instances(id,base_url,public_key,label,status) VALUES(1,'https://peer.test',?1,'Peer','ACTIVE')",[hex::encode(peer.verifying_key().to_bytes())]).unwrap();
        conn.execute("INSERT INTO federation_agreements(local_server_id,remote_instance_id,alignment_status,transfer_scope,agreement_json,active) VALUES(1,1,'ALIGNED','REFLECTION_SUMMARIES_ONLY','{}',1)",[]).unwrap();
    }
    (Arc::new(state), peer)
}
fn channel(state: &AppState, id: &str) {
    let conn = state.pool.get().unwrap();
    annex_channels::create_channel(
        &conn,
        &annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: id.into(),
            name: id.into(),
            channel_type: annex_types::ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: annex_types::FederationScope::Federated,
        },
    )
    .unwrap();
    annex_channels::add_member(&conn, 1, id, "operator").unwrap();
}
fn request(
    state: &AppState,
    peer: &ed25519_dalek::SigningKey,
    after: Option<String>,
    limit: usize,
) -> directory::DirectoryRequest {
    let mut request = directory::DirectoryRequest {
        version: 1,
        originating_server: "https://peer.test".into(),
        requesting_identity: authority::server_identity(&hex::encode(
            peer.verifying_key().to_bytes(),
        ))
        .unwrap(),
        authority_identity: annex_server::device_certificates::server_identity(state),
        after_channel_id: after,
        limit,
        nonce: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        created_at: chrono::Utc::now().timestamp(),
        signature: String::new(),
    };
    request.signature = hex::encode(
        peer.sign(authority::canonical_json(&request.signing_value()).as_bytes())
            .to_bytes(),
    );
    request
}
async fn initialize(
    state: Arc<AppState>,
    expected: String,
) -> Result<serde_json::Value, authority::AuthorityError> {
    let identity =
        annex_identity::get_platform_identity(&state.pool.get().unwrap(), 1, "operator").unwrap();
    directory::http_initialize(
        Extension(state),
        Extension(IdentityContext(identity)),
        Path("retained".into()),
        Json(directory::InitializeRequest {
            expected_authority_identity: expected,
        }),
    )
    .await
    .map(|Json(value)| value)
}

#[tokio::test]
async fn signed_directory_pages_are_bound_to_nonce_peer_policy_and_cursor() {
    let (state, peer) = fixture();
    for i in 0..67 {
        channel(&state, &format!("c{i:03}"));
    }
    let first = request(&state, &peer, None, 50);
    let page = directory::receive(state.clone(), first.clone())
        .await
        .unwrap();
    state
        .signing_key
        .verifying_key()
        .verify_strict(
            authority::canonical_json(&page.signing_value()).as_bytes(),
            &ed25519_dalek::Signature::from_slice(&hex::decode(&page.signature).unwrap()).unwrap(),
        )
        .unwrap();
    assert_eq!(
        page.request_sha256,
        authority::digest(&first.signing_value())
    );
    assert_eq!(page.channels.len(), 50);
    assert_eq!(page.next_after_channel_id.as_deref(), Some("c049"));
    assert!(page
        .channels
        .iter()
        .all(|row| !row.initialized && row.head_seq == 0 && row.head_hash.is_empty()));
    assert!(
        directory::receive(state.clone(), first).await.is_err(),
        "nonce replay"
    );
    let second = directory::receive(
        state.clone(),
        request(&state, &peer, page.next_after_channel_id, 50),
    )
    .await
    .unwrap();
    assert_eq!(second.channels.len(), 17);
    assert_eq!(second.channels[0].channel_id, "c050");
    assert!(second.next_after_channel_id.is_none());
    let mut tampered = request(&state, &peer, None, 50);
    tampered.limit = 51;
    assert!(directory::receive(state.clone(), tampered).await.is_err());
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federation_agreements SET transfer_scope='NO_TRANSFER'",
            [],
        )
        .unwrap();
    assert!(
        directory::receive(state.clone(), request(&state, &peer, None, 50))
            .await
            .is_err()
    );
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federation_agreements SET transfer_scope='REFLECTION_SUMMARIES_ONLY'",
            [],
        )
        .unwrap();
    state.pool.get().unwrap().execute("UPDATE servers SET policy_json=json_set(policy_json,'$.federation_enabled',json('false'))",[]).unwrap();
    assert!(
        directory::receive(state.clone(), request(&state, &peer, None, 50))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn initialize_checks_confirmed_fingerprint_and_preserves_history_namespaces() {
    let (state, _) = fixture();
    channel(&state, "retained");
    {
        let conn = state.pool.get().unwrap();
        annex_channels::create_message(
            &conn,
            &annex_channels::CreateMessageParams {
                channel_id: "retained".into(),
                message_id: "before".into(),
                sender_pseudonym: "operator".into(),
                content: state
                    .message_cipher()
                    .encrypt("existing current body")
                    .unwrap(),
                reply_to_message_id: None,
            },
        )
        .unwrap();
        conn.execute("UPDATE messages SET created_at='2020-01-01 00:00:00',edited_at='2021-01-01 00:00:00' WHERE message_id='before'",[]).unwrap();
        conn.execute("INSERT INTO message_edits(message_id,old_content,edited_at) VALUES('before',?1,'2021-01-01 00:00:00')",[state.message_cipher().encrypt("existing old body").unwrap()]).unwrap();
        // This is a lower-level migration fixture of a retained pre-manifest
        // epoch, not a newly authorized commitment or a decryption proof.
        conn.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device,authority_namespace) VALUES(1,'retained',1,?1,0,'operator','legacy-device','http://old-authority:3000')",["ab".repeat(32)]).unwrap();
    }
    assert!(
        initialize(state.clone(), format!("annex-server:{}", "00".repeat(32)))
            .await
            .is_err()
    );
    assert!(authority::binding(&state.pool.get().unwrap(), &state, "retained").is_err());
    let response = initialize(
        state.clone(),
        annex_server::device_certificates::server_identity(&state),
    )
    .await
    .unwrap();
    assert_eq!(response["imported_messages"], 1);
    assert_eq!(response["imported_edits"], 1);
    let conn = state.pool.get().unwrap();
    let namespace: String = conn
        .query_row(
            "SELECT authority_namespace FROM channel_key_epochs WHERE channel_id='retained'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(namespace, "http://old-authority:3000");
    let count:i64=conn.query_row("SELECT COUNT(*) FROM channel_authority_events WHERE channel_id='retained' AND event_kind=json_quote('history_imported')",[],|r|r.get(0)).unwrap();
    assert_eq!(count, 1);
    drop(conn);
    let repeat = initialize(
        state.clone(),
        annex_server::device_certificates::server_identity(&state),
    )
    .await
    .unwrap();
    assert_eq!(response, repeat);
}

#[tokio::test]
async fn corrupt_retained_body_rolls_back_authority_pin_and_all_imports() {
    let (state, _) = fixture();
    channel(&state, "retained");
    {
        let conn = state.pool.get().unwrap();
        for (id, body) in [
            ("good", state.message_cipher().encrypt("good").unwrap()),
            (
                "bad",
                format!("{}!", state.message_cipher().encrypt("bad").unwrap()),
            ),
        ] {
            annex_channels::create_message(
                &conn,
                &annex_channels::CreateMessageParams {
                    channel_id: "retained".into(),
                    message_id: id.into(),
                    sender_pseudonym: "operator".into(),
                    content: body,
                    reply_to_message_id: None,
                },
            )
            .unwrap();
        }
    }
    assert!(initialize(
        state.clone(),
        annex_server::device_certificates::server_identity(&state)
    )
    .await
    .is_err());
    let conn = state.pool.get().unwrap();
    assert!(authority::binding(&conn, &state, "retained").is_err());
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM channel_authority_events", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE channel_id='retained'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}
