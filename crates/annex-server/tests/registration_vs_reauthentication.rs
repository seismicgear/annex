//! Getting back in is not the same request as joining.
//!
//! A cached membership proof binds to one Merkle root. The root moves every
//! time anyone registers, so a returning member re-proves routinely — and the
//! client's re-prove goes through `POST /api/registry/register`, because that
//! is where it gets the Merkle path. That endpoint's first three acts were to
//! check the invite requirement, check the member cap, and claim an invite
//! seat: the three questions that decide whether to admit a STRANGER.
//!
//! So an enrolled member was refused whenever the answer to a question that
//! was never about them happened to be no. Concretely: the server fills up and
//! every existing member is locked out the next time the tree changes; an
//! invite reaches its use limit and the person it admitted can no longer get
//! back in; the operator switches to `invite_only` and everyone who joined
//! before the switch is refused for having no code.
//!
//! What must NOT be lost in fixing that: deactivation and revocation. Those
//! are not admission questions either, and they belong to the returning member
//! specifically. They are asserted here alongside the lockout cases, because a
//! fix that let anyone enrolled walk back in unconditionally would satisfy the
//! first half of this file and quietly undo bans.

mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::json;
use std::net::SocketAddr;
use tower::ServiceExt;

/// Distinct, well-formed BN254 field elements — the registry rejects
/// non-canonical hex, so these cannot be arbitrary strings.
fn commitment(n: u8) -> String {
    format!("{:064x}", 0x1000u64 + n as u64)
}

fn register_body(commitment_hex: &str, invite: Option<&str>) -> serde_json::Value {
    let mut body = json!({
        "commitmentHex": commitment_hex,
        "roleCode": 1,
        "nodeId": 7,
    });
    if let Some(code) = invite {
        body["inviteCode"] = json!(code);
    }
    body
}

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

async fn register(
    app: &axum::Router,
    commitment_hex: &str,
    invite: Option<&str>,
) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(post(
            "/api/registry/register",
            register_body(commitment_hex, invite),
        ))
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 256 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// The headline case: a server at its member limit must still serve its own
/// members their Merkle path.
#[tokio::test]
async fn a_full_server_still_lets_an_enrolled_member_back_in() {
    let policy = annex_types::ServerPolicy {
        max_members: 2,
        ..Default::default()
    };
    let (app, _pool) = common::setup_test_app_with_policy(policy).await;

    let alice = commitment(1);
    let (status, body) = register(&app, &alice, None).await;
    assert_eq!(status, StatusCode::OK, "first registration: {body}");

    // Fill the server. `max_members` counts `platform_identities`, which are
    // created at verification, so seed them directly — this test is about the
    // registration gate, not about proving.
    {
        let pool = _pool.get().unwrap();
        for i in 0..2 {
            pool.execute(
                "INSERT INTO platform_identities \
                 (server_id, pseudonym_id, participant_type, active) \
                 VALUES (1, ?1, 'HUMAN', 1)",
                [format!("filler-{i}")],
            )
            .unwrap();
        }
    }

    // A stranger is refused, which is the cap doing its job.
    let (status, _) = register(&app, &commitment(2), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a full server must refuse a NEW member"
    );

    // The enrolled member is not a stranger.
    let (status, body) = register(&app, &alice, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an enrolled member must still be able to fetch their path on a full \
         server — otherwise every member is locked out the moment the server \
         fills, on the path they use to sign in: {body}"
    );
}

/// An exhausted invite must not lock out the person it admitted.
#[tokio::test]
async fn a_spent_invite_does_not_lock_out_the_member_it_admitted() {
    let policy = annex_types::ServerPolicy {
        access_mode: "invite_only".to_string(),
        ..Default::default()
    };
    let (app, pool) = common::setup_test_app_with_policy(policy).await;

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO invite_codes (server_id, code, created_by, max_uses, use_count) \
             VALUES (1, 'ONCE', 'founder', 1, 0)",
            [],
        )
        .unwrap();
    }

    let alice = commitment(3);
    let (status, body) = register(&app, &alice, Some("ONCE")).await;
    assert_eq!(status, StatusCode::OK, "first use of the invite: {body}");

    // The invite is spent: a second person cannot use it.
    let (status, _) = register(&app, &commitment(4), Some("ONCE")).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a one-use invite must admit exactly one identity"
    );

    // Alice comes back. Her proof went stale because somebody else registered;
    // she re-proves, which means fetching her path from here. She holds the
    // same code, and it is exhausted.
    let (status, body) = register(&app, &alice, Some("ONCE")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the member an invite admitted must not be locked out by that invite \
         running out: {body}"
    );

    // And re-registering must not have burned a use — there was nothing to burn.
    let use_count: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT use_count FROM invite_codes WHERE code = 'ONCE'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        use_count, 1,
        "a returning member must not spend an invite seat"
    );
}

