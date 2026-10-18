//! Component fault tests use real SQLite, Ed25519 and files. They do not claim
//! a real registered browser or cross-host federation acceptance session.
#![cfg(test)]
use super::*;
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_types::ServerPolicy;
use std::sync::{Mutex, RwLock};
// Exact public identities generated through the live registry and certificate API.
const ALICE: &str = "a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9";
const ALICE_DEVICE: &str = "dd2c3e188c79af41e0306fe45d663659851ce03ed11856afe8cdc61d87934e41";
const BOB: &str = "55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe";
const BOB_DEVICE: &str = "fddf24e3925a6f7fa3735559b659672c50c1b8d3d9ed4c37c5f059f3b6d2b6c1";

fn certified_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../tests/fixtures/attachment-certified-public.json"
    ))
    .unwrap()
}
fn install_certified_member(state: &AppState, index: usize) {
    let certificate: crate::device_certificates::DeviceCertificate =
        serde_json::from_value(certified_fixture()["certificates"][index].clone()).unwrap();
    crate::device_certificates::verify_certificate(
        &certificate,
        &certificate.server_identity,
        &certificate.pseudonym_id,
        state.membership_vkey_v2.as_ref().unwrap(),
    )
    .unwrap();
    state.pool.get().unwrap().execute("UPDATE member_device_keys SET device_certificate=?1,certificate_sha256=?2 WHERE server_id=1 AND pseudonym_id=?3 AND device_id=?4",
        params![serde_json::to_string(&certificate).unwrap(),crate::device_certificates::certificate_digest(&certificate),certificate.pseudonym_id,certificate.device_id]).unwrap();
}
fn install_certified_manifest(state: &AppState) {
    use crate::services::channel_manifest::{self, SignedManifestEvent};
    let conn = state.pool.get().unwrap();
    let mut projection = None;
    for value in certified_fixture()["channels"]["general"]
        .as_array()
        .unwrap()
    {
        let event: SignedManifestEvent = serde_json::from_value(value.clone()).unwrap();
        let next = channel_manifest::verify_transition(
            projection.as_ref(),
            &event,
            state.membership_vkey_v2.as_ref().unwrap(),
        )
        .unwrap();
        channel_manifest::persist(&conn, state, "general", &event, &next).unwrap();
        projection = Some(next);
    }
    channel_manifest::require_admitted_device(&conn, state, "general", ALICE, ALICE_DEVICE)
        .unwrap();
}
fn state(pool: annex_db::DbPool, root: &Path) -> AppState {
    AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(annex_identity::MerkleTree::new(20).unwrap())),
        membership_vkey: Arc::new(annex_identity::zk::generate_dummy_vkey()),
        membership_vkey_v2: Some(Arc::new(
            annex_identity::zk::parse_verification_key(&certified_fixture()["vkey"].to_string())
                .unwrap(),
        )),
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3002".into())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: crate::middleware::RateLimiter::new(),
        connection_manager: crate::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::WebRtcConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("unused", "unused")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(100).0,
        upload_dir: root.to_string_lossy().into_owned(),
        preview_cache: crate::api_link_preview::PreviewCache::new(),
        ws_token_secret: Arc::new([0; 32]),
        voice_token_secret: Arc::new([0; 32]),
        cors_origins: vec![],
        enforce_zk_proofs: true,
        invite_base_url: "http://localhost:3002/invite".into(),
        federation_config: Default::default(),
        federation_transport: Default::default(),
        storage_config: Default::default(),
        storage_health: Arc::new(crate::storage_health::StorageHealth::new()),
        metrics: Default::default(),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
    }
}
struct Fixture {
    state: AppState,
    dir: tempfile::TempDir,
    accepted: AcceptedAttachment,
    bytes: Vec<u8>,
    peer_key: Arc<ed25519_dalek::SigningKey>,
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let pool = create_pool(
        dir.path().join("test.db").to_str().unwrap(),
        DbRuntimeSettings::default(),
    )
    .unwrap();
    let conn = pool.get().unwrap();
    run_migrations(&conn).unwrap();
    // When this module is first registered, these migrations may still be
    // waiting for the shared registry checkpoint. Execute only if absent.
    let authority_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='channel_authorities')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !authority_exists {
        conn.execute_batch(include_str!(
            "../../../annex-db/src/migrations/054_channel_authority.sql"
        ))
        .unwrap();
    }
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='federated_upload_transfers')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !exists {
        conn.execute_batch(include_str!(
            "../../../annex-db/src/migrations/055_federated_attachments.sql"
        ))
        .unwrap();
    }
    conn.execute(
        "INSERT INTO servers(slug,label,policy_json) VALUES('replica','Replica',?1)",
        [serde_json::to_string(&ServerPolicy::default()).unwrap()],
    )
    .unwrap();
    conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_voice,can_moderate,can_invite,can_federate,can_bridge,active) VALUES(1,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9','HUMAN',1,0,0,0,0,1)",[]).unwrap();
    conn.execute("INSERT INTO channels(channel_id,server_id,name,channel_type,federation_scope) VALUES('general',1,'General',json_quote('Text'),json_quote('Federated'))",[]).unwrap();
    conn.execute("INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES(1,'general','a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9')",[]).unwrap();
    conn.execute("INSERT INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex) VALUES(1,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?1,?2)",params![ALICE_DEVICE.to_string(),certified_fixture()["certificates"][0]["x25519_pub_hex"].as_str().unwrap()]).unwrap();
    conn.execute("INSERT INTO messages(server_id,channel_id,message_id,sender_pseudonym,content) VALUES(1,'general','message','author','opaque')",[]).unwrap();
    let origin = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let key = hex::encode(origin.verifying_key().to_bytes());
    conn.execute("INSERT INTO instances(base_url,public_key,label,status) VALUES('http://localhost:3000',?1,'Origin','ACTIVE')",[&key]).unwrap();
    conn.execute("INSERT INTO federation_agreements(local_server_id,remote_instance_id,alignment_status,transfer_scope,agreement_json,active) VALUES(1,1,'ALIGNED','FULL_KNOWLEDGE_BUNDLE','{}',1)",[]).unwrap();
    let bytes = (0..(wire::CHUNK_BYTES as usize + 177))
        .map(|i| ((i * 13 + 31) % 251) as u8)
        .collect::<Vec<_>>();
    let accepted = AcceptedAttachment {
        authority_identity: certified_fixture()["authority_identity"]
            .as_str()
            .unwrap()
            .into(),
        channel_id: "general".into(),
        message_id: "message".into(),
        accepted_event_seq: 7,
        accepted_event_hash: "ab".repeat(32),
        descriptor: wire::AttachmentDescriptor {
            attachment_id: uuid::Uuid::new_v4().to_string(),
            origin_identity: wire::stable_identity(&origin.verifying_key().to_bytes()),
            originating_server: "http://localhost:3000".into(),
            size_bytes: bytes.len() as u64,
            ciphertext_sha256: hex::encode(Sha256::digest(&bytes)),
            category: "file".into(),
            epoch: 1,
            key_id: "cd".repeat(32),
            plaintext: None,
        },
    };
    let authority_key = certified_fixture()["authority_public_key"]
        .as_str()
        .unwrap()
        .to_string();
    conn.execute("INSERT INTO instances(id,base_url,public_key,label,status) VALUES(2,'http://localhost:3004',?1,'Authority','ACTIVE')",[&authority_key]).unwrap();
    conn.execute("INSERT INTO federation_agreements(local_server_id,remote_instance_id,alignment_status,transfer_scope,agreement_json,active) VALUES(1,2,'ALIGNED','FULL_KNOWLEDGE_BUNDLE','{}',1)",[]).unwrap();
    conn.execute("INSERT INTO channel_authorities(server_id,channel_id,authority_identity,authority_key_hex,authority_url,authority_peer_id,local_authority,head_seq,head_hash) VALUES(1,'general',?1,?2,'http://localhost:3004',2,0,7,?3)",params![accepted.authority_identity,authority_key,accepted.accepted_event_hash]).unwrap();
    drop(conn);
    let state = state(pool, dir.path());
    install_certified_member(&state, 0);
    install_certified_manifest(&state);
    Fixture {
        state,
        dir,
        accepted,
        bytes,
        peer_key: Arc::new(origin),
    }
}
fn record(f: &Fixture) {
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    record_expected(&tx, &f.state, &f.accepted, "author", ALICE, ALICE_DEVICE).unwrap();
    tx.commit().unwrap();
}
fn progress(f: &Fixture) -> (u64, String) {
    f.state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT committed_bytes,status FROM federated_upload_transfers",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}
fn stage(f: &Fixture) -> PathBuf {
    let relative: String = f
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT staging_path FROM federated_upload_transfers WHERE upload_id=?1",
            [&f.accepted.descriptor.attachment_id],
            |r| r.get(0),
        )
        .unwrap();
    f.dir.path().join(relative)
}

