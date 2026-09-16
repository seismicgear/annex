use crate::error::VoiceError;
use crate::service::VoiceService;
use crate::stt::SttService;
use crate::tts::encode_pcm_to_opus_frames;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

const DEFAULT_TRANSCRIPTION_BROADCAST_CAPACITY: usize = 256;

/// The STT tap carries 48 kHz mono signed-16-bit PCM, so one second of
/// one speaker is this many bytes.
const STT_BYTES_PER_SECOND: usize = 48_000 * 2;

/// How much audio to accumulate before transcribing.
///
/// The tap emits one frame per RTP packet — 20 ms, 1920 bytes — and this
/// loop used to call `transcribe()` on each one. That is a whisper.cpp
/// process fork, model load and inference **fifty times a second per
/// speaker**, over 20 ms of audio, which is below the length at which
/// whisper produces anything at all. Even with a correct WAV container it
/// could not have transcribed speech; it would have burned a core per
/// speaker to return empty strings.
///
/// Two seconds is the trade: long enough for whisper to have words to
/// work with, short enough that a caption is still current when it lands.
const STT_WINDOW_BYTES: usize = 2 * STT_BYTES_PER_SECOND;

/// Flush a partial window after this much quiet, so the last thing a
/// speaker says before they stop is not held until they speak again.
const STT_IDLE_FLUSH: Duration = Duration::from_millis(700);

/// Never transcribe a window shorter than this. Below ~200 ms whisper
/// returns either nothing or a hallucinated fragment, and a hallucinated
/// caption is worse than no caption.
const STT_MIN_WINDOW_BYTES: usize = STT_BYTES_PER_SECOND / 5;

/// Transcriptions allowed to run at once across all speakers in a room.
///
/// The recv loop must keep draining the tap broadcast while whisper
/// works; if it blocks, the 1024-deep channel overflows and every
/// speaker's audio is lost, not just the one being transcribed. So each
/// window is handed to its own task, and this bounds how many of those
/// can exist. When both permits are taken the window is dropped rather
/// than queued: a caption that arrives after the conversation has moved
/// on has no value, and an unbounded queue under sustained speech is how
/// a server runs out of memory.
const STT_MAX_INFLIGHT: usize = 2;

/// Whisper's transcript for one window, or `None` if it did not contain
/// speech.
///
/// Whisper never returns an empty string for a silent window. It returns
/// an annotation — `[BLANK_AUDIO]`, `(silence)`, `[ Pause ]`, `[MUSIC]` —
/// and those annotations are always a single bracketed or parenthesised
/// token. Passing one through renders it in the caption strip as though
/// somebody had said it: a failure shown as an ordinary result, which is
/// this codebase's most common defect class. A transcript that merely
/// CONTAINS an annotation ("[MUSIC] and then we shipped it") is speech
/// and is kept whole; deciding which part to strip is guesswork, and the
/// annotation in that position is information rather than noise.
fn speech_or_none(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    // Remove every bracketed group, then ask whether any words are left.
    // "[BLANK_AUDIO]" and "[BLANK_AUDIO] [BLANK_AUDIO]" both reduce to
    // whitespace; "[MUSIC] and then we shipped it" does not.
    let mut depth = 0i32;
    // Asterisks are a toggle, not a nesting level: whisper writes
    // `*coughs*`, never `**`.
    let mut starred = false;
    let mut outside = String::new();
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            '*' => starred = !starred,
            _ if depth == 0 && !starred => outside.push(c),
            _ => {}
        }
    }
    if !outside.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(text.to_string())
}

