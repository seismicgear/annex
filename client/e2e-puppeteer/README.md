# Puppeteer E2E harness

A second, independent browser-automation lane next to the Playwright suite in
`client/e2e/`. It drives the real production flow through a different browser
driver — including a cold start — and writes a full-page PNG at every milestone
to `client/e2e-puppeteer/screenshots/`.

**It is a gate, not a gallery.** The screenshots are the post-mortem, not the
product: `fail()` prints and calls `process.exit(1)`, it is used a dozen times,
and `main()` ends `.catch((err) => fail(...))`. This lane was very nearly
written off as "a screenshot tour with no assertions to fail" and left out of
CI on that basis — one `grep` disproved it. It runs in CI now, as a step in the
`ui-audit` job.

## Flow

```
IdentitySetup → "Create New Identity"
  → StartupModeSelector → "Continue"
    → in-browser Groth16 membership proof (WASM, 30–60s)
      → main Chat UI → join #General → send a message
```

## Running

The harness needs a running Annex server with the built client dist. Use the
shared helper:

```bash
# 1. Build client + start the Axum server on :3000 (fresh DB)
bash scripts/e2e-server.sh start

# 2. Drive it with Puppeteer (screenshots → client/e2e-puppeteer/screenshots/)
cd client && npm run test:e2e:puppeteer

# 3. Stop the server
bash scripts/e2e-server.sh stop
```

Run both lanes (Playwright + Puppeteer) in one shot:

```bash
bash scripts/e2e-all.sh          # or: e2e-all.sh both
```

**Give this lane its own server.** Channel creation needs a moderator and
`ensure_founder` grants that to the earliest registrant, so on a server the
Playwright suite has already registered against, this harness comes up as an
ordinary member and logs `no create-channel control (identity is not a
moderator) — skipping channel-create`. It still passes, having quietly tested
less. `e2e-all.sh both` restarts the server between the two lanes for exactly
this reason.

## Browser resolution

Uses `puppeteer-core` with a **caller-supplied** Chrome (no multi-hundred-MB
download on `npm install`). Executable resolution order:

1. `$PUPPETEER_EXECUTABLE_PATH`
2. Playwright-managed Chromium under `/opt/pw-browsers` (Claude Code env, and CI
   after `npx playwright install chromium`)
3. system `google-chrome` / `chromium` on `PATH`

## Options

```bash
node e2e-puppeteer/run.mjs --url http://127.0.0.1:3000   # target server
node e2e-puppeteer/run.mjs --headful                     # show the browser
```

Exit code is `0` only if every milestone (identity → server → main UI) is
reached; screenshots are written even on failure (e.g. `*-MISSING.png`) for
post-mortem.