#[test]
fn expected_attachment_is_atomic_and_unreadable_before_actual_transfer() {
    let f = fixture();
    let conn = f.state.pool.get().unwrap();
    assert!(record_expected(&conn, &f.state, &f.accepted, "author", ALICE, ALICE_DEVICE).is_err());
    drop(conn);
    {
        let mut conn = f.state.pool.get().unwrap();
        let tx = conn.transaction().unwrap();
        record_expected(&tx, &f.state, &f.accepted, "author", ALICE, ALICE_DEVICE).unwrap();
    }
    assert_eq!(
        f.state
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM uploads", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    record(&f);
    assert_eq!(progress(&f), (0, "pending".into()));
    assert!(!stage(&f).exists());
    let relative = format!("chat/files/{}.bin", f.accepted.descriptor.attachment_id);
    assert!(upload_service::read_upload_for_grant(
        &f.state,
        &relative,
        &f.accepted.descriptor.attachment_id,
        ALICE,
        0
    )
    .is_err());
}
#[test]
fn exact_retry_is_idempotent_but_uuid_rebinding_and_tombstone_replay_fail() {
    let f = fixture();
    record(&f);
    record(&f);
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn.transaction().unwrap();
    let mut changed = f.accepted.clone();
    changed.descriptor.ciphertext_sha256 = "11".repeat(32);
    assert!(record_expected(&tx, &f.state, &changed, "author", ALICE, ALICE_DEVICE).is_err());
    drop(tx);
    let tx = conn.transaction().unwrap();
    remove_expected(
        &tx,
        1,
        "general",
        Some("message"),
        None,
        "deleted",
        (9, &"ef".repeat(32)),
    )
    .unwrap();
    tx.commit().unwrap();
    let tx = conn.transaction().unwrap();
    assert!(record_expected(&tx, &f.state, &f.accepted, "author", ALICE, ALICE_DEVICE).is_err());
}
#[test]
fn sync_before_progress_crash_truncates_uncommitted_tail_then_repeats_exact_range() {
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    let (mut file, _) = open_prefix(f.dir.path(), &t).unwrap();
    file.write_all(&f.bytes[..wire::CHUNK_BYTES as usize])
        .unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert_eq!(progress(&f).0, 0);
    persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    assert_eq!(progress(&f).0, wire::CHUNK_BYTES);
    assert_eq!(
        fs::read(stage(&f)).unwrap(),
        f.bytes[..wire::CHUNK_BYTES as usize]
    );
    assert!(persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).is_err());
    let next = claim(&f.state).unwrap().unwrap();
    assert_eq!(next.committed, wire::CHUNK_BYTES);
    persist_chunk(&f.state, &next, &f.bytes[wire::CHUNK_BYTES as usize..]).unwrap();
    assert_eq!(fs::read(stage(&f)).unwrap(), f.bytes);
    assert_eq!(progress(&f).0, f.bytes.len() as u64);
}
#[test]
fn database_progress_failure_keeps_old_offset_and_recovery_discards_uncommitted_bytes() {
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER fail_progress BEFORE UPDATE OF committed_bytes ON federated_upload_transfers BEGIN SELECT RAISE(ABORT,'deliberate transaction fault'); END").unwrap();
    assert!(persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).is_err());
    assert_eq!(progress(&f).0, 0);
    assert!(stage(&f).exists());
    f.state
        .pool
        .get()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_progress")
        .unwrap();
    persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    assert_eq!(fs::metadata(stage(&f)).unwrap().len(), wire::CHUNK_BYTES);
}
#[test]
fn restart_same_disk_recovers_committed_prefix_and_expired_lease() {
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let lost = claim(&f.state).unwrap().unwrap();
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federated_upload_transfers SET lease_expires_at=0",
            [],
        )
        .unwrap();
    let Fixture {
        state: old_state,
        dir,
        accepted,
        bytes,
        peer_key,
    } = f;
    let signing_key = old_state.signing_key.clone();
    drop(old_state);
    let reopened = create_pool(
        dir.path().join("test.db").to_str().unwrap(),
        DbRuntimeSettings::default(),
    )
    .unwrap();
    let mut new_state = state(reopened, dir.path());
    new_state.signing_key = signing_key;
    let f = Fixture {
        state: new_state,
        dir,
        accepted,
        bytes,
        peer_key,
    };
    let recovered = claim(&f.state).unwrap().unwrap();
    assert_ne!(recovered.lease, lost.lease);
    assert!(persist_chunk(&f.state, &lost, &f.bytes[wire::CHUNK_BYTES as usize..]).is_err());
    let (file, _) = open_prefix(f.dir.path(), &recovered).unwrap();
    assert_eq!(file.metadata().unwrap().len(), wire::CHUNK_BYTES);
    persist_chunk(&f.state, &recovered, &f.bytes[wire::CHUNK_BYTES as usize..]).unwrap();
    assert_eq!(fs::read(stage(&f)).unwrap(), f.bytes);
}
#[test]
fn corrupt_or_missing_committed_prefix_is_not_silently_recreated() {
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let t = claim(&f.state).unwrap().unwrap();
    let mut file = OpenOptions::new().write(true).open(stage(&f)).unwrap();
    file.write_all(b"changed").unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert!(persist_chunk(&f.state, &t, &f.bytes[wire::CHUNK_BYTES as usize..]).is_err());
    assert_eq!(progress(&f).0, wire::CHUNK_BYTES);
    fs::remove_file(stage(&f)).unwrap();
    assert!(open_prefix(f.dir.path(), &t).is_err());
    assert!(!stage(&f).exists());
}
#[test]
fn final_hash_mismatch_never_advances_progress_or_exposes_ready() {
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let t = claim(&f.state).unwrap().unwrap();
    let mut last = f.bytes[wire::CHUNK_BYTES as usize..].to_vec();
    last[0] ^= 1;
    assert!(persist_chunk(&f.state, &t, &last).is_err());
    assert_eq!(progress(&f), (wire::CHUNK_BYTES, "receiving".into()));
    persist_chunk(&f.state, &t, &f.bytes[wire::CHUNK_BYTES as usize..]).unwrap();
    assert_eq!(fs::read(stage(&f)).unwrap(), f.bytes);
}
#[test]
fn deletion_during_inflight_chunk_is_serialized_cleans_partial_and_never_resurrects() {
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let late = claim(&f.state).unwrap().unwrap();
    {
        let mut conn = f.state.pool.get().unwrap();
        let tx = conn.transaction().unwrap();
        remove_expected(
            &tx,
            1,
            "general",
            Some("message"),
            None,
            "deleted",
            (8, &"ee".repeat(32)),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert!(persist_chunk(&f.state, &late, &f.bytes[wire::CHUNK_BYTES as usize..]).is_err());
    assert_eq!(progress(&f).1, "removed");
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(!stage(&f).exists());
    assert!(claim(&f.state).unwrap().is_none());
    assert!(retry_transfer(&f.state.pool.get().unwrap(), 1, late.id).is_err());
}

#[test]
fn expired_unswept_messages_block_claim_late_progress_operator_retry_and_bootstrap() {
    let f = fixture();
    record(&f);
    let first = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &first, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let late = claim(&f.state).unwrap().unwrap();
    let before = progress(&f);
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET expires_at='2000-01-01 00:00:00' WHERE message_id='message'",
            [],
        )
        .unwrap();
    assert!(stage(&f).exists());
    assert!(persist_chunk(&f.state, &late, &f.bytes[wire::CHUNK_BYTES as usize..]).is_err());
    assert_eq!(progress(&f), before);
    let conn = f.state.pool.get().unwrap();
    conn.execute(
        "UPDATE federated_upload_transfers SET lease_expires_at=0",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(claim(&f.state).unwrap().is_none());
    let mut conn = f.state.pool.get().unwrap();
    conn.execute("UPDATE federated_upload_transfers SET status='failed'", [])
        .unwrap();
    assert!(retry_transfer(&conn, 1, late.id).is_err());
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert!(descriptors_for_bootstrap_message(&tx, &f.state, "general", "message").is_err());
    assert!(record_expected(&tx, &f.state, &f.accepted, "author", ALICE, ALICE_DEVICE).is_err());
    assert_eq!(
        tx.query_row(
            "SELECT COUNT(*) FROM messages WHERE message_id='message' AND deleted_at IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn expired_unswept_message_refuses_origin_chunk_and_local_origin_browser_read() {
    let remote = remote_origin_fixture();
    serve_chunk(Arc::new(remote.state.clone()), origin_request(&remote, 0))
        .await
        .unwrap();
    remote
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET expires_at='2000-01-01 00:00:00' WHERE message_id='message'",
            [],
        )
        .unwrap();
    assert!(serve_chunk(
        Arc::new(remote.state.clone()),
        origin_request(&remote, wire::CHUNK_BYTES)
    )
    .await
    .is_err());
    let local = origin_fixture();
    let id = &local.accepted.descriptor.attachment_id;
    let relative = local.accepted.descriptor.public_path();
    local
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "DELETE FROM federated_identities WHERE pseudonym_id=?1",
            [ALICE],
        )
        .unwrap();
    let state = Arc::new(local.state.clone());
    read_origin_for_grant(state.clone(), &relative, id, ALICE, ALICE_DEVICE, 0)
        .await
        .unwrap();
    local
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET expires_at='2000-01-01 00:00:00' WHERE message_id='message'",
            [],
        )
        .unwrap();
    assert!(Path::new(&local.state.upload_dir).join(&relative).exists());
    assert!(
        read_origin_for_grant(state.clone(), &relative, id, ALICE, ALICE_DEVICE, 0)
            .await
            .is_err()
    );
    assert!(
        retry_for_member(state, "general", id, ALICE, ALICE_DEVICE, 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn expired_unswept_message_refuses_finalization_replica_read_and_lazy_admission() {
    let f = local_authority_replica_fixture();
    let complete = complete_prefix(&f);
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET expires_at='2000-01-01 00:00:00' WHERE message_id='message'",
            [],
        )
        .unwrap();
    assert!(finalize(Arc::new(f.state.clone()), &complete)
        .await
        .is_err());
    assert!(stage(&f).exists());
    assert_ne!(progress(&f).1, "ready");
    let ready = local_authority_replica_fixture();
    let complete = complete_prefix(&ready);
    finalize(Arc::new(ready.state.clone()), &complete)
        .await
        .unwrap();
    ready
        .state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET expires_at='2000-01-01 00:00:00' WHERE message_id='message'",
            [],
        )
        .unwrap();
    let id = &ready.accepted.descriptor.attachment_id;
    let relative = ready.accepted.descriptor.public_path();
    let stored: String = ready.state.pool.get().unwrap().query_row(
        "SELECT storage_path FROM uploads WHERE server_id=1 AND upload_id=?1 AND lifecycle='ready'",
        [id], |row| row.get(0)).unwrap();
    // Replica URLs identify the immutable descriptor; their actual files use
    // generation-specific storage paths so retries cannot overwrite stale data.
    assert_eq!(
        fs::read(Path::new(&ready.state.upload_dir).join(&stored)).unwrap(),
        ready.bytes
    );
    assert!(read_replica_for_grant(
        Arc::new(ready.state.clone()),
        &relative,
        id,
        ALICE,
        ALICE_DEVICE,
        0
    )
    .await
    .is_err());
    let lazy = deferred_fixture();
    let id = &lazy.accepted.descriptor.attachment_id;
    let relative = lazy.accepted.descriptor.public_path();
    lazy.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE messages SET expires_at='2000-01-01 00:00:00' WHERE message_id='message'",
            [],
        )
        .unwrap();
    assert!(!admit_deferred_for_reader(
        Arc::new(lazy.state.clone()),
        id,
        Some("general"),
        Some(&relative),
        ALICE,
        ALICE_DEVICE,
        0
    )
    .await
    .unwrap());
    assert_eq!(
        lazy.state
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM federated_upload_transfers WHERE upload_id=?1",
                [id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
#[test]
fn soft_hard_and_channel_deletion_cancel_replica_state_through_existing_lifecycle() {
    for sql in [
        "UPDATE messages SET deleted_at=datetime('now') WHERE message_id='message'",
        "DELETE FROM messages WHERE message_id='message'",
        "DELETE FROM messages WHERE channel_id='general'; DELETE FROM channel_members WHERE channel_id='general'; DELETE FROM channels WHERE channel_id='general'",
    ] {
        let f = fixture();
        record(&f);
        let t = claim(&f.state).unwrap().unwrap();
        persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
        let mut conn=f.state.pool.get().unwrap();
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        tx.execute_batch(sql).unwrap();
        tx.commit().unwrap();
        drop(conn);
        assert_eq!(progress(&f).1, "removed");
        upload_service::drain_deletion_queue(&f.state).unwrap();
        assert!(!stage(&f).exists());
    }
}
#[test]
fn revoked_agreement_prevents_persisting_even_an_already_received_valid_chunk() {
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    f.state
        .pool
        .get()
        .unwrap()
        .execute("UPDATE federation_agreements SET active=0", [])
        .unwrap();
    assert!(persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).is_err());
    assert!(!stage(&f).exists());
    assert_eq!(progress(&f).0, 0);
}
#[test]
fn revoked_local_reader_prevents_committing_an_inflight_chunk() {
    for sql in [
        "DELETE FROM channel_members WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE member_device_keys SET revoked_at=datetime('now') WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE platform_identities SET active=0 WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
    ] {
        let f = fixture();
        record(&f);
        let transfer = claim(&f.state).unwrap().unwrap();
        f.state.pool.get().unwrap().execute(sql, []).unwrap();
        assert!(
            persist_chunk(&f.state, &transfer, &f.bytes[..wire::CHUNK_BYTES as usize]).is_err()
        );
        assert_eq!(progress(&f).0, 0);
        assert!(!stage(&f).exists());
    }
}
#[test]
fn terminal_failure_is_visible_and_operator_retry_preserves_exact_bound_progress() {
    let f = fixture();
    record(&f);
    for _ in 0..MAX_FAILURES {
        let t = claim(&f.state).unwrap().unwrap();
        fail_attempt(
            &f.state,
            &t,
            &TransferError::Unavailable("origin offline".into()),
        )
        .unwrap();
        f.state
            .pool
            .get()
            .unwrap()
            .execute("UPDATE federated_upload_transfers SET next_retry_at=0", [])
            .unwrap();
    }
    assert_eq!(progress(&f).1, "failed");
    assert!(claim(&f.state).unwrap().is_none());
    let conn = f.state.pool.get().unwrap();
    let rows = list_transfers(&conn, 1, 50).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].operation_kind, "attachment");
    assert!(rows[0].id < 0);
    assert_eq!(rows[0].attempts, MAX_FAILURES);
    retry_transfer(&conn, 1, -rows[0].id).unwrap();
    assert_eq!(progress(&f), (0, "pending".into()));
    let rebound = claim(&f.state).unwrap().unwrap();
    assert_eq!(rebound.accepted, f.accepted);
}
#[test]
fn permanent_remote_refusal_stops_automatic_retry_but_preserves_verified_prefix() {
    let f = fixture();
    record(&f);
    let first = claim(&f.state).unwrap().unwrap();
    let prefix = &f.bytes[..wire::CHUNK_BYTES as usize];
    persist_chunk(&f.state, &first, prefix).unwrap();
    let second = claim(&f.state).unwrap().unwrap();
    let error = TransferError::Remote(annex_federation::transport::ApplicationError::new(
        403,
        "current reader entitlement refused",
    ));
    assert_eq!(error.status_code().as_u16(), 403);
    fail_attempt(&f.state, &second, &error).unwrap();
    assert_eq!(progress(&f), (prefix.len() as u64, "failed".into()));
    assert_eq!(fs::read(stage(&f)).unwrap(), prefix);
    assert!(claim(&f.state).unwrap().is_none());
    let conn = f.state.pool.get().unwrap();
    let rows = list_transfers(&conn, 1, 50).unwrap();
    assert_eq!(rows[0].attempts, 1);
    retry_transfer(&conn, 1, -rows[0].id).unwrap();
    let resumed = claim(&f.state).unwrap().unwrap();
    assert_eq!(resumed.committed, prefix.len() as u64);
    assert_eq!(resumed.accepted, f.accepted);
}

#[test]
fn transient_remote_refusal_keeps_bounded_backoff_instead_of_terminal_failure() {
    for status in [408, 425, 429, 500, 503] {
        let f = fixture();
        record(&f);
        let transfer = claim(&f.state).unwrap().unwrap();
        fail_attempt(
            &f.state,
            &transfer,
            &TransferError::Remote(annex_federation::transport::ApplicationError::new(
                status,
                "temporarily unavailable",
            )),
        )
        .unwrap();
        assert_eq!(progress(&f), (0, "pending".into()));
        assert!(
            claim(&f.state).unwrap().is_none(),
            "must honor backoff for {status}"
        );
        let conn = f.state.pool.get().unwrap();
        let row: (i64, bool) = conn
            .query_row(
                "SELECT attempts,next_retry_at>unixepoch() FROM federated_upload_transfers",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, (1, true));
    }
}

#[cfg(windows)]
#[test]
fn real_windows_file_lock_retains_cleanup_until_handle_is_released() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(stage(&f))
        .unwrap();
    f.state
        .pool
        .get()
        .unwrap()
        .execute("DELETE FROM messages WHERE message_id='message'", [])
        .unwrap();
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(stage(&f).exists());
    assert!(
        f.state
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM upload_deletion_queue WHERE attempts>0",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap()
            > 0
    );
    drop(lock);
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(!stage(&f).exists());
}

