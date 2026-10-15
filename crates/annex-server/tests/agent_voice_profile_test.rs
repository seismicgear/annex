use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::zk::{G1Affine, G2Affine, VerifyingKey};
use annex_server::{app, middleware, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;
mod common;

// These fixtures exercise assignment authorization and backend configuration
// validation. The inert executable/model are not speech synthesis evidence.
fn configured_test_speech(dir: &std::path::Path) -> Arc<annex_voice::TtsService> {
    std::fs::write(dir.join("test.onnx"), b"assignment configuration fixture").unwrap();
    std::fs::write(
        dir.join("test.onnx.json"),
        r#"{"audio":{"sample_rate":22050},"num_speakers":1}"#,
    )
    .unwrap();
    let executable = common::process_fixture::python_process(
        dir,
        "assignment-provider",
        "raise SystemExit(0)\n",
    );
    Arc::new(annex_voice::TtsService::new(
        dir,
        executable,
        dir.join("missing-bark"),
    ))
}

#[tokio::test]
async fn test_update_agent_voice_profile() {
    let speech_dir = tempfile::tempdir().unwrap();
    // 1. Setup App with Temp DB
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap();
    let pool = create_pool(db_path, DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert Server
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'default', 'Default Server', '{}')",
        [],
    ).unwrap();

    let vk = VerifyingKey {
        alpha_g1: G1Affine::default(),
        beta_g2: G2Affine::default(),
        gamma_g2: G2Affine::default(),
        delta_g2: G2Affine::default(),
        gamma_abc_g1: vec![],
    };

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(annex_identity::MerkleTree::new(20).unwrap())),
        membership_vkey: Arc::new(vk),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new(
            "http://localhost:3000".to_string(),
        )),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: middleware::RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::WebRtcConfig::default(),
        )),
        tts_service: configured_test_speech(speech_dir.path()),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
        voice_token_secret: std::sync::Arc::new([0u8; 32]),
        federation_transport: Default::default(),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };

    state
        .tts_service
        .add_profile(annex_types::VoiceProfile {
            id: "piper-test".into(),
            name: "Test Voice".into(),
            model: annex_types::VoiceModel::Piper,
            model_path: "test.onnx".into(),
            config_path: None,
            speed: 1.0,
            pitch: 1.0,
            speaker_id: None,
        })
        .await;
    state
        .tts_service
        .add_profile(annex_types::VoiceProfile {
            id: "default".into(),
            model: annex_types::VoiceModel::Piper,
            model_path: "test.onnx".into(),
            config_path: None,
            ..Default::default()
        })
        .await;
    let app = app(state);

    // 2. Setup Data
    let moderator = "mod_user";
    let normal_user = "norm_user";
    let agent_pseudonym = "agent_007";
    let voice_profile_id_str = "piper-test";

    // Create moderator identity
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
         VALUES (1, ?1, 'HUMAN', 1, 1)",
        rusqlite::params![moderator],
    ).unwrap();

    // Create normal user identity
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
         VALUES (1, ?1, 'HUMAN', 0, 1)",
        rusqlite::params![normal_user],
    ).unwrap();

    // Create agent registration
    conn.execute(
        "INSERT INTO agent_registrations (
            server_id, pseudonym_id, alignment_status, transfer_scope,
            capability_contract_json, reputation_score, last_handshake_at
        ) VALUES (1, ?1, 'ALIGNED', 'FULL_KNOWLEDGE_BUNDLE', '{}', 1.0, datetime('now'))",
        rusqlite::params![agent_pseudonym],
    )
    .unwrap();

    // Create voice profile
    conn.execute(
        "INSERT INTO voice_profiles (server_id, profile_id, name, model, model_path)
         VALUES (1, ?1, 'Test Voice', 'piper', 'test.onnx')",
        rusqlite::params![voice_profile_id_str],
    )
    .unwrap();
    let voice_profile_db_id: i64 = conn.last_insert_rowid();

    // 3. Test: Moderator assigns voice profile
    let uri = format!("/api/agents/{agent_pseudonym}/voice-profile");
    let payload = json!({ "voice_profile_id": voice_profile_id_str });

    let req = Request::builder()
        .uri(&uri)
        .method("PUT")
        .header("X-Annex-Pseudonym", moderator)
        .header("Content-Type", "application/json")
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))))
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify DB update
    let stored_id: Option<i64> = conn
        .query_row(
            "SELECT voice_profile_id FROM agent_registrations WHERE pseudonym_id = ?1",
            rusqlite::params![agent_pseudonym],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_id, Some(voice_profile_db_id));

    // 4. Test: Non-moderator tries to assign (Forbidden)
    let req_forbidden = Request::builder()
        .uri(&uri)
        .method("PUT")
        .header("X-Annex-Pseudonym", normal_user)
        .header("Content-Type", "application/json")
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))))
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp_forbidden = app.clone().oneshot(req_forbidden).await.unwrap();
    assert_eq!(resp_forbidden.status(), StatusCode::FORBIDDEN);

    // 5. Test: Moderator unsets voice profile
    let payload_unset = json!({ "voice_profile_id": null });
    let req_unset = Request::builder()
        .uri(&uri)
        .method("PUT")
        .header("X-Annex-Pseudonym", moderator)
        .header("Content-Type", "application/json")
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))))
        .body(Body::from(serde_json::to_vec(&payload_unset).unwrap()))
        .unwrap();

    let resp_unset = app.clone().oneshot(req_unset).await.unwrap();
    assert_eq!(resp_unset.status(), StatusCode::OK);

    // Verify DB update (should be NULL)
    let stored_id_null: Option<i64> = conn
        .query_row(
            "SELECT voice_profile_id FROM agent_registrations WHERE pseudonym_id = ?1",
            rusqlite::params![agent_pseudonym],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_id_null, None);

    // 6. Test: Invalid Profile ID (BadRequest)
    let payload_invalid = json!({ "voice_profile_id": "non_existent" });
    let req_invalid = Request::builder()
        .uri(&uri)
        .method("PUT")
        .header("X-Annex-Pseudonym", moderator)
        .header("Content-Type", "application/json")
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))))
        .body(Body::from(serde_json::to_vec(&payload_invalid).unwrap()))
        .unwrap();

    let resp_invalid = app.clone().oneshot(req_invalid).await.unwrap();
    assert_eq!(resp_invalid.status(), StatusCode::BAD_REQUEST);

    // 7. Test: Invalid Agent (NotFound)
    let uri_404 = "/api/agents/unknown_agent/voice-profile";
    let req_404 = Request::builder()
        .uri(uri_404)
        .method("PUT")
        .header("X-Annex-Pseudonym", moderator)
        .header("Content-Type", "application/json")
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))))
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp_404 = app.clone().oneshot(req_404).await.unwrap();
    assert_eq!(resp_404.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn captured_moderator_assignment_rechecks_epoch_role_and_loaded_configuration() {
    use annex_server::{
        api::ApiError,
        api_agent::{update_agent_voice_profile_handler, UpdateAgentVoiceProfileRequest},
    };
    use axum::{extract::Path, Extension, Json};
    let speech_dir = tempfile::tempdir().unwrap();
    let pool = create_pool(":memory:", Default::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'s','S','{}')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_moderate) VALUES(1,'mod','HUMAN',1)",[]).unwrap();
        conn.execute("INSERT INTO agent_registrations(server_id,pseudonym_id,alignment_status,transfer_scope,capability_contract_json,reputation_score,last_handshake_at) VALUES(1,'agent','ALIGNED','FULL_KNOWLEDGE_BUNDLE','{}',1,datetime('now'))",[]).unwrap();
        conn.execute("INSERT INTO voice_profiles(server_id,profile_id,name,model,model_path) VALUES(1,'voice','Voice','piper','test.onnx')",[]).unwrap();
    }
    let mut state = common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        ServerPolicy::default(),
    );
    state.tts_service = configured_test_speech(speech_dir.path());
    let state = Arc::new(state);
    let profile = annex_types::VoiceProfile {
        id: "voice".into(),
        name: "Voice".into(),
        model: annex_types::VoiceModel::Piper,
        model_path: "test.onnx".into(),
        config_path: None,
        speed: 1.0,
        pitch: 1.0,
        speaker_id: None,
    };
    state.tts_service.add_profile(profile).await;
    let captured =
        annex_identity::get_platform_identity(&state.pool.get().unwrap(), 1, "mod").unwrap();
    let assign = || {
        update_agent_voice_profile_handler(
            Extension(state.clone()),
            Extension(middleware::IdentityContext(captured.clone())),
            Path("agent".into()),
            Json(UpdateAgentVoiceProfileRequest {
                voice_profile_id: Some("voice".into()),
            }),
        )
    };
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET token_epoch=1 WHERE pseudonym_id='mod'",
            [],
        )
        .unwrap();
    assert!(matches!(assign().await, Err(ApiError::Unauthorized(_))));
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET token_epoch=0,can_moderate=0 WHERE pseudonym_id='mod'",
            [],
        )
        .unwrap();
    assert!(matches!(assign().await, Err(ApiError::Forbidden(_))));
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET can_moderate=1 WHERE pseudonym_id='mod'",
            [],
        )
        .unwrap();
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE voice_profiles SET model='unsupported' WHERE profile_id='voice'",
            [],
        )
        .unwrap();
    assert!(matches!(assign().await, Err(ApiError::BadRequest(_))));
    state.pool.get().unwrap().execute("UPDATE voice_profiles SET model='piper',model_path='changed-unloaded' WHERE profile_id='voice'",[]).unwrap();
    assert!(matches!(assign().await, Err(ApiError::BadRequest(_))));
    assert_eq!(
        state
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT voice_profile_id FROM agent_registrations WHERE pseudonym_id='agent'",
                [],
                |r| r.get::<_, Option<i64>>(0)
            )
            .unwrap(),
        None
    );
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE voice_profiles SET model_path='test.onnx' WHERE profile_id='voice'",
            [],
        )
        .unwrap();
    assert!(assign().await.is_ok());
    // A profile can remain in the runtime directory after its backing config
    // becomes invalid. A new assignment must refuse that state without writing.
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE agent_registrations SET voice_profile_id=NULL WHERE pseudonym_id='agent'",
            [],
        )
        .unwrap();
    std::fs::write(speech_dir.path().join("test.onnx.json"), b"not JSON").unwrap();
    assert!(matches!(assign().await, Err(ApiError::BadRequest(_))));
    assert_eq!(
        state
            .pool
            .get()
            .unwrap()
            .query_row(
                "SELECT voice_profile_id FROM agent_registrations WHERE pseudonym_id='agent'",
                [],
                |r| r.get::<_, Option<i64>>(0)
            )
            .unwrap(),
        None
    );
    let unloaded = update_agent_voice_profile_handler(
        Extension(state.clone()),
        Extension(middleware::IdentityContext(captured)),
        Path("agent".into()),
        Json(UpdateAgentVoiceProfileRequest {
            voice_profile_id: Some("unloaded".into()),
        }),
    )
    .await;
    assert!(matches!(unloaded, Err(ApiError::BadRequest(_))));
}
