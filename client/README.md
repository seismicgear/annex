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

```bash
npm test             # vitest, jsdom
npm run test:watch
npm run lint         # eslint
npx tsc -b --noEmit  # types only
```

Browser suites live in `e2e/` (Playwright, Chromium). The one that matters is
the UI audit: it screenshots every surface at four viewports, runs axe-core and
a dialog keyboard contract against each, and diffs against committed baselines.

```bash
bash ../scripts/ui-audit.sh                    # full sweep
bash ../scripts/ui-audit.sh --grep chat        # one surface, while iterating
bash ../scripts/ui-audit.sh --update-baselines # re-record an intended change
```

`docs/ui-audit/README.md` explains the manifest, the masking rules, and why the
server restarts on every run.

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