#[test]
fn expired_failed_staging_cannot_race_delayed_cleanup_with_operator_retry() {
    let f = fixture();
    record(&f);
    let t = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &t, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federated_upload_transfers SET status='failed',updated_at=0",
            [],
        )
        .unwrap();
    expire_failed_staging(&f.state).unwrap();
    assert_eq!(progress(&f).0, 0);
    assert!(retry_transfer(&f.state.pool.get().unwrap(), 1, t.id).is_err());
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert!(!stage(&f).exists());
    retry_transfer(&f.state.pool.get().unwrap(), 1, t.id).unwrap();
    let fresh = claim(&f.state).unwrap().unwrap();
    assert_eq!(fresh.committed, 0);
    assert_ne!(fresh.staging, t.staging);
    assert_ne!(fresh.storage, t.storage);
    persist_chunk(&f.state, &fresh, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    assert!(stage(&f).exists());
    // A cleanup drainer that captured the previous generation before retry
    // must be harmless even if it wakes only after new bytes have arrived.
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "INSERT INTO upload_deletion_queue(server_id,relative_path) VALUES(1,?1)",
            [&t.staging],
        )
        .unwrap();
    upload_service::drain_deletion_queue(&f.state).unwrap();
    assert_eq!(
        fs::read(stage(&f)).unwrap(),
        f.bytes[..wire::CHUNK_BYTES as usize]
    );
}

#[test]
fn integrity_failure_discards_untrusted_generation_and_retry_downloads_clean_bytes() {
    let f = fixture();
    record(&f);
    let first = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &first, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let second = claim(&f.state).unwrap().unwrap();
    let mut file = OpenOptions::new().write(true).open(stage(&f)).unwrap();
    file.write_all(b"corrupt prefix").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let error =
        persist_chunk(&f.state, &second, &f.bytes[wire::CHUNK_BYTES as usize..]).unwrap_err();
    assert!(matches!(error, TransferError::Integrity(_)));
    fail_attempt(&f.state, &second, &error).unwrap();
    assert_eq!(progress(&f), (0, "failed".into()));
    assert!(retry_transfer(&f.state.pool.get().unwrap(), 1, second.id).is_err());
    upload_service::drain_deletion_queue(&f.state).unwrap();
    retry_transfer(&f.state.pool.get().unwrap(), 1, second.id).unwrap();
    let fresh = claim(&f.state).unwrap().unwrap();
    assert_ne!(fresh.staging, second.staging);
    persist_chunk(&f.state, &fresh, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    assert_eq!(
        fs::read(stage(&f)).unwrap(),
        f.bytes[..wire::CHUNK_BYTES as usize]
    );
}

fn origin_fixture() -> Fixture {
    let mut f = fixture();
    f.state.signing_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0x55; 32]));
    let local_key = hex::encode(f.state.signing_key.verifying_key().to_bytes());
    let local = local_identity(&f.state);
    f.accepted.authority_identity = local.clone();
    f.accepted.descriptor.origin_identity = local.clone();
    f.accepted.descriptor.originating_server = f.state.get_public_url();
    let d = &f.accepted.descriptor;
    let device = ALICE_DEVICE.to_string();
    let conn = f.state.pool.get().unwrap();
    conn.execute("INSERT INTO federated_identities(server_id,remote_instance_id,commitment_hex,pseudonym_id,vrp_topic) VALUES(1,1,'aa','a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9','general')",[]).unwrap();
    conn.execute(
        "UPDATE channels SET e2e_enabled=1 WHERE channel_id='general'",
        [],
    )
    .unwrap();
    conn.execute("UPDATE channel_key_state SET current_epoch=1,required_epoch=1,current_key_id=?1 WHERE channel_id='general'",[&d.key_id]).unwrap();
    conn.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device) VALUES(1,'general',1,?1,1,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?2)",params![d.key_id,device]).unwrap();
    conn.execute("INSERT INTO channel_device_key_wraps(server_id,channel_id,recipient_pseudonym_id,recipient_device_id,epoch,key_id,sender_pseudonym_id,sender_device_id,wrapped_key_b64) VALUES(1,'general','a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?1,1,?2,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?1,'controlled-test-wrap')",params![device,d.key_id]).unwrap();
    conn.execute("UPDATE channel_authorities SET authority_identity=?1,authority_key_hex=?2,authority_url=?3,authority_peer_id=NULL,local_authority=1,head_seq=7,head_hash=?4 WHERE server_id=1 AND channel_id='general'",params![local,local_key,f.state.get_public_url(),f.accepted.accepted_event_hash]).unwrap();
    conn.execute("INSERT INTO channel_authority_attachment_bindings(server_id,channel_id,message_id,attachment_id,accepted_seq,accepted_hash,descriptor_json) VALUES(1,'general','message',?1,7,?2,?3)",params![d.attachment_id,f.accepted.accepted_event_hash,serde_json::to_string(d).unwrap()]).unwrap();
    let relative = format!("chat/files/{}.bin", d.attachment_id);
    conn.execute("INSERT INTO uploads(server_id,upload_id,uploader_pseudonym,original_filename,content_type,size_bytes,purpose,channel_id,category,encrypted,epoch,key_id,ciphertext_sha256,storage_path,lifecycle) VALUES(1,?1,'author','encrypted.bin','application/octet-stream',?2,'chat','general','file',1,1,?3,?4,?5,'ready')",params![d.attachment_id,d.size_bytes,d.key_id,d.ciphertext_sha256,relative]).unwrap();
    drop(conn);
    let path = upload_service::confined_path(f.dir.path(), &relative, true).unwrap();
    fs::write(path, &f.bytes).unwrap();
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn.transaction().unwrap();
    record_expected(&tx, &f.state, &f.accepted, "author", ALICE, &device).unwrap();
    tx.commit().unwrap();
    drop(conn);
    f
}

