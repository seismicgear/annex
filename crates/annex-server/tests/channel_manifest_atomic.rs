//! Transactional component tests. Public fixture material contains genuine
//! independently generated proofs/signatures; the fixture seeding below is not
//! enrollment or federation HTTP acceptance.
mod common;
use annex_server::{
    services::{channel_manifest as manifest, e2e_service as e2e},
    AppState,
};
use rusqlite::{params, TransactionBehavior};
use serde_json::{json, Value};
use std::sync::{Arc, Barrier};

struct Fixture {
    state: Arc<AppState>,
    document: Value,
    _disk: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let document: Value =
            serde_json::from_str(include_str!("fixtures/signed-manifest-public.json")).unwrap();
        let verifier: Value =
            serde_json::from_str(include_str!("fixtures/device-certificate-public.json")).unwrap();
        let disk = tempfile::tempdir().unwrap();
        let pool = annex_db::create_pool(
            disk.path().join("manifest.db").to_str().unwrap(),
            annex_db::DbRuntimeSettings {
                pool_max_size: 4,
                ..Default::default()
            },
        )
        .unwrap();
        let mut state = common::build_app_state(
            pool,
            annex_identity::MerkleTree::new(20).unwrap(),
            annex_types::ServerPolicy::default(),
        );
        state.membership_vkey_v2 = Some(Arc::new(
            annex_identity::zk::parse_verification_key(&verifier["vkey"].to_string()).unwrap(),
        ));
        let f = Self {
            state: Arc::new(state),
            document,
            _disk: disk,
        };
        {
            let conn = f.state.pool.get().unwrap();
            annex_db::run_migrations(&conn).unwrap();
            conn.execute(
                "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'test','Test',?1)",
                [serde_json::to_string(&annex_types::ServerPolicy::default()).unwrap()],
            )
            .unwrap();
            conn.execute("INSERT INTO channels(server_id,channel_id,name,channel_type,federation_scope,e2e_enabled) VALUES(1,?1,'Signed fixture','\"Text\"','\"Local\"',1)",[f.channel()]).unwrap();
            // Preserve the fixture's exact authority; we do not possess or relabel
            // its private key. These tests exercise the member-signed service.
            conn.execute("INSERT INTO channel_authorities(server_id,channel_id,authority_identity,authority_key_hex,authority_url,local_authority) VALUES(1,?1,?2,?3,?4,0)",params![f.channel(),f.document["authority_identity"].as_str().unwrap(),f.document["contexts"][0]["server_ed25519_pub_hex"].as_str().unwrap(),f.document["authority_server"].as_str().unwrap()]).unwrap();
            for i in 0..2 {
                let cert: annex_server::device_certificates::DeviceCertificate =
                    serde_json::from_value(
                        f.document["events"][i]["operation"]["certificate"].clone(),
                    )
                    .unwrap();
                let digest = annex_server::device_certificates::verify_certificate(
                    &cert,
                    &cert.server_identity,
                    &cert.pseudonym_id,
                    f.state.membership_vkey_v2.as_ref().unwrap(),
                )
                .unwrap();
                conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_moderate) VALUES(1,?1,'HUMAN',?2)",params![cert.pseudonym_id,i==0]).unwrap();
                conn.execute("INSERT INTO channel_members(server_id,channel_id,pseudonym_id) VALUES(1,?1,?2)",params![f.channel(),cert.pseudonym_id]).unwrap();
                conn.execute("INSERT INTO member_device_keys(server_id,pseudonym_id,device_id,x25519_pub_hex,device_certificate,certificate_sha256) VALUES(1,?1,?2,?3,?4,?5)",params![cert.pseudonym_id,cert.device_id,cert.x25519_pub_hex,serde_json::to_string(&cert).unwrap(),digest]).unwrap();
            }
            // The independently signed authorization targets revision two: seed
            // an empty epoch state, then let the two actual events advance it.
            conn.execute(
                "UPDATE channel_key_state SET revision=0 WHERE channel_id=?1",
                [f.channel()],
            )
            .unwrap();
        }
        f.append(0).unwrap();
        f.append(1).unwrap();
        assert_eq!(f.current().revision, 2);
        f
    }
    fn channel(&self) -> &str {
        self.document["channel_id"].as_str().unwrap()
    }
    fn append(&self, index: usize) -> Result<Value, e2e::E2eError> {
        let event: manifest::SignedManifestEvent =
            serde_json::from_value(self.document["events"][index].clone()).unwrap();
        let mut conn = self.state.pool.get().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let result = manifest::append(
            &tx,
            &self.state,
            self.channel(),
            &event.signer_pseudonym_id,
            &event,
        )?;
        tx.commit().unwrap();
        Ok(result)
    }
    fn request(&self) -> e2e::PostWrapsRequest {
        let bundle = &self.document["authorizations"][0];
        let r = &bundle["receipt"];
        serde_json::from_value(json!({"version":2,"operation":r["operation"],"expected_revision":r["expected_revision"],"epoch":r["epoch"],"key_id":r["key_id"],"sender_device_id":r["sender_device_id"],"wraps":bundle["wraps"],"authorization":{"manifest_seq":r["manifest_seq"],"manifest_hash":r["manifest_hash"],"signature_hex":r["signature_hex"]}})).unwrap()
    }
    fn commit(&self, request: &e2e::PostWrapsRequest) -> Result<Value, e2e::E2eError> {
        commit(
            &self.state,
            self.channel(),
            self.document["authorizations"][0]["receipt"]["sender_pseudonym_id"]
                .as_str()
                .unwrap(),
            request,
        )
    }
    fn current(&self) -> e2e::KeyState {
        e2e::read_state(&self.state.pool.get().unwrap(), &self.state, self.channel()).unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        self.state
            .pool
            .get()
            .unwrap()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}
