//! `/api/admin/members` — the moderation roster and the capability grant.
//!
//! These two routes had no integration test at all, which is a poor place for a
//! gap: `PATCH /api/admin/members/{id}/capabilities` is how moderation itself is
//! granted, so a missing authorisation check here would let any member promote
//! themselves. The handler also carries a `would_remove_last_moderator` guard
//! whose failure mode is an irreversible server lockout, and nothing exercised
//! it.

mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use common::setup_test_app;
use std::net::SocketAddr;
use tower::ServiceExt;

const ADDR: &str = "127.0.0.1:9000";

/// Insert a member directly. `can_moderate` decides whether they are an admin.
fn add_member(pool: &annex_db::DbPool, pseudonym: &str, can_moderate: bool) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities
           (server_id, pseudonym_id, participant_type, can_voice, can_moderate,
            can_invite, can_federate, can_bridge, active)
         VALUES (1, ?1, 'HUMAN', 1, ?2, 1, 0, 0, 1)",
        rusqlite::params![pseudonym, can_moderate as i64],
    )
    .unwrap();
}

fn can_moderate(pool: &annex_db::DbPool, pseudonym: &str) -> bool {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT can_moderate FROM platform_identities WHERE pseudonym_id = ?1",
        [pseudonym],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    caller: &str,
    body: Option<&str>,
) -> (StatusCode, String) {
    let addr: SocketAddr = ADDR.parse().unwrap();
    let mut req = Request::builder()
        .uri(uri)
        .method(method)
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", caller)
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

const ALL_CAPS: &str = r#"{"can_voice":true,"can_moderate":true,"can_invite":true,"can_federate":true,"can_bridge":true}"#;
const NO_MODERATE: &str = r#"{"can_voice":true,"can_moderate":false,"can_invite":true,"can_federate":false,"can_bridge":false}"#;

// ── Listing the roster ────────────────────────────────────────────────────

#[tokio::test]
async fn listing_members_requires_moderation() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "regular_member", false);

    let (status, _) = send(&app, "GET", "/api/admin/members", "regular_member", None).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the member roster names every account on the server; a non-moderator must not read it",
    );
}

#[tokio::test]
async fn a_moderator_sees_every_member() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "the_mod", true);
    add_member(&pool, "member_a", false);
    add_member(&pool, "member_b", false);

    let (status, body) = send(&app, "GET", "/api/admin/members", "the_mod", None).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    for expected in ["the_mod", "member_a", "member_b"] {
        assert!(body.contains(expected), "{expected} missing from {body}");
    }
}

// ── Granting and revoking capabilities ────────────────────────────────────

#[tokio::test]
async fn a_member_cannot_promote_themselves() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "the_mod", true);
    add_member(&pool, "ambitious", false);

    let (status, _) = send(
        &app,
        "PATCH",
        "/api/admin/members/ambitious/capabilities",
        "ambitious",
        Some(ALL_CAPS),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        !can_moderate(&pool, "ambitious"),
        "a refused request must not have applied anything",
    );
}

#[tokio::test]
async fn a_moderator_can_promote_another_member() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "the_mod", true);
    add_member(&pool, "deputy", false);

    let (status, body) = send(
        &app,
        "PATCH",
        "/api/admin/members/deputy/capabilities",
        "the_mod",
        Some(ALL_CAPS),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(can_moderate(&pool, "deputy"));
}

#[tokio::test]
async fn a_moderator_can_demote_another_moderator() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "the_mod", true);
    add_member(&pool, "other_mod", true);

    let (status, body) = send(
        &app,
        "PATCH",
        "/api/admin/members/other_mod/capabilities",
        "the_mod",
        Some(NO_MODERATE),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(!can_moderate(&pool, "other_mod"));
    assert!(
        can_moderate(&pool, "the_mod"),
        "the caller keeps their own rights"
    );
}

// ── The lockout guard ─────────────────────────────────────────────────────
//
// Without this, a moderator can clear `can_moderate` on themselves and on
// everyone else, leaving a server with zero admins and no way back — and
// dropping it into the no-moderator self-heal path, where an unauthenticated
// identity read re-promotes whichever account has the lowest id.

#[tokio::test]
async fn the_last_moderator_cannot_demote_themselves() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "only_mod", true);
    add_member(&pool, "member", false);

    let (status, _) = send(
        &app,
        "PATCH",
        "/api/admin/members/only_mod/capabilities",
        "only_mod",
        Some(NO_MODERATE),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "demoting the last moderator locks the server out irreversibly",
    );
    assert!(
        can_moderate(&pool, "only_mod"),
        "the demotion must not have landed"
    );
}