#[test]
fn accepted_origin_associates_actual_message_once_and_refuses_cross_message_reuse() {
    let f = origin_fixture();
    let mut conn = f.state.pool.get().unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM message_uploads WHERE upload_id=?1",
            [&f.accepted.descriptor.attachment_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let tx = conn.transaction().unwrap();
    record_expected(&tx, &f.state, &f.accepted, "author", ALICE, ALICE_DEVICE).unwrap();
    tx.commit().unwrap();
    conn.execute("INSERT INTO messages(server_id,channel_id,message_id,sender_pseudonym,content) VALUES(1,'general','other-message','author','opaque')",[]).unwrap();
    let mut changed = f.accepted.clone();
    changed.message_id = "other-message".into();
    let tx = conn.transaction().unwrap();
    assert!(record_expected(&tx, &f.state, &changed, "author", ALICE, ALICE_DEVICE).is_err());
    drop(tx);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM message_uploads WHERE upload_id=?1",
            [&f.accepted.descriptor.attachment_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
fn origin_request(f: &Fixture, offset: u64) -> ChunkRequest {
    let intent = ReadIntent {
        version: 1,
        purpose: ReadPurpose::Chunk,
        requesting_identity: wire::stable_identity(&f.peer_key.verifying_key().to_bytes()),
        requesting_server: "http://localhost:3000".into(),
        member_pseudonym: ALICE.into(),
        device_id: ALICE_DEVICE.to_string(),
        accepted: f.accepted.clone(),
        offset: Some(offset),
        length: Some(wire::CHUNK_BYTES.min(f.bytes.len() as u64 - offset)),
        nonce: uuid::Uuid::new_v4().to_string(),
        created_at: now(),
    };
    let mut request = ChunkRequest {
        version: 1,
        requesting_server: intent.requesting_server.clone(),
        intent_signature: wire::sign(&f.peer_key, &wire::intent_signing_input(&intent).unwrap()),
        intent,
        signature: String::new(),
    };
    request.signature = wire::sign(&f.peer_key, &wire::request_signing_input(&request).unwrap());
    request
}

/// Distinct authority66 and requester55. The genuine issuer55 certificate is
/// unchanged; a NEW member-signed authority66 lineage is independently verified.
/// Human acceptance performs the production proof/signature checks before any
/// read permit can rely on its current typed membership row.
fn remote_origin_fixture() -> Fixture {
    use crate::services::{
        channel_authority as authority,
        channel_manifest::{self, SignedManifestEvent},
        human_federation,
    };
    use ed25519_dalek::Signer;
    let mut f = origin_fixture();
    f.state.signing_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0x66; 32]));
    f.peer_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0x55; 32]));
    let local = local_identity(&f.state);
    f.accepted.authority_identity = local.clone();
    f.accepted.descriptor.origin_identity = local.clone();
    let certificate: crate::device_certificates::DeviceCertificate =
        serde_json::from_value(certified_fixture()["certificates"][0].clone()).unwrap();
    assert_eq!(
        certificate.server_identity,
        wire::stable_identity(&f.peer_key.verifying_key().to_bytes())
    );
    assert_ne!(certificate.server_identity, local);
    let mut conn = f.state.pool.get().unwrap();
    conn.execute(
        "UPDATE instances SET public_key=?1 WHERE id=1",
        [hex::encode(f.peer_key.verifying_key().to_bytes())],
    )
    .unwrap();
    conn.execute("UPDATE federated_identities SET commitment_hex=?1,vrp_topic=?2 WHERE server_id=1 AND pseudonym_id=?3",params![certificate.commitment_hex,certificate.topic,ALICE]).unwrap();
    conn.execute("UPDATE channel_authorities SET authority_identity=?1,authority_key_hex=?2 WHERE channel_id='general'",params![local,hex::encode(f.state.signing_key.verifying_key().to_bytes())]).unwrap();
    conn.execute("UPDATE channel_authority_attachment_bindings SET descriptor_json=?1 WHERE attachment_id=?2",params![serde_json::to_string(&f.accepted.descriptor).unwrap(),f.accepted.descriptor.attachment_id]).unwrap();
    conn.execute("UPDATE federated_upload_origins SET authority_identity=?1,accepted_json=?2 WHERE upload_id=?3",params![local,serde_json::to_string(&f.accepted).unwrap(),f.accepted.descriptor.attachment_id]).unwrap();
    conn.execute(
        "DELETE FROM channel_manifest_events WHERE channel_id='general'",
        [],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM channel_manifest_state WHERE channel_id='general'",
        [],
    )
    .unwrap();
    let public: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/attachment-remote-reader-public.json"
    ))
    .unwrap();
    assert_eq!(public["authority_identity"], local);
    let mut projection = None;
    for value in public["channels"]["general"].as_array().unwrap() {
        let event: SignedManifestEvent = serde_json::from_value(value.clone()).unwrap();
        let next = channel_manifest::verify_transition(
            projection.as_ref(),
            &event,
            f.state.membership_vkey_v2.as_ref().unwrap(),
        )
        .unwrap();
        channel_manifest::persist(&conn, &f.state, "general", &event, &next).unwrap();
        projection = Some(next);
    }
    let issued_at = now();
    let mut attestation = human_federation::HumanMembership {
        version: 1,
        participant_type: "HUMAN".into(),
        originating_server: "http://localhost:3000".into(),
        originating_identity: certificate.server_identity.clone(),
        authority_identity: local,
        channel_id: "general".into(),
        certificate,
        token_epoch: 0,
        nonce: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        issued_at,
        expires_at: issued_at + 120,
        signature: String::new(),
    };
    attestation.signature = hex::encode(
        f.peer_key
            .sign(authority::canonical_json(&attestation.signing_value()).as_bytes())
            .to_bytes(),
    );
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let receipt = human_federation::accept(&tx, &f.state, &attestation).unwrap();
    receipt
        .verify(
            &attestation,
            &hex::encode(f.state.signing_key.verifying_key().to_bytes()),
        )
        .unwrap();
    tx.commit().unwrap();
    drop(conn);
    f
}

#[tokio::test]
async fn certified_origin_browser_read_refuses_actual_authority_outage_without_local_fallback() {
    use crate::services::channel_manifest::{self, SignedManifestEvent};
    let mut f = origin_fixture();
    f.state.federation_config.allow_private_peer_addresses = true;
    let public: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/attachment-remote-reader-public.json"
    ))
    .unwrap();
    let authority = public["authority_identity"].as_str().unwrap();
    let authority_key = public["authority_public_key"].as_str().unwrap();
    let certificate: crate::device_certificates::DeviceCertificate =
        serde_json::from_value(certified_fixture()["certificates"][0].clone()).unwrap();
    assert_eq!(certificate.server_identity, local_identity(&f.state));
    assert_ne!(authority, local_identity(&f.state));
    // A real closed listener supplies the outage; no authority reply is faked.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    assert!(std::net::TcpStream::connect(address).is_err());
    let authority_url = format!("http://{address}");
    let conn = f.state.pool.get().unwrap();
    conn.execute(
        "DELETE FROM federated_identities WHERE pseudonym_id=?1",
        [ALICE],
    )
    .unwrap();
    // Exact public registration values were genuinely admitted in the source
    // fixture session. This component reconstructs that verified local projection.
    conn.execute("INSERT INTO zk_nullifiers(topic,nullifier_hex,pseudonym_id,commitment_hex) VALUES(?1,?2,?3,?4)",params![certificate.topic,certificate.nullifier_hex,ALICE,certificate.commitment_hex]).unwrap();
    conn.execute(
        "INSERT INTO vrp_roots(root_hex) VALUES(?1)",
        [&certificate.root_hex],
    )
    .unwrap();
    conn.execute(
        "UPDATE instances SET base_url=?1,public_key=?2 WHERE id=2",
        params![authority_url, authority_key],
    )
    .unwrap();
    conn.execute("UPDATE channel_authorities SET authority_identity=?1,authority_key_hex=?2,authority_url=?3,authority_peer_id=2,local_authority=0 WHERE channel_id='general'",params![authority,authority_key,authority_url]).unwrap();
    conn.execute(
        "DELETE FROM channel_manifest_events WHERE channel_id='general'",
        [],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM channel_manifest_state WHERE channel_id='general'",
        [],
    )
    .unwrap();
    let mut projection = None;
    for value in public["channels"]["general"].as_array().unwrap() {
        let event: SignedManifestEvent = serde_json::from_value(value.clone()).unwrap();
        let next = channel_manifest::verify_transition(
            projection.as_ref(),
            &event,
            f.state.membership_vkey_v2.as_ref().unwrap(),
        )
        .unwrap();
        channel_manifest::persist(&conn, &f.state, "general", &event, &next).unwrap();
        projection = Some(next);
    }
    f.accepted.authority_identity = authority.into();
    conn.execute("UPDATE federated_upload_origins SET authority_identity=?1,accepted_json=?2 WHERE upload_id=?3",params![authority,serde_json::to_string(&f.accepted).unwrap(),f.accepted.descriptor.attachment_id]).unwrap();
    drop(conn);
    let d = &f.accepted.descriptor;
    let relative = format!("chat/files/{}.bin", d.attachment_id);
    let fallback = upload_service::read_upload_for_device_grant(
        &f.state,
        &relative,
        &d.attachment_id,
        ALICE,
        0,
        Some(ALICE_DEVICE),
    )
    .unwrap_err();
    assert!(fallback.to_string().contains("fresh authority read permit"));
    let error = read_origin_for_grant(
        Arc::new(f.state.clone()),
        &relative,
        &d.attachment_id,
        ALICE,
        ALICE_DEVICE,
        0,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, TransferError::Unavailable(_)),
        "unexpected error: {error}"
    );
    assert!(
        error.to_string().contains("error sending request"),
        "expected real network failure: {error}"
    );
    assert_eq!(fs::read(f.dir.path().join(relative)).unwrap(), f.bytes);
}

