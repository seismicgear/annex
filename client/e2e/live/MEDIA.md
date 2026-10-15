# Native media acceptance

Run this lane alone against a fresh Annex database with native voice enabled.
It creates two independent participants and a nonparticipant through normal UI registration. The
first must be the server founder. It creates and ultimately deletes a Hybrid
channel. Existing session/audit state cannot substitute for this prerequisite.

From `client`:

```powershell
$env:ANNEX_LIVE_URL = 'http://127.0.0.1:3000'
$env:ANNEX_MEDIA_AUDIO_WAV = 'C:\absolute\path\annex-tts.wav'
npx playwright test --config e2e/live/media.playwright.config.ts
```

The default **controlled** lane uses an actual speech WAV as Chromium's
microphone input and Chromium's generated camera. The WAV should contain real
speech, for example Piper's generated “Annex live session speech verification.
The channel carries this spoken message.” The expected Annex flow remains real:
UI → browser capture → RTP → native Rust SFU → independent browser receiver →
decoder → playback elements. No application API, capture method, signaling,
STT, TTS, database, or application state is stubbed or patched.

Screen capture uses `getDisplayMedia` on a dedicated animated browser tab, with
Chromium's capture picker selecting that exact title. It must report
`displaySurface: browser`; desktop capture is not accepted. Camera and screen
must arrive with distinct, sender-attributed track IDs and increasing decoded
frame counters. The test records native `getStats()` samples, received codec,
decoded video element dimensions, audio energy, and capture track lifetimes.
Mute, deafen, both video-off actions, leave, rejoin, and deletion must change the
actual capture/remote receiver state, with the independent direction preserved
where appropriate. Closed peers and ended sender tracks are recorded.

For an explicitly attended physical camera/microphone run:

```powershell
$env:ANNEX_MEDIA_SOURCE = 'physical'
npx playwright test --config e2e/live/media.playwright.config.ts
```

That mode removes both fake device/file flags. Supply audible speech during the
audio assertions and keep the camera available. This remains a single-machine
two-browser-context run. Neither mode alone proves physical speaker audibility,
multi-host NAT/TURN traversal, mobile/native packaging, or agent speech. The
controlled result must never be described as physical hardware acceptance.

Caption verification is required. A server that cannot deliver actual speech
captions to both human participants fails the test. To use a different spoken
phrase, override:

```powershell
$env:ANNEX_EXPECTED_CAPTION = 'Annex Live Session Speech Verification'
```

The phrase is a case-insensitive regular expression against text produced by
the actual server pipeline in both browsers, aggregated across speech windows
with the other participant's attribution. Missing captions fail the run; a
registered nonparticipant receiving any room transcript also fails. The source
snapshot initially only created STT subscriptions for agent voice sessions and
forwarded each transcript only to that agent, so human-only caption delivery
must be repaired before that stricter condition can pass.

Reports are separate for physical and controlled modes under
`client/e2e-results/media-<mode>/`. Evidence contains counters and public
attribution only; no token, proof, private identity material, SDP, ICE address,
trace, or full-session recording is exported.
