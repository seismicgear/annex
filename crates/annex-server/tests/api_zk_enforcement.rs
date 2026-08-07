//! `enforce_zk_proofs = true` — the production posture — at the HTTP boundary.
//!
//! Every other integration test in this crate runs with
//! `enforce_zk_proofs: false`, because that is what `common::build_app_state`
//! hard-codes. `Config::default()` ships the opposite (`config.rs` has a test
//! asserting it must stay `true`), so the entire test suite exercised the
//! development posture and *nothing* exercised the one operators actually run.
//!
//! That gap hides a specific class of defect: a security gate that is
//! implemented, unit-tested, and never reached. `verify_zk_membership_header`
//! can be flawless and still be decoration if no route calls it; the
//! `X-Annex-Pseudonym` rejection in `auth_middleware` can be deleted in a
//! refactor and every existing test still passes, because every existing test
//! runs in the branch where that header is legal. The same is true in reverse:
//! if someone flips the enforced branch to fail open, nothing here would have
//! noticed.
//!
//! So this file asserts the *difference* enforcement makes, route by route:
//!
//!   * the dev `X-Annex-Pseudonym` header authenticates when enforcement is
//!     off and is refused when it is on — the same request, two answers;
//!   * a valid session token with no `x-annex-zk-proof` header is refused on
//!     the routes that call `ChannelService::enforce_zk`, and adding that one
//!     header turns the identical request into a 200;
//!   * the routes that deliberately do *not* carry the gate still answer, so a
//!     route silently losing (or silently gaining) its gate fails here;
//!   * the unauthenticated bootstrap routes stay open, because a client that
//!     cannot reach `/api/zk/verify-membership` can never mint the session
//!     token that enforced mode requires — gating them would brick sign-in.
//!
//! Writing those down surfaced one asymmetry that is almost certainly a bug
//! rather than a decision: `GET /api/channels/{id}/messages` requires a
//! membership proof and `GET /api/messages/search` returns the same channel's
//! decrypted content without one. That is pinned by
//! `search_currently_returns_channel_content_that_history_would_refuse`, which
//! documents the hole and fails the day it is closed.
//!
//! ## Why this file builds its own verifying key
//!
//! One test needs a proof the server *accepts*. Two options were unavailable:
//!
//!   * `common::load_vkey_or_dummy()`'s dummy fallback does not "accept
//!     anything" — it is built with `gamma_abc_g1: vec![g1; 2]`, which
//!     declares **one** public input, while the v1 verifier submits **two**
//!     (root, commitment). ark-groth16 returns `MalformedVerifyingKey` and the
//!     middleware maps that to 403. Under the dummy vkey, enforced mode
//!     rejects *every* proof, valid or not, so it cannot demonstrate the
//!     accept branch.
//!   * The real `zk/keys/membership_vkey.json` only accepts proofs produced by
//!     the membership circuit, which means shelling out to snarkjs
//!     (`api_zk_verify.rs` does exactly that, and skips when the toolchain is
//!     missing). Tests here must be deterministic and offline, and a test that
//!     skips itself on most machines is not coverage of the accept path.
//!
//! So [`accepting_vkey`] constructs a real BN254 verifying key under which one
//! specific proof satisfies the Groth16 pairing equation, with no prover and no
//! I/O. It is *not* a rubber stamp — [`a_valid_proof_is_accepted_and_the_key_is_not_a_rubber_stamp`]
//! submits a second proof under the same key and requires it to be refused.
//! What is under test is the server's enforcement plumbing (does the header get
//! decoded, bound to the identity, dispatched to the right verifier, checked
//! against the root epoch, and does the answer reach the HTTP layer), not
//! arkworks' pairing arithmetic, which has its own tests.

mod common;

use annex_db::{create_pool, run_migrations, DbPool, DbRuntimeSettings};
use annex_identity::zk::{
    generate_dummy_vkey, serialize_vkey_to_snarkjs_json, Bn254, G1Affine, G2Affine, VerifyingKey,
};
use annex_identity::MerkleTree;
use annex_server::api_ws::{generate_session_token, SESSION_TOKEN_TTL_SECS};
use annex_server::app;
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use base64::Engine;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

// ── The gate map ─────────────────────────────────────────────────────────
//
// These two lists are the coverage contract. `ChannelService` calls
// `enforce_zk` in exactly three places (`join_channel`, `get_history`,
// `join_voice_channel` in services/channel_service.rs); everything else
// behind `auth_middleware` is authenticated but not proof-gated. Both
// directions are asserted, so a route that loses its gate fails
// `only_the_gated_routes_demand_a_proof` and a route that gains one fails
// `the_ungated_authenticated_routes_still_answer_without_a_proof`. Either
// failure is a prompt to update this list on purpose rather than by accident.

