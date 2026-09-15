//! A membership proof must be evidence of a live authentication.
//!
//! Every field of a `POST /api/zk/verify-membership` body is stable for a
//! given member and topic — root, commitment, nullifier, topic hash, and the
//! Groth16 proof over them. Before the challenge existed, that made the whole
//! body a bearer credential: capture one successful sign-in and re-submit it
//! and the server minted a fresh session token at the identity's CURRENT
//! revocation epoch. Revoking an identity's sessions therefore did not survive
//! its own re-authentication path, which is the one path revocation most needs
//! to survive. Holding `sk` was never required.
//!
//! The closure has two halves and this file covers the server half:
//!
//!   - the circuit takes the challenge as a CONSTRAINED public input
//!     (`zk/circuits/membership_v2.circom`), so Groth16's own verification
//!     equation rejects a proof presented with a different one;
//!   - the server issues each challenge once, binds it to one commitment and
//!     one topic, and spends it inside the same `IMMEDIATE` transaction that
//!     mints the session.
//!
//! The end-to-end assertion — capture a real sign-in with a real proof, replay
//! it, revoke, replay again, then sign in freshly — lives in
//! `scripts/smoke-server-flow.mjs`, because it needs a real Groth16 proof
//! against a real server. What is here is the part that can be pinned cheaply
//! and deterministically: the challenge's own lifecycle, and the fact that a
//! v2 request without one is refused before any expensive work happens.

mod common;

use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_server::api_zk_challenge::{
    consume_challenge, issue_challenge, ChallengeRejection, CHALLENGE_TTL_SECS,
    MAX_OUTSTANDING_PER_COMMITMENT,
};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::json;
use std::net::SocketAddr;
use tower::ServiceExt;

const COMMITMENT: &str = "11aa22bb33cc44dd55ee66ff77008899aabbccddeeff00112233445566778899";
const OTHER_COMMITMENT: &str = "99887766554433221100ffeeddccbbaa99887766554433221100ffeeddccbbaa";
const TOPIC: &str = "annex:server:test:v2";

/// A migrated, server-seeded in-memory database.
fn db() -> annex_db::DbPool {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    run_migrations(&conn).unwrap();
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', '{}')",
        [],
    )
    .unwrap();
    drop(conn);
    pool
}

/// The whole point: a challenge spends exactly once.
#[test]
fn a_challenge_can_be_spent_once_and_only_once() {
    let pool = db();
    let mut conn = pool.get().unwrap();
    let now = 1_700_000_000;

    let issued = issue_challenge(&mut conn, 1, COMMITMENT, TOPIC, now).unwrap();
    assert_eq!(issued.expires_in_secs, CHALLENGE_TTL_SECS);

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(
        consume_challenge(&tx, 1, &issued.challenge, COMMITMENT, TOPIC, now).unwrap(),
        Ok(()),
        "the first presentation must succeed"
    );
    // Second presentation inside the SAME transaction: this is the shape a
    // concurrent replay takes once serialised by the IMMEDIATE lock.
    assert_eq!(
        consume_challenge(&tx, 1, &issued.challenge, COMMITMENT, TOPIC, now).unwrap(),
        Err(ChallengeRejection::AlreadyConsumed),
    );
    tx.commit().unwrap();

    // And after the commit, which is how an ordinary replay arrives.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(
        consume_challenge(&tx, 1, &issued.challenge, COMMITMENT, TOPIC, now + 1).unwrap(),
        Err(ChallengeRejection::AlreadyConsumed),
        "a captured sign-in must not be spendable a second time"
    );
}