fn local_authority_replica_fixture() -> Fixture {
    let mut f = fixture();
    f.state.signing_key = Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0x55; 32]));
    let key = hex::encode(f.state.signing_key.verifying_key().to_bytes());
    let identity = local_identity(&f.state);
    f.accepted.authority_identity = identity.clone();
    let d = &f.accepted.descriptor;
    let device = ALICE_DEVICE.to_string();
    let conn = f.state.pool.get().unwrap();
    conn.execute(
        "UPDATE channels SET e2e_enabled=1 WHERE channel_id='general'",
        [],
    )
    .unwrap();
    conn.execute("UPDATE channel_key_state SET current_epoch=1,required_epoch=1,current_key_id=?1 WHERE channel_id='general'",[&d.key_id]).unwrap();
    conn.execute("INSERT INTO channel_key_epochs(server_id,channel_id,epoch,key_id,committed_revision,committed_by_pseudonym,committed_by_device) VALUES(1,'general',1,?1,1,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?2)",params![d.key_id,device]).unwrap();
    conn.execute("INSERT INTO channel_device_key_wraps(server_id,channel_id,recipient_pseudonym_id,recipient_device_id,epoch,key_id,sender_pseudonym_id,sender_device_id,wrapped_key_b64) VALUES(1,'general','a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?1,1,?2,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?1,'controlled-test-wrap')",params![device,d.key_id]).unwrap();
    conn.execute("UPDATE channel_authorities SET authority_identity=?1,authority_key_hex=?2,authority_url=?3,authority_peer_id=NULL,local_authority=1,head_seq=7,head_hash=?4 WHERE server_id=1 AND channel_id='general'",params![identity,key,f.state.get_public_url(),f.accepted.accepted_event_hash]).unwrap();
    conn.execute("INSERT INTO channel_authority_attachment_bindings(server_id,channel_id,message_id,attachment_id,accepted_seq,accepted_hash,descriptor_json) VALUES(1,'general','message',?1,7,?2,?3)",params![d.attachment_id,f.accepted.accepted_event_hash,serde_json::to_string(d).unwrap()]).unwrap();
    drop(conn);
    record(&f);
    f
}

fn complete_prefix(f: &Fixture) -> Transfer {
    let first = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &first, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let last = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &last, &f.bytes[wire::CHUNK_BYTES as usize..]).unwrap();
    claim(&f.state).unwrap().unwrap()
}

#[tokio::test]
async fn verified_replica_finalization_and_browser_read_require_current_authority_entitlement() {
    for sql in [
        "DELETE FROM channel_members WHERE channel_id='general' AND pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE member_device_keys SET revoked_at=datetime('now') WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "DELETE FROM channel_device_key_wraps WHERE recipient_pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE platform_identities SET token_epoch=token_epoch+1 WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE federation_agreements SET active=0",
    ] {
        let f = local_authority_replica_fixture();
        let complete = complete_prefix(&f);
        finalize(Arc::new(f.state.clone()), &complete)
            .await
            .unwrap();
        assert_eq!(progress(&f), (f.bytes.len() as u64, "ready".into()));
        let relative = format!("chat/files/{}.bin", f.accepted.descriptor.attachment_id);
        let (bytes, mime) = read_replica_for_grant(
            Arc::new(f.state.clone()),
            &relative,
            &f.accepted.descriptor.attachment_id,
            ALICE,
            ALICE_DEVICE,
            0,
        )
        .await
        .unwrap();
        assert_eq!(bytes, f.bytes);
        assert_eq!(mime, "application/octet-stream");
        f.state.pool.get().unwrap().execute(sql, []).unwrap();
        assert!(
            read_replica_for_grant(
                Arc::new(f.state.clone()),
                &relative,
                &f.accepted.descriptor.attachment_id,
                ALICE,
                ALICE_DEVICE,
                0
            )
            .await
            .is_err(),
            "revoked read must fail: {sql}"
        );
    }
}

#[tokio::test]
async fn rename_before_failed_database_commit_recovers_exact_ciphertext_on_retry() {
    let f = local_authority_replica_fixture();
    let complete = complete_prefix(&f);
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER test_fail_finalize BEFORE UPDATE OF lifecycle ON uploads WHEN NEW.lifecycle='ready' BEGIN SELECT RAISE(ABORT,'controlled final commit failure'); END;").unwrap();
    let error = finalize(Arc::new(f.state.clone()), &complete)
        .await
        .unwrap_err();
    assert!(!f.dir.path().join(&complete.staging).exists());
    assert_eq!(
        fs::read(f.dir.path().join(&complete.storage)).unwrap(),
        f.bytes
    );
    assert_eq!(progress(&f).1, "receiving");
    fail_attempt(&f.state, &complete, &error).unwrap();
    f.state.pool.get().unwrap().execute_batch("DROP TRIGGER test_fail_finalize; UPDATE federated_upload_transfers SET next_retry_at=unixepoch();").unwrap();
    let retry = claim(&f.state).unwrap().unwrap();
    finalize(Arc::new(f.state.clone()), &retry).await.unwrap();
    assert_eq!(progress(&f).1, "ready");
    assert_eq!(
        fs::read(f.dir.path().join(&retry.storage)).unwrap(),
        f.bytes
    );
}

#[tokio::test]
async fn corrupted_ready_replica_fails_closed_and_retries_from_a_new_generation() {
    let f = local_authority_replica_fixture();
    let complete = complete_prefix(&f);
    finalize(Arc::new(f.state.clone()), &complete)
        .await
        .unwrap();
    let mut corrupt = f.bytes.clone();
    corrupt[2] ^= 1;
    fs::write(f.dir.path().join(&complete.storage), corrupt).unwrap();
    let relative = format!("chat/files/{}.bin", f.accepted.descriptor.attachment_id);
    assert!(matches!(
        read_replica_for_grant(
            Arc::new(f.state.clone()),
            &relative,
            &f.accepted.descriptor.attachment_id,
            ALICE,
            ALICE_DEVICE,
            0
        )
        .await,
        Err(TransferError::Integrity(_))
    ));
    assert_eq!(progress(&f), (0, "failed".into()));
    assert!(retry_transfer(&f.state.pool.get().unwrap(), 1, complete.id).is_err());
    upload_service::drain_deletion_queue(&f.state).unwrap();
    retry_transfer(&f.state.pool.get().unwrap(), 1, complete.id).unwrap();
    let retry = claim(&f.state).unwrap().unwrap();
    assert_ne!(retry.storage, complete.storage);
    assert_eq!(retry.committed, 0);
}

#[tokio::test]
async fn authorized_member_retry_retargets_exact_prefix_and_invalidates_old_reader_lease() {
    let f = local_authority_replica_fixture();
    let first = claim(&f.state).unwrap().unwrap();
    persist_chunk(&f.state, &first, &f.bytes[..wire::CHUNK_BYTES as usize]).unwrap();
    let old = claim(&f.state).unwrap().unwrap();
    let device = BOB_DEVICE.to_string();
    {
        let conn = f.state.pool.get().unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,active) VALUES(1,'55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe','HUMAN',1)",[]).unwrap();
        conn.execute("INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES(1,'general','55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe')",[]).unwrap();
        conn.execute("INSERT INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex) VALUES(1,'55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe',?1,?2)",params![device,certified_fixture()["certificates"][1]["x25519_pub_hex"].as_str().unwrap()]).unwrap();
        conn.execute("INSERT INTO channel_device_key_wraps(server_id,channel_id,recipient_pseudonym_id,recipient_device_id,epoch,key_id,sender_pseudonym_id,sender_device_id,wrapped_key_b64) VALUES(1,'general','55120518bd6a31a9c76521d0fd5a14b9f0382ec11ef3710d801a6094a9d91abe',?1,1,?2,'a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9',?3,'controlled-test-wrap')",params![device,f.accepted.descriptor.key_id,ALICE_DEVICE.to_string()]).unwrap();
        conn.execute(
            "UPDATE member_device_keys SET revoked_at=datetime('now') WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
            [],
        )
        .unwrap();
    }
    install_certified_member(&f.state, 1);
    assert!(retry_for_member(
        Arc::new(f.state.clone()),
        "general",
        &f.accepted.descriptor.attachment_id,
        BOB,
        ALICE_DEVICE,
        0
    )
    .await
    .is_err());
    assert_eq!(
        retry_for_member(
            Arc::new(f.state.clone()),
            "general",
            &f.accepted.descriptor.attachment_id,
            BOB,
            &device,
            0
        )
        .await
        .unwrap(),
        "pending"
    );
    assert!(persist_chunk(&f.state, &old, &f.bytes[wire::CHUNK_BYTES as usize..]).is_err());
    let current = claim(&f.state).unwrap().unwrap();
    assert_eq!(current.member, BOB);
    assert_eq!(current.device, device);
    assert_eq!(current.accepted, f.accepted);
    assert_eq!(current.committed, wire::CHUNK_BYTES);
    persist_chunk(&f.state, &current, &f.bytes[wire::CHUNK_BYTES as usize..]).unwrap();
    assert_eq!(fs::read(stage(&f)).unwrap(), f.bytes);
}