/// Hand one window of a speaker's audio to whisper, off the recv loop.
#[allow(clippy::too_many_arguments)]
fn spawn_transcription(
    stt: Arc<SttService>,
    tx: broadcast::Sender<TranscriptionEvent>,
    inflight: Arc<Semaphore>,
    warned: Arc<AtomicBool>,
    channel_id: String,
    speaker: String,
    pcm: Vec<u8>,
) {
    if pcm.len() < STT_MIN_WINDOW_BYTES {
        return;
    }
    // Readiness is two `stat` calls and is checked per window rather than
    // once at connect, so a server that gains a model while a call is
    // running starts captioning without a reconnect.
    let readiness = stt.readiness();
    if !readiness.is_ready() {
        if !warned.swap(true, Ordering::Relaxed) {
            warn!(
                channel = %channel_id,
                detail = %readiness.detail(),
                "voice transcription is unavailable; captions will not appear for this call",
            );
        }
        return;
    }
    let permit = match Arc::clone(&inflight).try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            debug!(
                channel = %channel_id,
                speaker = %speaker,
                "stt is saturated; dropping a transcription window",
            );
            return;
        }
    };
    tokio::spawn(async move {
        let _permit = permit;
        match stt.transcribe(&pcm).await {
            Ok(text) => {
                let Some(text) = speech_or_none(&text) else {
                    return;
                };
                let _ = tx.send(TranscriptionEvent {
                    channel_id,
                    speaker_pseudonym: speaker,
                    text,
                });
            }
            Err(e) => {
                // The first failure of a call is reported at warn. It was
                // debug, which meant a subsystem that could not work in
                // any container ever built from this repo's Dockerfile
                // said so only under `RUST_LOG=debug`.
                if !warned.swap(true, Ordering::Relaxed) {
                    warn!(channel = %channel_id, error = %e, "stt transcription failed");
                } else {
                    debug!(channel = %channel_id, error = %e, "stt transcription failed");
                }
            }
        }
    });
}

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
    /// Connect to the in-process SFU room as a voice agent.
    ///
    /// `token` MUST be an HMAC-signed voice-join token (see
    /// [`crate::token`]). Verification happens here before any room is
    /// created: an expired token, a tampered token, or a token issued
    /// for a different room is rejected. The token MUST be bound to
    /// `room_name`. `voice_token_secret` is the HMAC key returned by
    /// [`crate::token::derive_voice_token_secret`].
    ///
    /// This is the defensive gate that prevents a process holding only
    /// an old base64 blob from joining the SFU after the membership
    /// check has been bypassed.
    #[allow(clippy::too_many_arguments)] // signal-carrying call site; refactoring to a builder hides the binding contract
    pub async fn connect(
        _url: &str,
        token: &str,
        room_name: &str,
        voice_token_secret: &[u8; 32],
        stt_service: Arc<SttService>,
        _api_key: &str,
        _api_secret: &str,
        voice_service: Arc<VoiceService>,
    ) -> Result<Self, VoiceError> {
        let (tx, _) = broadcast::channel(DEFAULT_TRANSCRIPTION_BROADCAST_CAPACITY);

        let claims =
            crate::token::verify_join_token(token, voice_token_secret, Some(room_name), None)
                .map_err(|e| VoiceError::RoomService(format!("invalid voice join token: {e}")))?;
        let agent_id = claims.sub;

        voice_service.create_room(room_name).await?;

        let mut tap_rx = voice_service.subscribe_stt_taps();
        let tx_clone = tx.clone();
        let room = room_name.to_string();
        let stt = Arc::clone(&stt_service);
        // Differentiate `Lagged` from `Closed` so a brief burst of STT
        // tap frames that overflows the 1024-deep broadcast window does
        // NOT terminate this transcription task permanently. See [F36].
        let transcription_task = tokio::spawn(async move {
            // One accumulating window per speaker. Two people talking
            // over each other must not have their audio concatenated
            // into one transcript.
            let mut windows: HashMap<String, Vec<u8>> = HashMap::new();
            let inflight = Arc::new(Semaphore::new(STT_MAX_INFLIGHT));
            let warned = Arc::new(AtomicBool::new(false));

            loop {
                // The timeout is the idle flush: a speaker who stops
                // mid-window has their tail transcribed instead of held
                // until they speak again.
                let received = match tokio::time::timeout(STT_IDLE_FLUSH, tap_rx.recv()).await {
                    Ok(r) => r,
                    Err(_elapsed) => {
                        for (speaker, pcm) in windows.drain() {
                            spawn_transcription(
                                Arc::clone(&stt),
                                tx_clone.clone(),
                                Arc::clone(&inflight),
                                Arc::clone(&warned),
                                room.clone(),
                                speaker,
                                pcm,
                            );
                        }
                        continue;
                    }
                };

                let frame = match received {
                    Ok(f) => f,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!(
                            skipped = n,
                            "stt tap broadcast lagged; some frames skipped for agent transcription",
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        for (speaker, pcm) in windows.drain() {
                            spawn_transcription(
                                Arc::clone(&stt),
                                tx_clone.clone(),
                                Arc::clone(&inflight),
                                Arc::clone(&warned),
                                room.clone(),
                                speaker,
                                pcm,
                            );
                        }
                        break;
                    }
                };
                if frame.channel_id != room {
                    continue;
                }

                let window = windows.entry(frame.speaker_pseudonym.clone()).or_default();
                window.extend_from_slice(&frame.pcm_s16le);
                if window.len() >= STT_WINDOW_BYTES {
                    let pcm = std::mem::take(window);
                    windows.remove(&frame.speaker_pseudonym);
                    spawn_transcription(
                        Arc::clone(&stt),
                        tx_clone.clone(),
                        Arc::clone(&inflight),
                        Arc::clone(&warned),
                        room.clone(),
                        frame.speaker_pseudonym,
                        pcm,
                    );
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
        let secret = [0xABu8; 32];
        let token = voice_service
            .generate_join_token("ch-abort-test", "agent-1", "agent-1", &secret, 60)
            .expect("generate_join_token should succeed");

        let agent = AgentVoiceClient::connect(
            "ws://test",
            &token,
            "ch-abort-test",
            &secret,
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
    async fn connect_rejects_token_for_wrong_room() {
        // Defence in depth: an attacker who somehow obtains a valid
        // token for channel A must NOT be able to use it to enter
        // channel B even if the membership check upstream is bypassed.
        let voice_service = test_voice_service();
        let stt = test_stt_service();
        let secret = [0xEFu8; 32];
        let token = voice_service
            .generate_join_token("ch-allowed", "agent-3", "agent-3", &secret, 60)
            .expect("token should sign cleanly");

        let err = AgentVoiceClient::connect(
            "ws://test",
            &token,
            "ch-different",
            &secret,
            stt,
            "test-key",
            "test-secret",
            voice_service,
        )
        .await
        .expect_err("token bound to a different room must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("WrongRoom") || msg.contains("does not match expected"),
            "expected wrong-room error, got {msg:?}"
        );
    }

    #[tokio::test]
    async fn connect_rejects_unsigned_legacy_blob() {
        // The pre-token format was a base64 JSON blob. Verify that a
        // process holding only that legacy blob cannot connect.
        let voice_service = test_voice_service();
        let stt = test_stt_service();
        let secret = [0x77u8; 32];

        use base64::Engine;
        let legacy_blob = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"room":"ch-legacy","sub":"agent-x"}"#);

        let err = AgentVoiceClient::connect(
            "ws://test",
            &legacy_blob,
            "ch-legacy",
            &secret,
            stt,
            "test-key",
            "test-secret",
            voice_service,
        )
        .await
        .expect_err("legacy unsigned token must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("invalid voice join token") || msg.contains("Tampered"),
            "expected tampered/malformed error, got {msg:?}"
        );
    }

    #[tokio::test]
    async fn drop_does_not_panic_when_task_already_finished() {
        // Defensive test: if the task somehow exits naturally before
        // Drop runs, the second `abort()` from Drop must be a no-op.
        let voice_service = test_voice_service();
        let stt = test_stt_service();
        let secret = [0xCDu8; 32];
        let token = voice_service
            .generate_join_token("ch-drop-test", "agent-2", "agent-2", &secret, 60)
            .expect("generate_join_token should succeed");

        let agent = AgentVoiceClient::connect(
            "ws://test",
            &token,
            "ch-drop-test",
            &secret,
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

/// The windowing in `connect`'s transcription task, driven through the
/// real STT tap.
///
/// These tests exist because the loop they cover called `transcribe()`
/// once per 20 ms RTP packet. Every layer read correctly in isolation —
/// the tap emits frames, the task consumes them, `transcribe` spawns
/// whisper — and the composition could not work: whisper on 20 ms of
/// audio returns nothing, fifty times a second, per speaker.
#[cfg(test)]
mod window_tests {
    use super::*;
    use crate::config::WebRtcConfig;
    use crate::service::VoiceService;
    use crate::SttTapFrame;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// 20 ms of 48 kHz mono s16-le — one RTP packet's worth, exactly what
    /// `tap_for_stt` produces.
    fn tap_frame(room: &str, speaker: &str) -> SttTapFrame {
        SttTapFrame {
            channel_id: room.to_string(),
            speaker_pseudonym: speaker.to_string(),
            pcm_s16le: (0..960)
                .flat_map(|i| (((i % 200) as i16 - 100) * 90).to_le_bytes())
                .collect(),
        }
    }

    fn service() -> Arc<VoiceService> {
        Arc::new(VoiceService::new(WebRtcConfig {
            url: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            public_url: String::new(),
            token_ttl_seconds: 3600,
            ice_servers: vec![],
        }))
    }

    /// A whisper stand-in that records one line per invocation and prints
    /// the number of audio frames it was handed.
    fn counting_whisper(dir: &Path) -> (PathBuf, PathBuf) {
        let model = dir.join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let log = dir.join("invocations.log");
        let bin = dir.join("whisper");
        std::fs::write(
            &bin,
            format!(
                r#"#!/usr/bin/env python3
import sys, struct
data = sys.stdin.buffer.read()
frames = struct.unpack("<I", data[40:44])[0] // 2 if len(data) >= 44 else 0
with open({log:?}, "a") as fh:
    fh.write("%d\n" % frames)
sys.stdout.write("heard %d frames" % frames)
"#,
                log = log.to_string_lossy(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (model, log)
    }

    async fn connected_agent(
        room: &str,
        voice: Arc<VoiceService>,
        stt: Arc<SttService>,
    ) -> AgentVoiceClient {
        let secret = [0x5Au8; 32];
        let token = voice
            .generate_join_token(room, "agent-win", "agent-win", &secret, 60)
            .unwrap();
        AgentVoiceClient::connect(
            "ws://test",
            &token,
            room,
            &secret,
            stt,
            "k",
            "s",
            Arc::clone(&voice),
        )
        .await
        .unwrap()
    }

    fn invocations(log: &Path) -> Vec<u32> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }

    #[tokio::test]
    async fn two_seconds_of_speech_is_one_transcription_not_a_hundred() {
        let dir = tempfile::tempdir().unwrap();
        let (model, log) = counting_whisper(dir.path());
        let bin = dir.path().join("whisper");
        let voice = service();
        let agent = connected_agent(
            "room-window",
            Arc::clone(&voice),
            Arc::new(SttService::new(&model, &bin)),
        )
        .await;
        let mut rx = agent.subscribe_transcriptions();

        // Exactly one window: 100 packets x 20 ms.
        for _ in 0..100 {
            voice.emit_stt_tap_for_test(tap_frame("room-window", "speaker-a"));
        }

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a transcription should arrive within 5s")
            .expect("broadcast should deliver");
        assert_eq!(ev.speaker_pseudonym, "speaker-a");
        assert_eq!(ev.channel_id, "room-window");

        // 2 s at 48 kHz downsampled to 16 kHz is 32000 frames. The old
        // loop would have spawned 100 processes over 320 frames each.
        let calls = invocations(&log);
        assert_eq!(
            calls.len(),
            1,
            "one 2 s window must be one whisper invocation, got {calls:?}",
        );
        assert_eq!(calls[0], 32_000, "the window must carry all 2 s of audio");
    }

    #[tokio::test]
    async fn a_speaker_who_stops_mid_window_still_gets_their_tail() {
        let dir = tempfile::tempdir().unwrap();
        let (model, log) = counting_whisper(dir.path());
        let bin = dir.path().join("whisper");
        let voice = service();
        let agent = connected_agent(
            "room-tail",
            Arc::clone(&voice),
            Arc::new(SttService::new(&model, &bin)),
        )
        .await;
        let mut rx = agent.subscribe_transcriptions();

        // 500 ms — a quarter of a window, then silence.
        for _ in 0..25 {
            voice.emit_stt_tap_for_test(tap_frame("room-tail", "speaker-b"));
        }

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("the idle flush should transcribe the tail")
            .expect("broadcast should deliver");
        assert_eq!(ev.speaker_pseudonym, "speaker-b");
        assert_eq!(invocations(&log), vec![8_000], "500 ms at 16 kHz");
    }

    #[tokio::test]
    async fn two_speakers_are_not_concatenated_into_one_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let (model, _log) = counting_whisper(dir.path());
        let bin = dir.path().join("whisper");
        let voice = service();
        let agent = connected_agent(
            "room-two",
            Arc::clone(&voice),
            Arc::new(SttService::new(&model, &bin)),
        )
        .await;
        let mut rx = agent.subscribe_transcriptions();

        // Interleaved, as overlapping speech actually arrives.
        for _ in 0..30 {
            voice.emit_stt_tap_for_test(tap_frame("room-two", "speaker-c"));
            voice.emit_stt_tap_for_test(tap_frame("room-two", "speaker-d"));
        }

        let mut seen = std::collections::HashSet::new();
        for _ in 0..2 {
            let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("both speakers should be transcribed")
                .expect("broadcast should deliver");
            // 600 ms each, not 1200 ms of the two mixed together.
            assert_eq!(
                ev.text, "heard 9600 frames",
                "{} got merged audio",
                ev.speaker_pseudonym
            );
            seen.insert(ev.speaker_pseudonym);
        }
        assert_eq!(seen.len(), 2, "one event per speaker, got {seen:?}");
    }

    #[tokio::test]
    async fn frames_below_the_minimum_window_are_not_transcribed() {
        // 100 ms is under STT_MIN_WINDOW_BYTES. whisper on that returns
        // nothing or a hallucination; spending a process on it is worse
        // than staying quiet.
        let dir = tempfile::tempdir().unwrap();
        let (model, log) = counting_whisper(dir.path());
        let bin = dir.path().join("whisper");
        let voice = service();
        let agent = connected_agent(
            "room-short",
            Arc::clone(&voice),
            Arc::new(SttService::new(&model, &bin)),
        )
        .await;
        let mut rx = agent.subscribe_transcriptions();

        for _ in 0..5 {
            voice.emit_stt_tap_for_test(tap_frame("room-short", "speaker-e"));
        }

        assert!(
            tokio::time::timeout(Duration::from_millis(1_500), rx.recv())
                .await
                .is_err(),
            "a 100 ms fragment must not produce a caption",
        );
        assert!(invocations(&log).is_empty(), "no process should be spawned");
    }

    #[tokio::test]
    async fn a_window_of_silence_produces_no_caption() {
        // whisper writes `[BLANK_AUDIO]` for a window with no speech in
        // it. Rendering that in the caption strip is a failure shown as
        // an ordinary result.
        let dir = tempfile::tempdir().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, b"ggml").unwrap();
        let bin = dir.path().join("whisper");
        std::fs::write(&bin, "#!/bin/sh\ncat >/dev/null\nprintf '[BLANK_AUDIO]'\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        let voice = service();
        let agent = connected_agent(
            "room-silent",
            Arc::clone(&voice),
            Arc::new(SttService::new(&model, &bin)),
        )
        .await;
        let mut rx = agent.subscribe_transcriptions();

        for _ in 0..100 {
            voice.emit_stt_tap_for_test(tap_frame("room-silent", "speaker-f"));
        }

        assert!(
            tokio::time::timeout(Duration::from_millis(2_000), rx.recv())
                .await
                .is_err(),
            "[BLANK_AUDIO] must not reach the caption strip",
        );
    }
}

#[cfg(test)]
mod speech_filter_tests {
    use super::speech_or_none;

    #[test]
    fn whisper_silence_annotations_are_not_speech() {
        for annotation in [
            "[BLANK_AUDIO]",
            "  [BLANK_AUDIO]  ",
            "(silence)",
            "[ Pause ]",
            "[MUSIC]",
            "*coughs*",
            "",
            "   ",
            ".",
            "...",
            " - ",
        ] {
            assert_eq!(
                speech_or_none(annotation),
                None,
                "{annotation:?} must not reach the caption strip",
            );
        }
    }

    #[test]
    fn real_speech_survives_including_speech_that_contains_an_annotation() {
        assert_eq!(
            speech_or_none("  the deploy is going out at four  "),
            Some("the deploy is going out at four".to_string()),
        );
        assert_eq!(
            speech_or_none("[MUSIC] and then we shipped it"),
            Some("[MUSIC] and then we shipped it".to_string()),
        );
        // Two annotations in a row is still not speech.
        assert_eq!(speech_or_none("[BLANK_AUDIO] [BLANK_AUDIO]"), None);
        // A sentence that merely ends in a bracket is speech.
        assert_eq!(
            speech_or_none("we shipped it (finally)"),
            Some("we shipped it (finally)".to_string()),
        );
    }
}