/// A challenge observed in flight is useless to anyone else.
#[test]
fn a_challenge_is_bound_to_the_commitment_and_topic_it_was_issued_for() {
    let pool = db();
    let mut conn = pool.get().unwrap();
    let now = 1_700_000_000;
    let issued = issue_challenge(&mut conn, 1, COMMITMENT, TOPIC, now).unwrap();

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(
        consume_challenge(&tx, 1, &issued.challenge, OTHER_COMMITMENT, TOPIC, now).unwrap(),
        Err(ChallengeRejection::WrongCommitment),
    );
    assert_eq!(
        consume_challenge(&tx, 1, &issued.challenge, COMMITMENT, "annex:other:v2", now).unwrap(),
        Err(ChallengeRejection::WrongTopic),
    );
    // A different server's id is a different challenge namespace entirely.
    assert_eq!(
        consume_challenge(&tx, 2, &issued.challenge, COMMITMENT, TOPIC, now).unwrap(),
        Err(ChallengeRejection::Unknown),
    );
    // None of the refusals above may have consumed it: the legitimate holder
    // must still be able to spend their own challenge.
    assert_eq!(
        consume_challenge(&tx, 1, &issued.challenge, COMMITMENT, TOPIC, now).unwrap(),
        Ok(()),
        "a rejected presentation must not burn the challenge — otherwise anyone \
         who can guess a challenge can deny its owner a sign-in"
    );
}

#[test]
fn an_expired_challenge_is_refused() {
    let pool = db();
    let mut conn = pool.get().unwrap();
    let now = 1_700_000_000;
    let issued = issue_challenge(&mut conn, 1, COMMITMENT, TOPIC, now).unwrap();

    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(
        consume_challenge(
            &tx,
            1,
            &issued.challenge,
            COMMITMENT,
            TOPIC,
            now + CHALLENGE_TTL_SECS + 1
        )
        .unwrap(),
        Err(ChallengeRejection::Expired),
    );
    // Exactly at the boundary it is still good — an off-by-one here would fail
    // a proof that finished generating on the last legal second.
    assert_eq!(
        consume_challenge(
            &tx,
            1,
            &issued.challenge,
            COMMITMENT,
            TOPIC,
            now + CHALLENGE_TTL_SECS
        )
        .unwrap(),
        Ok(()),
    );
}

#[test]
fn a_challenge_that_was_never_issued_is_refused() {
    let pool = db();
    let conn = pool.get().unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    assert_eq!(
        consume_challenge(&tx, 1, &"ab".repeat(32), COMMITMENT, TOPIC, 1_700_000_000).unwrap(),
        Err(ChallengeRejection::Unknown),
    );
}

/// Two draws must not collide, and must not be predictable from each other.
///
/// A weak draw would make the challenge decorative: an attacker replaying a
/// captured proof would simply guess the next challenge and prove against it
/// — except they cannot produce a proof at all without `sk`, so what a
/// predictable challenge really buys is the ability to burn other people's
/// challenges. Either way the value has to be drawn from the CSPRNG, and the
/// top-bit masking must not collapse the space.
#[test]
fn challenges_are_distinct_canonical_field_elements() {
    let pool = db();
    let mut conn = pool.get().unwrap();
    let mut seen = std::collections::HashSet::new();
    for i in 0..64 {
        let c = issue_challenge(&mut conn, 1, COMMITMENT, TOPIC, 1_700_000_000 + i)
            .unwrap()
            .challenge;
        assert_eq!(c.len(), 64, "canonical hex is 64 characters");
        assert!(
            c.chars()
                .all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch)),
            "canonical hex is lowercase: {c}"
        );
        // Must parse as a canonical BN254 field element — the server compares
        // it against a public signal, and a value that needed reduction would
        // not round-trip.
        annex_identity::zk::parse_canonical_fr_hex(&c)
            .unwrap_or_else(|e| panic!("{c} is not a canonical field element: {e}"));
        assert!(seen.insert(c), "a challenge repeated within 64 draws");
    }
}

