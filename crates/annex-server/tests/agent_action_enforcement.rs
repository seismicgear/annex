//! Alignment has to still mean something after the join.
//!
//! `check_join_policy` reads `agent_registrations` and refuses a `Conflict`
//! agent, a `Partial` agent in a non-text channel, and an agent that misses the
//! channel's stated minimum — and then nothing looked again. So
//! `recalculate_agent_alignments`, whose entire purpose is to cut off an agent
//! whose principles no longer match the server's, did two things: it set
//! `agent_registrations.active = 0` and it closed the agent's WebSocket. It did
//! not touch `channel_members`, and it did not touch
//! `platform_identities.active`, so the agent's session token still verified,
//! it reconnected, and it kept sending, editing, deleting, speaking and
//! creating channels in every channel it had already joined. The sweep wrote
//! `AgentDisconnected` into the hash-chained audit log about an agent that had
//! not been disconnected in any durable sense.
//!
//! Every test here drives the real WebSocket or the real HTTP route. A unit
//! test on the gate would have passed against the defect, because the gate was
//! correct — it was simply never consulted.

use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_db::run_migrations;
use annex_identity::MerkleTree;
use annex_server::middleware::RateLimiter;
use annex_server::{api_ws, app, AppState};
use annex_types::{ChannelType, FederationScope, ServerPolicy};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const CHANNEL: &str = "chan-enforced";

struct Harness {
    addr: SocketAddr,
    pool: annex_db::DbPool,
    /// The same `AppState` the router holds, so a test can drive the conflict
    /// sweep directly. The sweep is the automatic path — it runs from
    /// `PUT /api/admin/policy` and from startup when the scorer changes — and
    /// reaching it over HTTP would mean minting an admin session for a test
    /// about something else entirely.
    state: Arc<AppState>,
    _db: tempfile::NamedTempFile,
}

/// One server, one channel, and whichever participants the caller asks for.
///
/// A file-backed database on purpose: `:memory:` is clamped to a single pooled
/// connection, which hides every interaction between the socket's connection
/// and the service's.
async fn harness(channel_type: ChannelType) -> Harness {
    let db = tempfile::NamedTempFile::new().unwrap();
    let pool = annex_db::create_pool(
        db.path().to_str().unwrap(),
        annex_db::DbRuntimeSettings::default(),
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
        create_channel(
            &conn,
            &CreateChannelParams {
                server_id: 1,
                channel_id: CHANNEL.to_string(),
                name: "Enforced".to_string(),
                channel_type,
                topic: None,
                vrp_topic_binding: None,
                required_capabilities_json: None,
                // Deliberately NOT set. The channel states no alignment
                // requirement, so nothing about these refusals comes from the
                // channel's own policy — they come from the agent's standing.
                agent_min_alignment: None,
                retention_days: None,
                federation_scope: FederationScope::Local,
            },
        )
        .unwrap();
    }

    let tree = {
        let conn = pool.get().unwrap();
        MerkleTree::restore(&conn, 20).unwrap()
    };
    let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(annex_identity::zk::generate_dummy_vkey()),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: Arc::new(signing_key),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::WebRtcConfig::new("http://localhost:7880", "devkey", "devsecret"),
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
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };

    let app = app(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    Harness {
        addr,
        pool,
        state: Arc::new(state),
        _db: db,
    }
}

