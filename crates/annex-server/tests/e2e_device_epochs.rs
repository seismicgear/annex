//! Genuine certified-device/member-signature API regressions. Deterministic
//! PUBLIC TEST device seeds are explicit vectors; no membership proving secret
//! or session credential is embedded in this fixture.
mod common;
use annex_server::{
    device_certificates::{certificate_digest, DeviceCertificate},
    services::{channel_manifest as manifest, e2e_service, ChannelService},
    AppState,
};
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::Signer;
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    sync::{Arc, OnceLock},
};
use tower::ServiceExt;

fn vectors() -> &'static Value {
    static VALUE: OnceLock<Value> = OnceLock::new();
    VALUE.get_or_init(|| {
        serde_json::from_str(include_str!("fixtures/device-epoch-public.json")).unwrap()
    })
}
fn entry(seed: u8) -> &'static Value {
    vectors()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["public_test_x25519_byte"] == seed)
        .unwrap()
}
fn pid(name: &str) -> String {
    vectors()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .map_or_else(
            || name.into(),
            |e| e["certificate"]["pseudonym_id"].as_str().unwrap().into(),
        )
}
fn cert(seed: u8) -> DeviceCertificate {
    serde_json::from_value(entry(seed)["certificate"].clone()).unwrap()
}
fn signer(device: &str) -> ed25519_dalek::SigningKey {
    let e = vectors()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["certificate"]["device_id"] == device)
        .unwrap();
    ed25519_dalek::SigningKey::from_bytes(
        &hex::decode(e["public_test_ed25519_seed"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
    )
}
struct Fixture {
    state: Arc<AppState>,
    app: axum::Router,
}
impl Fixture {
    fn new() -> Self {
        Self::with_pool(
            annex_db::create_pool(":memory:", annex_db::DbRuntimeSettings::default()).unwrap(),
        )
    }
    fn with_pool(pool: annex_db::DbPool) -> Self {
        {
            let conn = pool.get().unwrap();
            annex_db::run_migrations(&conn).unwrap();
            conn.execute(
                "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'test','Test',?1)",
                [serde_json::to_string(&annex_types::ServerPolicy::default()).unwrap()],
            )
            .unwrap();
            for name in ["alice", "bob", "outsider"] {
                conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_moderate) VALUES(1,?1,'HUMAN',?2)",params![pid(name),name=="alice"]).unwrap();
            }
            conn.execute("INSERT INTO channels(server_id,channel_id,name,channel_type,federation_scope,e2e_enabled) VALUES(1,'secure','Secure','\"Text\"','\"Local\"',1)",[]).unwrap();
            for name in ["alice", "bob"] {
                conn.execute("INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES(1,'secure',?1)",[pid(name)]).unwrap();
            }
            // Seed the exact public registration/root records underlying the genuinely
            // generated certificate proofs. Publication still executes the real verifier.
            for e in vectors()["entries"].as_array().unwrap() {
                let c: DeviceCertificate =
                    serde_json::from_value(e["certificate"].clone()).unwrap();
                conn.execute("INSERT OR IGNORE INTO zk_nullifiers(topic,nullifier_hex,pseudonym_id,commitment_hex) VALUES(?1,?2,?3,?4)",params![c.topic,c.nullifier_hex,c.pseudonym_id,c.commitment_hex]).unwrap();
                conn.execute("INSERT INTO vrp_roots(root_hex) VALUES(?1)", [c.root_hex])
                    .unwrap();
            }
        }
        let mut state = common::build_app_state(
            pool,
            annex_identity::MerkleTree::new(20).unwrap(),
            annex_types::ServerPolicy::default(),
        );
        state.enforce_zk_proofs = true;
        state.signing_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0x55; 32]));
        state.membership_vkey_v2 = Some(Arc::new(
            annex_identity::zk::parse_verification_key(&vectors()["vkey"].to_string()).unwrap(),
        ));
        let state = Arc::new(state);
        Self {
            app: annex_server::app((*state).clone()),
            state,
        }
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        name: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let identity = pid(name);
        let path = path
            .replace("/api/keys/alice", &format!("/api/keys/{}", pid("alice")))
            .replace("/api/keys/bob", &format!("/api/keys/{}", pid("bob")));
        let token = annex_server::api_ws::generate_session_token(
            &identity,
            &self.state.ws_token_secret,
            3600,
            0,
        );
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo("127.0.0.1:3333".parse::<SocketAddr>().unwrap()));
        let response = self.app.clone().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 20 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&bytes)})),
        )
    }
    async fn event(&self, name: &str, device: &str, operation: manifest::Operation) -> Value {
        let history = self
            .call("GET", "/api/channels/secure/key-manifest", name, None)
            .await
            .1;
        let certificate = vectors()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["certificate"]["device_id"] == device)
            .unwrap()["certificate"]
            .clone();
        let certificate: DeviceCertificate = serde_json::from_value(certificate).unwrap();
        let mut event = manifest::SignedManifestEvent {
            version: 1,
            authority_identity: certificate.server_identity.clone(),
            channel_id: "secure".into(),
            seq: history["head_seq"].as_i64().unwrap() + 1,
            previous_hash: history["head_hash"].as_str().unwrap().into(),
            operation,
            signer_pseudonym_id: pid(name),
            signer_device_id: device.into(),
            signer_certificate_sha256: certificate_digest(&certificate),
            event_hash: String::new(),
            signature_hex: String::new(),
        };
        let input = manifest::signing_input(&event);
        event.event_hash = hex::encode(Sha256::digest(&input));
        event.signature_hex = hex::encode(signer(device).sign(&input).to_bytes());
        let (status, value) = self
            .call(
                "POST",
                "/api/channels/secure/key-manifest",
                name,
                Some(json!(event)),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        value
    }
    async fn publish(&self, name: &str, seed: u8) -> String {
        let certificate = cert(seed);
        assert_eq!(certificate.pseudonym_id, pid(name));
        let (status,value)=self.call("PUT","/api/keys/me",name,Some(json!({"version":2,"x25519_pub_hex":certificate.x25519_pub_hex,"device_certificate":certificate}))).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        let directory = self.directory().await;
        let projection = &directory["manifest"];
        if projection.is_null() {
            self.event(
                name,
                &certificate.device_id,
                manifest::Operation::Genesis {
                    join_policy: manifest::JoinPolicy::EligibleSelfJoin,
                    history_policy: "all_members".into(),
                    certificate: certificate.clone(),
                },
            )
            .await;
        } else if let Some(member) = projection["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["pseudonym_id"] == pid(name))
        {
            if !member["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["device_id"] == certificate.device_id)
            {
                let prior = member["devices"][0]["device_id"].as_str().unwrap();
                self.event(
                    name,
                    prior,
                    manifest::Operation::AdmitDevice {
                        certificate: certificate.clone(),
                    },
                )
                .await;
            }
        } else {
            self.event(
                name,
                &certificate.device_id,
                manifest::Operation::Join {
                    certificate: certificate.clone(),
                },
            )
            .await;
        }
        certificate.device_id
    }
    async fn directory(&self) -> Value {
        let (status, value) = self
            .call("GET", "/api/channels/secure/member-keys", "alice", None)
            .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        value
    }
    async fn commit(&self, cek: u8) -> Value {
        let directory = self.directory().await;
        let request = wrap_request(
            &directory,
            "commit_epoch",
            directory["required_epoch"].as_i64().unwrap(),
            cek,
        );
        let (status, value) = self
            .call(
                "POST",
                "/api/channels/secure/key-wraps",
                "alice",
                Some(request),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        value
    }
    async fn mode(&self, enabled: bool) {
        let directory = self.directory().await;
        let device = directory["member_keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["pseudonym_id"] == pid("alice"))
            .unwrap()["device_id"]
            .as_str()
            .unwrap();
        self.event(
            "alice",
            device,
            manifest::Operation::SetEncryption { enabled },
        )
        .await;
    }
}
fn key_id(cek: u8) -> String {
    let mut hash = Sha256::new();
    hash.update(b"annex-channel-key-v2\0");
    hash.update([cek; 32]);
    hex::encode(hash.finalize())
}
fn sign_wrap(request: &mut Value, directory: &Value) {
    let sender = request["sender_device_id"].as_str().unwrap();
    let cert = vectors()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["certificate"]["device_id"] == sender)
        .unwrap()["certificate"]
        .clone();
    let cert: DeviceCertificate = serde_json::from_value(cert).unwrap();
    let mut digests: Vec<_> = request["wraps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            let d = directory["member_keys"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| {
                    d["device_id"] == w["recipient_device_id"]
                        && d["pseudonym_id"] == w["recipient_pseudonym_id"]
                })
                .unwrap();
            (
                w["recipient_pseudonym_id"].as_str().unwrap().to_owned(),
                w["recipient_device_id"].as_str().unwrap().to_owned(),
                d["certificate_sha256"].as_str().unwrap().to_owned(),
                hex::encode(Sha256::digest(
                    STANDARD
                        .decode(w["wrapped_key_b64"].as_str().unwrap())
                        .unwrap(),
                )),
            )
        })
        .collect();
    digests.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let p = &directory["manifest"];
    let receipt = manifest::WrapReceipt {
        version: 1,
        authority_identity: directory["authority_identity"].as_str().unwrap().into(),
        channel_id: "secure".into(),
        sender_pseudonym_id: cert.pseudonym_id.clone(),
        sender_device_id: sender.into(),
        sender_certificate_sha256: certificate_digest(&cert),
        manifest_seq: p["head_seq"].as_i64().unwrap(),
        manifest_hash: p["head_hash"].as_str().unwrap().into(),
        operation: request["operation"].as_str().unwrap().into(),
        expected_revision: request["expected_revision"].as_i64().unwrap(),
        epoch: request["epoch"].as_i64().unwrap(),
        key_id: request["key_id"].as_str().unwrap().into(),
        wrap_digests: digests,
        signature_hex: String::new(),
        authorization_sha256: String::new(),
    };
    request["authorization"] = json!({"manifest_seq":receipt.manifest_seq,"manifest_hash":receipt.manifest_hash,"signature_hex":hex::encode(signer(sender).sign(&manifest::wrap_signing_input(&receipt)).to_bytes())});
}
fn wrap_request(directory: &Value, operation: &str, epoch: i64, cek: u8) -> Value {
    let key = key_id(cek);
    let sender = directory["member_keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["pseudonym_id"] == pid("alice"))
        .unwrap()["device_id"]
        .as_str()
        .unwrap();
    let authority = if operation == "admit_legacy_devices" {
        &directory["authority_server"]
    } else {
        &directory["authority_identity"]
    };
    let wraps=directory["member_keys"].as_array().unwrap().iter().map(|d|{let public:[u8;32]=hex::decode(d["x25519_pub_hex"].as_str().unwrap()).unwrap().try_into().unwrap();let domain=if operation=="admit_legacy_devices"{"annex-legacy-key-wrap-v2"}else{"annex-key-wrap-v2"};let plain=json!([domain,authority,"secure",epoch,key,d["pseudonym_id"],d["device_id"],STANDARD.encode([cek;32])]).to_string();json!({"recipient_pseudonym_id":d["pseudonym_id"],"recipient_device_id":d["device_id"],"wrapped_key_b64":STANDARD.encode(annex_federation::seal::seal_x25519(plain.as_bytes(),&public).unwrap())})}).collect::<Vec<_>>();
    let mut req = json!({"version":2,"operation":operation,"expected_revision":directory["revision"],"epoch":epoch,"key_id":key,"sender_device_id":sender,"wraps":wraps});
    sign_wrap(&mut req, directory);
    req
}
fn body(epoch: i64, cek: u8) -> String {
    format!(
        "e2e2:{epoch}:{}:{}",
        key_id(cek),
        STANDARD.encode([1u8; 48])
    )
}

#[tokio::test]
async fn authority_bootstrap_replays_genuine_manifest_and_signed_wraps_without_resealing() {
    use annex_server::services::channel_authority as authority;
    let f = Fixture::new();
    f.publish("alice", 11).await;
    let bob = f.publish("bob", 21).await;
    f.commit(47).await;
    let (bound, records, original_wrap, original_namespace) = {
        let mut conn = f.state.pool.get().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        tx.execute("UPDATE channels SET federation_scope=json_quote('Federated') WHERE channel_id='secure'", []).unwrap();
        authority::initialize_local(&tx, &f.state, "secure", &pid("alice")).unwrap();
        let bound = authority::binding(&tx, &f.state, "secure").unwrap();
        let records = (1..=bound.head_seq)
            .map(|seq| authority::event_record(&tx, &f.state, "secure", seq).unwrap())
            .collect::<Vec<_>>();
        let wrap:String=tx.query_row("SELECT wrapped_key_b64 FROM channel_device_key_wraps WHERE channel_id='secure' AND recipient_pseudonym_id=?1 AND recipient_device_id=?2",params![pid("bob"),bob],|r|r.get(0)).unwrap();
        let namespace:String=tx.query_row("SELECT authority_namespace FROM channel_key_epochs WHERE channel_id='secure' AND epoch=1",[],|r|r.get(0)).unwrap();
        tx.commit().unwrap();
        (bound, records, wrap, namespace)
    };
    // Feed actual signed authority records into the replica transaction. This
    // is a materialization test; HTTP membership admission is proved separately.
    let pool = annex_db::create_pool(":memory:", Default::default()).unwrap();
    let mut replica = (*f.state).clone();
    replica.pool = pool;
    replica.signing_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0x66; 32]));
    let mut conn = replica.pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    conn.execute(
        "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'replica','Replica',?1)",
        [serde_json::to_string(&annex_types::ServerPolicy::default()).unwrap()],
    )
    .unwrap();
    conn.execute("INSERT INTO channel_authorities(server_id,channel_id,authority_identity,authority_key_hex,authority_url,local_authority) VALUES(1,'secure',?1,?2,?3,0)",params![bound.authority_identity,bound.authority_key_hex,bound.authority_url]).unwrap();
    let request = authority::SignedRequest {
        version: 1,
        requesting_identity: annex_server::device_certificates::server_identity(&replica),
        originating_server: "https://replica.test".into(),
        authority_identity: bound.authority_identity.clone(),
        channel_id: "secure".into(),
        pseudonym_id: pid("bob"),
        device_id: Some(bob.clone()),
        operation_id: uuid::Uuid::new_v4().to_string(),
        operation: json!({"type":"catch_up","after_seq":0,"limit":64}),
        nonce: "1".repeat(64),
        created_at: chrono::Utc::now().to_rfc3339(),
        signature: String::new(),
    };
    assert!(records.len() <= 64);
    let mut batch = authority::CatchupBatch {
        version: 1,
        authority_identity: bound.authority_identity.clone(),
        channel_id: "secure".into(),
        request_hash: authority::digest(&request.signing_value()),
        after_seq: 0,
        head_seq: bound.head_seq,
        head_hash: bound.head_hash.clone(),
        records,
        access_revoked: false,
        more: false,
        signature: String::new(),
    };
    let signed = json!([
        "annex-channel-authority-batch-v1",
        batch.version,
        batch.authority_identity,
        batch.channel_id,
        batch.request_hash,
        batch.after_seq,
        batch.head_seq,
        batch.head_hash,
        batch.records,
        batch.access_revoked,
        batch.more
    ]);
    batch.signature = hex::encode(
        f.state
            .signing_key
            .sign(authority::canonical_json(&signed).as_bytes())
            .to_bytes(),
    );
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    authority::apply_batch(&tx, &replica, &request, &batch, &pid("bob"), &bob).unwrap();
    tx.commit().unwrap();
    let (replica_wrap,namespace,authorization):(String,String,String)=conn.query_row("SELECT w.wrapped_key_b64,e.authority_namespace,w.authorization_sha256 FROM channel_device_key_wraps w JOIN channel_key_epochs e ON e.server_id=w.server_id AND e.channel_id=w.channel_id AND e.epoch=w.epoch WHERE w.channel_id='secure' AND w.recipient_pseudonym_id=?1 AND w.recipient_device_id=?2",params![pid("bob"),bob],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(replica_wrap, original_wrap);
    assert_eq!(namespace, original_namespace);
    assert_eq!(namespace, bound.authority_identity);
    assert!(!authorization.is_empty());
    let plain =
        annex_federation::seal::open_x25519(&STANDARD.decode(replica_wrap).unwrap(), &[21; 32])
            .unwrap();
    let opened: Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(opened[1], namespace);
    assert_eq!(opened[7], STANDARD.encode([47; 32]));
    let projection = manifest::projection(&conn, &replica, "secure")
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(projection).unwrap(),
        serde_json::to_value(
            manifest::projection(&f.state.pool.get().unwrap(), &f.state, "secure")
                .unwrap()
                .unwrap()
        )
        .unwrap()
    );
}

