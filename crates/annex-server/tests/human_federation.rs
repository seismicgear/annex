//! Cryptographic/transactional components with genuinely generated membership
//! proofs and explicitly public test signing seeds. Peer rows are fixtures;
//! this suite is not a claim of a live federation handshake.
mod common;
use annex_server::{
    device_certificates::{self, DeviceCertificate},
    services::{channel_authority as authority, human_federation as human},
    AppState,
};
use ed25519_dalek::Signer;
use rusqlite::{params, TransactionBehavior};
use serde_json::Value;
use std::sync::Arc;

struct Pair {
    origin: Arc<AppState>,
    authority: Arc<AppState>,
    certificate: DeviceCertificate,
    peer: i64,
}

struct ReconcilePair {
    pair: Pair,
    peer: i64,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for ReconcilePair {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl ReconcilePair {
    async fn new(messages: usize, local_body: &str) -> Self {
        use axum::{routing::post, Router};
        let p = Pair::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        *p.authority.public_url.write().unwrap() = url.clone();
        {
            let conn = p.origin.pool.get().unwrap();
            conn.execute(
                "UPDATE platform_identities SET can_moderate=1 WHERE pseudonym_id=?1",
                [&p.certificate.pseudonym_id],
            )
            .unwrap();
            conn.execute("INSERT INTO channels(server_id,channel_id,name,channel_type,federation_scope) VALUES(1,'shared','Existing replica','\"Text\"','\"Federated\"')",[]).unwrap();
            annex_channels::add_member(&conn, 1, "shared", &p.certificate.pseudonym_id).unwrap();
        }
        for (state, body) in [(&p.origin, local_body), (&p.authority, "accepted revision")] {
            let conn = state.pool.get().unwrap();
            conn.execute("INSERT OR IGNORE INTO platform_identities(server_id,pseudonym_id,participant_type,can_moderate) VALUES(1,'historical-author','HUMAN',1)",[]).unwrap();
            annex_channels::add_member(&conn, 1, "shared", "historical-author").unwrap();
            for index in 0..messages {
                let id = format!("retained-{index:04}");
                annex_channels::create_message(
                    &conn,
                    &annex_channels::CreateMessageParams {
                        channel_id: "shared".into(),
                        message_id: id.clone(),
                        sender_pseudonym: "historical-author".into(),
                        content: state.message_cipher().encrypt(body).unwrap(),
                        reply_to_message_id: None,
                    },
                )
                .unwrap();
                conn.execute("UPDATE messages SET created_at='2025-01-01 00:00:00',edited_at='2025-01-01 00:01:00',expires_at='2099-01-01 00:00:00' WHERE message_id=?1",[&id]).unwrap();
                conn.execute("INSERT INTO message_edits(message_id,old_content,edited_at) VALUES(?1,?2,'2025-01-01 00:01:00')",params![id,state.message_cipher().encrypt("original revision").unwrap()]).unwrap();
            }
            // Explicit lower-level historical metadata: verifies immutable URL
            // namespace retention, not a claim that a synthetic CEK decrypts.
            conn.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device,created_at,authority_namespace) VALUES(1,'shared',1,?1,0,'historical-author','legacy','2025-01-01 00:00:00','http://original-key-namespace:3000')",["ab".repeat(32)]).unwrap();
        }
        {
            let mut conn = p.authority.pool.get().unwrap();
            conn.execute("UPDATE channel_authorities SET authority_url=?1", [&url])
                .unwrap();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            authority::initialize_local(&tx, &p.authority, "shared", "historical-author").unwrap();
            tx.commit().unwrap();
        }
        let peer = {
            let conn = p.origin.pool.get().unwrap();
            conn.execute("INSERT INTO instances(base_url,public_key,label,status) VALUES(?1,?2,'Authority','ACTIVE')",params![url,hex::encode(p.authority.signing_key.verifying_key().to_bytes())]).unwrap();
            let peer = conn.last_insert_rowid();
            conn.execute("INSERT INTO federation_agreements(local_server_id,remote_instance_id,alignment_status,transfer_scope,agreement_json,active) VALUES(1,?1,'ALIGNED','REFLECTION_SUMMARIES_ONLY','{}',1)",[peer]).unwrap();
            peer
        };
        let router = Router::new()
            .route("/api/federation/human-membership", post(human::http_attest))
            .route(
                "/api/federation/channel-authority/operations",
                post(authority::http_operation),
            )
            .route(
                "/api/federation/channel-authority/catch-up",
                post(authority::http_catchup),
            )
            .layer(axum::Extension(p.authority.clone()));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            pair: p,
            peer,
            server,
        }
    }
    fn identity(&self) -> annex_identity::PlatformIdentity {
        annex_identity::get_platform_identity(
            &self.pair.origin.pool.get().unwrap(),
            1,
            &self.pair.certificate.pseudonym_id,
        )
        .unwrap()
    }
    fn input(&self) -> annex_server::services::authority_reconciliation::ReconcileRequest {
        use annex_server::services::authority_reconciliation as reconcile;
        let preview = reconcile::preview(
            &self.pair.origin.pool.get().unwrap(),
            &self.pair.origin,
            "shared",
        )
        .unwrap();
        reconcile::ReconcileRequest {
            peer_id: self.peer,
            authority_identity: device_certificates::server_identity(&self.pair.authority),
            device_id: self.pair.certificate.device_id.clone(),
            operation_id: uuid::Uuid::new_v4().to_string(),
            expected_snapshot_sha256: preview.snapshot_sha256,
        }
    }
    async fn run(
        &self,
        input: annex_server::services::authority_reconciliation::ReconcileRequest,
    ) -> Result<annex_server::services::authority_reconciliation::Outcome, authority::AuthorityError>
    {
        annex_server::services::authority_reconciliation::reconcile(
            self.pair.origin.clone(),
            &self.identity(),
            "shared",
            input,
        )
        .await
    }
}