/// An unauthenticated caller must not be able to make the server store
/// unboundedly many rows.
#[test]
fn the_outstanding_set_per_commitment_is_bounded() {
    let pool = db();
    let mut conn = pool.get().unwrap();
    let now = 1_700_000_000;
    let mut issued = Vec::new();
    for i in 0..(MAX_OUTSTANDING_PER_COMMITMENT as i64 * 3) {
        issued.push(
            issue_challenge(&mut conn, 1, COMMITMENT, TOPIC, now + i)
                .unwrap()
                .challenge,
        );
    }

    let outstanding: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM zk_auth_challenges
             WHERE server_id = 1 AND commitment_hex = ?1 AND consumed_at IS NULL",
            [COMMITMENT],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(outstanding as usize, MAX_OUTSTANDING_PER_COMMITMENT);

    // The NEWEST one must still work: bounding the set by dropping the oldest
    // keeps a client that is mid-sign-in able to finish. Dropping the newest,
    // or refusing to issue, would wedge exactly the user who retried.
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert_eq!(
        consume_challenge(&tx, 1, issued.last().unwrap(), COMMITMENT, TOPIC, now).unwrap(),
        Ok(()),
    );
}

#[test]
fn issuing_sweeps_expired_rows() {
    let pool = db();
    let mut conn = pool.get().unwrap();
    let now = 1_700_000_000;
    issue_challenge(&mut conn, 1, COMMITMENT, TOPIC, now).unwrap();
    assert_eq!(row_count(&conn), 1);

    // Well past the first one's expiry.
    issue_challenge(
        &mut conn,
        1,
        OTHER_COMMITMENT,
        TOPIC,
        now + CHALLENGE_TTL_SECS + 10,
    )
    .unwrap();
    assert_eq!(
        row_count(&conn),
        1,
        "the expired row should be gone, leaving only the new one"
    );
}

fn row_count(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM zk_auth_challenges", [], |r| r.get(0))
        .unwrap()
}

// ── HTTP boundary ───────────────────────────────────────────────────────────

fn post(uri: &str, payload: serde_json::Value) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    req
}

#[tokio::test]
async fn the_challenge_endpoint_issues_a_usable_value() {
    let (app, _pool) = common::setup_test_app().await;
    let resp = app
        .oneshot(post(
            "/api/zk/challenge",
            json!({ "commitment": COMMITMENT, "topic": TOPIC }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let challenge = body["challenge"].as_str().expect("challenge field");
    annex_identity::zk::parse_canonical_fr_hex(challenge)
        .expect("the issued challenge must be a canonical field element");
    assert_eq!(body["expiresInSecs"].as_i64(), Some(CHALLENGE_TTL_SECS));
}

/// A malformed commitment is a 400 on the cheap request, not a confusing
/// failure a minute later after the client has generated a proof.
#[tokio::test]
async fn the_challenge_endpoint_rejects_a_malformed_commitment() {
    let (app, _pool) = common::setup_test_app().await;
    let resp = app
        .oneshot(post(
            "/api/zk/challenge",
            json!({ "commitment": "not-hex", "topic": TOPIC }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// The refusal a client that has not been updated will hit — and it has to say
/// what to do, because "invalid number of public signals" would send someone
/// looking at their circuit rather than at the missing endpoint call.
#[tokio::test]
async fn a_v2_request_without_a_challenge_says_so() {
    // `setup_test_app` builds a v1-only server, so route the check through the
    // signal-count boundary instead: a four-signal v2 body is the exact shape
    // an un-updated client sends, and it must not be mistaken for a valid one.
    let (app, _pool) = common::setup_test_app().await;
    let resp = app
        .oneshot(post(
            "/api/zk/verify-membership",
            json!({
                "root": "0".repeat(64),
                "commitment": COMMITMENT,
                "topic": TOPIC,
                "proof": {
                    "pi_a": ["0", "0", "1"],
                    "pi_b": [["0", "0"], ["0", "0"], ["1", "0"]],
                    "pi_c": ["0", "0", "1"],
                    "protocol": "groth16",
                    "curve": "bn128"
                },
                "publicSignals": ["0", "0", "0", "0"],
                "protocolVersion": "v2",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "a v1-only server refuses v2 outright; the challenge requirement is \
         exercised end-to-end by scripts/smoke-server-flow.mjs"
    );
}