/// Routes that call `ChannelService::enforce_zk`.
const ZK_GATED_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/channels/chan/join"),
    ("GET", "/api/channels/chan/messages"),
    ("POST", "/api/channels/chan/voice/join"),
    // Moved here from the ungated list: search returns the same decrypted
    // content as the history route, so it needs the same proof. It was
    // the one asymmetry writing this file surfaced.
    ("GET", "/api/messages/search?q=a"),
];

/// Routes behind `auth_middleware` that deliberately carry no proof gate.
/// Each one is here because it returns metadata a session-token holder is
/// entitled to, not channel content. Anything that returns message bodies
/// belongs in `ZK_GATED_ROUTES`.
const UNGATED_AUTHENTICATED_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/channels"),
    ("GET", "/api/channels/chan"),
    ("GET", "/api/channels/chan/voice/status"),
    ("GET", "/api/channels/chan/messages/msg-1/edits"),
    ("POST", "/api/ws/token"),
];

// ── Fixture ──────────────────────────────────────────────────────────────

struct Fixture {
    app: axum::Router,
    /// Kept so individual tests can seed extra rows into the same database
    /// the router is serving from.
    pool: DbPool,
    /// Copied out of the `AppState` before it is moved into the router, so
    /// the tests mint tokens with the server's real secret rather than a
    /// hard-coded copy that could silently drift from `build_app_state`.
    ws_token_secret: Arc<[u8; 32]>,
}

/// The Merkle root and identity commitment used throughout. Both are the
/// all-zero field element: `verify_zk_membership_header` feeds them to the
/// verifier as the two public inputs, and [`accepting_vkey`]'s IC vector is
/// built for exactly that pair. The values are otherwise arbitrary — nothing
/// in the enforcement path treats zero specially.
fn zero_hex() -> String {
    "0".repeat(64)
}

/// Builds a router whose `enforce_zk_proofs` flag is the ONLY thing the
/// caller varies. Everything else — schema, policy, identities, channel
/// membership, registered commitment, active root epoch — is identical, so a
/// difference in response can only be attributed to enforcement.
///
/// Seeds:
///   * `alice` — active identity, member of `chan`, commitment registered.
///   * `legacy` — active identity, member of `chan`, NO commitment row
///     (a pre-ZK identity, which enforced mode must refuse outright).
async fn build(enforce: bool, vkey: Arc<VerifyingKey<Bn254>>) -> Fixture {
    let policy = ServerPolicy::default();
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [serde_json::to_string(&policy).unwrap()],
        )
        .unwrap();
    }

    seed_identity(&pool, "alice");
    seed_identity(&pool, "legacy");
    seed_channel(&pool, "chan", &["alice", "legacy"]);
    seed_commitment(&pool, "alice", &zero_hex());
    seed_active_root(&pool, &zero_hex());

    let tree = MerkleTree::new(20).unwrap();
    let mut state = common::build_app_state(pool.clone(), tree, policy);
    state.enforce_zk_proofs = enforce;
    state.membership_vkey = vkey;

    let ws_token_secret = state.ws_token_secret.clone();
    Fixture {
        app: app(state),
        pool,
        ws_token_secret,
    }
}

/// The common case: the real (or dummy) key from `common`, exactly as every
/// other test file loads it. Sufficient for every *rejection* test, because
/// no key accepts a proof that never gets built.
async fn build_with_shipped_vkey(enforce: bool) -> Fixture {
    build(enforce, common::load_vkey_or_dummy()).await
}

fn seed_identity(pool: &DbPool, pseudonym: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities
           (server_id, pseudonym_id, participant_type, can_voice, can_moderate,
            can_invite, can_federate, can_bridge, active)
         VALUES (1, ?1, 'HUMAN', 1, 0, 1, 0, 0, 1)",
        [pseudonym],
    )
    .unwrap();
}

/// Creates the channel through `annex_channels::create_channel` rather than
/// raw SQL. The `channel_type` / `federation_scope` columns hold serde JSON
/// (`"Text"`, *with* the quotes), so a hand-written `INSERT` produces a row
/// that every reader fails to deserialise — `GET /api/channels` answers 500
/// and the route looks gated when it is merely broken.
fn seed_channel(pool: &DbPool, channel_id: &str, members: &[&str]) {
    let conn = pool.get().unwrap();
    annex_channels::create_channel(
        &conn,
        &annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: channel_id.to_string(),
            name: "zk-enforcement".to_string(),
            channel_type: annex_types::ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: annex_types::FederationScope::Local,
        },
    )
    .unwrap();
    for m in members {
        conn.execute(
            "INSERT INTO channel_members (channel_id, pseudonym_id, server_id)
             VALUES (?1, ?2, 1)",
            [channel_id, m],
        )
        .unwrap();
    }
}