#[tokio::test]
async fn actual_http_populated_reconciliation_preserves_revisions_namespaces_and_is_idempotent() {
    let p = ReconcilePair::new(3, "accepted revision").await;
    let input = p.input();
    let result = p.run(input.clone()).await.unwrap();
    assert_eq!(result.status, "complete");
    assert!(result.remote_join_accepted);
    assert_eq!(result.retained_messages, 3);
    assert_eq!(result.removed_messages, 0);
    let conn = p.pair.origin.pool.get().unwrap();
    let message = annex_channels::get_message(&conn, "retained-0000").unwrap();
    assert_eq!(
        p.pair
            .origin
            .message_cipher()
            .decrypt(&message.content)
            .unwrap(),
        "accepted revision"
    );
    assert_eq!(message.expires_at.as_deref(), Some("2099-01-01 00:00:00"));
    let (body, at): (String, String) = conn
        .query_row(
            "SELECT old_content,edited_at FROM message_edits WHERE message_id='retained-0000'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        p.pair.origin.message_cipher().decrypt(&body).unwrap(),
        "original revision"
    );
    assert_eq!(at, "2025-01-01 00:01:00");
    assert_eq!(conn.query_row("SELECT authority_namespace FROM channel_key_epochs WHERE channel_id='shared' AND epoch=1",[],|r|r.get::<_,String>(0)).unwrap(),"http://original-key-namespace:3000");
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM channel_reconciliation_guards",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM channel_reconciliation_records",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(conn);
    let repeated = p.run(input).await.unwrap();
    assert_eq!(
        serde_json::to_value(&repeated).unwrap(),
        serde_json::to_value(&result).unwrap()
    );
}

#[tokio::test]
async fn actual_http_reconciliation_conflict_preserves_local_body_edit_outbox_and_explicit_cancel()
{
    use annex_server::{
        middleware::IdentityContext, services::authority_reconciliation as reconcile,
    };
    use axum::{extract::Path, Extension};
    let p = ReconcilePair::new(1, "unacknowledged local revision").await;
    {
        let conn = p.pair.origin.pool.get().unwrap();
        conn.execute("INSERT INTO federation_outbox(peer_instance_id,message_id,source_message_id,channel_id,envelope_json,status) VALUES(?1,'pending-local','retained-0000','shared',?2,'pending')",params![p.peer,p.pair.origin.message_cipher().encrypt("{\"body\":\"pending local revision\"}").unwrap()]).unwrap();
    }
    let input = p.input();
    let result = p.run(input.clone()).await.unwrap();
    assert_eq!(result.status, "conflict");
    assert!(result.remote_join_accepted);
    assert!(result
        .conflicts
        .iter()
        .any(|c| c.kind == "message_body" && c.id == "retained-0000"));
    let conn = p.pair.origin.pool.get().unwrap();
    assert_eq!(
        reconcile::preview(&conn, &p.pair.origin, "shared")
            .unwrap()
            .snapshot_sha256,
        input.expected_snapshot_sha256
    );
    assert!(authority::binding(&conn, &p.pair.origin, "shared").is_err());
    let (status, body): (String, String) = conn
        .query_row(
            "SELECT status,envelope_json FROM federation_outbox WHERE message_id='pending-local'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(
        p.pair.origin.message_cipher().decrypt(&body).unwrap(),
        "{\"body\":\"pending local revision\"}"
    );
    drop(conn);
    let cancelled = reconcile::http_cancel(
        Extension(p.pair.origin.clone()),
        Extension(IdentityContext(p.identity())),
        Path(("shared".into(), input.operation_id.clone())),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(cancelled.status, "cancelled");
    assert!(cancelled.remote_join_accepted);
    assert_eq!(p.run(input).await.unwrap().status, "cancelled");
    assert_eq!(
        p.pair
            .origin
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM channel_reconciliation_records",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn actual_http_reconciliation_resumes_durable_pages_and_rejects_snapshot_tamper_and_revocation(
) {
    let p = ReconcilePair::new(540, "accepted revision").await;
    let input = p.input();
    let first = p.run(input.clone()).await.unwrap();
    assert_eq!(first.status, "pending");
    assert!(first.after_seq > 0 && first.after_seq < first.head_seq);
    // No partially adopted projection is exposed while bounded paging pauses.
    assert!(
        authority::binding(&p.pair.origin.pool.get().unwrap(), &p.pair.origin, "shared").is_err()
    );
    let result = p.run(input.clone()).await.unwrap();
    assert_eq!(result.status, "complete");
    assert_eq!(result.retained_messages, 540);

    let p = ReconcilePair::new(540, "accepted revision").await;
    let input = p.input();
    assert_eq!(p.run(input.clone()).await.unwrap().status, "pending");
    p.pair
        .origin
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET content=?1 WHERE message_id='retained-0000'",
            [p.pair
                .origin
                .message_cipher()
                .encrypt("concurrent local change")
                .unwrap()],
        )
        .unwrap();
    let result = p.run(input).await.unwrap();
    assert_eq!(result.status, "conflict");
    assert!(result
        .conflicts
        .iter()
        .any(|c| c.kind == "local_snapshot_changed"));
    assert!(
        authority::binding(&p.pair.origin.pool.get().unwrap(), &p.pair.origin, "shared").is_err()
    );

    let p = ReconcilePair::new(540, "accepted revision").await;
    let input = p.input();
    assert_eq!(p.run(input.clone()).await.unwrap().status, "pending");
    p.pair
        .origin
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE channel_reconciliation_records SET record_ciphertext=?1 WHERE event_seq=1",
            ["\u{1}ar1:corrupt-authenticated-storage"],
        )
        .unwrap();
    assert!(p.run(input.clone()).await.is_err());
    assert!(
        authority::binding(&p.pair.origin.pool.get().unwrap(), &p.pair.origin, "shared").is_err()
    );
    p.pair
        .origin
        .pool
        .get()
        .unwrap()
        .execute("UPDATE federation_agreements SET active=0", [])
        .unwrap();
    assert!(matches!(
        p.run(input).await,
        Err(authority::AuthorityError::Forbidden(_))
    ));
}
impl Pair {
    fn new() -> Self {
        let vectors: Value =
            serde_json::from_str(include_str!("fixtures/device-epoch-public.json")).unwrap();
        let certificate: DeviceCertificate =
            serde_json::from_value(vectors["entries"][0]["certificate"].clone()).unwrap();
        let state = |seed: u8, url: &str| {
            let pool = annex_db::create_pool(":memory:", Default::default()).unwrap();
            let policy = annex_types::ServerPolicy {
                federation_enabled: true,
                ..Default::default()
            };
            {
                let conn = pool.get().unwrap();
                annex_db::run_migrations(&conn).unwrap();
                conn.execute(
                    "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'test','Test',?1)",
                    [serde_json::to_string(&policy).unwrap()],
                )
                .unwrap();
            }
            let mut state =
                common::build_app_state(pool, annex_identity::MerkleTree::new(20).unwrap(), policy);
            state.signing_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&[seed; 32]));
            state.federation_config.allow_private_peer_addresses = true;
            state.membership_vkey_v2 = Some(Arc::new(
                annex_identity::zk::parse_verification_key(&vectors["vkey"].to_string()).unwrap(),
            ));
            *state.public_url.write().unwrap() = url.into();
            Arc::new(state)
        };
        let origin = state(0x55, "https://origin.test");
        let authority = state(0x66, "https://authority.test");
        {
            let conn = origin.pool.get().unwrap();
            let cert = &certificate;
            conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type) VALUES(1,?1,'HUMAN')",[&cert.pseudonym_id]).unwrap();
            conn.execute("INSERT INTO zk_nullifiers(topic,nullifier_hex,pseudonym_id,commitment_hex) VALUES(?1,?2,?3,?4)",params![cert.topic,cert.nullifier_hex,cert.pseudonym_id,cert.commitment_hex]).unwrap();
            conn.execute(
                "INSERT INTO vrp_roots(root_hex) VALUES(?1)",
                [&cert.root_hex],
            )
            .unwrap();
            conn.execute("INSERT INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex,device_certificate,certificate_sha256) VALUES(1,?1,?2,?3,?4,?5)",params![cert.pseudonym_id,cert.device_id,cert.x25519_pub_hex,serde_json::to_string(cert).unwrap(),device_certificates::certificate_digest(cert)]).unwrap();
        }
        let peer = {
            let mut conn = authority.pool.get().unwrap();
            conn.execute(
                "INSERT INTO instances(base_url,public_key,label,status) VALUES(?1,?2,'Public test peer','ACTIVE')",
                params![
                    origin.get_public_url(),
                    hex::encode(origin.signing_key.verifying_key().to_bytes())
                ],
            )
            .unwrap();
            let peer = conn.last_insert_rowid();
            conn.execute("INSERT INTO federation_agreements(local_server_id,remote_instance_id,alignment_status,transfer_scope,agreement_json,active) VALUES(1,?1,'ALIGNED','REFLECTION_SUMMARIES_ONLY','{}',1)",[peer]).unwrap();
            conn.execute("INSERT INTO channels(server_id,channel_id,name,channel_type,federation_scope) VALUES(1,'shared','Shared','\"Text\"','\"Federated\"')",[]).unwrap();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            authority::register_local(&tx, &authority, "shared").unwrap();
            tx.commit().unwrap();
            peer
        };
        Self {
            origin,
            authority,
            certificate,
            peer,
        }
    }
    fn request(&self) -> human::HumanMembership {
        let conn = self.origin.pool.get().unwrap();
        human::build(
            &conn,
            &self.origin,
            &self.certificate.pseudonym_id,
            0,
            &self.certificate.device_id,
            "shared",
            &device_certificates::server_identity(&self.authority),
        )
        .unwrap()
    }
    fn sign(&self, request: &mut human::HumanMembership) {
        request.signature = hex::encode(
            self.origin
                .signing_key
                .sign(authority::canonical_json(&request.signing_value()).as_bytes())
                .to_bytes(),
        );
    }
    fn accept(
        &self,
        request: &human::HumanMembership,
    ) -> Result<human::MembershipReceipt, authority::AuthorityError> {
        let mut conn = self.authority.pool.get().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = human::accept(&tx, &self.authority, request)?;
        tx.commit()?;
        Ok(result)
    }
    fn count(&self, table: &str) -> i64 {
        self.authority
            .pool
            .get()
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}

