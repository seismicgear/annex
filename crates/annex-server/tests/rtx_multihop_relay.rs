//! Multi-hop RTX relay: the signed hop chain, origin attestation and loop
//! prevention.
//!
//! ROADMAP Phase 9 recorded "single-hop only" as the gap and marked the
//! circular-relay check and the origin check as done. The gap was larger and the
//! two completed items were guarding a path no bundle could take:
//!
//!   * `relay_rtx_bundles` set `relay_path = vec![local_public_url]` on every
//!     send, so the list could never hold more than one entry and the cycle
//!     check could only ever detect a cycle back to this server.
//!   * `receive_federated_rtx` stored the bundle, fanned it out to local
//!     subscribers, and stopped. Nothing re-relayed, so no bundle had ever taken
//!     a second hop.
//!   * `relay_path` was a `Vec<String>` any relayer could rewrite freely, and
//!     the only signature on the envelope was the immediate relayer's — so
//!     "B relayed A's bundle" and "B wrote a bundle and put A's name on it" were
//!     the same envelope.
//!
//! Every test here posts a real envelope to the real route.

use annex_db::{create_pool, DbRuntimeSettings};
use annex_federation::FederatedRtxEnvelope;
use annex_identity::MerkleTree;
use annex_rtx::{BundleProvenance, OriginAttestation, ReflectionSummaryBundle, RelayHop};
use annex_server::{
    api_rtx::{rtx_bundle_content_hash, rtx_relay_signing_payload},
    app,
    middleware::RateLimiter,
    AppState,
};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

const LOCAL_URL: &str = "http://local.example";
const ORIGIN_URL: &str = "http://origin.example";
const MIDDLE_URL: &str = "http://middle.example";
const RELAYER_URL: &str = "http://relayer.example";

fn v3_envelope(env: &Env, id: &str) -> FederatedRtxEnvelope {
    let handshake = annex_vrp::VrpFederationHandshake {
        anchor_snapshot: annex_vrp::VrpAnchorSnapshot::new(&[], &[]).unwrap(),
        capability_contract: annex_vrp::VrpCapabilitySharingContract::from_policy(
            &ServerPolicy::default(),
        ),
        scorer: None,
    };
    env.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federation_agreements SET remote_handshake_json=?1",
            [serde_json::to_string(&handshake).unwrap()],
        )
        .unwrap();
    let mut b = bundle(id, Some("controlled reasoning"));
    b.created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let mut contract = annex_vrp::NegotiatedRtxContract {
        knowledge_domains_allowed: Some(vec!["rust".into()]),
        retention_seconds: Some(60),
        max_exchange_size: 4096,
    };
    let commitment = annex_rtx::reasoning_commitment(b.reasoning_chain.as_deref());
    let payload =
        annex_rtx::origin_attestation_payload_v3(&b, ORIGIN_URL, &commitment, 3, &contract);
    let attestation = OriginAttestation {
        version: 3,
        contract: Some(contract.clone()),
        reasoning_commitment: commitment,
        max_hops: 3,
        signature: hex::encode(
            env.origin
                .key
                .sign(&Sha256::digest(payload.as_bytes()))
                .to_bytes(),
        ),
    };
    let mut p = BundleProvenance {
        origin_server: ORIGIN_URL.into(),
        relay_path: vec![],
        bundle_id: b.bundle_id.clone(),
        hops: vec![],
        origin: Some(attestation),
    };
    let mut previous = String::new();
    for (index, peer) in [&env.origin, &env.relayer].iter().enumerate() {
        if index == 1 {
            contract.retention_seconds = Some(30);
            b.reasoning_chain = None;
        }
        let next = if index == 0 { RELAYER_URL } else { LOCAL_URL };
        let content_hash = rtx_bundle_content_hash(&b);
        let payload = annex_rtx::relay_hop_payload_with_contract(
            id,
            ORIGIN_URL,
            &p.origin.as_ref().unwrap().signature,
            index,
            &peer.url,
            next,
            &content_hash,
            &previous,
            Some(&contract),
        );
        previous = annex_rtx::chain_digest(&payload);
        p.hops.push(RelayHop {
            server: peer.url.clone(),
            content_hash,
            contract: Some(contract.clone()),
            signature: hex::encode(
                peer.key
                    .sign(&Sha256::digest(payload.as_bytes()))
                    .to_bytes(),
            ),
        });
    }
    p.relay_path = p.hops.iter().map(|h| h.server.clone()).collect();
    envelope(b, p, &env.relayer)
}

