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

<!-- BEGIN KI-OWNER-EXECUTION-AND-SPENDING-DIRECTIVE -->
## Owner directive: execution continuity and spending controls

This owner directive applies to every agent working in this repository. It governs these two subjects wherever older repository instructions conflict; all other existing requirements remain in effect.

### GitHub Actions is never an excuse to stop or skip verification

- GitHub Actions outages, disabled workflows, unavailable runners, queued jobs, billing restrictions, exhausted quotas, or missing hosted CI results are never an excuse to stop otherwise authorized work or skip verification.
- Run the relevant tests, builds, linting, and other checks on the available local development machine or in an existing authorized environment. Use free tooling or capacity already paid for. Continue useful independent work while resolving any specific blocker.
- Record the commands actually run, their results, and the precise checks that remain blocked. Never claim that an unrun or failed check passed, or that missing hosted CI results prove completion.
- Do not buy runners, enable paid CI, increase budgets, or create another paid requirement to restore verification. This directive does not authorize falsifying checks or bypassing access controls.

### No spending, budget increases, or new paid requirements without asking

- Ask Montgomery Kuykendall and receive explicit approval before spending money, incurring any additional charge, increasing any budget or spending limit, or enabling paid usage. General authorization to complete a task is not spending authorization.
- Never purchase, subscribe to, upgrade, activate, or introduce a required paid service, infrastructure resource, license, tool, or dependency without that explicit approval. Never make building, testing, running, deploying, or maintaining this repository depend on a new paid requirement without asking first.
- Always use a free option or an option already paid for, within its existing approved entitlement and included capacity. Access to an account, credentials, payment method, or paid subscription does not authorize additional charges.
- Do not enable paid overages, automatic upgrades, extra metered usage charges, or trials that require payment or can convert to paid service without explicit approval.
- If a necessary step would create a new cost, paid requirement, or budget increase, complete the useful work that can proceed without it, state the exact requirement and cost, and ask before taking that step. Continue already-authorized work that creates no new cost without asking for permission again.
<!-- END KI-OWNER-EXECUTION-AND-SPENDING-DIRECTIVE -->
