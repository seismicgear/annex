//! HTTP read-model tests. These controlled DB/runtime fixtures do not claim
//! synthesis, ZK admission, or real browser speech acceptance.
mod common;

use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_types::{
    voice::{VoiceModel, VoiceProfile},
    ServerPolicy,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

async fn fixture(
    assigned: Option<(&str, &str)>,
    loaded: Option<(&str, VoiceModel)>,
) -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'test','Test','{}')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,active) VALUES(1,'reader','HUMAN',1)", []).unwrap();
        let contract =
            serde_json::to_string(&annex_vrp::VrpCapabilitySharingContract::default()).unwrap();
        conn.execute("INSERT INTO agent_registrations(server_id,pseudonym_id,alignment_status,transfer_scope,capability_contract_json,reputation_score,last_handshake_at) VALUES(1,'agent','ALIGNED','REFLECTION_SUMMARIES_ONLY',?1,1,datetime('now'))", [contract]).unwrap();
        if let Some((id, stored_model)) = assigned {
            conn.execute("INSERT INTO voice_profiles(server_id,profile_id,name,model,model_path) VALUES(1,?1,'Test profile',?2,'private-model-path')", [id,stored_model]).unwrap();
            conn.execute("UPDATE agent_registrations SET voice_profile_id=?1 WHERE server_id=1 AND pseudonym_id='agent'", [conn.last_insert_rowid()]).unwrap();
        }
    }
    let state = common::build_app_state(
        pool.clone(),
        MerkleTree::new(20).unwrap(),
        ServerPolicy::default(),
    );
    if let Some((id, model)) = loaded {
        state
            .tts_service
            .add_profile(VoiceProfile {
                id: id.into(),
                model,
                ..VoiceProfile::default()
            })
            .await;
    }
    (annex_server::app(state), pool)
}

async fn read(app: axum::Router) -> Value {
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/agents/agent")
                .header("X-Annex-Pseudonym", "reader")
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    1234,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("private-model-path"));
    value
}

#[tokio::test]
async fn assigned_profile_reports_loaded_backend_without_exposing_paths() {
    for (id, model, expected) in [
        ("bark-live", VoiceModel::Bark, "bark"),
        ("system-live", VoiceModel::System, "system"),
    ] {
        let (app, _) = fixture(Some((id, "piper")), Some((id, model))).await;
        let value = read(app).await;
        assert_eq!(value["voice_profile_id"], id);
        // Loaded execution state is authoritative; a stale DB model label
        // must not falsely identify the runtime used for this profile.
        assert_eq!(value["voice_backend"], expected);
    }
}

#[tokio::test]
async fn configured_but_unloaded_profile_does_not_claim_backend_or_default_fallback() {
    let (app, _) = fixture(
        Some(("unloaded", "bark")),
        Some(("default", VoiceModel::System)),
    )
    .await;
    let value = read(app).await;
    assert_eq!(value["voice_profile_id"], "unloaded");
    assert!(value["voice_backend"].is_null());
}

#[tokio::test]
async fn unassigned_profile_reports_actual_default_and_missing_default_is_explicit() {
    let (app, _) = fixture(None, Some(("default", VoiceModel::System))).await;
    let value = read(app).await;
    assert!(value["voice_profile_id"].is_null());
    assert_eq!(value["voice_backend"], "system");
    let (app, _) = fixture(None, None).await;
    let value = read(app).await;
    assert!(value["voice_profile_id"].is_null());
    assert!(value["voice_backend"].is_null());
}

#[tokio::test]
async fn foreign_explicit_assignment_is_unavailable_and_never_reports_default() {
    let (app, pool) = fixture(None, Some(("default", VoiceModel::System))).await;
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO servers(id,slug,label,policy_json) VALUES(2,'other','Other','{}')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO voice_profiles(server_id,profile_id,name,model,model_path) VALUES(2,'foreign-private-profile','Other voice','bark','foreign-private-path')", []).unwrap();
        conn.execute("UPDATE agent_registrations SET voice_profile_id=?1 WHERE server_id=1 AND pseudonym_id='agent'", [conn.last_insert_rowid()]).unwrap();
    }
    let value = read(app).await;
    assert!(value["voice_profile_id"].is_null());
    assert!(value["voice_backend"].is_null());
    assert!(!value.to_string().contains("foreign-private"));
}