impl Harness {
    /// A member of the channel. `can_voice` is set explicitly because this
    /// bypasses `create_platform_identity`, which always sets it; the column
    /// defaults to 0, so an omission would model a member whose voice had been
    /// revoked rather than an ordinary one.
    fn member(&self, pseudonym: &str, participant_type: &str) {
        let conn = self.pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities \
             (server_id, pseudonym_id, participant_type, active, can_voice) \
             VALUES (1, ?1, ?2, 1, 1)",
            rusqlite::params![pseudonym, participant_type],
        )
        .unwrap();
        add_member(&conn, 1, CHANNEL, pseudonym).unwrap();
    }

    /// An agent registration. `'{}'` is the stored contract in every fixture in
    /// this repo and it does NOT deserialize as a
    /// `VrpCapabilitySharingContract` — the gate has to carry it as an opaque
    /// string, so it is what the tests store.
    fn register_agent(&self, pseudonym: &str, alignment: &str, active: i64) {
        let conn = self.pool.get().unwrap();
        conn.execute(
            "INSERT INTO agent_registrations \
             (server_id, pseudonym_id, alignment_status, transfer_scope, \
              capability_contract_json, last_handshake_at, active) \
             VALUES (1, ?1, ?2, 'NO_TRANSFER', '{}', datetime('now'), ?3)",
            rusqlite::params![pseudonym, alignment, active],
        )
        .unwrap();
    }

    fn set_alignment(&self, pseudonym: &str, alignment: &str, active: i64) {
        let conn = self.pool.get().unwrap();
        let changed = conn
            .execute(
                "UPDATE agent_registrations SET alignment_status = ?2, active = ?3 \
                 WHERE server_id = 1 AND pseudonym_id = ?1",
                rusqlite::params![pseudonym, alignment, active],
            )
            .unwrap();
        assert_eq!(changed, 1, "fixture did not update {pseudonym}");
    }

    fn token_epoch(&self, pseudonym: &str) -> i64 {
        let conn = self.pool.get().unwrap();
        conn.query_row(
            "SELECT token_epoch FROM platform_identities WHERE pseudonym_id = ?1",
            [pseudonym],
            |r| r.get(0),
        )
        .unwrap()
    }

    async fn connect(&self, pseudonym: &str) -> WsClient {
        let url = format!("ws://{}/ws?pseudonym={pseudonym}", self.addr);
        let (stream, _) = connect_async(url).await.expect("ws connect");
        let mut client = WsClient { stream };
        client
            .send(json!({"type": "subscribe", "channelId": CHANNEL}))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        client
    }
}

struct WsClient {
    stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl WsClient {
    async fn send(&mut self, frame: Value) {
        self.stream
            .send(Message::Text(frame.to_string().into()))
            .await
            .expect("ws send");
    }

    /// The next frame, or a panic naming what was waited for.
    async fn recv(&mut self, what: &str) -> Value {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), self.stream.next())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
            .unwrap_or_else(|| panic!("socket closed while waiting for {what}"))
            .unwrap_or_else(|e| panic!("ws error while waiting for {what}: {e}"));
        match msg {
            Message::Text(t) => serde_json::from_str(&t).expect("json"),
            other => panic!("expected a text frame for {what}, got {other:?}"),
        }
    }
}

fn is_error(frame: &Value) -> bool {
    frame["type"] == "error"
}

/// An error frame is not enough. A refusal for the WRONG reason — a NotFound
/// from a mistyped id, a TTS failure, an edit-window expiry — looks identical
/// to the one under test, and a test that accepts any error passes against the
/// defect it was written for. Every refusal here has to name alignment.
fn assert_refused_for_alignment(frame: &Value, what: &str) {
    assert!(is_error(frame), "{what} was accepted: {frame}");
    let msg = message_of(frame).to_lowercase();
    assert!(
        msg.contains("conflict") || msg.contains("not active") || msg.contains("partially-aligned"),
        "{what} was refused, but not for the reason under test: {frame}",
    );
}

fn message_of(frame: &Value) -> String {
    frame["message"]
        .as_str()
        .or_else(|| frame["error"].as_str())
        .unwrap_or_default()
        .to_string()
}