#[tokio::test]
async fn actual_origin_reads_two_ranges_with_fresh_authority_permits_and_rejects_same_nonce() {
    let f = remote_origin_fixture();
    let state = Arc::new(f.state.clone());
    let first = origin_request(&f, 0);
    let response = serve_chunk(state.clone(), first.clone()).await.unwrap();
    let key = hex::encode(f.state.signing_key.verifying_key().to_bytes());
    let bytes = wire::verify_response(&response, &first, &key, &key, now())
        .unwrap()
        .unwrap();
    assert_eq!(bytes, f.bytes[..wire::CHUNK_BYTES as usize]);
    assert!(serve_chunk(state.clone(), first).await.is_err());
    let last = origin_request(&f, wire::CHUNK_BYTES);
    let response = serve_chunk(state, last.clone()).await.unwrap();
    assert_eq!(
        wire::verify_response(&response, &last, &key, &key, now())
            .unwrap()
            .unwrap(),
        f.bytes[wire::CHUNK_BYTES as usize..]
    );
}
#[tokio::test]
async fn fresh_authority_refuses_later_chunk_after_member_device_or_epoch_entitlement_revocation() {
    for sql in [
        "DELETE FROM channel_members WHERE channel_id='general' AND pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE member_device_keys SET revoked_at=datetime('now') WHERE pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "DELETE FROM channel_device_key_wraps WHERE recipient_pseudonym_id='a4e3db74fa4189195e2b75ce950a0ef56ebe4fe8b809960f1ed78d647ee4c4d9'",
        "UPDATE federation_agreements SET active=0",
        "UPDATE federated_human_memberships SET expires_at=unixepoch()-1",
        "UPDATE federated_human_memberships SET channel_id='wrong-channel'",
        "UPDATE federated_human_memberships SET device_id='wrong-device'",
        "UPDATE federated_human_memberships SET certificate_sha256='wrong-certificate'",
        "DELETE FROM federated_human_memberships",
    ] {
        let f = remote_origin_fixture();
        let state = Arc::new(f.state.clone());
        serve_chunk(state.clone(), origin_request(&f, 0))
            .await
            .unwrap();
        f.state.pool.get().unwrap().execute(sql, []).unwrap();
        assert!(
            serve_chunk(state, origin_request(&f, wire::CHUNK_BYTES))
                .await
                .is_err(),
            "revoked authority must refuse: {sql}"
        );
    }
}
#[tokio::test]
async fn signed_tombstone_has_no_bytes_or_permission_and_matches_exact_removed_message() {
    let f = remote_origin_fixture();
    let request = origin_request(&f, 0);
    {
        let mut conn = f.state.pool.get().unwrap();
        let tx = conn.transaction().unwrap();
        remove_expected(
            &tx,
            1,
            "general",
            Some("message"),
            None,
            "deleted",
            (8, &"ef".repeat(32)),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let response = serve_chunk(Arc::new(f.state.clone()), request.clone())
        .await
        .unwrap();
    let key = hex::encode(f.state.signing_key.verifying_key().to_bytes());
    assert!(
        wire::verify_response(&response, &request, &key, &key, now())
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        response.outcome,
        ChunkOutcome::Removed {
            tombstone_seq: 8,
            ..
        }
    ));
    assert!(response.permit.is_none());
}

fn descriptor_fixture() -> Fixture {
    let mut f = fixture();
    f.accepted.descriptor.origin_identity = local_identity(&f.state);
    f.accepted.descriptor.originating_server = f.state.get_public_url();
    let d = &f.accepted.descriptor;
    let conn = f.state.pool.get().unwrap();
    conn.execute(
        "UPDATE messages SET sender_pseudonym=?1 WHERE message_id='message'",
        [ALICE],
    )
    .unwrap();
    conn.execute("INSERT INTO uploads(server_id,upload_id,uploader_pseudonym,original_filename,content_type,size_bytes,purpose,channel_id,category,encrypted,epoch,key_id,ciphertext_sha256,storage_path,lifecycle) VALUES(1,?1,?2,'encrypted.bin','application/octet-stream',?3,'chat','general','file',1,1,?4,?5,?6,'ready')",
        params![d.attachment_id,ALICE,d.size_bytes,d.key_id,d.ciphertext_sha256,format!("chat/files/{}.bin",d.attachment_id)]).unwrap();
    drop(conn);
    f
}

fn bootstrap_fixture(plaintext: bool) -> Fixture {
    let mut f = descriptor_fixture();
    if plaintext {
        f.bytes = b"X".to_vec();
        let d = &mut f.accepted.descriptor;
        d.epoch = 0;
        d.key_id.clear();
        d.size_bytes = 1;
        d.ciphertext_sha256 = hex::encode(Sha256::digest(&f.bytes));
        d.plaintext = Some(wire::PlainAttachmentMetadata {
            filename: "history.txt".into(),
            content_type: "text/plain".into(),
        });
    }
    let d = &f.accepted.descriptor;
    let relative = d.public_path();
    let path =
        upload_service::confined_path(Path::new(&f.state.upload_dir), &relative, true).unwrap();
    fs::write(path, &f.bytes).unwrap();
    let conn = f.state.pool.get().unwrap();
    conn.execute("UPDATE uploads SET storage_path=?1,encrypted=?2,epoch=?3,key_id=?4,size_bytes=?5,ciphertext_sha256=?6,original_filename=?7,content_type=?8 WHERE upload_id=?9",
        params![relative,!plaintext,(!plaintext).then_some(d.epoch),(!plaintext).then_some(d.key_id.as_str()),d.size_bytes,d.ciphertext_sha256,d.filename(),d.content_type(),d.attachment_id]).unwrap();
    conn.execute(
        "INSERT INTO message_uploads(server_id,message_id,upload_id) VALUES(1,'message',?1)",
        [&d.attachment_id],
    )
    .unwrap();
    drop(conn);
    f
}

#[test]
fn origin_waits_for_its_ordered_receipt_without_exposing_unaccepted_bytes() {
    for plaintext in [false, true] {
        let f = bootstrap_fixture(plaintext);
        let conn = f.state.pool.get().unwrap();
        conn.execute(
            "UPDATE channel_authorities SET head_seq=6 WHERE channel_id='general'",
            [],
        )
        .unwrap();
        let intent = origin_request(&f, 0).intent;
        assert!(matches!(
            origin_outcome(&conn, &f.state, &intent).unwrap(),
            Some(ChunkOutcome::Pending {
                retry_after_seconds: 3
            })
        ));
        assert!(require_origin_binding(&conn, &f.state, &intent).is_err());
        let mut changed = intent.clone();
        changed.accepted.descriptor.ciphertext_sha256 = "00".repeat(32);
        assert!(matches!(
            origin_outcome(&conn, &f.state, &changed),
            Err(TransferError::NotFound(_))
        ));
        conn.execute(
            "UPDATE channel_authorities SET head_seq=7 WHERE channel_id='general'",
            [],
        )
        .unwrap();
        assert!(matches!(
            origin_outcome(&conn, &f.state, &intent),
            Err(TransferError::NotFound(_))
        ));
        drop(conn);
        let mut conn = f.state.pool.get().unwrap();
        let tx = conn.transaction().unwrap();
        record_expected(&tx, &f.state, &f.accepted, ALICE, ALICE, ALICE_DEVICE).unwrap();
        tx.commit().unwrap();
        assert!(origin_outcome(&conn, &f.state, &intent).unwrap().is_none());
    }
}

#[test]
fn bootstrap_keeps_departed_authors_real_opaque_and_ordinary_bytes_without_republishing() {
    for plain in [false, true] {
        let f = bootstrap_fixture(plain);
        let mut conn = f.state.pool.get().unwrap();
        conn.execute("DELETE FROM channel_members WHERE server_id=1 AND channel_id='general' AND pseudonym_id=?1",[ALICE]).unwrap();
        assert!(descriptors_for_bootstrap_message(&conn, &f.state, "general", "message").is_err());
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            descriptors_for_bootstrap_message(&tx, &f.state, "general", "message").unwrap(),
            vec![f.accepted.descriptor.clone()]
        );
        record_expected(&tx, &f.state, &f.accepted, ALICE, "", "").unwrap();
        assert_eq!(
            descriptors_for_bootstrap_message(&tx, &f.state, "general", "message").unwrap(),
            vec![f.accepted.descriptor.clone()]
        );
        tx.commit().unwrap();
    }
}