/// Real HTTP sockets and production signed handlers; identity/peer admission is
/// seeded explicitly above, so this is not an enrollment/handshake UI receipt.
#[tokio::test]
async fn actual_http_replica_join_message_edit_delete_and_revocation_converge() {
    use annex_server::services::channel_replication as replica;
    use axum::{routing::post, Router};
    use serde_json::json;
    let p = Pair::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    *p.authority.public_url.write().unwrap() = url.clone();
    {
        let mut conn = p.authority.pool.get().unwrap();
        conn.execute("UPDATE channel_authorities SET authority_url=?1", [&url])
            .unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_moderate) VALUES(1,'local-operator','HUMAN',1)",[]).unwrap();
        annex_channels::add_member(&conn, 1, "shared", "local-operator").unwrap();
        for (id, body) in [
            ("retained-before-adoption", "retained edited text"),
            ("expired-before-adoption", "expired private text"),
            ("deleted-before-adoption", "deleted private text"),
        ] {
            annex_channels::create_message(
                &conn,
                &annex_channels::CreateMessageParams {
                    channel_id: "shared".into(),
                    message_id: id.into(),
                    sender_pseudonym: "local-operator".into(),
                    content: p.authority.message_cipher().encrypt(body).unwrap(),
                    reply_to_message_id: None,
                },
            )
            .unwrap();
        }
        conn.execute("UPDATE messages SET created_at='2020-01-01 00:00:00',edited_at='2021-01-01 00:00:00' WHERE message_id='retained-before-adoption'",[]).unwrap();
        conn.execute("INSERT INTO message_edits(message_id,old_content,edited_at) VALUES('retained-before-adoption',?1,'2021-01-01 00:00:00')",[p.authority.message_cipher().encrypt("retained original text").unwrap()]).unwrap();
        conn.execute("UPDATE messages SET expires_at='2020-01-01 00:00:00' WHERE message_id='expired-before-adoption'",[]).unwrap();
        conn.execute("UPDATE messages SET deleted_at=datetime('now'),content='' WHERE message_id='deleted-before-adoption'",[]).unwrap();
        // Retained pre-manifest migration metadata: no new key authorization
        // or decryption success is asserted by this lower-level fixture.
        conn.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device,authority_namespace) VALUES(1,'shared',1,?1,0,'local-operator','legacy-device','http://original-history:3000')",["cd".repeat(32)]).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        authority::initialize_local(&tx, &p.authority, "shared", "local-operator").unwrap();
        tx.commit().unwrap();
    }
    let peer = {
        let conn = p.origin.pool.get().unwrap();
        conn.execute("INSERT INTO instances(base_url,public_key,label,status) VALUES(?1,?2,'Public test authority','ACTIVE')",params![url,hex::encode(p.authority.signing_key.verifying_key().to_bytes())]).unwrap();
        let peer = conn.last_insert_rowid();
        conn.execute("INSERT INTO federation_agreements(local_server_id,remote_instance_id,alignment_status,transfer_scope,agreement_json,active) VALUES(1,?1,'ALIGNED','REFLECTION_SUMMARIES_ONLY','{}',1)",[peer]).unwrap();
        peer
    };
    let router = Router::new()
        .route("/api/federation/human-membership", post(human::http_attest))
        .route(
            "/api/federation/channel-authority/operations",
            post(authority::http_operation),
        )
        .route(
            "/api/federation/channel-authority/catch-up",
            post(authority::http_catchup),
        )
        .layer(axum::Extension(p.authority.clone()));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let identity = annex_identity::get_platform_identity(
        &p.origin.pool.get().unwrap(),
        1,
        &p.certificate.pseudonym_id,
    )
    .unwrap();
    let joined = replica::attach(
        p.origin.clone(),
        &identity,
        "shared",
        replica::AttachRequest {
            peer_id: peer,
            authority_identity: device_certificates::server_identity(&p.authority),
            device_id: p.certificate.device_id.clone(),
        },
    )
    .await
    .unwrap();
    assert!(joined.accepted_seq > 1);
    {
        let conn = p.origin.pool.get().unwrap();
        let retained = annex_channels::get_message(&conn, "retained-before-adoption").unwrap();
        assert_eq!(
            p.origin
                .message_cipher()
                .decrypt(&retained.content)
                .unwrap(),
            "retained edited text"
        );
        assert_eq!(retained.created_at, "2020-01-01 00:00:00");
        let namespace:String=conn.query_row("SELECT authority_namespace FROM channel_key_epochs WHERE channel_id='shared' AND epoch=1",[],|r|r.get(0)).unwrap();
        assert_eq!(
            namespace, "http://original-history:3000",
            "replica must retain ciphertext's original authenticated namespace"
        );
        let old: String = conn
            .query_row(
                "SELECT old_content FROM message_edits WHERE message_id='retained-before-adoption'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            p.origin.message_cipher().decrypt(&old).unwrap(),
            "retained original text"
        );
        for id in ["expired-before-adoption", "deleted-before-adoption"] {
            assert!(
                annex_channels::get_message(&conn, id).is_err(),
                "discarded content must not be resurrected by adoption"
            );
        }
    }
    assert!(annex_channels::is_member(
        &p.origin.pool.get().unwrap(),
        1,
        "shared",
        &identity.pseudonym_id
    )
    .unwrap());
    let reader = replica::Reader {
        pseudonym_id: identity.pseudonym_id.clone(),
        device_id: p.certificate.device_id.clone(),
        token_epoch: 0,
    };
    let (local, _, _) = annex_server::services::ChannelService::new(p.authority.clone())
        .send_message(
            "local-operator",
            "shared",
            "authority-local ordinary action".into(),
            None,
            Some("local-one".into()),
        )
        .await
        .unwrap();
    let (expired, _, _) = annex_server::services::ChannelService::new(p.authority.clone())
        .send_message(
            "local-operator",
            "shared",
            "must expire before any replica sees this".into(),
            None,
            Some("expiry-before-catchup".into()),
        )
        .await
        .unwrap();
    annex_server::services::ChannelService::new(p.authority.clone())
        .edit_message(
            "local-operator",
            "shared",
            &expired.message_id,
            "expired edited version",
        )
        .await
        .unwrap();
    p.authority
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET expires_at=datetime('now') WHERE message_id=?1",
            [&expired.message_id],
        )
        .unwrap();
    assert!(
        annex_channels::get_message(&p.authority.pool.get().unwrap(), &expired.message_id).is_ok(),
        "no retention sweep has run"
    );
    replica::synchronize(p.origin.clone(), "shared", &reader)
        .await
        .unwrap();
    for state in [&p.authority, &p.origin] {
        let conn = state.pool.get().unwrap();
        assert!(annex_channels::get_message(&conn, &expired.message_id).is_err());
        let bodies:i64=conn.query_row("SELECT count(*) FROM channel_authority_events WHERE message_id=?1 AND event_kind IN(json_quote('message_accepted'),json_quote('message_edited')) AND payload_json IS NOT NULL",[&expired.message_id],|r|r.get(0)).unwrap();
        assert_eq!(
            bodies, 0,
            "catch-up may deliver only signed expiry removals"
        );
    }
    assert_eq!(
        p.origin
            .message_cipher()
            .decrypt(
                &annex_channels::get_message(&p.origin.pool.get().unwrap(), &local.message_id)
                    .unwrap()
                    .content
            )
            .unwrap(),
        "authority-local ordinary action"
    );
    let sent = replica::forward(p.origin.clone(),"shared",reader.clone(),"one-send",json!({"kind":"send","content":"actual signed HTTP message","reply_to":null,"attachments":[]})).await.unwrap();
    let authority::EventRecord::Present(event) = sent.event else {
        panic!("new message was removed")
    };
    let id = event.payload["message"]["message_id"].as_str().unwrap();
    assert_eq!(
        p.origin
            .message_cipher()
            .decrypt(
                &annex_channels::get_message(&p.origin.pool.get().unwrap(), id)
                    .unwrap()
                    .content
            )
            .unwrap(),
        "actual signed HTTP message"
    );
    replica::forward(p.origin.clone(),"shared",reader.clone(),"one-edit",json!({"kind":"edit","message_id":id,"content":"edited through authority","attachments":null})).await.unwrap();
    assert_eq!(
        p.origin
            .message_cipher()
            .decrypt(
                &annex_channels::get_message(&p.origin.pool.get().unwrap(), id)
                    .unwrap()
                    .content
            )
            .unwrap(),
        "edited through authority"
    );
    replica::forward(
        p.origin.clone(),
        "shared",
        reader.clone(),
        "one-delete",
        json!({"kind":"delete","message_id":id}),
    )
    .await
    .unwrap();
    assert!(
        annex_channels::get_message(&p.origin.pool.get().unwrap(), id)
            .unwrap()
            .deleted_at
            .is_some()
    );
    for state in [&p.origin, &p.authority] {
        let conn = state.pool.get().unwrap();
        let head = authority::binding(&conn, state, "shared").unwrap().head_seq;
        let retained:i64=conn.query_row("SELECT COUNT(*) FROM channel_authority_events WHERE message_id=?1 AND payload_json IS NOT NULL AND event_seq<?2",params![id,head],|r|r.get(0)).unwrap();
        assert_eq!(retained, 0, "deleted original/edited payload retained");
    }
    annex_channels::remove_member(
        &p.authority.pool.get().unwrap(),
        1,
        "shared",
        &identity.pseudonym_id,
    )
    .unwrap();
    assert!(replica::synchronize(p.origin.clone(), "shared", &reader)
        .await
        .is_err());
    assert!(!annex_channels::is_member(
        &p.origin.pool.get().unwrap(),
        1,
        "shared",
        &identity.pseudonym_id
    )
    .unwrap());
    replica::attach(
        p.origin.clone(),
        &identity,
        "shared",
        replica::AttachRequest {
            peer_id: peer,
            authority_identity: device_certificates::server_identity(&p.authority),
            device_id: p.certificate.device_id.clone(),
        },
    )
    .await
    .unwrap();
    let left = replica::forward(
        p.origin.clone(),
        "shared",
        reader.clone(),
        "self-leave-once",
        json!({"kind":"leave"}),
    )
    .await
    .unwrap();
    assert!(!annex_channels::is_member(
        &p.origin.pool.get().unwrap(),
        1,
        "shared",
        &identity.pseudonym_id
    )
    .unwrap());
    let retry = replica::forward(
        p.origin.clone(),
        "shared",
        reader.clone(),
        "self-leave-once",
        json!({"kind":"leave"}),
    )
    .await
    .unwrap();
    assert_eq!(left.accepted_hash, retry.accepted_hash);
    assert_eq!(left.accepted_seq, retry.accepted_seq);
    // A departed replica cannot read new history, but it must still receive
    // signed deletion controls after its last local identity/device is revoked.
    let (private, _, _) = annex_server::services::ChannelService::new(p.authority.clone())
        .send_message(
            "local-operator",
            "shared",
            "after departure, never disclose".into(),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(!replica::cleanup_replica(p.origin.clone(), "shared")
        .await
        .unwrap());
    assert!(
        annex_channels::get_message(&p.origin.pool.get().unwrap(), &private.message_id).is_err()
    );
    {
        let conn = p.origin.pool.get().unwrap();
        conn.execute("UPDATE platform_identities SET active=0", [])
            .unwrap();
        conn.execute(
            "UPDATE member_device_keys SET revoked_at=datetime('now')",
            [],
        )
        .unwrap();
    }
    let moderator = annex_identity::get_platform_identity(
        &p.authority.pool.get().unwrap(),
        1,
        "local-operator",
    )
    .unwrap();
    annex_server::services::ChannelService::new(p.authority.clone())
        .delete_channel(&moderator, "shared")
        .await
        .unwrap();
    assert!(replica::cleanup_replica(p.origin.clone(), "shared")
        .await
        .unwrap());
    assert!(annex_channels::get_channel(&p.origin.pool.get().unwrap(), "shared").is_err());
    assert!(
        authority::binding(&p.origin.pool.get().unwrap(), &p.origin, "shared")
            .unwrap()
            .deleted_seq
            .is_some()
    );
    server.abort();
    let _ = server.await;
}
#[test]
fn genuine_human_attestation_grants_no_channel_membership_role_or_bearer() {
    let p = Pair::new();
    let request = p.request();
    let receipt = p.accept(&request).unwrap();
    receipt
        .verify(
            &request,
            &hex::encode(p.authority.signing_key.verifying_key().to_bytes()),
        )
        .unwrap();
    assert_eq!(p.count("federated_human_memberships"), 1);
    assert_eq!(p.count("channel_members"), 0);
    assert_eq!(p.count("channel_manifest_events"), 0);
    assert_eq!(p.count("zk_auth_challenges"), 0);
    let conn = p.authority.pool.get().unwrap();
    let role:(String,bool,bool)=conn.query_row("SELECT participant_type,can_moderate,can_federate FROM platform_identities WHERE pseudonym_id=?1",[&p.certificate.pseudonym_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(role, ("HUMAN".into(), false, false));
}

#[test]
fn authority_authentication_requires_current_exact_human_channel_device_attestation() {
    let p = Pair::new();
    p.accept(&p.request()).unwrap();
    let check = |device: Option<String>| {
        let mut request = authority::SignedRequest {
            version: 1,
            requesting_identity: device_certificates::server_identity(&p.origin),
            originating_server: p.origin.get_public_url(),
            authority_identity: device_certificates::server_identity(&p.authority),
            channel_id: "shared".into(),
            pseudonym_id: p.certificate.pseudonym_id.clone(),
            device_id: device,
            operation_id: uuid::Uuid::new_v4().to_string(),
            operation: serde_json::json!({"kind":"join"}),
            nonce: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            created_at: chrono::Utc::now().to_rfc3339(),
            signature: String::new(),
        };
        request.signature = hex::encode(
            p.origin
                .signing_key
                .sign(authority::canonical_json(&request.signing_value()).as_bytes())
                .to_bytes(),
        );
        let mut conn = p.authority.pool.get().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        authority::authenticate_request(&tx, &p.authority, &request).map(|_| ())
    };
    assert!(check(Some(p.certificate.device_id.clone())).is_ok());
    assert!(check(None).is_err());
    assert!(check(Some("ef".repeat(32))).is_err());
    p.authority
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federated_human_memberships SET expires_at=unixepoch()-1",
            [],
        )
        .unwrap();
    assert!(check(Some(p.certificate.device_id.clone())).is_err());
    p.accept(&p.request()).unwrap();
    assert!(check(Some(p.certificate.device_id.clone())).is_ok());
    p.authority
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE member_device_keys SET revoked_at=datetime('now')",
            [],
        )
        .unwrap();
    assert!(check(Some(p.certificate.device_id.clone())).is_err());
}
#[test]
fn signed_origin_cannot_forge_genuine_membership_proof() {
    let p = Pair::new();
    let mut request = p.request();
    request.certificate.public_signals[0] = "1".into();
    p.sign(&mut request);
    assert!(p.accept(&request).is_err());
    assert_eq!(p.count("federated_identities"), 0);
    assert_eq!(p.count("channel_authority_nonces"), 0);
}
#[test]
fn origin_signature_destination_binding_and_nonce_are_all_required() {
    let p = Pair::new();
    let request = p.request();
    let mut forged = request.clone();
    forged.signature = "00".repeat(64);
    assert!(p.accept(&forged).is_err());
    let mut redirected = request.clone();
    redirected.authority_identity = device_certificates::server_identity(&p.origin);
    p.sign(&mut redirected);
    assert!(p.accept(&redirected).is_err());
    p.accept(&request).unwrap();
    assert!(p.accept(&request).is_err());
    assert_eq!(p.count("federated_human_memberships"), 1);
}
#[test]
fn refresh_cannot_reactivate_local_ban_or_restore_revoked_device() {
    let p = Pair::new();
    p.accept(&p.request()).unwrap();
    let conn = p.authority.pool.get().unwrap();
    conn.execute(
        "UPDATE platform_identities SET active=0 WHERE pseudonym_id=?1",
        [&p.certificate.pseudonym_id],
    )
    .unwrap();
    drop(conn);
    assert!(p.accept(&p.request()).is_err());
    let q = Pair::new();
    q.accept(&q.request()).unwrap();
    q.authority
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE member_device_keys SET revoked_at=datetime('now')",
            [],
        )
        .unwrap();
    assert!(q.accept(&q.request()).is_err());
}
#[test]
fn attestation_cannot_replace_local_registration_or_another_participant_type() {
    for role in ["HUMAN", "AI_AGENT"] {
        let p = Pair::new();
        let conn = p.authority.pool.get().unwrap();
        if role == "HUMAN" {
            conn.execute("INSERT INTO zk_nullifiers(topic,nullifier_hex,pseudonym_id,commitment_hex) VALUES(?1,?2,?3,?4)",params![p.certificate.topic,p.certificate.nullifier_hex,p.certificate.pseudonym_id,p.certificate.commitment_hex]).unwrap();
        }
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type) VALUES(1,?1,?2)",params![p.certificate.pseudonym_id,role]).unwrap();
        drop(conn);
        assert!(p.accept(&p.request()).is_err());
        assert_eq!(p.count("federated_identities"), 0);
    }
}
#[test]
fn revoked_agreement_expired_attestation_and_known_new_epoch_refuse_replay() {
    let p = Pair::new();
    let mut expired = p.request();
    expired.issued_at -= 180;
    expired.expires_at -= 180;
    p.sign(&mut expired);
    assert!(p.accept(&expired).is_err());
    let mut current = p.request();
    current.token_epoch = 1;
    p.sign(&mut current);
    p.accept(&current).unwrap();
    assert!(p.accept(&p.request()).is_err());
    p.authority
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federation_agreements SET active=0 WHERE remote_instance_id=?1",
            [p.peer],
        )
        .unwrap();
    assert!(p.accept(&p.request()).is_err());
}
#[test]
fn origin_builder_rechecks_current_session_and_real_local_membership() {
    let p = Pair::new();
    let conn = p.origin.pool.get().unwrap();
    conn.execute("UPDATE platform_identities SET token_epoch=1", [])
        .unwrap();
    assert!(human::build(
        &conn,
        &p.origin,
        &p.certificate.pseudonym_id,
        0,
        &p.certificate.device_id,
        "shared",
        &device_certificates::server_identity(&p.authority)
    )
    .is_err());
    assert!(human::build(
        &conn,
        &p.origin,
        &p.certificate.pseudonym_id,
        1,
        &p.certificate.device_id,
        "shared",
        &device_certificates::server_identity(&p.authority)
    )
    .is_ok());
    conn.execute("DELETE FROM zk_nullifiers", []).unwrap();
    assert!(human::build(
        &conn,
        &p.origin,
        &p.certificate.pseudonym_id,
        1,
        &p.certificate.device_id,
        "shared",
        &device_certificates::server_identity(&p.authority)
    )
    .is_err());
}