// ── Send ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_conflict_agent_cannot_send_in_a_channel_it_already_joined() {
    let h = harness(ChannelType::Text).await;
    h.member("agent-conflict", "AI_AGENT");
    h.register_agent("agent-conflict", "CONFLICT", 0);

    let mut ws = h.connect("agent-conflict").await;
    ws.send(json!({
        "type": "message", "channelId": CHANNEL,
        "content": "still here", "replyTo": null
    }))
    .await;

    assert_refused_for_alignment(&ws.recv("the refusal").await, "the send");

    // And nothing was persisted. An error frame beside a stored row would be
    // the worse outcome of the two.
    let conn = h.pool.get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0, "the refused message was stored anyway");
}

#[tokio::test]
async fn a_verdict_reached_mid_session_applies_to_the_open_socket() {
    // The defect, exactly as it happened. The sweep closes the socket; a
    // reconnect is free, and `channel_members` still lists the agent. If the
    // gate is only at join time, the second send succeeds.
    let h = harness(ChannelType::Text).await;
    h.member("agent-drift", "AI_AGENT");
    h.register_agent("agent-drift", "ALIGNED", 1);

    let mut ws = h.connect("agent-drift").await;
    ws.send(json!({
        "type": "message", "channelId": CHANNEL,
        "content": "before the sweep", "replyTo": null
    }))
    .await;
    let accepted = ws.recv("the accepted message").await;
    assert_eq!(accepted["type"], "message", "{accepted}");

    // The sweep's verdict, written the way `recalculate_agent_alignments`
    // writes it.
    h.set_alignment("agent-drift", "CONFLICT", 0);

    ws.send(json!({
        "type": "message", "channelId": CHANNEL,
        "content": "after the sweep", "replyTo": null
    }))
    .await;
    let frame = ws.recv("the refusal").await;
    assert_refused_for_alignment(&frame, "the post-sweep send");

    let conn = h.pool.get().unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "only the pre-sweep message should exist");
}

#[tokio::test]
async fn a_conflict_agent_cannot_edit_or_delete_what_it_already_sent() {
    let h = harness(ChannelType::Text).await;
    h.member("agent-editor", "AI_AGENT");
    h.register_agent("agent-editor", "ALIGNED", 1);

    let mut ws = h.connect("agent-editor").await;
    ws.send(json!({
        "type": "message", "channelId": CHANNEL,
        "content": "original", "replyTo": null
    }))
    .await;
    let sent = ws.recv("the accepted message").await;
    let message_id = sent["messageId"]
        .as_str()
        .or_else(|| sent["id"].as_str())
        .unwrap_or_else(|| panic!("no message id in {sent}"))
        .to_string();

    h.set_alignment("agent-editor", "CONFLICT", 0);

    ws.send(json!({
        "type": "edit_message", "channelId": CHANNEL,
        "messageId": message_id, "content": "rewritten"
    }))
    .await;
    let edit = ws.recv("the edit refusal").await;
    assert_refused_for_alignment(&edit, "the edit");

    ws.send(json!({
        "type": "delete_message", "channelId": CHANNEL, "messageId": message_id
    }))
    .await;
    let del = ws.recv("the delete refusal").await;
    assert_refused_for_alignment(&del, "the delete");

    let conn = h.pool.get().unwrap();
    let (edited, deleted): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*) FROM message_edits, \
             (SELECT COUNT(*) AS d FROM messages WHERE deleted_at IS NOT NULL)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, 0));
    assert_eq!(edited, 0, "an edit was recorded for a refused request");
    assert_eq!(deleted, 0, "a delete was applied for a refused request");
}

#[tokio::test]
async fn an_inactive_registration_refuses_a_send_even_when_the_status_reads_aligned() {
    // `recalculate_agent_alignments` sets `active = 0` and leaves
    // `alignment_status` at whatever it computed, so this is a state the sweep
    // actually produces. Keying only on the status would miss it.
    let h = harness(ChannelType::Text).await;
    h.member("agent-inactive", "AI_AGENT");
    h.register_agent("agent-inactive", "ALIGNED", 0);

    let mut ws = h.connect("agent-inactive").await;
    ws.send(json!({
        "type": "message", "channelId": CHANNEL,
        "content": "hello", "replyTo": null
    }))
    .await;
    let frame = ws.recv("the refusal").await;
    assert_refused_for_alignment(&frame, "the send by an inactive registration");
}