#[test]
fn bootstrap_refuses_wrong_owner_staging_missing_corrupt_and_removed_attachments() {
    for fault in [
        "owner", "staging", "missing", "corrupt", "removed", "message",
    ] {
        let f = bootstrap_fixture(false);
        let mut conn = f.state.pool.get().unwrap();
        let d = &f.accepted.descriptor;
        match fault {
            "owner" => {
                conn.execute(
                    "UPDATE uploads SET uploader_pseudonym=?1 WHERE upload_id=?2",
                    params![BOB, d.attachment_id],
                )
                .unwrap();
            }
            "staging" => {
                conn.execute(
                    "UPDATE uploads SET lifecycle='staging' WHERE upload_id=?1",
                    [&d.attachment_id],
                )
                .unwrap();
            }
            "missing" => {
                fs::remove_file(Path::new(&f.state.upload_dir).join(d.public_path())).unwrap();
            }
            "corrupt" => {
                fs::write(
                    Path::new(&f.state.upload_dir).join(d.public_path()),
                    vec![0x91; f.bytes.len()],
                )
                .unwrap();
            }
            "removed" => {
                conn.execute("INSERT INTO channel_authority_attachment_bindings(server_id,channel_id,message_id,attachment_id,accepted_seq,accepted_hash,descriptor_json,removed_reason) VALUES(1,'general','message',?1,7,?2,?3,'removed')",params![d.attachment_id,f.accepted.accepted_event_hash,serde_json::to_string(d).unwrap()]).unwrap();
            }
            _ => {
                conn.execute(
                    "UPDATE messages SET deleted_at=datetime('now') WHERE message_id='message'",
                    [],
                )
                .unwrap();
            }
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(
            descriptors_for_bootstrap_message(&tx, &f.state, "general", "message").is_err(),
            "{fault}"
        );
    }
}

#[test]
fn ordinary_one_byte_replica_job_keeps_typed_metadata_and_null_encryption_fields() {
    let mut f = fixture();
    let d = &mut f.accepted.descriptor;
    d.plaintext = Some(wire::PlainAttachmentMetadata {
        filename: "one.txt".into(),
        content_type: "text/plain".into(),
    });
    d.epoch = 0;
    d.key_id.clear();
    d.size_bytes = 1;
    d.ciphertext_sha256 = hex::encode(Sha256::digest(b"X"));
    let d = f.accepted.descriptor.clone();
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let author: String = tx
        .query_row(
            "SELECT sender_pseudonym FROM messages WHERE message_id='message'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    record_expected(&tx, &f.state, &f.accepted, &author, ALICE, ALICE_DEVICE).unwrap();
    let row:(bool,Option<i64>,Option<String>,String,String,i64)=tx.query_row("SELECT encrypted,epoch,key_id,original_filename,content_type,size_bytes FROM uploads WHERE upload_id=?1",[&d.attachment_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
    assert_eq!(
        row,
        (false, None, None, "one.txt".into(), "text/plain".into(), 1)
    );
    assert_eq!(
        descriptors_for_bootstrap_message(&tx, &f.state, "general", "message").unwrap(),
        vec![d.clone()]
    );
    tx.commit().unwrap();
    drop(conn);
    let transfer = claim(&f.state).unwrap().unwrap();
    let intent = make_intent(&f.state, &transfer, ReadPurpose::Chunk);
    assert_eq!(intent.version, 2);
    assert_eq!(intent.length, Some(1));
    intent.validate(now()).unwrap();
}

#[tokio::test]
async fn ordinary_origin_reads_require_certified_current_member_but_no_encryption_wrap() {
    let mut f = origin_fixture();
    // origin_fixture also installs a legacy remote identity for chunk-path
    // tests. This browser reader instead belongs to this actual issuer55
    // authority, as its genuine public certificate independently establishes.
    assert_eq!(
        certified_fixture()["certificates"][0]["server_identity"]
            .as_str()
            .unwrap(),
        local_identity(&f.state)
    );
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "DELETE FROM federated_identities WHERE server_id=1 AND pseudonym_id=?1",
            [ALICE],
        )
        .unwrap();
    f.bytes = b"X".to_vec();
    let d = &mut f.accepted.descriptor;
    d.plaintext = Some(wire::PlainAttachmentMetadata {
        filename: "retained.txt".into(),
        content_type: "text/plain".into(),
    });
    d.epoch = 0;
    d.key_id.clear();
    d.size_bytes = 1;
    d.ciphertext_sha256 = hex::encode(Sha256::digest(&f.bytes));
    let d = f.accepted.descriptor.clone();
    let relative = d.public_path();
    let path =
        upload_service::confined_path(Path::new(&f.state.upload_dir), &relative, true).unwrap();
    fs::write(path, &f.bytes).unwrap();
    let conn = f.state.pool.get().unwrap();
    conn.execute("UPDATE uploads SET encrypted=0,epoch=NULL,key_id=NULL,size_bytes=1,ciphertext_sha256=?1,original_filename='retained.txt',content_type='text/plain',storage_path=?2 WHERE upload_id=?3",params![d.ciphertext_sha256,relative,d.attachment_id]).unwrap();
    conn.execute("UPDATE channel_authority_attachment_bindings SET descriptor_json=?1 WHERE attachment_id=?2",params![serde_json::to_string(&d).unwrap(),d.attachment_id]).unwrap();
    conn.execute(
        "UPDATE federated_upload_origins SET accepted_json=?1 WHERE upload_id=?2",
        params![serde_json::to_string(&f.accepted).unwrap(), d.attachment_id],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM channel_device_key_wraps WHERE channel_id='general'",
        [],
    )
    .unwrap();
    drop(conn);
    let state = Arc::new(f.state.clone());
    let (actual, mime) = read_origin_for_grant(
        state.clone(),
        &relative,
        &d.attachment_id,
        ALICE,
        ALICE_DEVICE,
        0,
    )
    .await
    .unwrap();
    assert_eq!(actual, b"X");
    assert_eq!(mime, "application/octet-stream");
    assert!(read_origin_for_grant(
        state.clone(),
        &relative,
        &d.attachment_id,
        ALICE,
        &"0".repeat(64),
        0
    )
    .await
    .is_err());
    f.state.pool.get().unwrap().execute("DELETE FROM channel_members WHERE server_id=1 AND channel_id='general' AND pseudonym_id=?1",[ALICE]).unwrap();
    assert!(
        read_origin_for_grant(state, &relative, &d.attachment_id, ALICE, ALICE_DEVICE, 0)
            .await
            .is_err()
    );
}

#[test]
fn origin_descriptors_derive_opaque_metadata_and_require_writer_transaction() {
    let f = descriptor_fixture();
    let mut conn = f.state.pool.get().unwrap();
    let ids = vec![f.accepted.descriptor.attachment_id.clone()];
    assert!(matches!(
        descriptors_for_origin_message(&conn, &f.state, "general", ALICE, &ids, None),
        Err(TransferError::Internal(_))
    ));
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(
        descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, None).unwrap(),
        vec![f.accepted.descriptor.clone()]
    );
    assert!(
        descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &[], None)
            .unwrap()
            .is_empty()
    );
    let encoded = serde_json::to_string(
        &descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, None).unwrap(),
    )
    .unwrap();
    assert!(
        !encoded.contains("encrypted.bin")
            && !encoded.contains("fileKey")
            && !encoded.contains("original_filename")
    );
}

#[test]
fn origin_descriptors_refuse_duplicate_malformed_and_unbounded_ids() {
    let f = descriptor_fixture();
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let id = f.accepted.descriptor.attachment_id.clone();
    for ids in [
        vec![id.clone(), id],
        vec!["../attachment".into()],
        (0..17).map(|_| uuid::Uuid::new_v4().to_string()).collect(),
    ] {
        assert!(matches!(
            descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, None),
            Err(TransferError::BadRequest(_))
        ));
    }
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM uploads", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn origin_descriptors_reject_foreign_owner_channel_plaintext_staging_and_wrong_author() {
    let f = descriptor_fixture();
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let ids = vec![f.accepted.descriptor.attachment_id.clone()];
    for update in [
        "uploader_pseudonym='somebody-else'",
        "channel_id='elsewhere'",
        "encrypted=0",
        "lifecycle='staging'",
    ] {
        tx.execute_batch("SAVEPOINT metadata_case").unwrap();
        tx.execute_batch(&format!("UPDATE uploads SET {update}"))
            .unwrap();
        assert!(
            descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, None).is_err()
        );
        tx.execute_batch("ROLLBACK TO metadata_case; RELEASE metadata_case")
            .unwrap();
    }
    assert!(matches!(
        descriptors_for_origin_message(
            &tx,
            &f.state,
            "general",
            "somebody-else",
            &ids,
            Some("message")
        ),
        Err(TransferError::Forbidden(_))
    ));
}

#[test]
fn origin_descriptors_preserve_retained_remote_descriptor_without_a_local_blob() {
    let f = fixture();
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute(
        "UPDATE messages SET sender_pseudonym=?1 WHERE message_id='message'",
        [ALICE],
    )
    .unwrap();
    let d = &f.accepted.descriptor;
    tx.execute("INSERT INTO channel_authority_attachment_bindings(server_id,channel_id,message_id,attachment_id,accepted_seq,accepted_hash,descriptor_json) VALUES(1,'general','message',?1,7,?2,?3)",params![d.attachment_id,f.accepted.accepted_event_hash,serde_json::to_string(d).unwrap()]).unwrap();
    let ids = vec![d.attachment_id.clone()];
    assert_eq!(
        descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, Some("message"))
            .unwrap(),
        vec![d.clone()]
    );
    assert!(descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, None).is_err());
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM uploads", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn origin_descriptors_tombstone_and_cross_message_reuse_do_not_become_new_uploads() {
    let f = descriptor_fixture();
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let d = &f.accepted.descriptor;
    let ids = vec![d.attachment_id.clone()];
    tx.execute(
        "INSERT INTO message_uploads(server_id,message_id,upload_id) VALUES(1,'message',?1)",
        [&d.attachment_id],
    )
    .unwrap();
    assert!(matches!(
        descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, None),
        Err(TransferError::Conflict(_))
    ));
    tx.execute("INSERT INTO channel_authority_attachment_bindings(server_id,channel_id,message_id,attachment_id,accepted_seq,accepted_hash,descriptor_json,removed_reason) VALUES(1,'general','message',?1,7,?2,?3,'removed')",params![d.attachment_id,f.accepted.accepted_event_hash,serde_json::to_string(d).unwrap()]).unwrap();
    assert!(matches!(
        descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, Some("message")),
        Err(TransferError::Conflict(_))
    ));
}

#[test]
fn origin_descriptors_preserve_local_origin_reference_and_refuse_disagreeing_projection() {
    let f = descriptor_fixture();
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let d = &f.accepted.descriptor;
    let ids = vec![d.attachment_id.clone()];
    tx.execute("INSERT INTO federated_upload_origins(server_id,upload_id,authority_identity,channel_id,message_id,accepted_event_seq,accepted_event_hash,accepted_json) VALUES(1,?1,?2,'general','message',7,?3,?4)",params![d.attachment_id,f.accepted.authority_identity,f.accepted.accepted_event_hash,serde_json::to_string(&f.accepted).unwrap()]).unwrap();
    assert_eq!(
        descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, Some("message"))
            .unwrap(),
        vec![d.clone()]
    );
    let mut conflicting = d.clone();
    conflicting.epoch += 1;
    tx.execute("INSERT INTO channel_authority_attachment_bindings(server_id,channel_id,message_id,attachment_id,accepted_seq,accepted_hash,descriptor_json) VALUES(1,'general','message',?1,7,?2,?3)",params![d.attachment_id,f.accepted.accepted_event_hash,serde_json::to_string(&conflicting).unwrap()]).unwrap();
    assert!(matches!(
        descriptors_for_origin_message(&tx, &f.state, "general", ALICE, &ids, Some("message")),
        Err(TransferError::Conflict(_))
    ));
}

#[tokio::test]
async fn local_encrypted_attachment_retry_revalidates_actual_file_and_current_device() {
    let f = origin_fixture();
    f.state
        .pool
        .get()
        .unwrap()
        .execute("DELETE FROM federated_upload_origins", [])
        .unwrap();
    let id = f.accepted.descriptor.attachment_id.clone();
    let path = f.dir.path().join(format!("chat/files/{id}.bin"));
    let state = Arc::new(f.state.clone());
    assert_eq!(
        retry_for_member(state.clone(), "general", &id, ALICE, ALICE_DEVICE, 0)
            .await
            .unwrap(),
        "ready"
    );
    assert!(matches!(
        retry_for_member(state.clone(), "general", &id, ALICE, &"ef".repeat(32), 0).await,
        Err(TransferError::Forbidden(_))
    ));
    fs::write(&path, b"actual corrupted file").unwrap();
    assert!(
        retry_for_member(state.clone(), "general", &id, ALICE, ALICE_DEVICE, 0)
            .await
            .is_err()
    );
    fs::write(&path, &f.bytes).unwrap();
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "DELETE FROM channel_members WHERE channel_id='general' AND pseudonym_id=?1",
            [ALICE],
        )
        .unwrap();
    assert!(matches!(
        retry_for_member(state, "general", &id, ALICE, ALICE_DEVICE, 0).await,
        Err(TransferError::Forbidden(_))
    ));
}

