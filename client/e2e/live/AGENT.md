# Real model agent session

This lane requires a fresh isolated server, native voice, functioning Piper
TTS and Whisper STT, and an installed real Ollama model. It launches the actual
`scripts/live/agent.mjs` runtime, which registers the founder and two independent
AI agents and performs real alignment handshakes. Run alone from `client`:

```powershell
$env:ANNEX_LIVE_URL = 'http://127.0.0.1:3000'
$env:ANNEX_AGENT_MODEL_URL = 'http://127.0.0.1:11434'
$env:ANNEX_AGENT_MODEL = 'gpt-oss:20b'
npx playwright test --config e2e/live/agent.playwright.config.ts
```

The browser independently registers an ordinary human, joins the runtime's
Hybrid channel, and publishes the real Piper waveform through Chromium's
controlled microphone. Both agent sockets must receive Whisper transcripts
attributed to that human. Each agent receives a fresh nonce arithmetic prompt;
the actual installed model must generate the nonce and `26`, publish the answer
with its own identity and reply relationship, then speak it through Piper.
Native receiver counters must show packets, bytes, decoded audio samples and
energy advancing on distinct `audio-<agent pseudonym>` Opus tracks, connected to
playing remote audio elements. Agents enter the voice room through their first
voice intent; only then does the lane require human transcripts, since text
subscription alone must not expose a room's captions.

Moderator removal of `can_voice` interrupts an actively measured agent stream,
removes that agent from the room and stops its caption delivery while the other
agent continues hearing and speaking. The retained socket must still deliver
fresh model text, but its new voice intent must receive the exact capability
refusal. A capability grant must restore generated voice on that same socket.
The exact deliberate refusal and cancellation are preserved in the evidence;
other runtime errors fail acceptance.

The lane interrupts a currently flowing agent stream, proves clean disconnect,
keeps the other agent operational, reconnects with actual reauthentication and
proves another fresh model/speech round trip. It then revokes a speaking agent;
the runtime must confirm old credentials receive 401 and its socket closes,
while the browser proves removal from the room and stopped media. The remaining
agent must continue responding and speaking. Browser call leave closes native
peer connections and removes playback elements.

No model, credential, alignment, signaling, API, TTS or STT result is fabricated.
Microphone input is controlled speech, so this does not certify physical audio
devices or audibility through speakers. Evidence contains public identities,
fresh prompt/response/model receipts, and measured native receiver samples;
credentials and secret proofs are not exported. Runtime termination and browser
cleanup occur even after test failure. Results go to `client/e2e-results/agent/`.