/// Mirrors what a completed `verify-membership` flow persists: the
/// denormalised `zk_nullifiers` row that `find_commitment_for_pseudonym`
/// resolves so the proof can be bound to the authenticated identity.
fn seed_commitment(pool: &DbPool, pseudonym: &str, commitment_hex: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO zk_nullifiers (topic, nullifier_hex, pseudonym_id, commitment_hex)
         VALUES ('annex:server:v1', ?1, ?2, ?3)",
        rusqlite::params![
            format!("nullifier-for-{pseudonym}"),
            pseudonym,
            commitment_hex
        ],
    )
    .unwrap();
}

/// A root epoch that is still current (`active_until IS NULL`), which is what
/// `is_root_acceptable` requires.
fn seed_active_root(pool: &DbPool, root_hex: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO vrp_root_epochs (root_hex, root_epoch, leaf_count, active_from)
         VALUES (?1, 1, 1, datetime('now'))",
        [root_hex],
    )
    .unwrap();
}

/// A root that rotated out and whose grace window has already closed. Proofs
/// against it must stop verifying — that is the whole point of
/// `accepted_until`.
fn seed_expired_root(pool: &DbPool, root_hex: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO vrp_root_epochs
           (root_hex, root_epoch, leaf_count, active_from, active_until, accepted_until)
         VALUES (?1, 2, 2, datetime('now', '-2 hours'),
                 datetime('now', '-1 hours'), datetime('now', '-30 minutes'))",
        [root_hex],
    )
    .unwrap();
}

// ── Requests ─────────────────────────────────────────────────────────────

