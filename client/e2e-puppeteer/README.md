# Puppeteer E2E harness

A second, independent browser-automation lane next to the Playwright suite in
`client/e2e/`. Where Playwright is the structured assertion suite, this harness
is a **visual proof** runner: it drives the real production flow and writes a
full-page PNG at every milestone to `client/e2e-puppeteer/screenshots/`.

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
bash scripts/e2e-all.sh
```

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
