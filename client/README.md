# Annex client

The React 19 + TypeScript + Vite single-page app that Annex serves. One build
runs in two places: a browser against a remote server, and inside the Tauri
desktop shell (`crates/annex-desktop`), which bundles this `dist/` and points
it at an embedded server.

## Run it

```bash
npm ci
npm run dev          # Vite dev server; expects an Annex server on :3000
npm run build        # tsc -b && vite build → dist/
```

For a full stack with a real backend, fresh database and built client:

```bash
bash ../scripts/e2e-server.sh start   # :3000
bash ../scripts/e2e-server.sh stop
```

## Test

Five lanes, in ascending order of how long they take and how much they need.
All five run in CI, each against its own fresh server — which was not true
until recently: `playwright.config.ts` defines four projects, and for a while
only two of them were named by anything that ran. A project in a config is not
a lane until something invokes it.

### 1. Unit — no server

```bash
npm test             # vitest run, jsdom
npm run test:watch   # vitest, watch mode
npm run lint         # eslint
npx tsc -b --noEmit  # types only
```

### 2. E2E — needs a running server

`playwright.config.ts` sets no `webServer`, so nothing is started for you. The
suites in `e2e/` drive Chromium against `http://127.0.0.1:3000` and fail
immediately if nothing is listening there.

```bash
bash ../scripts/e2e-server.sh start   # build client + fresh DB + Axum on :3000
npm run test:e2e                      # playwright test
npm run test:e2e:headed               # watch it happen
npm run test:e2e:debug                # Playwright inspector
npx playwright test e2e/startup.spec.ts   # one file
bash ../scripts/e2e-server.sh stop
```

Tests run serially (`workers: 1`, `fullyParallel: false`) because they share
server state, and the per-test timeout is 120s because a real proof takes 30–60s.
Traces, screenshots and video land in `e2e-results/`; the HTML report in
`e2e-report/`.

### 3. UI audit — the one that gates visual change

`e2e/audit/` screenshots every surface in `e2e/audit/surfaces.ts` at four
viewports, runs axe-core and a dialog keyboard contract against each, and diffs
against committed baselines. `manifest.spec.ts` fails if a component renders a
modal no surface reaches, so the manifest is a coverage contract, not a list.

```bash
bash ../scripts/ui-audit.sh                    # full sweep, fresh server
bash ../scripts/ui-audit.sh --grep chat        # one surface, while iterating
bash ../scripts/ui-audit.sh --update-baselines # re-record an intended change
```

The script manages its own server — do not start `e2e-server.sh` first.
Recording a baseline is deliberate and never incidental: the config sets
`updateSnapshots: 'none'`, so a missing baseline fails rather than being minted
on first run, and only `--update-baselines` can write one. Re-record in its own
commit.

[`../docs/ui-audit/README.md`](../docs/ui-audit/README.md) explains the
manifest, the masking rules, and why the server restarts on every run.

### 4. Group call — three browsers in one room

`e2e/audit/group-call.spec.ts` is a separate Playwright project, not part of
the audit sweep. Three real browser contexts join one voice channel serially
with fake media devices, so each join exercises renegotiation against a room
that already has peers in it. It asserts every participant sees itself plus two
*distinct* others.

```bash
bash ../scripts/e2e-all.sh group-call
```

That distinctness is the whole point. The SFU used to write every sender's RTP
into a single outbound track per receiver, so two senders interleaved onto one
track and decoded to neither — calls were structurally limited to two people
and the client had no per-sender track to attribute a tile to. This lane is the
guard that replaced the test which existed to fail when that was fixed. Unit
tests can assert the track map is shaped correctly; only a real call proves the
renegotiation reaches the peers already in it and that they answer.

### 5. Puppeteer journey — a second driver over the same path

`e2e-puppeteer/` drives the same journey through a different browser driver,
including a cold start: identity, in-browser proof, chat, channel creation.

```bash
bash ../scripts/e2e-all.sh puppeteer
bash ../scripts/e2e-all.sh both        # functional suite, then this, on a fresh server each
```

Give it its own server. Channel creation needs a moderator and `ensure_founder`
grants that to the earliest registrant, so on a server the functional suite has
already registered against, this lane comes up as an ordinary member and
silently skips that check — it still passes, having tested less. `e2e-all.sh
both` restarts between lanes for exactly this reason.

## Startup flow

`AppShell` renders `StartupGate` whenever any pre-main gate matches, and
`MainLayout` only once none do. The gates are evaluated top-to-bottom, first
match wins ([`src/app/StartupGate.tsx`](src/app/StartupGate.tsx)):

1. **Identity check in flight** — loading splash while `src/lib/db.ts` is read.
2. **Fatal startup-init error** — retry and clear-state controls.
3. **`IdentitySetup`** — no keys in IndexedDB. Generate a new identity, or
   receive one from another device via the device-link flow.
4. **`StartupModeSelector`** — keys exist but no server is chosen. "Use this
   server" is the local path; a remote URL or an invite is the other.
5. **Registration** — password prompt for a `password` access-mode server,
   then registering → proving → verifying, with a distinct error gate.

Only after step 5 resolves does the chat UI render. Step 5 is where the real
Groth16 proof happens, which is why it is a gate and not a spinner.

Two hooks carry it. `useAppBootstrap` runs the cold start — load identities,
load the saved server list, and the Tauri fresh-install cleanup — which is what
resolves gates 1 and 2. `useServerSelection` takes over at "user picked a
server" and drives it through to registered-and-persisted, which is gate 5.
Cross-cutting state (`serverReady`, password, pending invite codes) is owned by
`AppShell` so the gate UI can read it; the hooks only drive the effects.

## Layout

| Path | What |
|---|---|
| `src/app/` | Shell, startup gate, and the hooks driving server selection, bootstrap and the session connection. |
| `src/components/` | Views and dialogs. `Modal.tsx` is the dialog primitive — every modal goes through it, and it owns the focus, ARIA and Escape contract. |
| `src/voice/` | Call UI over the native WebRTC SFU in `crates/annex-voice`. |
| `src/stores/` | Zustand stores: identity, channels, servers, voice, usernames. |
| `src/api/` | Typed wrappers per API area; `src/lib/api.ts` re-exports them. |
| `src/lib/` | Crypto, IndexedDB, ZK proving, personas, invites. |
| `src/workers/` | The Groth16 proof worker, kept off the main thread. |
| `e2e/audit/` | The UI audit harness: surface manifest, capture runner, checks. |

## Things that will surprise you

- **Reaching the main UI costs a real Groth16 proof** — 30–60s in the browser on
  a first run, with no client-side bypass. `ANNEX_ENFORCE_ZK_PROOFS=false`
  relaxes only the *server*. Identity, keys and the proof are cached in
  IndexedDB, which is how the audit skips it.
- **A cached proof binds to the Merkle root current when it was generated**, and
  every registration moves that root. Anything holding warm state has to be
  refreshed against the live root or the server re-verifies on every load.
- **Identity lives in IndexedDB, not localStorage.** Clearing site data is a
  destructive account action. There is a device-linking flow for moving between
  devices and a social-recovery flow for losing one.
- **Personas are local and randomly coloured.** `randomAccentColor()` picks one
  of twelve at creation and that colour drives buttons, badges and avatars
  app-wide, so two runs of the same flow do not look alike.
- **Invite links require an HTTPS public URL.** The link carries a join secret,
  so the server rejects an `http://` one; the admin panel says so rather than
  offering an action that cannot succeed.
