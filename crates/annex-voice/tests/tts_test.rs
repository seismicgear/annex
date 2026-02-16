use annex_types::voice::{VoiceModel, VoiceProfile};
use annex_voice::{TtsService, VoiceError};
use std::path::PathBuf;

#[tokio::test]
async fn test_tts_service_instantiation() {
    let voices_dir = PathBuf::from("assets/voices");
    let piper_path = PathBuf::from("piper"); // Assume in PATH or mock

    let service = TtsService::new(voices_dir, piper_path);

    // Add a profile
    let profile = VoiceProfile {
        id: "test-voice".to_string(),
        name: "Test Voice".to_string(),
        model: VoiceModel::Piper,
        model_path: "test.onnx".to_string(),
        config_path: None,
        speed: 1.0,
        pitch: 1.0,
        speaker_id: None,
    };

    service.add_profile(profile.clone()).await;

    let retrieved = service.get_profile("test-voice").await;
    assert_eq!(retrieved, Some(profile));
}

#[tokio::test]
async fn test_tts_missing_profile() {
    let service = TtsService::new("assets/voices", "piper");

    let result = service.synthesize("Hello", "non-existent").await;
    match result {
        Err(VoiceError::ProfileNotFound(id)) => assert_eq!(id, "non-existent"),
        _ => panic!("Expected ProfileNotFound error, got {:?}", result),
    }
}

#[tokio::test]
async fn test_tts_missing_model_file() {
    // This test ensures that if the model file is missing, it returns the correct error
    let temp_dir = tempfile::tempdir().unwrap();
    let voices_dir = temp_dir.path().to_path_buf();

    let service = TtsService::new(&voices_dir, "piper");

    let profile = VoiceProfile {
        id: "missing-model".to_string(),
        name: "Missing Model".to_string(),
        model: VoiceModel::Piper,
        model_path: "missing.onnx".to_string(),
        config_path: None,
        speed: 1.0,
        pitch: 1.0,
        speaker_id: None,
    };

    service.add_profile(profile).await;

    let result = service.synthesize("Hello", "missing-model").await;
    match result {
        Err(VoiceError::Tts(msg)) => assert!(msg.contains("Model file not found")),
        _ => panic!("Expected Tts error about missing model, got {:?}", result),
    }
}

#[tokio::test]
async fn test_tts_invalid_speed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let voices_dir = temp_dir.path().to_path_buf();

    // Create a dummy model file to pass the existence check
    let model_path = voices_dir.join("test.onnx");
    std::fs::File::create(&model_path).unwrap();

    let service = TtsService::new(&voices_dir, "piper");

    let profile = VoiceProfile {
        id: "invalid-speed".to_string(),
        name: "Invalid Speed".to_string(),
        model: VoiceModel::Piper,
        model_path: "test.onnx".to_string(),
        config_path: None,
        speed: 0.0, // Invalid speed
        pitch: 1.0,
        speaker_id: None,
    };

    service.add_profile(profile).await;

    let result = service.synthesize("Hello", "invalid-speed").await;
    match result {
        Err(VoiceError::Config(msg)) => assert_eq!(msg, "Speed must be positive"),
        _ => panic!("Expected Config error about speed, got {:?}", result),
    }
}