#[tokio::test]
async fn actual_http_reconciliation_expired_conflict_stays_cancelled_without_resurrecting_staging()
{
    let p = ReconcilePair::new(1, "unacknowledged local revision").await;
    let request = p.input();
    let first = p.run(request.clone()).await.unwrap();
    assert_eq!(first.status, "conflict");
    assert!(first.remote_join_accepted);
    {
        let conn = p.pair.origin.pool.get().unwrap();
        conn.execute(
            "UPDATE channel_reconciliations SET updated_at=unixepoch()-90000 WHERE operation_id=?1",
            [&request.operation_id],
        )
        .unwrap();
        assert!(
            conn.query_row(
                "SELECT COUNT(*) FROM channel_reconciliation_records WHERE operation_id=?1",
                [&request.operation_id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
                > 0
        );
    }
    let result = p.run(request.clone()).await.unwrap();
    assert_eq!(result.status, "cancelled");
    assert!(
        result.remote_join_accepted,
        "expiry cannot pretend the accepted remote Join was undone"
    );
    let again = p.run(request.clone()).await.unwrap();
    assert_eq!(again.status, "cancelled");
    let conn = p.pair.origin.pool.get().unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM channel_reconciliation_records WHERE operation_id=?1",
            [&request.operation_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert!(authority::binding(&conn, &p.pair.origin, "shared").is_err());
    let message = annex_channels::get_message(&conn, "retained-0000").unwrap();
    assert_eq!(
        p.pair
            .origin
            .message_cipher()
            .decrypt(&message.content)
            .unwrap(),
        "unacknowledged local revision"
    );
}

#[tokio::test]
async fn signed_authority_directory_cannot_use_a_virtual_channel_vertex_as_a_participant() {
    let p = ReconcilePair::new(1, "accepted revision").await;
    let invalid = annex_graph::channel_vertex(1, "other-private-channel");
    {
        // A controlled malicious peer directory. Its actual signed snapshot must
        // still obey the recipient's participant/virtual-vertex namespace rule.
        let conn = p.pair.authority.pool.get().unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type) VALUES(1,?1,'HUMAN')",[&invalid]).unwrap();
        annex_channels::add_member(&conn, 1, "shared", &invalid).unwrap();
    }
    let result = p.run(p.input()).await;
    // The real authority refuses its malformed directory and the origin
    // preserves the terminal HTTP403 instead of inventing a retryable503.
    match result {
        Err(authority::AuthorityError::Forbidden(message)) => assert_eq!(
            message,
            "{\"error\":\"invalid snapshot participant identifier in pseudonym_id\"}"
        ),
        other => panic!("signed invalid participant must be refused, got {other:?}"),
    }
    let conn = p.pair.origin.pool.get().unwrap();
    assert!(authority::binding(&conn, &p.pair.origin, "shared").is_err());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM platform_identities WHERE pseudonym_id=?1",
            [&invalid],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE from_node=?1",
            [&invalid],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM channel_reconciliation_guards",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn actual_http_terminal_refusal_is_persisted_without_retries_and_valid_action_still_works() {
    use annex_server::services::channel_replication as replica;
    let p = ReconcilePair::new(1, "accepted revision").await;
    assert_eq!(p.run(p.input()).await.unwrap().status, "complete");
    let reader = replica::Reader {
        pseudonym_id: p.pair.certificate.pseudonym_id.clone(),
        device_id: p.pair.certificate.device_id.clone(),
        token_epoch: p.identity().token_epoch,
    };
    let before: i64 = p
        .pair
        .authority
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT head_seq FROM channel_authorities WHERE channel_id='shared'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let refused = replica::forward(
        p.pair.origin.clone(),
        "shared",
        reader.clone(),
        "forbidden-unsigned-encryption",
        serde_json::json!({"kind":"set_encryption","enabled":true}),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.status_code(), axum::http::StatusCode::FORBIDDEN);
    assert!(refused.is_terminal_refusal());
    let persisted = || {
        p.pair.origin.pool.get().unwrap().query_row("SELECT status,attempts,operation_ciphertext FROM channel_authority_outgoing WHERE operation_id='forbidden-unsigned-encryption'",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<String>>(2)?))).unwrap()
    };
    assert_eq!(persisted(), ("failed".into(), 1, None));
    assert!(replica::forward(
        p.pair.origin.clone(),
        "shared",
        reader.clone(),
        "forbidden-unsigned-encryption",
        serde_json::json!({"kind":"set_encryption","enabled":true})
    )
    .await
    .is_err());
    assert_eq!(persisted(), ("failed".into(), 1, None));
    assert_eq!(
        p.pair
            .authority
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT head_seq FROM channel_authorities WHERE channel_id='shared'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        before
    );
    let valid = replica::forward(p.pair.origin.clone(),"shared",reader,"independent-valid-operation",serde_json::json!({"kind":"send","content":"after refused intent","reply_to":null,"attachments":[]})).await.unwrap();
    assert!(valid.accepted_seq > before);
}