fn with_conn_info(mut req: Request<Body>) -> Request<Body> {
    // `rate_limit_middleware` 500s without a source IP, so every request
    // needs one; the address itself is irrelevant to these tests.
    let addr: SocketAddr = "127.0.0.1:52000".parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

/// The development credential: a bare pseudonym in `X-Annex-Pseudonym`.
fn dev_header_request(method: &str, uri: &str, pseudonym: &str) -> Request<Body> {
    with_conn_info(
        Request::builder()
            .uri(uri)
            .method(method)
            .header("X-Annex-Pseudonym", pseudonym)
            .body(Body::empty())
            .unwrap(),
    )
}

/// The production credential: an HMAC-signed session token, optionally
/// accompanied by a base64 `x-annex-zk-proof` header.
fn token_request(
    fixture: &Fixture,
    method: &str,
    uri: &str,
    pseudonym: &str,
    proof_header: Option<&str>,
) -> Request<Body> {
    let token = generate_session_token(pseudonym, &fixture.ws_token_secret, SESSION_TOKEN_TTL_SECS);
    let mut builder = Request::builder()
        .uri(uri)
        .method(method)
        .header("Authorization", format!("Bearer {token}"));
    if let Some(p) = proof_header {
        builder = builder.header("x-annex-zk-proof", p);
    }
    with_conn_info(builder.body(Body::empty()).unwrap())
}

async fn status_of(app: &axum::Router, req: Request<Body>) -> StatusCode {
    app.clone().oneshot(req).await.unwrap().status()
}

// ── Proof construction ───────────────────────────────────────────────────

/// Round-trips a single G1 point into the snarkjs array form `parse_proof`
/// expects, by borrowing the vkey serialiser. Avoids hard-coding BN254
/// coordinates: every point in these tests is derived from the curve
/// generators that `generate_dummy_vkey` already exposes.
fn g1_json(point: G1Affine) -> Value {
    let mut vk = generate_dummy_vkey();
    vk.alpha_g1 = point;
    let json: Value = serde_json::from_str(&serialize_vkey_to_snarkjs_json(&vk)).unwrap();
    json["vk_alpha_1"].clone()
}

fn g2_json(point: G2Affine) -> Value {
    let mut vk = generate_dummy_vkey();
    vk.beta_g2 = point;
    let json: Value = serde_json::from_str(&serialize_vkey_to_snarkjs_json(&vk)).unwrap();
    json["vk_beta_2"].clone()
}

/// A verifying key under which [`satisfying_proof`] — and, as the tests
/// require, only proofs equivalent to it — verifies.
///
/// Groth16 checks `e(A,B) · e(IC(inputs), -gamma) · e(C, -delta) == e(alpha, beta)`.
/// With both public inputs zero, `IC(inputs)` collapses to `gamma_abc_g1[0]`,
/// so setting every G1 element to the generator `G`, `beta = gamma = H`,
/// `delta = -H`, and submitting `A = G, B = H, C = G` gives
/// `e(G,H) · e(G,-H) · e(G,H) == e(G,H)`. True — and false the moment any of
/// A, B or C changes.
///
/// The IC vector must hold three points: `prepare_inputs` requires
/// `public_inputs.len() + 1 == gamma_abc_g1.len()`, and the v1 path submits
/// two inputs. (The shipped dummy key has two, which is why it rejects
/// everything rather than accepting everything.)
fn accepting_vkey() -> Arc<VerifyingKey<Bn254>> {
    let mut vk = generate_dummy_vkey();
    vk.gamma_abc_g1 = vec![vk.alpha_g1; 3];
    vk.delta_g2 = -vk.gamma_g2;
    Arc::new(vk)
}

/// Same key with the two input coefficients set to the point at infinity, so
/// `IC(inputs)` is `gamma_abc_g1[0]` whatever the inputs are and the pairing
/// check passes for *any* root and commitment.
///
/// This exists so a rejection can be attributed. `verify_zk_membership_header`
/// runs the pairing check before it consults `vrp_root_epochs`; under
/// [`accepting_vkey`] a request citing a different root fails the pairing
/// check and never reaches the root logic, so a 403 would prove nothing about
/// root acceptance. Under this key the pairing check cannot be the reason, and
/// the only thing left to reject on is the root epoch.
fn input_blind_vkey() -> Arc<VerifyingKey<Bn254>> {
    let mut vk = generate_dummy_vkey();
    vk.gamma_abc_g1 = vec![vk.alpha_g1, G1Affine::default(), G1Affine::default()];
    vk.delta_g2 = -vk.gamma_g2;
    Arc::new(vk)
}

fn satisfying_proof() -> Value {
    let dummy = generate_dummy_vkey();
    json!({
        "pi_a": g1_json(dummy.alpha_g1),
        "pi_b": g2_json(dummy.beta_g2),
        "pi_c": g1_json(dummy.alpha_g1),
        "protocol": "groth16",
        "curve": "bn128",
    })
}

/// Same shape, same curve, same subgroup — `pi_a` negated. Parses cleanly and
/// fails the pairing check, which is what makes the accept test meaningful.
fn non_satisfying_proof() -> Value {
    let dummy = generate_dummy_vkey();
    json!({
        "pi_a": g1_json(-dummy.alpha_g1),
        "pi_b": g2_json(dummy.beta_g2),
        "pi_c": g1_json(dummy.alpha_g1),
        "protocol": "groth16",
        "curve": "bn128",
    })
}

/// Encodes a `ZkProofPayload` the way the client does: base64(JSON).
fn proof_header(proof: &Value, root_hex: &str, commitment_hex: &str) -> String {
    let payload = json!({
        "proof": proof,
        "root_hex": root_hex,
        "commitment_hex": commitment_hex,
    });
    base64::engine::general_purpose::STANDARD.encode(payload.to_string())
}

// ── The dev header: legal in one mode, refused in the other ──────────────

/// The headline differential. One request, byte for byte identical, answered
/// two different ways depending on a single boolean. If this test ever shows
/// the same status on both sides, `enforce_zk_proofs` is not wired to the
/// request path at all and every other assertion in this file is vacuous.
#[tokio::test]
async fn the_same_request_is_answered_differently_with_enforcement_on_and_off() {
    let uri = "/api/channels/chan/messages";

    let off = build_with_shipped_vkey(false).await;
    let off_status = status_of(&off.app, dev_header_request("GET", uri, "alice")).await;

    let on = build_with_shipped_vkey(true).await;
    let on_status = status_of(&on.app, dev_header_request("GET", uri, "alice")).await;

    assert_eq!(
        off_status,
        StatusCode::OK,
        "with enforcement off the dev pseudonym header must authenticate and \
         the ZK gate must be a no-op — if this is not 200, the fixture is \
         broken and the comparison below proves nothing",
    );
    assert_eq!(
        on_status,
        StatusCode::UNAUTHORIZED,
        "with enforcement on, `X-Annex-Pseudonym` is a public string anyone \
         can guess and must be refused",
    );
    assert_ne!(
        off_status, on_status,
        "enforce_zk_proofs made no difference to an identical request — the \
         flag is not reaching the request path",
    );
}

/// The dev header is refused on every authenticated route, not just the
/// proof-gated ones: the rejection lives in `auth_middleware`, before routing.
/// A regression that scoped it to the ZK-gated routes would leave admin,
/// invite, key-distribution and username routes impersonatable by anyone who
/// knows a pseudonym — and pseudonyms are published by `/api/public/agents`.
#[tokio::test]
async fn the_dev_pseudonym_header_is_refused_on_every_authenticated_route() {
    let fixture = build_with_shipped_vkey(true).await;

    for (method, uri) in ZK_GATED_ROUTES.iter().chain(UNGATED_AUTHENTICATED_ROUTES) {
        let status = status_of(&fixture.app, dev_header_request(method, uri, "alice")).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} accepted a raw X-Annex-Pseudonym under enforcement",
        );
    }
}