#[tokio::test]
async fn version_three_signed_contract_survives_reasoning_strip_and_tightens_retention_per_hop() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let e = v3_envelope(&env, "v3-tightened");
    let (status, body) = post(&env.state, &e).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let expiry: i64 = env
        .state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT expires_at_ms FROM rtx_bundles WHERE bundle_id='v3-tightened'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(expiry, e.bundle.created_at as i64 + 30_000);
    assert_eq!(post(&env.state, &e).await.0, StatusCode::OK);
}

#[tokio::test]
async fn version_three_origin_and_hop_contract_tampering_fails_before_storage() {
    for tamper in [
        "origin-age",
        "origin-size",
        "origin-domains",
        "hop-age",
        "missing-hop",
        "widened-hop",
    ] {
        let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
        let mut e = v3_envelope(&env, tamper);
        match tamper {
            "origin-age" => {
                e.provenance
                    .origin
                    .as_mut()
                    .unwrap()
                    .contract
                    .as_mut()
                    .unwrap()
                    .retention_seconds = Some(90)
            }
            "origin-size" => {
                e.provenance
                    .origin
                    .as_mut()
                    .unwrap()
                    .contract
                    .as_mut()
                    .unwrap()
                    .max_exchange_size = 8192
            }
            "origin-domains" => {
                e.provenance
                    .origin
                    .as_mut()
                    .unwrap()
                    .contract
                    .as_mut()
                    .unwrap()
                    .knowledge_domains_allowed = None
            }
            "hop-age" => {
                e.provenance.hops[1]
                    .contract
                    .as_mut()
                    .unwrap()
                    .retention_seconds = Some(20)
            }
            "missing-hop" => e.provenance.hops[1].contract = None,
            "widened-hop" => {
                e.provenance.hops[1]
                    .contract
                    .as_mut()
                    .unwrap()
                    .retention_seconds = Some(120)
            }
            _ => unreachable!(),
        }
        let (status, _) = post(&env.state, &e).await;
        assert_eq!(
            status,
            if matches!(tamper, "missing-hop" | "widened-hop") {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::UNAUTHORIZED
            },
            "{tamper}"
        );
        assert_eq!(
            env.state
                .pool
                .get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM rtx_bundles", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn a_restricted_v3_bundle_requires_the_immediate_peer_to_acknowledge_contract_v2() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let e = v3_envelope(&env, "v3-unacknowledged");
    env.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE federation_agreements SET remote_handshake_json=NULL",
            [],
        )
        .unwrap();
    let (status, body) = post(&env.state, &e).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(
        env.state
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM rtx_bundles", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// A server in the mesh: a URL and the key its `instances` row publishes.
struct Peer {
    url: String,
    key: SigningKey,
}

impl Peer {
    fn new(url: &str) -> Self {
        Self {
            url: url.to_string(),
            key: SigningKey::generate(&mut OsRng),
        }
    }
    fn pubkey_hex(&self) -> String {
        hex::encode(self.key.verifying_key().as_bytes())
    }
}

struct Env {
    state: AppState,
    origin: Peer,
    middle: Peer,
    relayer: Peer,
}

/// A local server with three known ACTIVE peers and an agreement with the
/// relayer. `known_middle = false` models the realistic case the trust model has
/// to answer for: in A → B → C → D, D may know A and C but not B.
fn setup(transfer_scope: &str, known_middle: bool, require_chain: bool) -> Env {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let origin = Peer::new(ORIGIN_URL);
    let middle = Peer::new(MIDDLE_URL);
    let relayer = Peer::new(RELAYER_URL);
    let policy = ServerPolicy {
        allow_full_knowledge: true,
        ..ServerPolicy::default()
    };

    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local', ?1)",
            rusqlite::params![serde_json::to_string(&policy).unwrap()],
        )
        .unwrap();

        let mut peers = vec![(&origin, "Origin"), (&relayer, "Relayer")];
        if known_middle {
            peers.push((&middle, "Middle"));
        }
        for (peer, label) in peers {
            conn.execute(
                "INSERT INTO instances (base_url, public_key, label, status) \
                 VALUES (?1, ?2, ?3, 'ACTIVE')",
                rusqlite::params![peer.url, peer.pubkey_hex(), label],
            )
            .unwrap();
        }

        // An agreement with the IMMEDIATE relayer, which is who this server has
        // a relationship with. It has none with the origin or the middle, and
        // that is the point of relaying.
        let relayer_id: i64 = conn
            .query_row(
                "SELECT id FROM instances WHERE base_url = ?1",
                [&relayer.url],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO federation_agreements \
             (local_server_id, remote_instance_id, alignment_status, transfer_scope, \
              agreement_json, active) VALUES (1, ?1, 'ALIGNED', ?2, '{}', 1)",
            rusqlite::params![relayer_id, transfer_scope],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let federation_config = annex_server::config::FederationConfig {
        rtx_require_hop_chain: require_chain,
        ..Default::default()
    };

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(annex_identity::zk::generate_dummy_vkey()),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: Arc::new(SigningKey::generate(&mut OsRng)),
        public_url: Arc::new(RwLock::new(LOCAL_URL.to_string())),
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::WebRtcConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: Arc::new([0u8; 32]),
        voice_token_secret: Arc::new([0u8; 32]),
        federation_config,
        federation_transport: Default::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };

    Env {
        state,
        origin,
        middle,
        relayer,
    }
}

fn bundle(id: &str, reasoning: Option<&str>) -> ReflectionSummaryBundle {
    ReflectionSummaryBundle {
        bundle_id: id.to_string(),
        source_pseudonym: "agent-origin".to_string(),
        source_server: ORIGIN_URL.to_string(),
        domain_tags: vec!["rust".to_string()],
        summary: "Ownership prevents data races.".to_string(),
        reasoning_chain: reasoning.map(|r| r.to_string()),
        caveats: vec!["safe Rust only".to_string()],
        created_at: 1_700_000_000_000,
        signature: "abcdef1234567890".to_string(),
        vrp_handshake_ref: "1:1:1".to_string(),
    }
}

fn attest(b: &ReflectionSummaryBundle, origin: &Peer, max_hops: u8) -> OriginAttestation {
    annex_server::api_rtx::sign_origin_attestation(b, &origin.url, &origin.key, max_hops)
}

/// Build the chain the way the relay path does, hop by hop, so the test's
/// envelope is the envelope production produces.
///
/// `chain` is (peer, the bundle that peer forwarded). The last entry is the
/// immediate sender, and its `next_peer` is `LOCAL_URL`.
fn provenance_for(
    b: &ReflectionSummaryBundle,
    attestation: &OriginAttestation,
    chain: &[(&Peer, &ReflectionSummaryBundle)],
) -> BundleProvenance {
    let mut hops: Vec<RelayHop> = Vec::new();
    let mut prev_digest = String::new();
    for (i, (peer, sent)) in chain.iter().enumerate() {
        let next_peer = chain
            .get(i + 1)
            .map(|(p, _)| p.url.as_str())
            .unwrap_or(LOCAL_URL);
        let content_hash = rtx_bundle_content_hash(sent);
        let payload = annex_rtx::relay_hop_payload(
            &b.bundle_id,
            ORIGIN_URL,
            &attestation.signature,
            i,
            &peer.url,
            next_peer,
            &content_hash,
            &prev_digest,
        );
        prev_digest = annex_rtx::chain_digest(&payload);
        hops.push(RelayHop {
            contract: None,
            server: peer.url.clone(),
            content_hash,
            signature: hex::encode(
                peer.key
                    .sign(&Sha256::digest(payload.as_bytes()))
                    .to_bytes(),
            ),
        });
    }
    BundleProvenance {
        origin_server: ORIGIN_URL.to_string(),
        relay_path: hops.iter().map(|h| h.server.clone()).collect(),
        bundle_id: b.bundle_id.clone(),
        hops,
        origin: Some(attestation.clone()),
    }
}

fn envelope(
    b: ReflectionSummaryBundle,
    provenance: BundleProvenance,
    sender: &Peer,
) -> FederatedRtxEnvelope {
    // The legacy envelope signature, still carried for one release.
    let legacy = rtx_relay_signing_payload(
        &b.bundle_id,
        &sender.url,
        &provenance.origin_server,
        &provenance.relay_path,
        &rtx_bundle_content_hash(&b),
    );
    FederatedRtxEnvelope {
        bundle: b,
        provenance,
        relaying_server: sender.url.clone(),
        signature: hex::encode(sender.key.sign(legacy.as_bytes()).to_bytes()),
    }
}

async fn post(state: &AppState, envelope: &FederatedRtxEnvelope) -> (StatusCode, String) {
    let mut req = Request::builder()
        .uri("/api/federation/rtx")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(envelope).unwrap()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9999))));
    let res = app(state.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    if status == StatusCode::OK {
        annex_server::background::drain_outbox_batch(Arc::new(state.clone()), 32)
            .await
            .unwrap();
    }
    (status, String::from_utf8_lossy(&body).to_string())
}