// Controlled state/peer fixtures with genuine origin55 device certification and
// real authority66 HTTP/signature/reconciliation services. Not a live UI receipt.
#[tokio::test]
async fn actual_http_reconciliation_keeps_origin_attachment_bytes_and_associations_on_adopt_and_conflict(
) {
    use annex_server::services::{
        authority_bootstrap::ImportedMessage,
        federated_upload_service,
        federated_upload_types::{AttachmentDescriptor, PlainAttachmentMetadata},
        upload_service::{self, UploadMetadata},
    };
    use sha2::{Digest, Sha256};
    for conflicting_history in [false, true] {
        let mut p = ReconcilePair::new(
            1,
            if conflicting_history {
                "unaccepted local revision"
            } else {
                "accepted revision"
            },
        )
        .await;
        let directory = tempfile::tempdir().unwrap();
        let mut isolated_origin = (*p.pair.origin).clone();
        isolated_origin.upload_dir = directory.path().to_string_lossy().into_owned();
        p.pair.origin = Arc::new(isolated_origin);
        // Real typed HUMAN acceptance preserves the genuine certificate issuer;
        // this does not forge a historical author or change signed device bytes.
        p.pair.accept(&p.pair.request()).unwrap();
        let owner = p.pair.certificate.pseudonym_id.clone();
        let id = uuid::Uuid::new_v4().to_string();
        let message_id = format!("retained-attachment-{id}");
        let bytes = b"retained upload body\n".to_vec();
        let metadata = UploadMetadata {
            upload_id: id.clone(),
            channel_id: "shared".into(),
            uploader: owner.clone(),
            token_epoch: 0,
            category: "file".into(),
            original_filename: "retained.txt".into(),
            content_type: "text/plain".into(),
            extension: "txt".into(),
            encrypted_epoch: None,
        };
        let (url, hash) =
            upload_service::persist_chat_upload(&p.pair.origin, &metadata, &bytes).unwrap();
        assert_eq!(hash, hex::encode(Sha256::digest(&bytes)));
        let descriptor = AttachmentDescriptor {
            attachment_id: id.clone(),
            origin_identity: device_certificates::server_identity(&p.pair.origin),
            originating_server: p.pair.origin.get_public_url(),
            size_bytes: bytes.len() as u64,
            ciphertext_sha256: hash.clone(),
            category: "file".into(),
            epoch: 0,
            key_id: String::new(),
            plaintext: Some(PlainAttachmentMetadata {
                filename: "retained.txt".into(),
                content_type: "text/plain".into(),
            }),
        };
        descriptor.validate().unwrap();
        assert_eq!(url, format!("/uploads/{}", descriptor.public_path()));
        let body = format!("[retained.txt]({url})");
        for state in [&p.pair.origin, &p.pair.authority] {
            let conn = state.pool.get().unwrap();
            annex_channels::create_message(
                &conn,
                &annex_channels::CreateMessageParams {
                    channel_id: "shared".into(),
                    message_id: message_id.clone(),
                    sender_pseudonym: owner.clone(),
                    content: state.message_cipher().encrypt(&body).unwrap(),
                    reply_to_message_id: None,
                },
            )
            .unwrap();
            conn.execute("UPDATE messages SET created_at='2025-01-02 00:00:00',expires_at='2099-01-01 00:00:00' WHERE message_id=?1", [&message_id]).unwrap();
        }
        {
            let mut conn = p.pair.origin.pool.get().unwrap();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            upload_service::associate_message_uploads(
                &tx,
                &p.pair.origin,
                "shared",
                &owner,
                &message_id,
                std::slice::from_ref(&id),
                None,
            )
            .unwrap();
            federated_upload_service::validate_origin_descriptor(
                &tx,
                &p.pair.origin,
                "shared",
                &owner,
                &descriptor,
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let accepted = {
            let mut conn = p.pair.authority.pool.get().unwrap();
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            let mut message = annex_channels::get_message(&tx, &message_id).unwrap();
            p.pair
                .authority
                .message_cipher()
                .decrypt_in_place(&mut message.content)
                .unwrap();
            let imported = ImportedMessage {
                provenance: "retained_local_store".into(),
                message,
                legacy_receipts: vec![],
                attachments: vec![descriptor.clone()],
            };
            let mut payload = serde_json::to_value(&imported).unwrap();
            payload["message"]["expires_at"] = serde_json::json!(imported.message.expires_at);
            let event = authority::append_event(
                &tx,
                &p.pair.authority,
                "shared",
                authority::EventKind::HistoryImported,
                payload,
            )
            .unwrap();
            authority::verify_event(
                &authority::binding(&tx, &p.pair.authority, "shared").unwrap(),
                &event,
            )
            .unwrap();
            authority::bind_accepted_attachments(
                &tx,
                &p.pair.authority,
                &event,
                std::slice::from_ref(&descriptor),
            )
            .unwrap();
            tx.commit().unwrap();
            event
        };
        let snapshot_upload = || {
            p.pair.origin.pool.get().unwrap().query_row(
                "SELECT json_array(upload_id,uploader_pseudonym,original_filename,content_type,size_bytes,purpose,channel_id,category,encrypted,epoch,key_id,ciphertext_sha256,storage_path,staging_path,lifecycle) FROM uploads WHERE server_id=1 AND upload_id=?1",
                [&id], |row| row.get::<_,String>(0)).unwrap()
        };
        let before = snapshot_upload();
        let disk_path = {
            let conn = p.pair.origin.pool.get().unwrap();
            let path: String = conn
                .query_row(
                    "SELECT storage_path FROM uploads WHERE upload_id=?1",
                    [&id],
                    |row| row.get(0),
                )
                .unwrap();
            directory.path().join(path)
        };
        assert_eq!(std::fs::read(&disk_path).unwrap(), bytes);
        let input = p.input();
        let result = p.run(input.clone()).await.unwrap();
        assert!(result.remote_join_accepted);
        assert_eq!(
            result.status,
            if conflicting_history {
                "conflict"
            } else {
                "complete"
            }
        );
        if conflicting_history {
            assert!(result
                .conflicts
                .iter()
                .any(|value| value.kind == "message_body" && value.id == "retained-0000"));
            assert!(authority::binding(
                &p.pair.origin.pool.get().unwrap(),
                &p.pair.origin,
                "shared"
            )
            .is_err());
        } else {
            assert_eq!(result.retained_messages, 2);
            let conn = p.pair.origin.pool.get().unwrap();
            let (seq, event_hash, raw): (i64,String,String) = conn.query_row(
                "SELECT accepted_seq,accepted_hash,descriptor_json FROM channel_authority_attachment_bindings WHERE channel_id='shared' AND message_id=?1 AND attachment_id=?2 AND removed_reason IS NULL",
                params![message_id,id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
            assert_eq!(seq, accepted.header.seq);
            assert_eq!(event_hash, accepted.event_hash);
            assert_eq!(
                serde_json::from_str::<AttachmentDescriptor>(&raw).unwrap(),
                descriptor
            );
        }
        assert_eq!(snapshot_upload(), before);
        let conn = p.pair.origin.pool.get().unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM message_uploads WHERE server_id=1 AND message_id=?1 AND upload_id=?2", params![message_id,id], |row| row.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM upload_deletion_queue", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM channel_reconciliation_guards",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        let retained = annex_channels::get_message(&conn, &message_id).unwrap();
        assert_eq!(
            p.pair
                .origin
                .message_cipher()
                .decrypt(&retained.content)
                .unwrap(),
            body
        );
        drop(conn);
        assert_eq!(
            upload_service::drain_deletion_queue(&p.pair.origin).unwrap(),
            0
        );
        assert_eq!(std::fs::read(&disk_path).unwrap(), bytes);
        assert_eq!(
            hex::encode(Sha256::digest(std::fs::read(&disk_path).unwrap())),
            hash
        );
    }
}
#[tokio::test]
async fn actual_http_reconciliation_retains_only_current_agent_serving_claims_and_rolls_back_conflicts(
) {
    for conflict in [false, true] {
        let p = ReconcilePair::new(
            1,
            if conflict {
                "unaccepted local revision"
            } else {
                "accepted revision"
            },
        )
        .await;
        let target = annex_graph::channel_vertex(1, "shared");
        for (name, active, role, remote_member) in [
            ("retained-agent", true, "AI_AGENT", true),
            ("departed-agent", true, "AI_AGENT", false),
            ("inactive-agent", false, "AI_AGENT", true),
            ("retyped-agent", true, "HUMAN", true),
        ] {
            let conn = p.pair.origin.pool.get().unwrap();
            conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,active) VALUES(1,?1,?2,?3)",params![name,role,active]).unwrap();
            annex_channels::add_member(&conn, 1, "shared", name).unwrap();
            conn.execute("INSERT INTO graph_edges(server_id,from_node,to_node,kind,weight,created_at) VALUES(1,?1,?2,'AGENT_SERVING',0.375,'2024-01-01 00:00:00')",params![name,target]).unwrap();
            if remote_member {
                let remote = p.pair.authority.pool.get().unwrap();
                remote.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,active) VALUES(1,?1,?2,1)",params![name,role]).unwrap();
                annex_channels::add_member(&remote, 1, "shared", name).unwrap();
            }
        }
        let result = p.run(p.input()).await.unwrap();
        assert_eq!(
            result.status,
            if conflict { "conflict" } else { "complete" }
        );
        let conn = p.pair.origin.pool.get().unwrap();
        let mut stmt=conn.prepare("SELECT from_node,weight,created_at FROM graph_edges WHERE server_id=1 AND to_node=?1 AND kind='AGENT_SERVING' ORDER BY from_node").unwrap();
        let edges = stmt
            .query_map([&target], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        if conflict {
            assert_eq!(edges.len(), 4);
        } else {
            assert_eq!(edges.len(), 1);
            assert_eq!(edges[0].0, "retained-agent");
        }
        for (_, weight, created) in edges {
            assert_eq!(weight, 0.375);
            assert_eq!(created, "2024-01-01 00:00:00");
        }
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM graph_edges WHERE server_id=1 AND from_node='retained-agent' AND to_node=?1 AND kind='MEMBER_OF'",[&target],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
}