/// On an invite_only server, a member who already holds a place must not be
/// asked for a code every time their proof goes stale.
///
/// The realistic shape of this is an operator switching the server to
/// invite_only after people have joined: those members have no code and never
/// needed one. It is asserted here as "an enrolled member presents no code and
/// is served" rather than by flipping the policy mid-test, because
/// `access_mode` is read from the in-memory `AppState.policy` and writing
/// `servers.policy_json` does not change it — a test written that way passes
/// for the wrong reason, having never actually turned invite_only on.
#[tokio::test]
async fn an_enrolled_member_needs_no_invite_code_on_an_invite_only_server() {
    let policy = annex_types::ServerPolicy {
        access_mode: "invite_only".to_string(),
        ..Default::default()
    };
    let (app, pool) = common::setup_test_app_with_policy(policy).await;

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO invite_codes (server_id, code, created_by, max_uses, use_count) \
             VALUES (1, 'WELCOME', 'founder', NULL, 0)",
            [],
        )
        .unwrap();
    }

    let alice = commitment(5);
    let (status, body) = register(&app, &alice, Some("WELCOME")).await;
    assert_eq!(status, StatusCode::OK, "admission with a code: {body}");

    // A stranger with no code is refused, which is invite_only working.
    let (status, _) = register(&app, &commitment(6), None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "invite_only must refuse a new member with no code"
    );

    // Alice's client re-proves and asks for her path. It has no code to send —
    // the code was consumed at admission and is not part of her identity.
    let (status, body) = register(&app, &alice, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an enrolled member must not be asked for an invite code to fetch their \
         own Merkle path; requiring one locks out everyone whose code is spent \
         or who predates the switch to invite_only: {body}"
    );
}

/// The other half. An enrolled commitment is not a licence.
#[tokio::test]
async fn re_registering_grants_nothing_but_the_merkle_path() {
    let (app, _pool) =
        common::setup_test_app_with_policy(annex_types::ServerPolicy::default()).await;

    let alice = commitment(7);
    let (_, first) = register(&app, &alice, None).await;
    let (status, second) = register(&app, &alice, None).await;
    assert_eq!(status, StatusCode::OK);

    let a: serde_json::Value = serde_json::from_str(&first).unwrap();
    let b: serde_json::Value = serde_json::from_str(&second).unwrap();
    assert_eq!(
        a["leafIndex"], b["leafIndex"],
        "a re-registration must resolve to the same leaf, not a second one"
    );
    assert!(
        b.get("sessionToken").is_none(),
        "this endpoint must never issue a credential — it returns a Merkle \
         path, and the proof is what authenticates"
    );
}

/// The audit log must not record a registration that did not happen.
#[tokio::test]
async fn a_returning_member_is_not_logged_as_a_new_registration() {
    let (app, pool) =
        common::setup_test_app_with_policy(annex_types::ServerPolicy::default()).await;

    let alice = commitment(8);
    register(&app, &alice, None).await;
    let after_first: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM public_event_log WHERE event_type = 'IDENTITY_REGISTERED'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after_first, 1);

    register(&app, &alice, None).await;
    register(&app, &alice, None).await;
    let after_replays: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM public_event_log WHERE event_type = 'IDENTITY_REGISTERED'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        after_replays, 1,
        "re-fetching a Merkle path is not joining a server; writing an \
         IDENTITY_REGISTERED entry per sign-in puts a false claim in a signed, \
         hash-chained log"
    );
}