// ── The happy path, which did not exist ───────────────────────────────────

#[tokio::test]
async fn a_two_hop_chain_is_accepted_and_stored() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-ok", Some("step 1; step 2"));
    let att = attest(&b, &env.origin, 3);
    // origin published it, middle relayed it, relayer sent it to us.
    let p = provenance_for(
        &b,
        &att,
        &[(&env.origin, &b), (&env.middle, &b), (&env.relayer, &b)],
    );
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let conn = env.state.pool.get().unwrap();
    let stored: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_bundles WHERE bundle_id = 'mh-ok'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn a_middle_hop_this_server_does_not_know_is_still_accepted() {
    // The trust-model decision, made explicitly: refusing here would make
    // multi-hop work only in a fully-provisioned mesh, which defeats the point
    // of relaying. The chained hop digests make the hop we DO verify — the
    // immediate relayer — accountable for what it claims the middle was.
    let env = setup("FULL_KNOWLEDGE_BUNDLE", false, true);
    let b = bundle("mh-unknown-middle", None);
    let att = attest(&b, &env.origin, 3);
    let p = provenance_for(
        &b,
        &att,
        &[(&env.origin, &b), (&env.middle, &b), (&env.relayer, &b)],
    );
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ── Forging the origin ────────────────────────────────────────────────────

#[tokio::test]
async fn a_relayer_cannot_claim_a_bundle_came_from_someone_else() {
    // The defect the origin attestation exists for. Before it, the only
    // signature on the envelope was the immediate relayer's, so a relayer could
    // put any server's URL in `origin_server` and nothing could tell.
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-forged-origin", None);
    // The relayer signs an attestation claiming to be the origin.
    let forged = attest(&b, &env.relayer, 3);
    let p = provenance_for(&b, &forged, &[(&env.origin, &b), (&env.relayer, &b)]);
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "a forged origin attestation was accepted"
    );
    assert!(
        body.to_lowercase().contains("attestation")
            || body.to_lowercase().contains("verify")
            || body.to_lowercase().contains("signature"),
        "the refusal should name the attestation: {body}",
    );
}