/// The other half of the auth-layer rule: a Bearer token must be an
/// HMAC-signed session token, not a raw pseudonym. Sending the pseudonym as a
/// bearer credential is the obvious workaround once the custom header stops
/// working, so it gets its own assertion.
#[tokio::test]
async fn a_raw_pseudonym_as_a_bearer_token_is_refused_when_enforcement_is_on() {
    let fixture = build_with_shipped_vkey(true).await;

    let req = with_conn_info(
        Request::builder()
            .uri("/api/channels")
            .method("GET")
            .header("Authorization", "Bearer alice")
            .body(Body::empty())
            .unwrap(),
    );
    assert_eq!(
        status_of(&fixture.app, req).await,
        StatusCode::UNAUTHORIZED,
        "a raw pseudonym passed as a Bearer token bypasses the whole session \
         token scheme if accepted",
    );
}

// ── The proof gate ───────────────────────────────────────────────────────

/// A correctly authenticated caller with no proof header is refused on every
/// gated route. Authentication is deliberately valid here: the failure must
/// come from the missing proof, which the companion accept test confirms by
/// turning the same request into a 200 with one header added.
#[tokio::test]
async fn a_session_token_without_a_proof_is_refused_on_the_gated_routes() {
    let fixture = build_with_shipped_vkey(true).await;

    for (method, uri) in ZK_GATED_ROUTES {
        let status = status_of(
            &fixture.app,
            token_request(&fixture, method, uri, "alice", None),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} served an authenticated request with no ZK proof \
             while enforcement was on",
        );
    }
}

/// The same routes, the same missing proof, enforcement off: all answered.
/// Together with the test above this pins that the 403s are produced by
/// enforcement rather than by membership, capability or channel-type checks
/// that would fail in both modes.
#[tokio::test]
async fn the_gated_routes_answer_without_a_proof_when_enforcement_is_off() {
    let fixture = build_with_shipped_vkey(false).await;

    for (method, uri) in ZK_GATED_ROUTES {
        let status = status_of(&fixture.app, dev_header_request(method, uri, "alice")).await;
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} refused a proofless request with enforcement OFF — \
             the ZK gate is firing in development mode",
        );
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {uri} rejected the dev credential with enforcement OFF",
        );
    }
}

/// Adding one header flips the answer. This is the accept branch of
/// `verify_zk_membership_header` reached over HTTP: header decoded, commitment
/// bound to the authenticated identity, v1 verifier selected, pairing checked,
/// root epoch consulted, `Ok(())` propagated back through `ChannelService` to
/// the router.
///
/// The second half is what keeps the first half honest. If the key accepted
/// anything, the 200 above would prove only that a header was present.
#[tokio::test]
async fn a_valid_proof_is_accepted_and_the_key_is_not_a_rubber_stamp() {
    let fixture = build(true, accepting_vkey()).await;
    let uri = "/api/channels/chan/messages";
    let root = zero_hex();

    let good = proof_header(&satisfying_proof(), &root, &root);
    assert_eq!(
        status_of(
            &fixture.app,
            token_request(&fixture, "GET", uri, "alice", Some(&good))
        )
        .await,
        StatusCode::OK,
        "a proof that satisfies the server's verifying key, is bound to the \
         caller's registered commitment, and cites the active root was refused",
    );

    assert_eq!(
        status_of(
            &fixture.app,
            token_request(&fixture, "GET", uri, "alice", None)
        )
        .await,
        StatusCode::FORBIDDEN,
        "the identical request without the proof header must be refused — \
         otherwise the 200 above says nothing about the gate",
    );

    let bad = proof_header(&non_satisfying_proof(), &root, &root);
    assert_eq!(
        status_of(
            &fixture.app,
            token_request(&fixture, "GET", uri, "alice", Some(&bad))
        )
        .await,
        StatusCode::FORBIDDEN,
        "a well-formed proof that fails the pairing check was accepted — the \
         verifier is not actually being consulted",
    );
}

/// Proof replay across identities. The proof itself verifies; it is simply
/// bound to somebody else's commitment. Without this check a single leaked
/// proof would authorise every session token on the server.
#[tokio::test]
async fn a_proof_bound_to_another_identitys_commitment_is_refused() {
    let fixture = build(true, accepting_vkey()).await;
    let root = zero_hex();
    let someone_else = format!("{}1", "0".repeat(63));

    let header = proof_header(&satisfying_proof(), &root, &someone_else);
    assert_eq!(
        status_of(
            &fixture.app,
            token_request(
                &fixture,
                "GET",
                "/api/channels/chan/messages",
                "alice",
                Some(&header)
            )
        )
        .await,
        StatusCode::FORBIDDEN,
        "a proof carrying a commitment that is not the caller's was accepted",
    );
}