// ── Who must NOT be affected ──────────────────────────────────────────────

#[tokio::test]
async fn a_partial_agent_keeps_text() {
    // ROADMAP 6.2: a partially-aligned agent is TEXT only. "Only" has to mean
    // it still HAS text, or the distinction between Partial and Conflict is
    // decorative.
    let h = harness(ChannelType::Text).await;
    h.member("agent-partial", "AI_AGENT");
    h.register_agent("agent-partial", "PARTIAL", 1);

    let mut ws = h.connect("agent-partial").await;
    ws.send(json!({
        "type": "message", "channelId": CHANNEL,
        "content": "partial but present", "replyTo": null
    }))
    .await;
    let frame = ws.recv("the accepted message").await;
    assert_eq!(
        frame["type"], "message",
        "a partial agent was refused: {frame}"
    );
}

#[tokio::test]
async fn a_human_member_is_untouched_by_the_agent_gate() {
    let h = harness(ChannelType::Text).await;
    h.member("human-1", "HUMAN");
    // No agent_registrations row, which is how every human looks.

    let mut ws = h.connect("human-1").await;
    ws.send(json!({
        "type": "message", "channelId": CHANNEL,
        "content": "hello", "replyTo": null
    }))
    .await;
    let frame = ws.recv("the accepted message").await;
    assert_eq!(frame["type"], "message", "a human was refused: {frame}");
}

#[tokio::test]
async fn an_agent_with_no_local_registration_is_untouched() {
    // A federated agent has no row in this server's `agent_registrations` —
    // only the local VRP handshake writes that table. Refusing on a missing row
    // would silently cut off every agent from every other server, which is a
    // different defect from the one being fixed. `channel_policy` documents the
    // same judgement at its own missing-row branch.
    let h = harness(ChannelType::Text).await;
    h.member("agent-remote", "AI_AGENT");

    let mut ws = h.connect("agent-remote").await;
    ws.send(json!({
        "type": "message", "channelId": CHANNEL,
        "content": "from elsewhere", "replyTo": null
    }))
    .await;
    let frame = ws.recv("the accepted message").await;
    assert_eq!(
        frame["type"], "message",
        "an unregistered (federated) agent was refused: {frame}",
    );
}

// ── Voice ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_partial_agent_may_not_join_voice() {
    let h = harness(ChannelType::Voice).await;
    h.member("agent-partial-voice", "AI_AGENT");
    h.register_agent("agent-partial-voice", "PARTIAL", 1);

    let client = reqwest::Client::new();
    let res = client
        .post(format!(
            "http://{}/api/channels/{CHANNEL}/voice/join",
            h.addr
        ))
        .header("X-Annex-Pseudonym", "agent-partial-voice")
        .send()
        .await
        .expect("request");
    assert_eq!(
        res.status(),
        403,
        "a partially-aligned agent joined voice: {}",
        res.text().await.unwrap_or_default(),
    );
}

#[tokio::test]
async fn an_aligned_agent_may_still_join_voice() {
    let h = harness(ChannelType::Voice).await;
    h.member("agent-voice-ok", "AI_AGENT");
    h.register_agent("agent-voice-ok", "ALIGNED", 1);

    let client = reqwest::Client::new();
    let res = client
        .post(format!(
            "http://{}/api/channels/{CHANNEL}/voice/join",
            h.addr
        ))
        .header("X-Annex-Pseudonym", "agent-voice-ok")
        .send()
        .await
        .expect("request");
    assert_eq!(
        res.status(),
        200,
        "an aligned agent was refused voice: {}",
        res.text().await.unwrap_or_default(),
    );
}