#[tokio::test]
async fn the_bundle_and_the_provenance_must_agree_on_the_origin() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let mut b = bundle("mh-disagree", None);
    b.source_server = MIDDLE_URL.to_string(); // the bundle says middle wrote it
    let att = attest(&b, &env.origin, 3); // the provenance says origin did
    let p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains("came from"), "{body}");
}

// ── The chain is a chain ──────────────────────────────────────────────────

#[tokio::test]
async fn a_spliced_chain_with_a_middle_hop_removed_is_rejected() {
    // Each hop signature covers the previous hop's payload digest, so removing
    // a hop invalidates every signature after it. Without that chaining, a
    // relayer could present any subset of hops in any order, each individually
    // valid.
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-spliced", None);
    let att = attest(&b, &env.origin, 3);
    let full = provenance_for(
        &b,
        &att,
        &[(&env.origin, &b), (&env.middle, &b), (&env.relayer, &b)],
    );
    let mut spliced = full.clone();
    spliced.hops.remove(1); // drop the middle, keep the other two signatures
    spliced.relay_path = spliced.hops.iter().map(|h| h.server.clone()).collect();
    let (status, body) = post(&env.state, &envelope(b, spliced, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK, "a spliced chain was accepted");
    assert!(body.to_lowercase().contains("verify"), "{body}");
}

#[tokio::test]
async fn the_last_hop_must_be_the_server_that_sent_the_envelope() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-wrong-sender", None);
    let att = attest(&b, &env.origin, 3);
    // Chain ends at middle, but the relayer posts it.
    let p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.middle, &b)]);
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains("last relay hop"), "{body}");
}

#[tokio::test]
async fn the_first_hop_must_be_the_origin() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-no-origin-hop", None);
    let att = attest(&b, &env.origin, 3);
    let p = provenance_for(&b, &att, &[(&env.middle, &b), (&env.relayer, &b)]);
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains("first relay hop"), "{body}");
}

#[tokio::test]
async fn the_last_hops_content_hash_must_be_the_bundle_that_arrived() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-tampered", Some("the original chain"));
    let att = attest(&b, &env.origin, 3);
    let p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    // Send a bundle whose caveats differ from what the last hop hashed. The
    // caveats are outside the scope-invariant digest, so this passes the origin
    // attestation and has to be caught by the hop's own content hash.
    let mut altered = b.clone();
    altered.caveats = vec!["a caveat the origin never wrote".to_string()];
    let (status, body) = post(&env.state, &envelope(altered, p, &env.relayer)).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "in-flight tampering outside the scope-invariant digest was accepted: {body}",
    );
}

// ── Loops and budgets ─────────────────────────────────────────────────────