/// A pre-ZK identity has no row in `zk_nullifiers`, so there is nothing to
/// bind a proof to. Enforced mode must refuse rather than skip the binding:
/// "no commitment on file" is exactly the state an attacker would engineer if
/// absence meant exemption.
///
/// `legacy` is otherwise indistinguishable from `alice` — active, a member of
/// the same channel, holding a valid session token and the same valid proof.
/// Registering a commitment mid-test turns the identical request into a 200,
/// which is what pins the refusal to the missing commitment rather than to
/// any of the other gates on the way.
#[tokio::test]
async fn an_identity_with_no_registered_commitment_cannot_pass_the_gate() {
    let fixture = build(true, accepting_vkey()).await;
    let root = zero_hex();
    let header = proof_header(&satisfying_proof(), &root, &root);
    let request = || {
        token_request(
            &fixture,
            "GET",
            "/api/channels/chan/messages",
            "legacy",
            Some(&header),
        )
    };

    assert_eq!(
        status_of(&fixture.app, request()).await,
        StatusCode::FORBIDDEN,
        "an identity with no registered commitment passed the ZK gate — a \
         legacy identity must re-register, not bypass",
    );

    seed_commitment(&fixture.pool, "legacy", &root);
    assert_eq!(
        status_of(&fixture.app, request()).await,
        StatusCode::OK,
        "the same identity, same proof, now with a commitment on file, was \
         still refused — the refusal above was not about the commitment",
    );
}

/// The proof verifies and is correctly bound, but cites a Merkle root the
/// server does not currently accept. Three roots, one identical proof, one
/// identical commitment: the ONLY thing that varies between the requests is
/// the root, so the answers can only be attributed to `is_root_acceptable`.
///
/// The active root must be accepted, or proof-carrying clients cannot work at
/// all. A root the server never published must be refused, or a prover can
/// invent a tree. A rotated-out root past its `accepted_until` grace window
/// must be refused, or a revoked member keeps proving membership forever with
/// a proof built against the tree that still contained them.
#[tokio::test]
async fn a_proof_is_accepted_only_against_a_currently_acceptable_root() {
    // Input-blind key: see `input_blind_vkey`. Under the input-sensitive key
    // a changed root fails the pairing check first and the root logic is
    // never reached, which would make every assertion below unattributable.
    let fixture = build(true, input_blind_vkey()).await;
    let commitment = zero_hex();
    let active_root = zero_hex();
    // `parse_fr_from_hex` accepts short even-length hex, so these are valid
    // field elements — they are simply not acceptable root epochs.
    let never_published_root = "01";
    let retired_root = "02";
    seed_expired_root(&fixture.pool, retired_root);

    let ask = |root: String| {
        let header = proof_header(&satisfying_proof(), &root, &commitment);
        let req = token_request(
            &fixture,
            "GET",
            "/api/channels/chan/messages",
            "alice",
            Some(&header),
        );
        async { status_of(&fixture.app, req).await }
    };

    assert_eq!(
        ask(active_root).await,
        StatusCode::OK,
        "the current root was refused — the control case for this test failed, \
         so the rejections below prove nothing",
    );
    assert_eq!(
        ask(never_published_root.to_string()).await,
        StatusCode::FORBIDDEN,
        "a proof citing a root the server has never published was accepted",
    );
    assert_eq!(
        ask(retired_root.to_string()).await,
        StatusCode::FORBIDDEN,
        "a proof citing a retired root, past its grace window, was accepted",
    );
}

