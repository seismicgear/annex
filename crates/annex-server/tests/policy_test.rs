use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{
    zk::{G1Affine, G2Affine, VerifyingKey},
    MerkleTree,
};
use annex_server::{
    api_ws::ConnectionManager, middleware::RateLimiter, policy::recalculate_agent_alignments,
    AppState,
};
use annex_types::ServerPolicy;
use annex_vrp::{VrpAnchorSnapshot, VrpCapabilitySharingContract};
use std::sync::{Arc, Mutex, RwLock};
use tempfile::NamedTempFile;
use tokio::sync::broadcast;

fn load_dummy_vkey() -> Arc<VerifyingKey<annex_identity::zk::Bn254>> {
    let g1 = G1Affine::default();
    let g2 = G2Affine::default();

    let vk = VerifyingKey {
        alpha_g1: g1,
        beta_g2: g2,
        gamma_g2: g2,
        delta_g2: g2,
        gamma_abc_g1: vec![g1; 1],
    };

    Arc::new(vk)
}

#[tokio::test]
async fn test_recalculate_agent_alignments() {
    // Use a temporary file instead of :memory: to ensure connections share the DB
    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();

    // 1. Setup AppState
    let pool = create_pool(&db_path, DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Create server
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test', 'Test', '{}')",
        [],
    )
    .unwrap();

    let policy = ServerPolicy::default();
    let policy_lock = Arc::new(RwLock::new(policy.clone()));

    let state = Arc::new(AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(MerkleTree::new(20).unwrap())),
        membership_vkey: load_dummy_vkey(),
        signing_key: Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0u8; 32])),
        public_url: "http://localhost:3000".to_string(),
        server_id: 1,
        policy: policy_lock.clone(),
        rate_limiter: RateLimiter::new(),
        connection_manager: ConnectionManager::new(),
        presence_tx: broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
    });

    // 2. Register an Agent (Aligned)
    let anchor = VrpAnchorSnapshot::new(&[], &[]);
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string(), "VRP".to_string()],
    };

    let anchor_json = serde_json::to_string(&anchor).unwrap();
    let contract_json = serde_json::to_string(&contract).unwrap();

    conn.execute(
        "INSERT INTO agent_registrations (
            server_id, pseudonym_id, alignment_status, transfer_scope,
            capability_contract_json, anchor_snapshot_json, reputation_score, last_handshake_at
        ) VALUES (1, 'agent-aligned', 'ALIGNED', 'REFLECTION_SUMMARIES_ONLY', ?1, ?2, 0.0, datetime('now'))",
        [&contract_json, &anchor_json],
    ).unwrap();

    let active: bool = conn
        .query_row(
            "SELECT active FROM agent_registrations WHERE pseudonym_id = 'agent-aligned'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(active);

    // Drop connection to return to pool (though r2d2 handles it, good practice)
    drop(conn);

    // 3. Change Server Policy to Conflict
    {
        let mut p = policy_lock.write().unwrap();
        p.principles.push("We value privacy".to_string());
    }

    // 4. Trigger Recalculation
    recalculate_agent_alignments(state.clone()).await.unwrap();

    // 5. Verify Agent is now Conflict and Inactive
    let conn = pool.get().unwrap();
    let (status, active): (String, bool) = conn.query_row(
        "SELECT alignment_status, active FROM agent_registrations WHERE pseudonym_id = 'agent-aligned'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?))
    ).unwrap();

    assert_eq!(status, "CONFLICT");
    assert!(!active);
}