#[tokio::test]
async fn a_chain_that_already_contains_this_server_is_refused() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-cycle", None);
    let att = attest(&b, &env.origin, 3);
    let mut p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    p.hops.insert(
        1,
        RelayHop {
            contract: None,
            server: LOCAL_URL.to_string(),
            content_hash: rtx_bundle_content_hash(&b),
            signature: "00".repeat(64),
        },
    );
    p.relay_path = p.hops.iter().map(|h| h.server.clone()).collect();
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains("circular"), "{body}");
}

#[tokio::test]
async fn a_repeated_server_in_the_chain_is_refused_structurally() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-repeat", None);
    let att = attest(&b, &env.origin, 3);
    let mut p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    let dup = p.hops[0].clone();
    p.hops.insert(1, dup);
    p.relay_path = p.hops.iter().map(|h| h.server.clone()).collect();
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains("twice"), "{body}");
}

#[tokio::test]
async fn a_chain_past_the_origins_budget_is_refused() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-budget", None);
    // The origin says one hop; the chain has three.
    let att = attest(&b, &env.origin, 1);
    let p = provenance_for(
        &b,
        &att,
        &[(&env.origin, &b), (&env.middle, &b), (&env.relayer, &b)],
    );
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains("budget"), "{body}");
}

#[tokio::test]
async fn a_chain_past_the_protocol_ceiling_is_refused_before_any_signature_work() {
    // The endpoint has no auth middleware in front of it, so an oversized chain
    // has to cost a bounds check rather than N signature verifications.
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-ceiling", None);
    let att = attest(&b, &env.origin, 255);
    let mut p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    for i in 0..annex_rtx::RTX_HOP_CEILING {
        p.hops.insert(
            1,
            RelayHop {
                contract: None,
                server: format!("http://filler{i}.example"),
                content_hash: "00".repeat(32),
                signature: "00".repeat(64),
            },
        );
    }
    p.relay_path = p.hops.iter().map(|h| h.server.clone()).collect();
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains("ceiling"), "{body}");
}

// ── The reasoning chain ───────────────────────────────────────────────────

#[tokio::test]
async fn a_relayer_cannot_add_a_reasoning_chain_the_origin_never_wrote() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let published = bundle("mh-added-reasoning", None);
    let att = attest(&published, &env.origin, 3);
    // The relayer invents a chain and hashes the bundle it actually sends, so
    // the hop content hash is internally consistent. Only the origin's
    // commitment catches it.
    let mut forged = published.clone();
    forged.reasoning_chain = Some("a chain the relayer invented".to_string());
    let p = provenance_for(
        &published,
        &att,
        &[(&env.origin, &forged), (&env.relayer, &forged)],
    );
    let (status, body) = post(&env.state, &envelope(forged, p, &env.relayer)).await;
    assert_ne!(
        status,
        StatusCode::OK,
        "an invented reasoning chain was accepted"
    );
    assert!(body.contains("not the one the origin published"), "{body}");
}

#[tokio::test]
async fn a_relayer_may_strip_a_reasoning_chain_for_scope() {
    // The other half, and the reason the origin cannot simply sign the content
    // hash: stripping the chain for a `ReflectionSummariesOnly` peer is the
    // policy working, and the origin's signature has to survive it.
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let published = bundle("mh-stripped-reasoning", Some("the real chain"));
    let att = attest(&published, &env.origin, 3);
    let mut stripped = published.clone();
    stripped.reasoning_chain = None;
    let p = provenance_for(
        &published,
        &att,
        &[(&env.origin, &stripped), (&env.relayer, &stripped)],
    );
    let (status, body) = post(&env.state, &envelope(stripped, p, &env.relayer)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a legitimately scope-stripped bundle was refused: {body}",
    );
}

