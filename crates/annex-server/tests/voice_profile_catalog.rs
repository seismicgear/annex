//! Real HTTP authorization/readiness boundaries with controlled provider
//! processes. These tests do not claim generated speech or model inference.
mod common;
use annex_server::{
    api::ApiError, api_voice_profiles::list_voice_profiles_handler, middleware::IdentityContext,
    AppState,
};
use annex_types::{VoiceModel, VoiceProfile};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    Extension,
};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

async fn fixture(dir: &std::path::Path) -> Arc<AppState> {
    let pool = annex_db::create_pool(":memory:", Default::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute("INSERT INTO servers(id,slug,label,policy_json) VALUES(1,'catalog','Catalog','{}'),(2,'other','Other','{}')",[]).unwrap();
        conn.execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,can_moderate) VALUES(1,'operator','HUMAN',1),(1,'member','HUMAN',0)",[]).unwrap();
        conn.execute("INSERT INTO voice_profiles(server_id,profile_id,name,model,model_path) VALUES(1,'configured','Configured','piper','secret-model.onnx'),(1,'unloaded','Unloaded','piper','secret-missing.onnx'),(2,'foreign','Foreign','piper','private-foreign.onnx')",[]).unwrap();
    }
    let provider =
        common::process_fixture::python_process(dir, "catalog-provider", "raise SystemExit(0)\n");
    std::fs::write(dir.join("secret-model.onnx"), b"configuration fixture").unwrap();
    std::fs::write(
        dir.join("secret-model.onnx.json"),
        br#"{"audio":{"sample_rate":22050},"num_speakers":1}"#,
    )
    .unwrap();
    let mut state = common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        Default::default(),
    );
    state.tts_service = Arc::new(annex_voice::TtsService::new(
        dir,
        &provider,
        dir.join("no-bark"),
    ));
    let profile = VoiceProfile {
        id: "configured".into(),
        name: "Configured".into(),
        model_path: "secret-model.onnx".into(),
        config_path: None,
        ..Default::default()
    };
    state.tts_service.add_profile(profile.clone()).await;
    state
        .tts_service
        .add_profile(VoiceProfile {
            id: "default".into(),
            name: "Default".into(),
            ..profile
        })
        .await;
    Arc::new(state)
}
async fn catalog(state: &Arc<AppState>, who: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().uri("/api/voice-profiles");
    if let Some(who) = who {
        request = request.header("X-Annex-Pseudonym", who);
    }
    annex_server::app((**state).clone())
        .oneshot(
            request
                .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                    [127, 0, 0, 1],
                    1234,
                ))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn operator_catalog_lists_validated_local_profiles_default_and_unavailable_rows_without_paths(
) {
    let dir = tempfile::tempdir().unwrap();
    let state = fixture(dir.path()).await;
    let response = catalog(&state, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!raw.contains("secret-") && !raw.contains("private-") && !raw.contains("foreign"));
    let result: Value = serde_json::from_str(&raw).unwrap();
    let rows = result["profiles"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        assert_eq!(
            row.as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "available",
                "id",
                "model",
                "name",
                "pitch",
                "speaker_id",
                "speed"
            ]
        );
        assert_eq!(row["available"], row["id"] != "unloaded");
    }
    std::fs::remove_file(dir.path().join("secret-model.onnx.json")).unwrap();
    let response = catalog(&state, Some("operator")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(result["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["available"] == false));
}
#[tokio::test]
async fn catalog_refuses_nonoperator_and_missing_session() {
    let dir = tempfile::tempdir().unwrap();
    let state = fixture(dir.path()).await;
    assert_eq!(
        catalog(&state, None).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        catalog(&state, Some("member")).await.status(),
        StatusCode::FORBIDDEN
    );
}
#[tokio::test]
async fn operator_revoked_during_actual_provider_validation_cannot_receive_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = (*fixture(dir.path()).await).clone();
    let provider = common::process_fixture::python_process(
        dir.path(),
        "blocked-provider",
        r#"import pathlib,time
p=pathlib.Path(__file__).parent
(p/'entered').write_text('ready')
deadline=time.monotonic()+10
while not (p/'released').exists():
    if time.monotonic()>deadline: raise SystemExit(1)
    time.sleep(.01)
"#,
    );
    state.tts_service = Arc::new(annex_voice::TtsService::new(
        dir.path(),
        dir.path().join("missing-piper"),
        provider,
    ));
    let profile = VoiceProfile {
        id: "blocked".into(),
        name: "Blocked".into(),
        model: VoiceModel::Bark,
        model_path: String::new(),
        config_path: None,
        ..Default::default()
    };
    state.tts_service.add_profile(profile).await;
    state
        .pool
        .get()
        .unwrap()
        .execute("DELETE FROM voice_profiles WHERE server_id=1", [])
        .unwrap();
    state.pool.get().unwrap().execute("INSERT INTO voice_profiles(server_id,profile_id,name,model,model_path) VALUES(1,'blocked','Blocked','bark','')",[]).unwrap();
    let captured =
        annex_identity::get_platform_identity(&state.pool.get().unwrap(), 1, "operator").unwrap();
    let state = Arc::new(state);
    let task = tokio::spawn(list_voice_profiles_handler(
        Extension(state.clone()),
        Extension(IdentityContext(captured)),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !dir.path().join("entered").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET token_epoch=1 WHERE pseudonym_id='operator'",
            [],
        )
        .unwrap();
    std::fs::write(dir.path().join("released"), b"release").unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(ApiError::Unauthorized(_))
    ));
}
