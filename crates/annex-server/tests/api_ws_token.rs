//! `POST /api/ws/token` and `GET /ws?token=…` — the one-shot WebSocket
//! token handshake, end to end.
//!
//! This is the one authenticated surface in the server that `auth_middleware`
//! never sees. `routes::create_router` mounts `/ws` on its own `Router`,
//! outside the `protected_routes` group, precisely because a browser cannot
//! attach an `Authorization` header to a WebSocket upgrade. Everything that
//! middleware would normally do — prove the caller is who they claim to be,
//! reject inactive identities, honour `enforce_zk_proofs` — has to be
//! re-implemented inside `api_ws::ws_handler`, and there was nothing pinning
//! that re-implementation down.
//!
//! The unit tests under `ws::tokens` are the wrong altitude for this. They
//! could all pass while `ws_handler` never calls `verify_ws_token` at all,
//! or calls it and ignores the pseudonym it returns in favour of the
//! attacker-supplied `?pseudonym=` parameter sitting right next to it in the
//! same query string. A verifier that is never consulted is decoration, and
//! from the outside a socket authenticated by a forged token looks exactly
//! like one authenticated properly.
//!
//! So these tests drive a real `TcpListener` and a real upgrade, and the
//! positive case asserts on the *bound* pseudonym — the name the server
//! stamps on a message the socket sends — rather than merely on the fact
//! that the handshake returned 101. Binding is the property that matters;
//! "the upgrade succeeded" is not.
//!
//! Known gap, deliberately pinned below by
//! `a_ws_token_is_replayable_within_its_ttl`: the doc comment on
//! `WS_TOKEN_TTL_SECS` claims "Tokens are single-use", and they are not.
//! Nothing consumes a token. See that test for why the claim cannot simply
//! be made true where it stands.

mod common;

use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_db::{create_pool, run_migrations, DbPool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::api_ws::{generate_session_token, WS_TOKEN_TTL_SECS};
use annex_server::app;
use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

/// The HMAC key the test server signs its tokens with. A fixed non-zero
/// pattern rather than the harness default of `[0u8; 32]`, so that "signed
/// with the wrong key" is a real distinction and not a comparison of two
/// all-zero arrays.
const SERVER_SECRET: [u8; 32] = [0x5a; 32];

/// A key the server has never seen — stands in for another Annex server, or
/// for an attacker who guessed the token format but not the key.
const FOREIGN_SECRET: [u8; 32] = [0xa5; 32];

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ── Harness ──────────────────────────────────────────────────────────────
//
// A real listener is unavoidable: `tower::ServiceExt::oneshot` cannot
// perform a WebSocket upgrade. The router is cloned before it is handed to
// `axum::serve` so the same `Arc<AppState>` is reachable both over the
// socket (for `/ws`) and in-process (for `POST /api/ws/token`), which is
// what makes "mint over HTTP, spend over WS" a genuine round trip rather
// than two unrelated halves.
//
// The database is a temp file rather than `:memory:` because `create_pool`
// clamps in-memory pools to a single connection, and a live WS session holds
// connections from several tasks at once (`touch_activity`, the membership
// gate, message persistence) while the test body also wants to read.

struct WsTokenApp {
    addr: SocketAddr,
    /// In-process handle to the same app, for the HTTP half of the flow.
    router: axum::Router,
    pool: DbPool,
    /// Held for the lifetime of the test: dropping it removes the database.
    _dir: tempfile::TempDir,
}

async fn setup(enforce_zk_proofs: bool) -> WsTokenApp {
    let dir = tempfile::tempdir().expect("temp db dir");
    let db_path = dir.path().join("annex-test.db");

    let pool = create_pool(
        db_path.to_str().expect("db path is utf-8"),
        DbRuntimeSettings::default(),
    )
    .unwrap();

    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();

        // `alice` and `carol` are both five characters long on purpose: the
        // token payload is `pseudonym|expires|signature`, so a same-length
        // substitution keeps every field boundary where it was and isolates
        // the HMAC as the only thing rejecting the swap.
        add_identity(&conn, "alice", true);
        add_identity(&conn, "carol", true);
        add_identity(&conn, "bob", true);
        add_identity(&conn, "zombie", false);

        add_channel(&conn, "chan-alice");
        add_member(&conn, 1, "chan-alice", "alice").unwrap();
        add_channel(&conn, "chan-bob");
        add_member(&conn, 1, "chan-bob", "bob").unwrap();
    }

    let tree = {
        let conn = pool.get().unwrap();
        MerkleTree::restore(&conn, 20).unwrap()
    };

    let mut state = common::build_app_state(pool.clone(), tree, ServerPolicy::default());
    state.ws_token_secret = std::sync::Arc::new(SERVER_SECRET);
    state.enforce_zk_proofs = enforce_zk_proofs;

    let router = app(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let served = router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            served.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });

    WsTokenApp {
        addr,
        router,
        pool,
        _dir: dir,
    }
}