#[tokio::test]
async fn origin_encrypted_attachment_retry_uses_current_authority_and_refuses_tombstone() {
    let f = origin_fixture();
    // The shared origin fixture represents a remote reader for signed chunk RPCs.
    // Browser Retry instead authenticates Alice at her genuine certificate issuer.
    assert_eq!(
        certified_fixture()["certificates"][0]["server_identity"]
            .as_str()
            .unwrap(),
        local_identity(&f.state)
    );
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "DELETE FROM federated_identities WHERE server_id=1 AND pseudonym_id=?1",
            [ALICE],
        )
        .unwrap();
    let id = f.accepted.descriptor.attachment_id.clone();
    let state = Arc::new(f.state.clone());
    assert_eq!(
        retry_for_member(state.clone(), "general", &id, ALICE, ALICE_DEVICE, 0)
            .await
            .unwrap(),
        "ready"
    );
    f.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federated_upload_origins SET removed_reason='deleted' WHERE upload_id=?1",
            [&id],
        )
        .unwrap();
    assert!(matches!(
        retry_for_member(state, "general", &id, ALICE, ALICE_DEVICE, 0).await,
        Err(TransferError::NotFound(_))
    ));
}

fn deferred_fixture() -> Fixture {
    let mut f = origin_fixture();
    let previous = f.accepted.descriptor.attachment_id.clone();
    let d = &mut f.accepted.descriptor;
    d.attachment_id = uuid::Uuid::new_v4().to_string();
    d.origin_identity = wire::stable_identity(&f.peer_key.verifying_key().to_bytes());
    d.originating_server = "http://localhost:3000".into();
    let conn = f.state.pool.get().unwrap();
    conn.execute(
        "DELETE FROM federated_identities WHERE server_id=1 AND pseudonym_id=?1",
        [ALICE],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM channel_authority_attachment_bindings WHERE attachment_id=?1",
        [previous],
    )
    .unwrap();
    conn.execute("INSERT INTO channel_authority_attachment_bindings(server_id,channel_id,message_id,attachment_id,accepted_seq,accepted_hash,descriptor_json) VALUES(1,'general','message',?1,7,?2,?3)",params![d.attachment_id,f.accepted.accepted_event_hash,serde_json::to_string(d).unwrap()]).unwrap();
    drop(conn);
    f
}

#[tokio::test]
async fn deferred_remote_attachment_admits_real_certified_reader_with_actual_authority_permit_once()
{
    let f = deferred_fixture();
    let id = &f.accepted.descriptor.attachment_id;
    let state = Arc::new(f.state.clone());
    let relative = format!("chat/files/{id}.bin");
    assert!(admit_deferred_for_reader(
        state.clone(),
        id,
        Some("general"),
        Some(&relative),
        ALICE,
        ALICE_DEVICE,
        0
    )
    .await
    .unwrap());
    assert!(!admit_deferred_for_reader(
        state.clone(),
        id,
        Some("general"),
        Some(&relative),
        ALICE,
        ALICE_DEVICE,
        0
    )
    .await
    .unwrap());
    let conn = state.pool.get().unwrap();
    let (member,device,status,accepted):(String,String,String,String)=conn.query_row("SELECT member_pseudonym,device_id,status,accepted_json FROM federated_upload_transfers WHERE upload_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(
        (member, device, status),
        (ALICE.into(), ALICE_DEVICE.into(), "pending".into())
    );
    assert_eq!(
        serde_json::from_str::<AcceptedAttachment>(&accepted).unwrap(),
        f.accepted
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM message_uploads WHERE upload_id=?1",
            [id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM channel_authority_nonces WHERE requesting_identity=?1",
            [local_identity(&state)],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn deferred_attachment_refuses_mismatched_path_current_device_epoch_and_removed_binding_without_job(
) {
    for fault in ["path", "device", "wrap", "removed", "deleted", "storage"] {
        let f = deferred_fixture();
        let id = &f.accepted.descriptor.attachment_id;
        let conn = f.state.pool.get().unwrap();
        if fault == "wrap" {
            conn.execute("DELETE FROM channel_device_key_wraps", [])
                .unwrap();
        }
        if fault == "removed" {
            conn.execute("UPDATE channel_authority_attachment_bindings SET removed_reason='detached' WHERE attachment_id=?1",[id]).unwrap();
        }
        if fault == "deleted" {
            conn.execute(
                "UPDATE messages SET deleted_at=datetime('now') WHERE message_id='message'",
                [],
            )
            .unwrap();
        }
        if fault == "storage" {
            f.state
                .storage_health
                .mark_degraded("component admission gate");
        }
        drop(conn);
        let relative = if fault == "path" {
            format!("chat/images/{id}.bin")
        } else {
            format!("chat/files/{id}.bin")
        };
        let invalid = "ef".repeat(32);
        let result = admit_deferred_for_reader(
            Arc::new(f.state.clone()),
            id,
            Some("general"),
            Some(&relative),
            ALICE,
            if fault == "device" {
                &invalid
            } else {
                ALICE_DEVICE
            },
            0,
        )
        .await;
        if matches!(fault, "removed" | "deleted") {
            assert!(!result.unwrap(), "{fault}");
        } else {
            assert!(result.is_err(), "{fault}");
        }
        assert_eq!(
            f.state
                .pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM federated_upload_transfers WHERE upload_id=?1",
                    [id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0,
            "{fault}"
        );
    }
}

#[tokio::test]
async fn guarded_replica_replay_restores_exact_association_and_metadata_failure_rolls_back() {
    use crate::services::channel_authority as authority;
    for corrupt_metadata in [false, true] {
        let f = local_authority_replica_fixture();
        let complete = complete_prefix(&f);
        finalize(Arc::new(f.state.clone()), &complete)
            .await
            .unwrap();
        let id = &f.accepted.descriptor.attachment_id;
        let mut conn = f.state.pool.get().unwrap();
        let snapshot=conn.query_row("SELECT json_array(uploader_pseudonym,original_filename,content_type,size_bytes,purpose,channel_id,category,encrypted,epoch,key_id,ciphertext_sha256,storage_path,staging_path,lifecycle) FROM uploads WHERE upload_id=?1",[id],|r|r.get::<_,String>(0)).unwrap();
        let stored = conn
            .query_row(
                "SELECT storage_path FROM uploads WHERE upload_id=?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        let original = annex_channels::get_message(&conn, "message").unwrap();
        assert_eq!(
            fs::read(Path::new(&f.state.upload_dir).join(&stored)).unwrap(),
            f.bytes
        );
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "INSERT INTO channel_reconciliation_guards(server_id,channel_id) VALUES(1,'general')",
            [],
        )
        .unwrap();
        tx.execute("DELETE FROM messages WHERE message_id='message'", [])
            .unwrap();
        assert_eq!(
            tx.query_row(
                "SELECT COUNT(*) FROM message_uploads WHERE upload_id=?1",
                [id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            tx.query_row(
                "SELECT COUNT(*) FROM uploads WHERE upload_id=?1",
                [id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        annex_channels::create_message(
            &tx,
            &annex_channels::CreateMessageParams {
                channel_id: original.channel_id.clone(),
                message_id: original.message_id.clone(),
                sender_pseudonym: original.sender_pseudonym.clone(),
                content: original.content.clone(),
                reply_to_message_id: original.reply_to_message_id.clone(),
            },
        )
        .unwrap();
        if corrupt_metadata {
            tx.execute(
                "UPDATE uploads SET uploader_pseudonym='changed-owner' WHERE upload_id=?1",
                [id],
            )
            .unwrap();
        }
        // Ordered replay encounters history before a new reader's Join. An
        // already retained file must survive without inventing a fetch member.
        tx.execute("DELETE FROM channel_members WHERE channel_id='general'", [])
            .unwrap();
        let event = authority::append_event(
            &tx,
            &f.state,
            "general",
            authority::EventKind::MessageEdited,
            serde_json::json!({"message": {"message_id": original.message_id,
                "sender_pseudonym": original.sender_pseudonym},
                "attachments": [f.accepted.descriptor.clone()]}),
        )
        .unwrap();
        let accepted = authority::materialize_accepted_attachments(
            &tx,
            &f.state,
            &event,
            std::slice::from_ref(&f.accepted.descriptor),
        );
        if corrupt_metadata {
            assert!(accepted.is_err());
            tx.rollback().unwrap();
        } else {
            accepted.unwrap();
            tx.execute(
                "DELETE FROM channel_reconciliation_guards WHERE channel_id='general'",
                [],
            )
            .unwrap();
            tx.execute("DELETE FROM uploads WHERE upload_id=?1 AND NOT EXISTS(SELECT 1 FROM message_uploads m WHERE m.upload_id=uploads.upload_id)",[id]).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(conn.query_row("SELECT json_array(uploader_pseudonym,original_filename,content_type,size_bytes,purpose,channel_id,category,encrypted,epoch,key_id,ciphertext_sha256,storage_path,staging_path,lifecycle) FROM uploads WHERE upload_id=?1",[id],|r|r.get::<_,String>(0)).unwrap(),snapshot);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM message_uploads WHERE message_id='message' AND upload_id=?1",
                [id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT status FROM federated_upload_transfers WHERE upload_id=?1",
                [id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "ready"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM upload_deletion_queue", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(conn);
        assert_eq!(upload_service::drain_deletion_queue(&f.state).unwrap(), 0);
        assert_eq!(
            fs::read(Path::new(&f.state.upload_dir).join(stored)).unwrap(),
            f.bytes
        );
    }
}

#[test]
fn new_replica_job_without_an_actual_reader_is_refused_without_persisting_metadata() {
    let f = fixture();
    let mut conn = f.state.pool.get().unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let error = record_expected(&tx, &f.state, &f.accepted, "author", "", "").unwrap_err();
    assert!(matches!(error, TransferError::BadRequest(_)));
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM uploads", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM federated_upload_transfers", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM message_uploads", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    tx.rollback().unwrap();
}
