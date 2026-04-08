use crate::error::VoiceError;
use crate::service::VoiceService;
use crate::stt::SttService;
use crate::tts::encode_pcm_to_opus_frames;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info};

const DEFAULT_TRANSCRIPTION_BROADCAST_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub struct TranscriptionEvent {
    pub channel_id: String,
    pub speaker_pseudonym: String,
    pub text: String,
}

#[derive(Debug)]
pub struct AgentVoiceClient {
    pub room_name: String,
    pub connected: bool,
    pub stt_service: Arc<SttService>,
    pub transcription_tx: broadcast::Sender<TranscriptionEvent>,
    voice_service: Arc<VoiceService>,
    agent_id: String,
}

impl AgentVoiceClient {
    pub async fn connect(
        _url: &str,
        token: &str,
        room_name: &str,
        stt_service: Arc<SttService>,
        _api_key: &str,
        _api_secret: &str,
        voice_service: Arc<VoiceService>,
    ) -> Result<Self, VoiceError> {
        let (tx, _) = broadcast::channel(DEFAULT_TRANSCRIPTION_BROADCAST_CAPACITY);

        // Token payload is URL-safe base64 JSON from VoiceService::generate_join_token.
        use base64::Engine;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|e| VoiceError::RoomService(format!("invalid join token: {e}")))?;
        let claims: serde_json::Value = serde_json::from_slice(&decoded)
            .map_err(|e| VoiceError::RoomService(format!("invalid join token JSON: {e}")))?;
        let agent_id = claims
            .get("sub")
            .and_then(|s| s.as_str())
            .unwrap_or("agent")
            .to_string();

        voice_service.create_room(room_name).await?;

        let mut tap_rx = voice_service.subscribe_stt_taps();
        let tx_clone = tx.clone();
        let room = room_name.to_string();
        let stt = Arc::clone(&stt_service);
        tokio::spawn(async move {
            while let Ok(frame) = tap_rx.recv().await {
                if frame.channel_id != room {
                    continue;
                }
                match stt.transcribe(&frame.pcm_s16le).await {
                    Ok(text) => {
                        let _ = tx_clone.send(TranscriptionEvent {
                            channel_id: frame.channel_id,
                            speaker_pseudonym: frame.speaker_pseudonym,
                            text,
                        });
                    }
                    Err(e) => debug!(error = %e, "stt transcription failed"),
                }
            }
        });

        info!(room = %room_name, "agent connected to native SFU room");

        Ok(Self {
            room_name: room_name.to_string(),
            connected: true,
            stt_service,
            transcription_tx: tx,
            voice_service,
            agent_id,
        })
    }

    pub async fn publish_audio(&self, pcm_data: &[u8]) -> Result<(), VoiceError> {
        if !self.connected {
            return Err(VoiceError::RoomService(
                "Agent is not connected to a room".to_string(),
            ));
        }

        let opus_frames = encode_pcm_to_opus_frames(pcm_data, 16_000, 1)?;
        for frame in opus_frames {
            self.voice_service
                .inject_agent_opus(
                    &self.room_name,
                    &self.agent_id,
                    &frame,
                    std::time::Duration::from_millis(20),
                )
                .await?;
        }

        Ok(())
    }

    pub async fn disconnect(&mut self) {
        self.connected = false;
    }

    pub async fn process_incoming_audio(
        &self,
        audio: &[u8],
        speaker: &str,
    ) -> Result<(), VoiceError> {
        let text = self.stt_service.transcribe(audio).await?;
        let _ = self.transcription_tx.send(TranscriptionEvent {
            channel_id: self.room_name.clone(),
            speaker_pseudonym: speaker.to_string(),
            text,
        });
        Ok(())
    }

    pub async fn simulate_hearing(&self, audio: &[u8], speaker: &str) -> Result<(), VoiceError> {
        self.process_incoming_audio(audio, speaker).await
    }

    pub fn subscribe_transcriptions(&self) -> broadcast::Receiver<TranscriptionEvent> {
        self.transcription_tx.subscribe()
    }
}