fn add_identity(conn: &rusqlite::Connection, pseudonym: &str, active: bool) {
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (1, ?1, 'HUMAN', ?2)",
        rusqlite::params![pseudonym, active as i64],
    )
    .unwrap();
}

fn add_channel(conn: &rusqlite::Connection, channel_id: &str) {
    create_channel(
        conn,
        &CreateChannelParams {
            server_id: 1,
            channel_id: channel_id.to_string(),
            name: channel_id.to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Aligned),
            retention_days: None,
            federation_scope: FederationScope::Local,
        },
    )
    .unwrap();
}

// ── Token helpers ────────────────────────────────────────────────────────

/// Signs a token the way `ws::tokens::generate_session_token` does, but with
/// the `expires` field supplied verbatim as a string.
///
/// Two things need this. An already-expired token cannot be produced by the
/// real minter — its TTL is a `u64` added to *now*, so the earliest expiry it
/// can express is the present second — and testing expiry by waiting would
/// mean a 60-second sleep in CI. And a correctly-signed token carrying a
/// non-numeric expiry can only come from a signer, which is the only way to
/// reach the `expires_str.parse()` guard that sits *after* HMAC verification.
fn sign_token(pseudonym: &str, expires_field: &str, secret: &[u8; 32]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let payload = format!("{pseudonym}|{expires_field}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let signature = mac.finalize().into_bytes();
    encode_token(&format!("{payload}|{}", hex::encode(signature)))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The decoded `pseudonym|expires|signature` body of a token, so tests can
/// tamper with one field and re-wrap.
fn decode_token(token: &str) -> String {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .expect("token should be url-safe base64");
    String::from_utf8(bytes).expect("token body should be utf-8")
}

fn encode_token(body: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body.as_bytes())
}

// ── Request helpers ──────────────────────────────────────────────────────

/// `POST /api/ws/token` as `caller`, authenticated the way an unenforced
/// server lets clients authenticate.
async fn post_ws_token(app: &axum::Router, caller: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().uri("/api/ws/token").method("POST");
    if let Some(caller) = caller {
        builder = builder.header("X-Annex-Pseudonym", caller);
    }
    let mut req = builder.body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9000".parse::<SocketAddr>().unwrap()));

    let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// Mints a token over HTTP and returns it, asserting the mint itself worked.
async fn mint_token_over_http(app: &axum::Router, caller: &str) -> String {
    let (status, body) = post_ws_token(app, Some(caller)).await;
    assert_eq!(status, StatusCode::OK, "mint failed: {body}");
    let json: Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"));
    json["token"]
        .as_str()
        .unwrap_or_else(|| panic!("no token in mint response: {body}"))
        .to_string()
}

/// Attempts a `/ws` upgrade with `query` appended verbatim.
///
/// Returns the live stream, or the HTTP status the server refused the
/// handshake with. The status matters: `ws_handler` distinguishes "I do not
/// believe you" (401) from "I believe you and the answer is still no" (403
/// for a deactivated identity), and collapsing both to `is_err()` — as the
/// pre-existing WebSocket tests do — would let those two swap places
/// unnoticed.
async fn upgrade(addr: SocketAddr, query: &str) -> Result<WsStream, StatusCode> {
    match connect_async(format!("ws://{addr}/ws?{query}")).await {
        Ok((stream, _)) => Ok(stream),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            Err(StatusCode::from_u16(resp.status().as_u16()).unwrap())
        }
        Err(e) => panic!("expected an HTTP refusal, got a transport error: {e}"),
    }
}

/// The refused status, for the many tests whose whole assertion is "no".
async fn upgrade_rejection(addr: SocketAddr, query: &str) -> StatusCode {
    match upgrade(addr, query).await {
        Ok(_) => panic!("the upgrade succeeded but should have been refused: ?{query}"),
        Err(status) => status,
    }
}

async fn send_json(ws: &mut WsStream, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .expect("failed to send frame");
}

/// The next frame, parsed. The timeout is a failure guard, not a wait: every
/// frame these tests expect is produced synchronously by the session's
/// dispatch loop, which handles one incoming frame fully before reading the
/// next.
async fn next_json(ws: &mut WsStream) -> Value {
    let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timed out waiting for a frame")
        .expect("socket closed before a frame arrived")
        .expect("frame error");
    match frame {
        Message::Text(text) => serde_json::from_str(&text).expect("frame was not JSON"),
        other => panic!("expected a text frame, got: {other:?}"),
    }
}

/// Subscribes to `channel`, sends `content`, and returns the frame that comes
/// back — either the broadcast of the stored message or an error frame.
///
/// This is how the tests observe *who the socket is*. The server stamps
/// `senderPseudonym` from the identity it resolved during the handshake, not
/// from anything the client says afterwards, so the value in this frame is
/// the authenticated binding.
async fn speak(ws: &mut WsStream, channel: &str, content: &str) -> Value {
    send_json(ws, json!({ "type": "subscribe", "channelId": channel })).await;
    send_json(
        ws,
        json!({
            "type": "message",
            "channelId": channel,
            "content": content,
            "replyTo": null,
        }),
    )
    .await;
    next_json(ws).await
}

// ── Minting: POST /api/ws/token ──────────────────────────────────────────

/// The mint endpoint is inside `protected_routes`, so it does have
/// `auth_middleware` in front of it. If it ever moved out — the way `/ws`
/// itself is mounted outside — anyone could mint a token for anyone, and
/// every check on the upgrade side would still pass, because the token would
/// be genuinely valid.
#[tokio::test]
async fn the_token_endpoint_refuses_an_unauthenticated_caller() {
    let app = setup(false).await;

    let (status, body) = post_ws_token(&app.router, None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an anonymous caller must not be able to mint a WS token: {body}"
    );
}

/// The minted token must name the *caller*, not an arbitrary pseudonym, and
/// must carry the short TTL. A mint that quietly issued a one-hour token
/// would widen the replay window twelvefold without changing any observable
/// behaviour until a leaked token was reused an hour later.
#[tokio::test]
async fn the_token_endpoint_mints_a_short_lived_token_bound_to_the_caller() {
    let app = setup(false).await;

    let (status, body) = post_ws_token(&app.router, Some("alice")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let json: Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"));
    assert_eq!(
        json["expires_in_secs"].as_u64(),
        Some(WS_TOKEN_TTL_SECS),
        "the advertised TTL drifted from WS_TOKEN_TTL_SECS: {body}"
    );

    let token = json["token"].as_str().expect("no token field");
    let decoded = decode_token(token);
    let mut fields = decoded.splitn(3, '|');
    assert_eq!(
        fields.next(),
        Some("alice"),
        "the token names someone other than the caller: {decoded}"
    );

    let expires: u64 = fields
        .next()
        .expect("no expires field")
        .parse()
        .expect("expires must be a unix timestamp");
    let now = now_unix();
    assert!(
        expires > now && expires <= now + WS_TOKEN_TTL_SECS,
        "expiry {expires} is not within {WS_TOKEN_TTL_SECS}s of now ({now})"
    );
}

// ── The happy path, and what it actually proves ──────────────────────────

/// The whole flow: mint over HTTP, spend over the WebSocket, and confirm the
/// session is bound to the minting pseudonym.
///
/// The binding assertion is the point. `ws_handler` could return 101 for a
/// token it never verified, or verify it and then bind whatever
/// `?pseudonym=` said; in both cases the handshake looks identical from the
/// client. Only the name the server puts on an outgoing message reveals
/// which identity the socket is actually operating as.
#[tokio::test]
async fn a_minted_token_upgrades_the_socket_and_binds_the_minting_pseudonym() {
    let app = setup(false).await;
    let token = mint_token_over_http(&app.router, "alice").await;

    let mut ws = upgrade(app.addr, &format!("token={token}"))
        .await
        .expect("a freshly minted token must upgrade");

    let frame = speak(&mut ws, "chan-alice", "minted-and-spent").await;
    assert_eq!(
        frame["type"], "message",
        "expected the message broadcast, got: {frame}"
    );
    assert_eq!(
        frame["senderPseudonym"], "alice",
        "the socket is not bound to the pseudonym the token was minted for: {frame}"
    );

    let conn = app.pool.get().unwrap();
    let stored: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE channel_id = 'chan-alice' \
             AND sender_pseudonym = 'alice'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored, 1,
        "the message was broadcast under alice but not persisted under alice"
    );
}

// ── Missing and malformed credentials ────────────────────────────────────

/// No credentials at all. This is the default state of anyone who points a
/// WebSocket client at the server, so it is the cheapest possible way in if
/// the `else` branch ever regresses to a permissive default.
#[tokio::test]
async fn an_upgrade_with_no_credentials_is_refused() {
    let app = setup(false).await;
    assert_eq!(
        upgrade_rejection(app.addr, "").await,
        StatusCode::UNAUTHORIZED,
    );
}

/// `?token=` with nothing after it. Serde deserializes this as `Some("")`,
/// not `None`, so it takes the token branch rather than the missing-
/// credentials branch — a different code path with its own way to go wrong.
/// An empty string base64-decodes successfully into an empty body, so the
/// only thing standing between it and a session is the field-count check.
#[tokio::test]
async fn an_empty_token_is_refused() {
    let app = setup(false).await;
    assert_eq!(
        upgrade_rejection(app.addr, "token=").await,
        StatusCode::UNAUTHORIZED,
    );
}

#[tokio::test]
async fn a_token_that_is_not_base64_is_refused() {
    let app = setup(false).await;
    assert_eq!(
        upgrade_rejection(app.addr, "token=not*valid*base64").await,
        StatusCode::UNAUTHORIZED,
    );
}

/// Well-formed base64 whose contents are nothing like a token. Decoding
/// succeeds, so this reaches the structural parse rather than bouncing off
/// the base64 layer.
#[tokio::test]
async fn a_base64_blob_that_is_not_a_token_is_refused() {
    let app = setup(false).await;
    let token = encode_token("just some text with no pipes at all");
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

/// Two fields instead of three. The parse uses `splitn(3, '|')`, which
/// happily returns fewer than three parts; without the explicit length check
/// this would index out of bounds or, worse, be silently accepted.
#[tokio::test]
async fn a_token_with_no_signature_field_is_refused() {
    let app = setup(false).await;
    let token = encode_token(&format!("alice|{}", now_unix() + 60));
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

/// A signature that is not hex at all. `hex::decode` fails before the
/// constant-time comparison ever runs, and that failure has to be fatal
/// rather than treated as "no signature to check".
#[tokio::test]
async fn a_token_with_a_non_hex_signature_is_refused() {
    let app = setup(false).await;
    let token = encode_token(&format!("alice|{}|zzzz", now_unix() + 60));
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

/// A signature of the right shape but the wrong length. `Mac::verify_slice`
/// is what rejects this; a hand-rolled `==` on a truncated slice would not.
#[tokio::test]
async fn a_token_with_a_truncated_signature_is_refused() {
    let app = setup(false).await;
    let real = generate_session_token("alice", &SERVER_SECRET, WS_TOKEN_TTL_SECS);
    let body = decode_token(&real);
    let (prefix, sig) = body.rsplit_once('|').expect("token has a signature field");
    let token = encode_token(&format!("{prefix}|{}", &sig[..8]));
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

// ── Forgery ──────────────────────────────────────────────────────────────

/// A token that is perfectly formed and correctly signed — with somebody
/// else's key. This is the federation-adjacent case: a token minted by a
/// different Annex server, or by anyone who read the format off this source
/// file, must be worthless here.
#[tokio::test]
async fn a_token_signed_with_a_different_secret_is_refused() {
    let app = setup(false).await;
    let token = generate_session_token("alice", &FOREIGN_SECRET, WS_TOKEN_TTL_SECS);
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
        "a token signed with a foreign key must not open a session"
    );
}

/// Rewriting the pseudonym in a genuine token. `alice` and `carol` are both
/// five characters and both real, active identities, so the forged token is
/// byte-for-byte the same length and shape as the original and names a user
/// the server would otherwise be happy to admit. The HMAC covers the
/// pseudonym, and this test fails the moment it stops doing so.
#[tokio::test]
async fn a_token_minted_for_alice_cannot_be_rewritten_to_name_carol() {
    let app = setup(false).await;
    let real = generate_session_token("alice", &SERVER_SECRET, WS_TOKEN_TTL_SECS);
    let forged = encode_token(&decode_token(&real).replacen("alice", "carol", 1));

    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={forged}")).await,
        StatusCode::UNAUTHORIZED,
        "the signature must cover the pseudonym"
    );
}

/// Extending a genuine token's own expiry. The HMAC covers `expires` as well
/// as the pseudonym; if it covered only the pseudonym, every token ever
/// issued would be permanent and the 60-second TTL would be a comment.
#[tokio::test]
async fn a_token_whose_expiry_was_extended_by_the_holder_is_refused() {
    let app = setup(false).await;
    let real = generate_session_token("alice", &SERVER_SECRET, WS_TOKEN_TTL_SECS);
    let body = decode_token(&real);
    let fields: Vec<&str> = body.splitn(3, '|').collect();
    let extended: u64 = fields[1].parse::<u64>().unwrap() + 86_400;
    let forged = encode_token(&format!("{}|{}|{}", fields[0], extended, fields[2]));

    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={forged}")).await,
        StatusCode::UNAUTHORIZED,
        "the signature must cover the expiry"
    );
}

// ── Expiry ───────────────────────────────────────────────────────────────

/// A token that was genuinely issued by this server and has simply run out.
/// The signature verifies; only the clock check stands in the way. Deleting
/// that check would leave every other test in this file passing.
#[tokio::test]
async fn an_expired_token_is_refused() {
    let app = setup(false).await;
    let token = sign_token("alice", &(now_unix() - 3600).to_string(), &SERVER_SECRET);
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
        "an expired token must not open a session"
    );
}

/// The `/ws` upgrade must NOT inherit the seven-day grace period that
/// `verify_token_allow_expired` grants `POST /api/session/refresh`. Those two
/// verifiers are near-identical copies sitting in the same file, and wiring
/// the lenient one into the upgrade would be a one-word edit that no other
/// test notices — turning a 60-second credential into a week-long one.
#[tokio::test]
async fn an_upgrade_does_not_get_the_session_refresh_grace_period() {
    let app = setup(false).await;
    // Two days past expiry: still inside the refresh endpoint's 7-day window,
    // long dead as far as the socket is concerned.
    let token = sign_token(
        "alice",
        &(now_unix() - 2 * 24 * 60 * 60).to_string(),
        &SERVER_SECRET,
    );
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

/// A correctly-signed token whose expiry field is not a number. Only a
/// signer can produce this, which is exactly why it is worth testing: the
/// expiry is parsed *after* the HMAC check, so this is the one input that
/// reaches the `parse()` guard with a valid signature behind it. A `parse()`
/// whose error was swallowed with `unwrap_or(u64::MAX)` would mint an
/// immortal token out of it.
#[tokio::test]
async fn a_signed_token_with_a_non_numeric_expiry_is_refused() {
    let app = setup(false).await;
    let token = sign_token("alice", "never", &SERVER_SECRET);
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

// ── Identity state behind the token ──────────────────────────────────────

/// A valid signature over a pseudonym that has no row in
/// `platform_identities`. The token proves the server signed *something*; it
/// does not prove the subject exists, so the DB lookup after verification is
/// load-bearing.
#[tokio::test]
async fn a_validly_signed_token_for_an_unknown_pseudonym_is_refused() {
    let app = setup(false).await;
    let token = generate_session_token("ghost", &SERVER_SECRET, WS_TOKEN_TTL_SECS);
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

/// Deactivating an identity has to take effect on the socket too. Tokens are
/// stateless — nothing revokes one — so `active = 0` is the only kill switch
/// there is, and it only works if the upgrade re-reads it. The status is 403
/// rather than 401 because the caller *is* who they say they are.
#[tokio::test]
async fn a_token_for_a_deactivated_identity_is_refused() {
    let app = setup(false).await;
    let token = generate_session_token("zombie", &SERVER_SECRET, WS_TOKEN_TTL_SECS);
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::FORBIDDEN,
        "a deactivated identity must not be able to spend a token minted \
         while it was still active"
    );
}

/// The same identity, deactivated between mint and spend. This is the real
/// shape of a ban: the token in the attacker's hand was legitimately issued
/// and has not expired.
#[tokio::test]
async fn a_token_minted_before_deactivation_stops_working_after_it() {
    let app = setup(false).await;
    let token = mint_token_over_http(&app.router, "alice").await;

    {
        let conn = app.pool.get().unwrap();
        conn.execute(
            "UPDATE platform_identities SET active = 0 WHERE pseudonym_id = 'alice'",
            [],
        )
        .unwrap();
    }

    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::FORBIDDEN,
        "deactivation must invalidate tokens that are already in the wild"
    );
}

// ── Parameter confusion ──────────────────────────────────────────────────

/// Both parameters at once. `token` must win, and win *silently* — the
/// dangerous outcome is not a rejection but a session that upgrades on
/// alice's signature and then operates as bob.
///
/// The assertion is deliberately on behaviour rather than on the handshake:
/// the socket tries to speak in bob's channel, and must be told it is not a
/// member, because it is alice.
#[tokio::test]
async fn a_token_for_alice_binds_alice_even_when_the_query_also_names_bob() {
    let app = setup(false).await;
    let token = mint_token_over_http(&app.router, "alice").await;

    let mut ws = upgrade(app.addr, &format!("token={token}&pseudonym=bob"))
        .await
        .expect("alice's token should still upgrade");

    let frame = speak(&mut ws, "chan-bob", "am i bob?").await;
    assert_eq!(
        frame["type"], "error",
        "the socket reached bob's channel while carrying alice's token: {frame}"
    );
    assert!(
        frame["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Not a member"),
        "expected a membership refusal, got: {frame}"
    );
}

/// The control for the test above. A "Not a member of chan-bob" refusal only
/// means "the socket is not bob" if bob himself would have been let in — if
/// the seed data ever stopped making bob a member, that test would keep
/// passing while proving nothing at all.
#[tokio::test]
async fn bobs_channel_is_reachable_when_the_socket_really_is_bob() {
    let app = setup(false).await;
    let token = mint_token_over_http(&app.router, "bob").await;

    let mut ws = upgrade(app.addr, &format!("token={token}"))
        .await
        .expect("bob's own token must upgrade");

    let frame = speak(&mut ws, "chan-bob", "i am bob").await;
    assert_eq!(
        frame["type"], "message",
        "bob cannot reach his own channel, so the refusal next door is \
         not evidence of anything: {frame}"
    );
    assert_eq!(frame["senderPseudonym"], "bob");
}

/// The mirror of the confusion test, from the other side: the same
/// doubly-parameterised socket is still fully alice, not a half-authenticated
/// stub. Without this,
/// `a_token_for_alice_binds_alice_even_when_the_query_also_names_bob` would
/// pass just as well against a handler that bound nobody at all.
#[tokio::test]
async fn a_token_for_alice_still_acts_as_alice_when_the_query_also_names_bob() {
    let app = setup(false).await;
    let token = mint_token_over_http(&app.router, "alice").await;

    let mut ws = upgrade(app.addr, &format!("token={token}&pseudonym=bob"))
        .await
        .expect("alice's token should still upgrade");

    let frame = speak(&mut ws, "chan-alice", "i am alice").await;
    assert_eq!(frame["type"], "message", "expected a broadcast: {frame}");
    assert_eq!(
        frame["senderPseudonym"], "alice",
        "the query parameter overrode the signed token: {frame}"
    );
}

/// A garbage token alongside a legitimate pseudonym must NOT fall through to
/// the legacy pseudonym branch. `ws_handler` returns early on a failed token
/// verification rather than trying the next credential, and that early return
/// is what stops `?token=junk&pseudonym=alice` from being a free pass.
#[tokio::test]
async fn a_bad_token_does_not_fall_back_to_the_legacy_pseudonym_parameter() {
    let app = setup(false).await;
    let forged = generate_session_token("alice", &FOREIGN_SECRET, WS_TOKEN_TTL_SECS);

    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={forged}&pseudonym=alice")).await,
        StatusCode::UNAUTHORIZED,
        "a failed token check must not degrade into unauthenticated pseudonym auth"
    );
}

// ── enforce_zk_proofs ────────────────────────────────────────────────────

/// With enforcement on, the legacy `?pseudonym=` parameter is the whole
/// vulnerability the token flow exists to close: it is a public string with
/// no cryptographic binding, so accepting it is accepting anyone.
#[tokio::test]
async fn a_raw_pseudonym_is_refused_when_zk_enforcement_is_on() {
    let app = setup(true).await;
    assert_eq!(
        upgrade_rejection(app.addr, "pseudonym=alice").await,
        StatusCode::UNAUTHORIZED,
        "enforce_zk_proofs must close the legacy pseudonym door"
    );
}

/// …and the token flow must keep working when it does, or enforcement locks
/// out every legitimate client and gets switched back off.
#[tokio::test]
async fn a_signed_token_still_upgrades_when_zk_enforcement_is_on() {
    let app = setup(true).await;
    // Minted directly: with enforcement on, `POST /api/ws/token` no longer
    // accepts the `X-Annex-Pseudonym` header, so the HTTP mint path is not
    // available to a test that has no ZK proof to trade in.
    let token = generate_session_token("alice", &SERVER_SECRET, WS_TOKEN_TTL_SECS);

    let mut ws = upgrade(app.addr, &format!("token={token}"))
        .await
        .expect("a signed token must still upgrade under enforcement");

    let frame = speak(&mut ws, "chan-alice", "enforced").await;
    assert_eq!(frame["type"], "message", "expected a broadcast: {frame}");
    assert_eq!(frame["senderPseudonym"], "alice");
}

/// Enforcement must not weaken the token check itself — the two are
/// independent, and a shortcut that trusted tokens more once enforcement was
/// on would invert the intent of the flag.
#[tokio::test]
async fn a_foreign_signed_token_is_still_refused_when_zk_enforcement_is_on() {
    let app = setup(true).await;
    let token = generate_session_token("alice", &FOREIGN_SECRET, WS_TOKEN_TTL_SECS);
    assert_eq!(
        upgrade_rejection(app.addr, &format!("token={token}")).await,
        StatusCode::UNAUTHORIZED,
    );
}

// ── Known gap: replay ────────────────────────────────────────────────────

/// Pins what the implementation actually does, which is not what its own doc
/// comment says.
///
/// `WS_TOKEN_TTL_SECS` is documented as "Tokens are single-use", but nothing
/// consumes a token: `verify_ws_token` checks an HMAC and a clock and keeps
/// no state, so a token can be spent as many times as its 60 seconds allow.
/// Anyone who observes one in a URL — a proxy log, a `Referer`, shell
/// history, a screen share — can open sessions as its subject until it
/// expires.
///
/// The sharp edge is `ConnectionManager::add_session`, which keeps at most
/// one session per pseudonym and *replaces* the old one. So a replayed token
/// does not merely give the attacker a socket: each replay silently evicts
/// the legitimate user's live connection from the broadcast registry. The
/// victim's socket stays open and stops receiving anything.
///
/// This is asserted rather than left silent because "you can reuse it" is the
/// live security property, and a reader who trusts the doc comment will
/// reason about a replay window that does not exist. The fix is not local:
/// `verify_ws_token` is shared with `verify_ws_token_for_auth`, which the
/// REST auth middleware calls on *every* request under `enforce_zk_proofs`
/// with a one-hour session token — burning a token there would log the user
/// out after a single API call. Making WS tokens single-use needs a
/// consumption store on the upgrade path only, plus a decision about what
/// reconnect storms do to it. Until then the behaviour is documented here.
///
/// If someone implements single-use, this test fails — correctly — and
/// should be replaced by its inverse and the doc comment left alone.
#[tokio::test]
async fn a_ws_token_is_replayable_within_its_ttl() {
    let app = setup(false).await;
    let token = mint_token_over_http(&app.router, "alice").await;

    let mut first = upgrade(app.addr, &format!("token={token}"))
        .await
        .expect("first use of the token must work");
    let frame = speak(&mut first, "chan-alice", "first use").await;
    assert_eq!(frame["senderPseudonym"], "alice", "frame: {frame}");

    let mut second = upgrade(app.addr, &format!("token={token}")).await.expect(
        "second use of the same token also succeeds — tokens are NOT \
             single-use despite the doc comment on WS_TOKEN_TTL_SECS",
    );
    let frame = speak(&mut second, "chan-alice", "second use").await;
    assert_eq!(
        frame["senderPseudonym"], "alice",
        "the replayed token opened a fully functional session: {frame}"
    );
}

/// The consequence of the replay above, made concrete: the replayed socket
/// takes over the original's place in the broadcast registry.
///
/// `first` sends a message and `second` is the socket it comes out of. That
/// can only happen because `add_session` overwrote the single entry keyed by
/// `alice`, so every broadcast addressed to alice — including the echo of
/// her own send — is now routed to whoever connected last. A leaked token is
/// therefore not just an eavesdropping risk; spending it silently detaches
/// the real user's client from the channels it is sitting in.
///
/// Asserted as a positive delivery rather than as `first` receiving nothing,
/// so there is no waiting-for-absence timeout to make the test flaky.
#[tokio::test]
async fn a_replayed_token_takes_over_the_original_sockets_delivery() {
    let app = setup(false).await;
    let token = mint_token_over_http(&app.router, "alice").await;

    let mut first = upgrade(app.addr, &format!("token={token}"))
        .await
        .expect("first use of the token must work");
    send_json(
        &mut first,
        json!({ "type": "subscribe", "channelId": "chan-alice" }),
    )
    .await;

    let mut second = upgrade(app.addr, &format!("token={token}"))
        .await
        .expect("the replay must succeed for this test to mean anything");
    send_json(
        &mut second,
        json!({ "type": "subscribe", "channelId": "chan-alice" }),
    )
    .await;

    send_json(
        &mut first,
        json!({
            "type": "message",
            "channelId": "chan-alice",
            "content": "sent by the original socket",
            "replyTo": null,
        }),
    )
    .await;

    let frame = next_json(&mut second).await;
    assert_eq!(
        frame["type"], "message",
        "expected the broadcast on the replayed socket, got: {frame}"
    );
    assert_eq!(
        frame["content"], "sent by the original socket",
        "the replayed socket did not inherit the original's delivery: {frame}"
    );
}