/// The root the client claims is fed to the verifier as a public input rather
/// than merely echoed back. Both roots below are seeded as acceptable epochs,
/// so the root table cannot be the reason for the rejection; the key here IS
/// input-sensitive, so the only thing that can reject the second request is
/// the public input reaching the pairing check.
///
/// Without this, a middleware that parsed `root_hex` for the epoch lookup but
/// verified the proof against a fixed or attacker-supplied input vector would
/// look identical from the outside.
#[tokio::test]
async fn the_claimed_root_is_fed_to_the_verifier_as_a_public_input() {
    let fixture = build(true, accepting_vkey()).await;
    let commitment = zero_hex();

    // A second acceptable epoch, so `is_root_acceptable` says yes to both.
    {
        let conn = fixture.pool.get().unwrap();
        conn.execute(
            "INSERT INTO vrp_root_epochs (root_hex, root_epoch, leaf_count, active_from)
             VALUES ('01', 3, 3, datetime('now'))",
            [],
        )
        .unwrap();
    }

    let matching = proof_header(&satisfying_proof(), &zero_hex(), &commitment);
    assert_eq!(
        status_of(
            &fixture.app,
            token_request(
                &fixture,
                "GET",
                "/api/channels/chan/messages",
                "alice",
                Some(&matching)
            )
        )
        .await,
        StatusCode::OK,
    );

    let other_root = proof_header(&satisfying_proof(), "01", &commitment);
    assert_eq!(
        status_of(
            &fixture.app,
            token_request(
                &fixture,
                "GET",
                "/api/channels/chan/messages",
                "alice",
                Some(&other_root)
            )
        )
        .await,
        StatusCode::FORBIDDEN,
        "swapping the claimed root for another acceptable one did not change \
         the verdict — the root is not reaching the verifier as a public input",
    );
}

/// Everything the client could put in the header that is not a usable proof.
/// Each of these decodes to a different failure point inside
/// `verify_zk_membership_header`; all of them must be 403 rather than a
/// 500 or, worse, a pass.
#[tokio::test]
async fn a_malformed_proof_header_is_refused_rather_than_crashing() {
    let fixture = build(true, accepting_vkey()).await;
    let root = zero_hex();

    let valid_proof = satisfying_proof();
    let not_base64 = "!!!not base64!!!".to_string();
    let base64_but_not_json =
        base64::engine::general_purpose::STANDARD.encode("this is not json at all");
    let json_but_wrong_shape =
        base64::engine::general_purpose::STANDARD.encode(json!({"nope": 1}).to_string());
    let unsupported_version = base64::engine::general_purpose::STANDARD.encode(
        json!({
            "proof": valid_proof,
            "root_hex": root,
            "commitment_hex": root,
            "protocolVersion": "v9",
        })
        .to_string(),
    );
    // v2 is not enabled on this server (`membership_vkey_v2: None`). The
    // server must refuse rather than silently downgrade the proof to the v1
    // verifier, which would accept a nullifier scheme it was not built for.
    let v2_when_v2_is_disabled = base64::engine::general_purpose::STANDARD.encode(
        json!({
            "proof": valid_proof,
            "root_hex": root,
            "commitment_hex": root,
            "protocolVersion": "v2",
            "publicSignals": ["0", "0", "0", "0"],
            "topic": "annex:server:v1",
        })
        .to_string(),
    );

    for (label, header) in [
        ("not base64", not_base64),
        ("base64 but not JSON", base64_but_not_json),
        ("JSON without the payload fields", json_but_wrong_shape),
        ("unsupported protocolVersion", unsupported_version),
        ("v2 proof on a v1-only server", v2_when_v2_is_disabled),
    ] {
        let status = status_of(
            &fixture.app,
            token_request(
                &fixture,
                "GET",
                "/api/channels/chan/messages",
                "alice",
                Some(&header),
            ),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "proof header ({label}) should have been refused with 403",
        );
    }
}

// ── The exemptions ───────────────────────────────────────────────────────

/// The gate map, asserted from both sides in one place: gated routes refuse a
/// proofless request, ungated ones answer it. Held in a single test so the
/// contrast is visible — the same credential, the same server, the same
/// moment, two behaviours that are supposed to differ.
#[tokio::test]
async fn only_the_gated_routes_demand_a_proof() {
    let fixture = build_with_shipped_vkey(true).await;

    for (method, uri) in ZK_GATED_ROUTES {
        assert_eq!(
            status_of(
                &fixture.app,
                token_request(&fixture, method, uri, "alice", None)
            )
            .await,
            StatusCode::FORBIDDEN,
            "{method} {uri} is listed as ZK-gated but served a proofless request",
        );
    }

    for (method, uri) in UNGATED_AUTHENTICATED_ROUTES {
        assert_ne!(
            status_of(
                &fixture.app,
                token_request(&fixture, method, uri, "alice", None)
            )
            .await,
            StatusCode::FORBIDDEN,
            "{method} {uri} is listed as ungated but refused a proofless \
             request — if the gate was added on purpose, move the route into \
             ZK_GATED_ROUTES",
        );
    }
}