#[tokio::test]
async fn the_last_moderator_may_still_change_their_other_capabilities() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "only_mod", true);

    // Everything off EXCEPT moderation: the guard is about the last admin, not
    // about freezing their whole row.
    let (status, body) = send(
        &app,
        "PATCH",
        "/api/admin/members/only_mod/capabilities",
        "only_mod",
        Some(r#"{"can_voice":false,"can_moderate":true,"can_invite":false,"can_federate":false,"can_bridge":false}"#),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(can_moderate(&pool, "only_mod"));
}

#[tokio::test]
async fn demoting_the_last_moderator_is_allowed_once_a_second_one_exists() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "founder", true);
    add_member(&pool, "deputy", false);

    // Promote first...
    let (status, _) = send(
        &app,
        "PATCH",
        "/api/admin/members/deputy/capabilities",
        "founder",
        Some(ALL_CAPS),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // ...then the founder can step down, because someone else is holding it.
    let (status, body) = send(
        &app,
        "PATCH",
        "/api/admin/members/founder/capabilities",
        "founder",
        Some(NO_MODERATE),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(!can_moderate(&pool, "founder"));
    assert!(
        can_moderate(&pool, "deputy"),
        "the server still has an admin"
    );
}

#[tokio::test]
async fn an_inactive_moderator_does_not_count_as_cover() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "only_mod", true);
    add_member(&pool, "departed_mod", true);
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE platform_identities SET active = 0 WHERE pseudonym_id = 'departed_mod'",
            [],
        )
        .unwrap();
    }

    let (status, _) = send(
        &app,
        "PATCH",
        "/api/admin/members/only_mod/capabilities",
        "only_mod",
        Some(NO_MODERATE),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "an account that cannot sign in is not a moderator anyone can fall back on",
    );
}

fn disk_state() -> (tempfile::TempDir, std::sync::Arc<annex_server::AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let pool = annex_db::create_pool(
        dir.path().join("moderation.db").to_str().unwrap(),
        annex_db::DbRuntimeSettings::default(),
    )
    .unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'test','Test',?1)",
            [serde_json::to_string(&annex_types::ServerPolicy::default()).unwrap()],
        )
        .unwrap();
    }
    let state = common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        Default::default(),
    );
    (dir, std::sync::Arc::new(state))
}

async fn apply_captured(
    state: std::sync::Arc<annex_server::AppState>,
    caller: annex_identity::PlatformIdentity,
    target: &str,
    body: &str,
) -> StatusCode {
    use axum::response::IntoResponse;
    annex_server::api_admin::update_member_capabilities_handler(
        axum::Extension(state),
        axum::Extension(annex_server::middleware::IdentityContext(caller)),
        axum::extract::Path(target.into()),
        axum::Json(serde_json::from_str(body).unwrap()),
    )
    .await
    .into_response()
    .status()
}

#[tokio::test]
async fn captured_moderator_context_cannot_mutate_after_revocation_or_demotion() {
    let (_dir, state) = disk_state();
    add_member(&state.pool, "moderator", true);
    add_member(&state.pool, "surviving_mod", true);
    add_member(&state.pool, "target", false);
    let captured =
        annex_identity::get_platform_identity(&state.pool.get().unwrap(), 1, "moderator").unwrap();
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET token_epoch=1 WHERE pseudonym_id='moderator'",
            [],
        )
        .unwrap();
    assert_eq!(
        apply_captured(state.clone(), captured, "target", ALL_CAPS).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(!can_moderate(&state.pool, "target"));
    let captured =
        annex_identity::get_platform_identity(&state.pool.get().unwrap(), 1, "moderator").unwrap();
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET can_moderate=0 WHERE pseudonym_id='moderator'",
            [],
        )
        .unwrap();
    assert_eq!(
        apply_captured(state.clone(), captured, "target", ALL_CAPS).await,
        StatusCode::FORBIDDEN
    );
    assert!(!can_moderate(&state.pool, "target"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_captured_moderators_cannot_demote_every_moderator() {
    let (_dir, state) = disk_state();
    add_member(&state.pool, "mod_a", true);
    add_member(&state.pool, "mod_b", true);
    let a = annex_identity::get_platform_identity(&state.pool.get().unwrap(), 1, "mod_a").unwrap();
    let b = annex_identity::get_platform_identity(&state.pool.get().unwrap(), 1, "mod_b").unwrap();
    // Both requests have already passed middleware. The writer transaction
    // must serialize the final privilege check, count, and update on disk.
    let (left, right) = tokio::join!(
        apply_captured(state.clone(), a, "mod_b", NO_MODERATE),
        apply_captured(state.clone(), b, "mod_a", NO_MODERATE),
    );
    assert_eq!(
        [left, right]
            .iter()
            .filter(|&&s| s == StatusCode::OK)
            .count(),
        1
    );
    assert!([left, right].contains(&StatusCode::FORBIDDEN));
    let count:i64 = state.pool.get().unwrap().query_row(
        "SELECT count(*) FROM platform_identities WHERE server_id=1 AND active=1 AND can_moderate=1", [], |row|row.get(0),
    ).unwrap();
    assert_eq!(count, 1);
}