#[tokio::test]
async fn a_conflict_agent_cannot_speak_through_voice_intent() {
    // VoiceIntent bypasses `ChannelService` entirely, so the gate in
    // `ensure_voice_allowed` does not reach it. Without its own check, a
    // conflict-aligned agent keeps the server's voice speaking its words.
    let h = harness(ChannelType::Voice).await;
    h.member("agent-intent", "AI_AGENT");
    h.register_agent("agent-intent", "CONFLICT", 0);

    let mut ws = h.connect("agent-intent").await;
    ws.send(json!({
        "type": "voice_intent", "channelId": CHANNEL, "text": "say this"
    }))
    .await;
    let frame = ws.recv("the refusal").await;
    // Named rather than merely present: with the gate removed this test still
    // saw an error frame, because VoiceIntent also fails when TTS is not
    // configured — which it is not, in this harness. An assertion that accepts
    // any error accepts the defect.
    assert_refused_for_alignment(&frame, "the VoiceIntent");
}

// ── The verdict has to survive a reconnect ────────────────────────────────

#[tokio::test]
async fn the_conflict_sweep_revokes_the_agents_sessions() {
    // The action gate above stops an action. This stops the credential.
    //
    // `recalculate_agent_alignments` set `active = 0` and closed the socket,
    // and that was all: `platform_identities.active` was untouched, so
    // `auth_middleware` still accepted the agent's session token and a
    // reconnect was free. The revocation now happens in the SAME transaction
    // as the deactivation — doing it afterwards would leave a window in which
    // the row reads inactive and the token still verifies.
    let h = harness(ChannelType::Text).await;
    h.member("agent-swept", "AI_AGENT");
    h.register_agent("agent-swept", "ALIGNED", 1);
    let before = h.token_epoch("agent-swept");

    // The server states a boundary; the agent's stored anchor states a
    // different one. A prohibited-action divergence is an immediate Conflict
    // regardless of principle similarity, so this does not depend on which
    // scorer is loaded.
    {
        let mut policy = h.state.policy.write().unwrap();
        policy.principles = vec!["treat every participant as a peer".to_string()];
        policy.prohibited_actions = vec!["impersonating another participant".to_string()];
    }
    let agent_anchor = annex_vrp::VrpAnchorSnapshot::new(
        &["maximise engagement".to_string()],
        &["nothing whatsoever".to_string()],
    )
    .unwrap();
    {
        let conn = h.pool.get().unwrap();
        conn.execute(
            "UPDATE agent_registrations SET anchor_snapshot_json = ?1, \
             capability_contract_json = ?2 WHERE pseudonym_id = 'agent-swept'",
            rusqlite::params![
                serde_json::to_string(&agent_anchor).unwrap(),
                // A parseable contract: the sweep SKIPS a row whose contract it
                // cannot deserialize, and `'{}'` does not deserialize. Without
                // this the test would pass by the sweep doing nothing.
                serde_json::to_string(&annex_vrp::VrpCapabilitySharingContract {
                    required_capabilities: vec![],
                    offered_capabilities: vec!["TEXT".to_string()],
                    redacted_topics: vec![],
                })
                .unwrap(),
            ],
        )
        .unwrap();
    }

    annex_server::policy::recalculate_all_alignments(h.state.clone())
        .await
        .expect("the sweep should complete");

    let conn = h.pool.get().unwrap();
    let (alignment, active): (String, i64) = conn
        .query_row(
            "SELECT alignment_status, active FROM agent_registrations \
             WHERE pseudonym_id = 'agent-swept'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    drop(conn);
    assert_eq!(active, 0, "the sweep did not deactivate the agent");
    assert_eq!(alignment, "CONFLICT");
    assert!(
        h.token_epoch("agent-swept") > before,
        "the agent's sessions were not revoked, so its existing session token \
         still verifies and a reconnect restores everything the sweep took away",
    );
}