#[tokio::test]
async fn devices_are_immutable_multiple_and_revocation_is_not_republication() {
    let f = Fixture::new();
    let first = f.publish("alice", 11).await;
    let second = f.publish("alice", 12).await;
    assert_ne!(first, second);
    let (_, directory) = f.call("GET", "/api/keys/alice", "bob", None).await;
    assert_eq!(directory["devices"].as_array().unwrap().len(), 2);
    let revision = f.directory().await["revision"].as_i64().unwrap();
    f.publish("alice", 11).await;
    assert_eq!(f.directory().await["revision"], revision);
    assert_eq!(
        f.call("DELETE", &format!("/api/keys/me/{first}"), "bob", None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.call("DELETE", &format!("/api/keys/me/{first}"), "alice", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.event(
        "alice",
        &second,
        manifest::Operation::RevokeDevice {
            device_id: first.clone(),
        },
    )
    .await;
    assert_eq!(
        f.call("DELETE", &format!("/api/keys/me/{first}"), "alice", None)
            .await
            .0,
        StatusCode::OK
    );
    let key = hex::encode(annex_federation::seal::x25519_public_key(&[11; 32]));
    assert_eq!(
        f.call(
            "PUT",
            "/api/keys/me",
            "alice",
            Some(json!({"version":2,"x25519_pub_hex":key,"device_certificate":cert(11)}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for key in [
        "00".repeat(32),
        format!("01{}", "00".repeat(31)),
        "FF".repeat(32),
    ] {
        assert_eq!(
            f.call(
                "PUT",
                "/api/keys/me",
                "alice",
                Some(json!({"version":2,"x25519_pub_hex":key}))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
}
#[tokio::test]
async fn cas_has_one_winner_and_every_device_opens_that_committed_key() {
    // Independent SQLite connections are necessary to exercise writer CAS.
    let disk = tempfile::tempdir().unwrap();
    let f = Fixture::with_pool(
        annex_db::create_pool(
            disk.path().join("epoch.db").to_str().unwrap(),
            annex_db::DbRuntimeSettings {
                pool_max_size: 4,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    f.publish("alice", 11).await;
    let bob = f.publish("bob", 21).await;
    let directory = f.directory().await;
    let first = wrap_request(&directory, "commit_epoch", 1, 31);
    let loser = wrap_request(&directory, "commit_epoch", 1, 32);
    let ((a, _), (b, value)) = tokio::join!(
        f.call(
            "POST",
            "/api/channels/secure/key-wraps",
            "alice",
            Some(first)
        ),
        f.call(
            "POST",
            "/api/channels/secure/key-wraps",
            "alice",
            Some(loser)
        )
    );
    assert!(
        (a == StatusCode::OK && b == StatusCode::CONFLICT)
            || (b == StatusCode::OK && a == StatusCode::CONFLICT),
        "{a} {b} {value}"
    );
    let (_, wraps) = f
        .call(
            "GET",
            &format!("/api/channels/secure/key-wraps?device_id={bob}"),
            "bob",
            None,
        )
        .await;
    let blob = STANDARD
        .decode(wraps["wraps"][0]["wrapped_key_b64"].as_str().unwrap())
        .unwrap();
    let plaintext = annex_federation::seal::open_x25519(&blob, &[21; 32]).unwrap();
    let opened: Value = serde_json::from_slice(&plaintext).unwrap();
    assert_eq!(opened[4], wraps["state"]["current_key_id"]);
    assert_eq!(opened[5], pid("bob"));
    assert_eq!(opened[6], bob);
    assert!(annex_federation::seal::open_x25519(&blob, &[11; 32]).is_err());
}
#[tokio::test]
async fn invalid_recipient_or_missing_device_rolls_back_the_entire_epoch() {
    let f = Fixture::new();
    f.publish("alice", 11).await;
    f.publish("bob", 21).await;
    for mutation in [0, 1, 2] {
        let mut request = wrap_request(&f.directory().await, "commit_epoch", 1, 31);
        if mutation == 0 {
            request["wraps"].as_array_mut().unwrap().pop();
        }
        if mutation == 1 {
            request["wraps"][1]["recipient_pseudonym_id"] = json!("outsider");
        }
        if mutation == 2 {
            request["wraps"][1]["wrapped_key_b64"] = json!("====");
        }
        let (status, value) = f
            .call(
                "POST",
                "/api/channels/secure/key-wraps",
                "alice",
                Some(request),
            )
            .await;
        assert_eq!(
            status,
            if mutation == 2 {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::FORBIDDEN
            },
            "{value}"
        );
        assert_eq!(f.directory().await["current_epoch"], 0);
        assert_eq!(
            f.state
                .pool
                .get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM channel_key_epochs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
#[tokio::test]
async fn departure_blocks_stale_sends_and_edits_but_preserves_idempotent_history() {
    let f = Fixture::new();
    f.publish("alice", 11).await;
    f.publish("bob", 21).await;
    f.commit(31).await;
    let svc = ChannelService::new(f.state.clone());
    assert!(svc
        .send_message(&pid("alice"), "secure", "plaintext".into(), None, None)
        .await
        .is_err());
    let (message, _, _) = svc
        .send_message(
            &pid("alice"),
            "secure",
            body(1, 31),
            None,
            Some("accepted".into()),
        )
        .await
        .unwrap();
    {
        f.state
            .pool
            .get()
            .unwrap()
            .execute(
                "DELETE FROM channel_members WHERE channel_id='secure' AND pseudonym_id=?1",
                [pid("bob")],
            )
            .unwrap();
    }
    assert_eq!(f.directory().await["required_epoch"], 2);
    assert!(svc
        .send_message(&pid("alice"), "secure", body(1, 31), None, None)
        .await
        .is_err());
    assert!(svc
        .edit_message(&pid("alice"), "secure", &message.message_id, &body(1, 31))
        .await
        .is_err());
    let (replay, _, _) = svc
        .send_message(
            &pid("alice"),
            "secure",
            body(1, 31),
            None,
            Some("accepted".into()),
        )
        .await
        .unwrap();
    assert_eq!(replay.message_id, message.message_id);
    f.commit(32).await;
    assert!(svc
        .send_message(&pid("alice"), "secure", body(2, 32), None, None)
        .await
        .is_ok());
    assert_eq!(
        f.call("GET", "/api/channels/secure/member-keys", "bob", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES(1,'secure',?1)",
            [pid("bob")],
        )
        .unwrap();
    let directory = f.directory().await;
    assert_eq!(directory["history_policy"], "all_members");
    assert_eq!(directory["historical_epochs"][0]["epoch"], 1);
    assert_eq!(directory["current_epoch"], 2);
}
#[tokio::test]
async fn device_admission_lists_only_missing_wraps_and_does_not_replace_existing() {
    let f = Fixture::new();
    f.publish("alice", 11).await;
    let bob = f.publish("bob", 21).await;
    f.commit(31).await;
    let new_device = f.publish("bob", 22).await;
    let directory = f.directory().await;
    assert_eq!(
        directory["missing_current_devices"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut request = wrap_request(&directory, "admit_devices", 1, 31);
    request["wraps"]
        .as_array_mut()
        .unwrap()
        .retain(|w| w["recipient_device_id"] == new_device);
    sign_wrap(&mut request, &directory);
    assert_eq!(
        f.call(
            "POST",
            "/api/channels/secure/key-wraps",
            "alice",
            Some(request.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(f.directory().await["missing_current_devices"], json!([]));
    request["expected_revision"] = f.directory().await["revision"].clone();
    sign_wrap(&mut request, &f.directory().await);
    assert_eq!(
        f.call(
            "POST",
            "/api/channels/secure/key-wraps",
            "alice",
            Some(request.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    request["wraps"][0]["wrapped_key_b64"] = json!(STANDARD.encode([5u8; 70]));
    sign_wrap(&mut request, &f.directory().await);
    assert_eq!(
        f.call(
            "POST",
            "/api/channels/secure/key-wraps",
            "alice",
            Some(request)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        f.call(
            "GET",
            &format!("/api/channels/secure/key-wraps?device_id={bob}"),
            "alice",
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
#[test]
fn message_header_parser_rejects_noncanonical_or_unsafe_epochs() {
    for n in ["0", "01", "+1", "-1", "1e1", "9007199254740992"] {
        assert!(e2e_service::parse_message_body(&format!(
            "e2e2:{n}:{}:{}",
            key_id(1),
            STANDARD.encode([0u8; 28])
        ))
        .is_err());
    }
    assert!(e2e_service::parse_message_body(&body(1, 1)).is_ok());
    assert!(e2e_service::parse_message_body("e2e1:old").is_err());
}

#[tokio::test]
async fn revocation_and_disable_enable_rotation_keep_history_but_block_old_keys() {
    let f = Fixture::new();
    let alice = f.publish("alice", 11).await;
    let remaining = f.publish("alice", 12).await;
    f.publish("bob", 21).await;
    f.commit(31).await;
    f.event(
        "alice",
        &remaining,
        manifest::Operation::RevokeDevice {
            device_id: alice.clone(),
        },
    )
    .await;
    assert_eq!(
        f.call("DELETE", &format!("/api/keys/me/{alice}"), "alice", None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(f.directory().await["required_epoch"], 2);
    assert_eq!(
        f.call(
            "GET",
            &format!("/api/channels/secure/key-wraps?device_id={alice}"),
            "alice",
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    f.commit(32).await;
    assert_eq!(
        f.call(
            "PUT",
            "/api/channels/secure/e2e",
            "bob",
            Some(json!({"enabled":false}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    f.mode(false).await;
    assert_eq!(f.directory().await["e2e_enabled"], false);
    let svc = ChannelService::new(f.state.clone());
    assert!(svc
        .send_message(&pid("alice"), "secure", "ordinary".into(), None, None)
        .await
        .is_ok());
    f.mode(true).await;
    assert_eq!(f.directory().await["e2e_enabled"], true);
    assert_eq!(f.directory().await["current_epoch"], 2);
    assert_eq!(f.directory().await["required_epoch"], 3);
    assert!(svc
        .send_message(&pid("alice"), "secure", body(2, 32), None, None)
        .await
        .is_err());
    assert_eq!(
        f.state
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM channel_key_epochs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[tokio::test]
async fn historical_wrap_pages_and_missing_devices_are_explicit() {
    let f = Fixture::new();
    let alice = f.publish("alice", 11).await;
    f.publish("bob", 21).await;
    f.commit(31).await;
    for cek in [32, 33] {
        let conn = f.state.pool.get().unwrap();
        conn.execute(
            "UPDATE platform_identities SET active=0 WHERE pseudonym_id=?1",
            [pid("bob")],
        )
        .unwrap();
        conn.execute(
            "UPDATE platform_identities SET active=1 WHERE pseudonym_id=?1",
            [pid("bob")],
        )
        .unwrap();
        drop(conn);
        f.commit(cek).await;
    }
    let new_device = f.publish("bob", 22).await;
    let (_, first) = f
        .call(
            "GET",
            "/api/channels/secure/member-keys?limit=1",
            "alice",
            None,
        )
        .await;
    assert_eq!(first["historical_epochs"].as_array().unwrap().len(), 1);
    assert_eq!(first["next_history_after_epoch"], 1);
    assert_eq!(
        first["historical_epochs"][0]["missing_devices"][0]["device_id"],
        new_device
    );
    let (_, page) = f
        .call(
            "GET",
            &format!("/api/channels/secure/key-wraps?device_id={alice}&limit=1&after_epoch=1"),
            "alice",
            None,
        )
        .await;
    assert_eq!(page["wraps"][0]["epoch"], 2);
    assert_eq!(page["next_after_epoch"], 2);
    let directory = f.directory().await;
    let mut req = wrap_request(&directory, "admit_history_devices", 1, 31);
    req["wraps"]
        .as_array_mut()
        .unwrap()
        .retain(|w| w["recipient_device_id"] == new_device);
    sign_wrap(&mut req, &directory);
    assert_eq!(
        f.call("POST", "/api/channels/secure/key-wraps", "alice", Some(req))
            .await
            .0,
        StatusCode::OK
    );
    let (_, wraps) = f
        .call(
            "GET",
            &format!("/api/channels/secure/key-wraps?device_id={new_device}"),
            "bob",
            None,
        )
        .await;
    assert_eq!(wraps["wraps"][0]["epoch"], 1);
    let svc = ChannelService::new(f.state.clone());
    assert!(svc
        .send_message(&pid("bob"), "secure", body(1, 31), None, None)
        .await
        .is_err());
}
#[tokio::test]
async fn legacy_history_bridges_without_reusing_a_legacy_write_epoch() {
    let f = Fixture::new();
    let alice = f.publish("alice", 11).await;
    let bob = f.publish("bob", 21).await;
    let old_blob = annex_federation::seal::seal_x25519(
        &[30; 32],
        &annex_federation::seal::x25519_public_key(&[11; 32]),
    )
    .unwrap();
    f.state.pool.get().unwrap().execute("INSERT INTO channel_key_wraps(server_id,channel_id,recipient_pseudonym_id,sender_pseudonym_id,key_epoch,wrapped_key_b64) VALUES(1,'secure',?1,?1,7,?2)",params![pid("alice"),STANDARD.encode(old_blob)]).unwrap();
    // An actual upgrade initializes from ALL prior epochs, even if their
    // recipients leave. Simulate that migration state independently of devices.
    f.state
        .pool
        .get()
        .unwrap()
        .execute("UPDATE channel_key_state SET required_epoch=8", [])
        .unwrap();
    let directory = f.directory().await;
    let req = wrap_request(&directory, "admit_legacy_devices", 7, 30);
    assert_eq!(
        f.call("POST", "/api/channels/secure/key-wraps", "alice", Some(req))
            .await
            .0,
        StatusCode::OK
    );
    let (_, response) = f
        .call(
            "GET",
            &format!("/api/channels/secure/key-wraps?device_id={bob}"),
            "bob",
            None,
        )
        .await;
    assert_eq!(response["state"]["current_epoch"], 0);
    assert_eq!(response["state"]["required_epoch"], 8);
    let blob = STANDARD
        .decode(
            response["legacy_device_wraps"][0]["wrapped_key_b64"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    let plaintext = annex_federation::seal::open_x25519(&blob, &[21; 32]).unwrap();
    let value: Value = serde_json::from_slice(&plaintext).unwrap();
    assert_eq!(value[0], "annex-legacy-key-wrap-v2");
    assert_eq!(value[7], STANDARD.encode([30; 32]));
    let (_, own) = f
        .call(
            "GET",
            &format!("/api/channels/secure/key-wraps?device_id={alice}"),
            "alice",
            None,
        )
        .await;
    assert_eq!(own["legacy_wraps"][0]["epoch"], 7);
    assert!(ChannelService::new(f.state.clone())
        .send_message(&pid("alice"), "secure", body(7, 30), None, None)
        .await
        .is_err());
}
#[tokio::test]
async fn staged_epoch_rolls_back_if_any_wrap_write_fails() {
    let f = Fixture::new();
    f.publish("alice", 11).await;
    f.publish("bob", 21).await;
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER deny_second_wrap BEFORE INSERT ON channel_device_key_wraps WHEN NEW.recipient_device_id=(SELECT device_id FROM member_device_keys ORDER BY device_id LIMIT 1) BEGIN SELECT RAISE(ABORT,'disk-equivalent fault'); END;").unwrap();
    let req = wrap_request(&f.directory().await, "commit_epoch", 1, 31);
    assert_eq!(
        f.call("POST", "/api/channels/secure/key-wraps", "alice", Some(req))
            .await
            .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let conn = f.state.pool.get().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM channel_key_epochs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM channel_device_key_wraps", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    drop(conn);
    assert_eq!(f.directory().await["current_epoch"], 0);
}
#[tokio::test]
async fn deleting_a_channel_cascades_all_key_material_and_pending_notifications() {
    let f = Fixture::new();
    f.publish("alice", 11).await;
    f.publish("bob", 21).await;
    f.commit(31).await;
    let conn = f.state.pool.get().unwrap();
    conn.execute("DELETE FROM channel_members WHERE channel_id='secure'", [])
        .unwrap();
    conn.execute("DELETE FROM channels WHERE channel_id='secure'", [])
        .unwrap();
    for table in [
        "channel_key_state",
        "channel_key_epochs",
        "channel_device_key_wraps",
        "channel_key_notifications",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM member_device_keys", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[tokio::test]
async fn legacy_writes_and_outsider_key_history_access_fail_closed() {
    let f = Fixture::new();
    let id = f.publish("alice", 11).await;
    assert_eq!(f.call("PUT","/api/keys/me","alice",Some(json!({"x25519_pub_hex":hex::encode(annex_federation::seal::x25519_public_key(&[11;32]))}))).await.0,StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        f.call(
            "POST",
            "/api/channels/secure/key-wraps",
            "alice",
            Some(json!({"epoch":1,"wraps":[]}))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for path in [
        "/api/channels/secure/member-keys".to_string(),
        "/api/channels/secure/key-status".to_string(),
        format!("/api/channels/secure/key-wraps?device_id={id}"),
    ] {
        assert_eq!(
            f.call("GET", &path, "outsider", None).await.0,
            StatusCode::FORBIDDEN
        );
    }
    let (_, missing) = f.call("GET", "/api/keys/outsider", "alice", None).await;
    assert_eq!(missing["devices"], json!([]));
}
