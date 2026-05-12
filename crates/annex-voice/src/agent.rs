use crate::error::VoiceError;
use crate::service::VoiceService;
use crate::stt::SttService;
use crate::tts::encode_pcm_to_opus_frames;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
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
    /// Handle to the spawned STT-tap → transcription forwarding task.
    /// Aborted on `Drop` so the task does not outlive the agent.
    ///
    /// Without this, every agent join/leave cycle leaked one tokio
    /// task that:
    ///
    /// * held an `Arc<SttService>` (preventing it from dropping),
    /// * consumed every STT tap frame (CPU + decode work for nothing),
    /// * and called `stt.transcribe(...)` on frames matching the
    ///   dead room name (every frame after the room was reaped no
    ///   longer matched, but pre-reap frames did).
    ///
    /// The task was kept alive by the global
    /// `VoiceService::stt_tap_tx` broadcast sender, which only drops
    /// at server shutdown. `Drop` aborting the JoinHandle is the
    /// minimal fix.
    transcription_task: JoinHandle<()>,
}

impl Drop for AgentVoiceClient {
    fn drop(&mut self) {
        // tokio's abort is fire-and-forget; the task observes
        // cancellation at its next `.await` point (the
        // `tap_rx.recv().await` inside the loop body).
        self.transcription_task.abort();
    }
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
        // Differentiate `Lagged` from `Closed` so a brief burst of STT
        // tap frames that overflows the 1024-deep broadcast window does
        // NOT terminate this transcription task permanently. See [F36].
        let transcription_task = tokio::spawn(async move {
            loop {
                let frame = match tap_rx.recv().await {
                    Ok(f) => f,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!(
                            skipped = n,
                            "stt tap broadcast lagged; some frames skipped for agent transcription",
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
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
            transcription_task,
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

    /// Test-only accessor for the spawned transcription task's abort
    /// handle. Used by the regression test that asserts `Drop` aborts
    /// the task; call sites outside `#[cfg(test)]` do not need this.
    #[cfg(test)]
    pub(crate) fn transcription_abort_handle(&self) -> tokio::task::AbortHandle {
        self.transcription_task.abort_handle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebRtcConfig;
    use std::time::Duration;

    fn test_voice_service() -> Arc<VoiceService> {
        Arc::new(VoiceService::new(WebRtcConfig {
            url: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            public_url: String::new(),
            token_ttl_seconds: 3600,
            ice_servers: vec![],
        }))
    }

    fn test_stt_service() -> Arc<SttService> {
        // SttService is a thin wrapper over two paths; constructing
        // one with phony paths is fine for tests that never call
        // `transcribe()`.
        Arc::new(SttService::new(
            "/tmp/nonexistent-model",
            "/tmp/nonexistent-binary",
        ))
    }

    #[tokio::test]
    async fn drop_aborts_transcription_task() {
        // Regression test for the spawn-and-forget transcription task
        // leak: every agent join used to spawn a task that lived for
        // the lifetime of the global `VoiceService::stt_tap_tx`
        // sender (i.e. the entire server lifetime). The fix stores
        // the JoinHandle on `AgentVoiceClient` and aborts it on Drop.
        let voice_service = test_voice_service();
        let stt = test_stt_service();
        let token = voice_service
            .generate_join_token("ch-abort-test", "agent-1", "agent-1")
            .expect("generate_join_token should succeed");

        let agent = AgentVoiceClient::connect(
            "ws://test",
            &token,
            "ch-abort-test",
            stt,
            "test-key",
            "test-secret",
            voice_service,
        )
        .await
        .expect("agent connect should succeed");

        let abort_handle = agent.transcription_abort_handle();
        assert!(
            !abort_handle.is_finished(),
            "task should be running before drop"
        );

        drop(agent);

        // Give the task time to observe the abort. The actual
        // observation happens at the next `.await` point inside the
        // loop, which is `tap_rx.recv().await`; tokio reaps the task
        // promptly after abort.
        let mut iters = 0;
        while !abort_handle.is_finished() && iters < 50 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            iters += 1;
        }
        assert!(
            abort_handle.is_finished(),
            "transcription task should be aborted within 500ms of agent drop"
        );
    }

    #[tokio::test]
    async fn drop_does_not_panic_when_task_already_finished() {
        // Defensive test: if the task somehow exits naturally before
        // Drop runs, the second `abort()` from Drop must be a no-op.
        let voice_service = test_voice_service();
        let stt = test_stt_service();
        let token = voice_service
            .generate_join_token("ch-drop-test", "agent-2", "agent-2")
            .expect("generate_join_token should succeed");

        let agent = AgentVoiceClient::connect(
            "ws://test",
            &token,
            "ch-drop-test",
            stt,
            "test-key",
            "test-secret",
            voice_service,
        )
        .await
        .expect("agent connect should succeed");

        // Pre-abort the task, then drop the agent. Drop's abort()
        // call must be a tokio-side no-op.
        agent.transcription_abort_handle().abort();
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Should not panic.
        drop(agent);
    }
}