fn commit(
    state: &AppState,
    channel: &str,
    pid: &str,
    request: &e2e::PostWrapsRequest,
) -> Result<Value, e2e::E2eError> {
    let mut conn = state.pool.get().unwrap();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = e2e::store_wraps(&tx, state, channel, pid, request)?;
    tx.commit()?;
    Ok(result)
}

#[test]
fn genuine_signed_commit_has_one_database_cas_winner_and_preserves_receipt_and_namespace() {
    let f = Fixture::new();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let state = f.state.clone();
            let channel = f.channel().to_owned();
            let pid = f.document["authorizations"][0]["receipt"]["sender_pseudonym_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let request = f.request();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                commit(&state, &channel, &pid, &request)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(e2e::E2eError::Conflict { .. })))
            .count(),
        1
    );
    assert_eq!(f.count("channel_key_epochs"), 1);
    assert_eq!(f.count("channel_device_key_wraps"), 2);
    assert_eq!(f.count("channel_key_authorizations"), 1);
    let conn = f.state.pool.get().unwrap();
    let stored: (String, String) = conn
        .query_row(
            "SELECT authority_namespace,manifest_hash FROM channel_key_epochs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored.0, f.document["authority_identity"].as_str().unwrap());
    assert_eq!(
        stored.1,
        f.document["events"][1]["event_hash"].as_str().unwrap()
    );
    assert!(conn
        .execute(
            "UPDATE channel_key_epochs SET authority_namespace='different'",
            []
        )
        .is_err());
}
#[test]
fn injected_wrap_failure_rolls_back_receipt_epoch_and_cas_revision_together() {
    let f = Fixture::new();
    f.state.pool.get().unwrap().execute_batch("CREATE TRIGGER test_wrap_failure BEFORE INSERT ON channel_device_key_wraps BEGIN SELECT RAISE(ABORT,'injected disk write refusal'); END;").unwrap();
    assert!(f.commit(&f.request()).is_err());
    assert_eq!(f.current().revision, 2);
    assert_eq!(f.current().current_epoch, 0);
    for table in [
        "channel_key_authorizations",
        "channel_key_epochs",
        "channel_device_key_wraps",
    ] {
        assert_eq!(f.count(table), 0, "{table}");
    }
    f.state
        .pool
        .get()
        .unwrap()
        .execute_batch("DROP TRIGGER test_wrap_failure;")
        .unwrap();
    f.commit(&f.request()).unwrap();
}
#[test]
fn tampered_sealed_blob_is_refused_before_any_epoch_or_receipt_is_persisted() {
    let f = Fixture::new();
    let mut request = f.request();
    request.wraps[0].wrapped_key_b64.replace_range(0..4, "AAAA");
    assert!(matches!(
        f.commit(&request),
        Err(e2e::E2eError::Forbidden(_))
    ));
    assert_eq!(f.count("channel_key_epochs"), 0);
    assert_eq!(f.count("channel_key_authorizations"), 0);
    f.commit(&f.request()).unwrap();
}
#[test]
fn signed_departure_requires_rotation_without_destroying_authorized_history() {
    let f = Fixture::new();
    let request = f.request();
    f.commit(&request).unwrap();
    let first = f.document["events"][0]["signer_pseudonym_id"]
        .as_str()
        .unwrap();
    assert!(e2e::require_current_encrypted_epoch(
        &f.state.pool.get().unwrap(),
        &f.state,
        f.channel(),
        first,
        1,
        &request.key_id
    )
    .is_ok());
    f.append(2).unwrap();
    f.append(3).unwrap();
    let replay = f.append(3).unwrap();
    assert_eq!(replay["applied"], false);
    assert_eq!(replay["manifest"]["head_seq"], 4);
    assert_eq!(f.current().required_epoch, 2);
    assert!(e2e::require_current_encrypted_epoch(
        &f.state.pool.get().unwrap(),
        &f.state,
        f.channel(),
        first,
        1,
        &request.key_id
    )
    .is_err());
    assert_eq!(f.count("channel_key_epochs"), 1);
    assert_eq!(f.count("channel_device_key_wraps"), 2);
    assert!(f.append(4).is_err()); // Signed admission cannot bypass ordinary Join.
    let rejoining = f.document["events"][4]["signer_pseudonym_id"]
        .as_str()
        .unwrap();
    annex_channels::add_member(
        &f.state.pool.get().unwrap(),
        f.state.server_id,
        f.channel(),
        rejoining,
    )
    .unwrap();
    f.append(4).unwrap(); // Same still-certified device can rejoin by signed policy.
                          // An old departure retry returns its exact old receipt and cannot leak a
                          // newer roster or undo the later rejoin.
    let replay = f.append(3).unwrap();
    assert_eq!(replay["manifest"]["head_seq"], 4);
    assert_eq!(
        manifest::projection(&f.state.pool.get().unwrap(), &f.state, f.channel())
            .unwrap()
            .unwrap()
            .members
            .len(),
        2
    );
}
#[test]
fn certificate_or_membership_revocation_between_snapshot_and_commit_fails_closed() {
    for revoke_device in [false, true] {
        let f = Fixture::new();
        let request = f.request();
        let pid = f.document["events"][1]["signer_pseudonym_id"]
            .as_str()
            .unwrap();
        let conn = f.state.pool.get().unwrap();
        if revoke_device {
            conn.execute(
                "UPDATE member_device_keys SET revoked_at=datetime('now') WHERE pseudonym_id=?1",
                [pid],
            )
            .unwrap();
        } else {
            conn.execute(
                "DELETE FROM channel_members WHERE channel_id=?1 AND pseudonym_id=?2",
                params![f.channel(), pid],
            )
            .unwrap();
        }
        drop(conn);
        assert!(matches!(
            f.commit(&request),
            Err(e2e::E2eError::Conflict { .. })
        ));
        assert_eq!(f.count("channel_key_epochs"), 0);
        assert_eq!(f.count("channel_key_authorizations"), 0);
    }
}
#[test]
fn stored_projection_cannot_be_reused_for_a_different_channel_or_authority() {
    let f = Fixture::new();
    let conn = f.state.pool.get().unwrap();
    let mut projection = manifest::projection(&conn, &f.state, f.channel())
        .unwrap()
        .unwrap();
    projection.channel_id = "other".into();
    conn.execute(
        "UPDATE channel_manifest_state SET projection_json=?1",
        [serde_json::to_string(&projection).unwrap()],
    )
    .unwrap();
    assert!(manifest::projection(&conn, &f.state, f.channel()).is_err());
    assert!(e2e::eligible_devices(&conn, &f.state, f.channel(), 0).is_err());
}