#[tokio::test]
async fn origin_v2_rejects_tags_and_caveats_tampered_after_legitimate_reasoning_strip() {
    for tags in [true, false] {
        let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
        let original = bundle(
            if tags { "v2-tags" } else { "v2-caveats" },
            Some("original controlled reasoning"),
        );
        let attestation = attest(&original, &env.origin, 3);
        assert_eq!(attestation.version, 2);
        let mut changed = original.clone();
        changed.reasoning_chain = None;
        if tags {
            changed.domain_tags.push("forged-filter-target".into());
        } else {
            changed.caveats.push("forged-caveat".into());
        }
        let provenance = provenance_for(
            &changed,
            &attestation,
            &[(&env.origin, &original), (&env.relayer, &changed)],
        );
        let (status, body) = post(&env.state, &envelope(changed, provenance, &env.relayer)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert!(body.contains("origin attestation"), "{body}");
        let count: i64 = env
            .state
            .pool
            .get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM rtx_bundles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn legacy_v1_origin_cannot_authorize_malleable_fields_through_a_relayer() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let original = bundle("v1-migration", Some("controlled reasoning"));
    let commitment = annex_rtx::reasoning_commitment(original.reasoning_chain.as_deref());
    let payload = annex_rtx::origin_attestation_payload(
        &original.bundle_id,
        ORIGIN_URL,
        &original.source_pseudonym,
        original.created_at,
        &original.vrp_handshake_ref,
        &annex_rtx::scope_invariant_content_digest(&original),
        &commitment,
        3,
    );
    let attestation = OriginAttestation {
        contract: None,
        version: 1,
        signature: hex::encode(
            env.origin
                .key
                .sign(&Sha256::digest(payload.as_bytes()))
                .to_bytes(),
        ),
        reasoning_commitment: commitment,
        max_hops: 3,
    };
    let provenance = provenance_for(
        &original,
        &attestation,
        &[(&env.origin, &original), (&env.relayer, &original)],
    );
    let (status, body) = post(&env.state, &envelope(original, provenance, &env.relayer)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("republish with attestation v2"), "{body}");
}

// ── Compatibility ─────────────────────────────────────────────────────────

#[tokio::test]
async fn an_envelope_with_no_chain_is_refused_when_the_operator_requires_one() {
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    let b = bundle("mh-legacy-refused", None);
    let p = BundleProvenance {
        origin_server: ORIGIN_URL.to_string(),
        relay_path: vec![env.relayer.url.clone()],
        bundle_id: b.bundle_id.clone(),
        hops: Vec::new(),
        origin: None,
    };
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(body.contains("rtx_require_hop_chain"), "{body}");
}

#[tokio::test]
async fn an_envelope_with_no_chain_is_accepted_while_the_shim_is_on() {
    // `rtx_require_hop_chain` defaults false for one release: an envelope from a
    // peer on an older build has no chain, and refusing those on the day this
    // ships would break federation with every peer that has not upgraded. Such
    // an envelope is delivered locally and NOT re-relayed — an unsigned chain is
    // not something to extend.
    let env = setup("FULL_KNOWLEDGE_BUNDLE", true, false);
    let mut b = bundle("mh-legacy-ok", None);
    b.source_server = RELAYER_URL.to_string();
    let p = BundleProvenance {
        origin_server: RELAYER_URL.to_string(),
        relay_path: vec![env.relayer.url.clone()],
        bundle_id: b.bundle_id.clone(),
        hops: Vec::new(),
        origin: None,
    };
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn the_shipped_default_still_accepts_a_peer_on_an_older_build() {
    // Pins the compatibility posture itself rather than a hand-set flag: if the
    // default flips, this test says so.
    assert!(
        !annex_server::config::FederationConfig::default().rtx_require_hop_chain,
        "rtx_require_hop_chain defaults true, which refuses every peer on an older build",
    );
    assert_eq!(
        annex_server::config::FederationConfig::default().rtx_max_hops,
        3
    );
    assert_eq!(annex_rtx::RTX_HOP_CEILING, 5);
}

// ── The second hop, which is the whole feature ─────────────────────────────
//
// Everything above verifies what this server ACCEPTS. None of it proves a
// bundle travels further, and before this work none did: `receive_federated_rtx`
// stored the bundle, delivered it to local subscribers and returned. The tests
// below stand up a real onward peer and assert the envelope arrives there with
// this server's hop appended.

/// An onward peer's `/api/federation/rtx`, which records what it is sent.
struct Onward {
    url: String,
    /// The raw JSON of each envelope received. `FederatedRtxEnvelope` is not
    /// `Clone`, and deriving it on a production type to make a test tidier is
    /// the wrong trade; parsing on read costs nothing here.
    received: Arc<Mutex<Vec<String>>>,
    key: SigningKey,
}

async fn start_onward_peer() -> Onward {
    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    let router = axum::Router::new().route(
        "/api/federation/rtx",
        axum::routing::post(move |body: String| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push(body);
                axum::Json(serde_json::json!({"ok": true}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Onward {
        url: format!("http://{addr}"),
        received,
        key: SigningKey::generate(&mut OsRng),
    }
}

/// Register an extra ACTIVE peer with an agreement, so the relay will fan out to
/// it.
fn add_peer(env: &Env, url: &str, pubkey_hex: &str, scope: &str) {
    let conn = env.state.pool.get().unwrap();
    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) \
         VALUES (?1, ?2, 'Onward', 'ACTIVE')",
        rusqlite::params![url, pubkey_hex],
    )
    .unwrap();
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO federation_agreements \
         (local_server_id, remote_instance_id, alignment_status, transfer_scope, \
          agreement_json, active) VALUES (1, ?1, 'ALIGNED', ?2, '{}', 1)",
        rusqlite::params![id, scope],
    )
    .unwrap();
}

/// Poll until the onward peer has something, or give up.
async fn await_delivery(onward: &Onward, ms: u64) -> Vec<FederatedRtxEnvelope> {
    let deadline = ms / 20;
    for _ in 0..deadline.max(1) {
        {
            let got = onward.received.lock().unwrap();
            if !got.is_empty() {
                return got
                    .iter()
                    .map(|j| serde_json::from_str(j).expect("the relayed envelope must parse"))
                    .collect();
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Vec::new()
}

#[tokio::test]
async fn a_received_bundle_is_relayed_onwards_with_this_servers_hop_appended() {
    let onward = start_onward_peer().await;
    let mut env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    // The onward peer is on 127.0.0.1, so the SSRF gate refuses it unless the
    // operator has said their peers are on a private network — which is exactly
    // the flag the RTX relay used to ignore while the message relay honoured it.
    env.state.federation_config.allow_private_peer_addresses = true;
    add_peer(
        &env,
        &onward.url,
        &hex::encode(onward.key.verifying_key().as_bytes()),
        "FULL_KNOWLEDGE_BUNDLE",
    );

    let b = bundle("mh-second-hop", Some("reasoning that must survive"));
    let att = attest(&b, &env.origin, 3);
    let p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    let (status, body) = post(&env.state, &envelope(b.clone(), p, &env.relayer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let delivered = await_delivery(&onward, 3_000).await;
    assert_eq!(
        delivered.len(),
        1,
        "the bundle was not relayed onwards — this is the second hop, and it did not exist \
         before: receive_federated_rtx stored, fanned out locally and stopped",
    );
    let out = &delivered[0];

    assert_eq!(out.bundle.bundle_id, "mh-second-hop");
    assert_eq!(out.relaying_server, LOCAL_URL);
    assert_eq!(
        out.provenance.hops.len(),
        3,
        "the chain should have grown by exactly one hop",
    );
    assert_eq!(out.provenance.hops[0].server, ORIGIN_URL);
    assert_eq!(out.provenance.hops[1].server, RELAYER_URL);
    assert_eq!(
        out.provenance.hops[2].server, LOCAL_URL,
        "the appended hop must name this server",
    );
    // The mirrored legacy field tracks it, for a peer on an older build.
    assert_eq!(
        out.provenance.relay_path,
        vec![ORIGIN_URL, RELAYER_URL, LOCAL_URL],
    );

    // The origin attestation is carried unchanged — this server cannot re-derive
    // it, so forwarding the one it received is the only option.
    assert_eq!(
        out.provenance.origin.as_ref().map(|o| o.signature.clone()),
        Some(att.signature.clone()),
    );

    // Our hop signature verifies under our own key, over the payload the
    // receiver will reconstruct.
    let payloads = annex_rtx::hop_payloads(&out.provenance, &att.signature, &onward.url);
    let our_payload = payloads.last().expect("a payload per hop");
    let sig = hex::decode(&out.provenance.hops[2].signature).unwrap();
    let sig: [u8; 64] = sig.try_into().unwrap();
    ed25519_dalek::Verifier::verify(
        &env.state.signing_key.verifying_key(),
        &Sha256::digest(our_payload.as_bytes()),
        &ed25519_dalek::Signature::from_bytes(&sig),
    )
    .expect("this server's hop signature must verify over the payload the peer rebuilds");
}

#[tokio::test]
async fn a_bundle_at_its_budget_is_stored_but_not_relayed_onwards() {
    let onward = start_onward_peer().await;
    let mut env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    env.state.federation_config.allow_private_peer_addresses = true;
    add_peer(
        &env,
        &onward.url,
        &hex::encode(onward.key.verifying_key().as_bytes()),
        "FULL_KNOWLEDGE_BUNDLE",
    );

    // max_hops = 2 and the chain already has 2. Accepting it is correct — the
    // budget bounds how far it TRAVELS, not whether the last server may read it.
    let b = bundle("mh-at-budget", None);
    let att = attest(&b, &env.origin, 2);
    let p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let delivered = await_delivery(&onward, 600).await;
    assert!(
        delivered.is_empty(),
        "a bundle at its hop budget was relayed anyway, so the TTL bounds nothing",
    );
}

#[tokio::test]
async fn an_unsigned_legacy_envelope_is_not_extended() {
    // While the compatibility shim is on, a chainless envelope is accepted and
    // delivered locally. It must not be re-relayed: this server would be signing
    // a hop onto a chain whose origin nobody attested.
    let onward = start_onward_peer().await;
    let mut env = setup("FULL_KNOWLEDGE_BUNDLE", true, false);
    env.state.federation_config.allow_private_peer_addresses = true;
    add_peer(
        &env,
        &onward.url,
        &hex::encode(onward.key.verifying_key().as_bytes()),
        "FULL_KNOWLEDGE_BUNDLE",
    );

    let mut b = bundle("mh-legacy-not-extended", None);
    b.source_server = RELAYER_URL.to_string();
    let p = BundleProvenance {
        origin_server: RELAYER_URL.to_string(),
        relay_path: vec![env.relayer.url.clone()],
        bundle_id: b.bundle_id.clone(),
        hops: Vec::new(),
        origin: None,
    };
    let (status, body) = post(&env.state, &envelope(b, p, &env.relayer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert!(
        await_delivery(&onward, 600).await.is_empty(),
        "an envelope with no signed chain was extended",
    );
}

#[tokio::test]
async fn a_duplicate_arrival_is_not_relayed_again() {
    // Two paths between two servers in a mesh means the same bundle arrives
    // twice. Re-forwarding on every arrival is how that becomes a broadcast
    // storm.
    let onward = start_onward_peer().await;
    let mut env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    env.state.federation_config.allow_private_peer_addresses = true;
    add_peer(
        &env,
        &onward.url,
        &hex::encode(onward.key.verifying_key().as_bytes()),
        "FULL_KNOWLEDGE_BUNDLE",
    );

    let b = bundle("mh-duplicate", None);
    let att = attest(&b, &env.origin, 3);
    let p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    let env1 = envelope(b.clone(), p.clone(), &env.relayer);
    assert_eq!(post(&env.state, &env1).await.0, StatusCode::OK);
    assert_eq!(await_delivery(&onward, 3_000).await.len(), 1);

    // The same envelope again.
    assert_eq!(post(&env.state, &env1).await.0, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(
        onward.received.lock().unwrap().len(),
        1,
        "the duplicate was relayed again",
    );
}

#[tokio::test]
async fn a_bundle_is_never_relayed_back_to_a_server_already_in_its_chain() {
    // The cycle check the ROADMAP recorded as complete could only ever see THIS
    // server, because `relay_path` was reset to `vec![local_public_url]` on
    // every send. Here the relayer and the origin are both eligible peers with
    // agreements, and neither may receive the bundle back.
    let onward = start_onward_peer().await;
    let mut env = setup("FULL_KNOWLEDGE_BUNDLE", true, true);
    env.state.federation_config.allow_private_peer_addresses = true;
    add_peer(
        &env,
        &onward.url,
        &hex::encode(onward.key.verifying_key().as_bytes()),
        "FULL_KNOWLEDGE_BUNDLE",
    );

    // Give the origin an agreement too, so it is in the fan-out list.
    {
        let conn = env.state.pool.get().unwrap();
        let origin_id: i64 = conn
            .query_row(
                "SELECT id FROM instances WHERE base_url = ?1",
                [ORIGIN_URL],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO federation_agreements \
             (local_server_id, remote_instance_id, alignment_status, transfer_scope, \
              agreement_json, active) VALUES (1, ?1, 'ALIGNED', 'FULL_KNOWLEDGE_BUNDLE', '{}', 1)",
            rusqlite::params![origin_id],
        )
        .unwrap();
    }

    let b = bundle("mh-no-backflow", None);
    let att = attest(&b, &env.origin, 4);
    let p = provenance_for(&b, &att, &[(&env.origin, &b), (&env.relayer, &b)]);
    assert_eq!(
        post(&env.state, &envelope(b, p, &env.relayer)).await.0,
        StatusCode::OK
    );

    let delivered = await_delivery(&onward, 3_000).await;
    assert_eq!(
        delivered.len(),
        1,
        "the onward peer should have received it"
    );
    // The onward peer is the ONLY one contacted: the origin and the relayer are
    // both in the chain. Their URLs are unroutable in this test, so the only
    // observable assertion is that the hop we appended names the onward peer as
    // its destination — which is signed, so it cannot be repurposed.
    let payloads = annex_rtx::hop_payloads(&delivered[0].provenance, &att.signature, &onward.url);
    assert!(
        payloads.last().unwrap().contains(&onward.url),
        "the appended hop must be bound to the peer it was sent to",
    );

    // Exactly one delivery: the origin has an agreement and is an eligible peer,
    // so a relay that did not consult the chain would have fanned out to it too.
    assert_eq!(
        onward.received.lock().unwrap().len(),
        1,
        "more than one peer was contacted, so a server already in the chain was sent the \
         bundle back",
    );
}