/// The ungated authenticated routes answer normally under enforcement: a
/// valid session token alone is enough.
///
/// This asserts the shipped behaviour, and the shipped behaviour is not
/// self-evidently correct. `GET /api/messages/search` returns decrypted
/// message content from the caller's channels with no proof, while
/// `GET /api/channels/{id}/messages` requires one for the same content in the
/// same channels. If that asymmetry is closed, this test is where it will be
/// noticed — move the route into `ZK_GATED_ROUTES` and the suite stays green.
#[tokio::test]
async fn the_ungated_authenticated_routes_still_answer_without_a_proof() {
    let fixture = build_with_shipped_vkey(true).await;

    for (method, uri) in UNGATED_AUTHENTICATED_ROUTES {
        let status = status_of(
            &fixture.app,
            token_request(&fixture, method, uri, "alice", None),
        )
        .await;
        assert!(
            status.is_success(),
            "{method} {uri} returned {status} for an authenticated, proofless \
             request under enforcement",
        );
    }
}

/// The bootstrap chain. Enforced mode requires an HMAC session token, and the
/// only way to obtain the first one is `POST /api/zk/verify-membership`, which
/// is deliberately outside `auth_middleware`. If enforcement ever reached
/// these routes, a fresh client could never sign in: no token, no way to get
/// a token. `/health` is here for the same reason at the infrastructure
/// level — a probe that starts returning 401 takes the deployment down.
#[tokio::test]
async fn the_unauthenticated_bootstrap_routes_stay_open_under_enforcement() {
    let fixture = build_with_shipped_vkey(true).await;

    for (method, uri) in [
        ("GET", "/health"),
        ("GET", "/api/registry/current-root"),
        ("GET", "/api/identity/alice"),
    ] {
        let status = status_of(
            &fixture.app,
            with_conn_info(
                Request::builder()
                    .uri(uri)
                    .method(method)
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await;
        assert!(
            status.is_success(),
            "{method} {uri} returned {status} — public routes must not acquire \
             an auth or proof gate",
        );
    }

    // Sent with a deliberately empty body: the point is that the request
    // reaches the handler's extractor rather than being turned away by auth.
    // A 401/403 here would mean the sign-in path had been gated behind the
    // credential it exists to issue.
    let verify = with_conn_info(
        Request::builder()
            .uri("/api/zk/verify-membership")
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    );
    let status = status_of(&fixture.app, verify).await;
    assert!(
        status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
        "/api/zk/verify-membership answered {status} under enforcement — it \
         issues the session token that enforced mode requires, so gating it \
         makes sign-in impossible",
    );
}

/// The token-mint route completes the chain: with a session token it issues a
/// WebSocket token, and it does so without a ZK proof. A proof gate here would
/// make the realtime connection unreachable for a client that has a valid
/// session but no cached proof.
#[tokio::test]
async fn the_ws_token_route_issues_a_token_to_a_session_without_a_proof() {
    let fixture = build_with_shipped_vkey(true).await;

    let resp = fixture
        .app
        .clone()
        .oneshot(token_request(
            &fixture,
            "POST",
            "/api/ws/token",
            "alice",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        json.get("token").and_then(Value::as_str).is_some(),
        "no token in the mint response: {json}",
    );
}

/// Search and history return the same content, so they answer the same way.
///
/// They did not. `GET /api/channels/{id}/messages` demanded a membership
/// proof and `GET /api/messages/search?channel_id={id}` returned the same
/// decrypted messages from the same channel to the same caller with a session
/// token alone. A session token outlives its holder's access to the ZK secret
/// that minted it, so in enforced mode the gate on channel history was
/// bypassable by asking for the content through the other door — the same
/// shape as the edit-history IDOR: two routes serving one dataset, one of them
/// checked.
///
/// Both halves are asserted together because the defect was the *difference*
/// between them. A future change that gates search but ungates history, or
/// vice versa, fails here rather than at whichever one someone remembered.
#[tokio::test]
async fn search_is_gated_exactly_as_channel_history_is() {
    let fixture = build_with_shipped_vkey(true).await;
    {
        let conn = fixture.pool.get().unwrap();
        annex_channels::create_message(
            &conn,
            &annex_channels::CreateMessageParams {
                channel_id: "chan".to_string(),
                message_id: "msg-1".to_string(),
                sender_pseudonym: "alice".to_string(),
                content: "zebrafish".to_string(),
                reply_to_message_id: None,
            },
        )
        .unwrap();
    }

    let history = status_of(
        &fixture.app,
        token_request(
            &fixture,
            "GET",
            "/api/channels/chan/messages",
            "alice",
            None,
        ),
    )
    .await;
    let search = status_of(
        &fixture.app,
        token_request(
            &fixture,
            "GET",
            "/api/messages/search?q=zebrafish&channel_id=chan",
            "alice",
            None,
        ),
    )
    .await;

    assert_eq!(
        history,
        StatusCode::FORBIDDEN,
        "history must stay gated under enforcement",
    );
    assert_eq!(
        search, history,
        "search answered {search} where history answered {history} — one door \
         into the channel's content is checked and the other is not",
    );
}
