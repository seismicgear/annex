# UI Audit

A repeatable pass over every surface a user can touch: screenshot it, audit it,
record what is wrong, and fail CI when it changes unintentionally.

Before this existed, `client/e2e/`, `client/e2e-puppeteer/` and
`scripts/e2e-server.sh` were referenced by **zero** CI workflows, there were no
screenshot baselines anywhere in the repo, and the browser suites covered
roughly 8 of the app's ~50 distinct visual states.

## Run it

```bash
bash scripts/ui-audit.sh                    # full run against a fresh server
bash scripts/ui-audit.sh --grep chat        # one surface, while iterating
bash scripts/ui-audit.sh --keep-server      # reuse a running server (see caveat)
bash scripts/ui-audit.sh --update-baselines # re-record after an intended change
```

Outputs:

| Path | What it is |
|---|---|
| `client/e2e/audit/baselines/<viewport>/<surface>.png` | Approved screenshots. **Tracked** — these are the diff target. |
| `docs/ui-audit/findings.json` | Machine-readable ledger, sorted for clean diffs. |
| `docs/ui-audit/index.html` | Contact sheet: every surface, every viewport, with its findings. |
| `client/e2e/audit/diagnostics/` | Screenshots of surfaces the run could not reach. Gitignored. |

### Why the server restarts by default

The audit's `founder` role must be the **earliest identity to register**,
because the server promotes the earliest registrant to moderator
(`ensure_founder`) and that is the only path to the admin surfaces. Running
against a server that already has identities produces a founder with no admin
rights, so the setup fails loudly instead of capturing a half-empty audit.
`--keep-server` skips the restart and will hit exactly that if identities
already exist.

## How it fits together

```
scripts/ui-audit.sh
  ├─ scripts/e2e-server.sh start        fresh DB, built client, :3000
  └─ playwright --project=audit
       ├─ audit-setup (roles.setup.ts)  warm auth state + seed fixtures
       └─ audit
            ├─ manifest.spec.ts         static guards on the manifest
            └─ capture.spec.ts          screenshot + audit every surface
```

| File | Role |
|---|---|
| `client/e2e/audit/surfaces.ts` | **The manifest.** Every capturable surface: id, journey stage, how to reach it, which role, what is masked, which audits are waived and why. |
| `client/e2e/audit/types.ts` | The contract: stages, roles, viewports, audits, findings. |
| `client/e2e/audit/nav.ts` | Navigation recipes and capture stabilisation. |
| `client/e2e/audit/roles.ts` / `roles.setup.ts` | Warm authentication state and fixture seeding. |
| `client/e2e/audit/audits.ts` | The automated checks. |
| `client/e2e/audit/capture.spec.ts` | The runner. |
| `client/e2e/audit/manifest.spec.ts` | Guards that keep the manifest honest. |
| `scripts/ui-audit-report.mjs` | Renders the contact sheet. |

## Adding a surface

Add an entry to `SURFACES` in `client/e2e/audit/surfaces.ts`:

```ts
{
  id: 'admin-server-policy',        // stable; becomes the screenshot filename
  stage: '09-admin',                // journey order
  title: 'Admin — server policy',
  role: 'founder',                  // fresh | member | founder | second-member
  intent: 'Why this screenshot exists — a reviewer reads this in the report.',
  navigate: async (page) => { await openAdminSection(page, 'Server Policy'); },
  clip: '.view-content',            // optional: element instead of viewport
  mask: ['.some-random-id'],        // optional: extra nondeterministic regions
}
```

Then record its baseline:

```bash
bash scripts/ui-audit.sh --update-baselines --grep admin-server-policy
```

`manifest.spec.ts` fails if a component renders a `.dialog-overlay` and no
surface reaches it, so a new dialog cannot quietly go unaudited. If a modal
really is unreachable, add it to `KNOWN_UNREACHABLE` there with a reason.

## What gets audited

| Audit | Catches |
|---|---|
| `a11y` | axe-core, WCAG 2.1 A/AA + best practices. |
| `console` | Uncaught page errors and `console.error` while reaching the surface. |
| `network` | Requests that failed or returned >= 400. |
| `overflow` | Content wider than the viewport; text clipped without an ellipsis. |
| `keyboard` | Dialogs: `role="dialog"`, focus moved in, focus trapped, Escape closes. |

Findings are **recorded, not asserted**. A surface with an accessibility
violation is still captured and the run still reaches the end — an exhaustive
audit is only useful if it finishes. Genuine failures (a surface that cannot be
reached, or one that no longer matches its baseline) do fail the run.

Waiving an audit requires a reason string, not a boolean:

```ts
waive: { network: 'the 500 is injected deliberately to reach the error state' }
```

`manifest.spec.ts` rejects waivers shorter than a sentence, because an
unexplained waiver is indistinguishable from a silenced bug.

## Determinism

Every run generates fresh cryptographic identities, so pseudonyms, leaf
indices, invite codes and timestamps differ each time and are rendered on
screen. Rather than faking the thing under test, those regions are **masked**
at capture time — see `NONDETERMINISTIC_SELECTORS` in `nav.ts`, and the
per-surface `mask` option. Components can opt in with `data-nondeterministic`.

Two colour sources are pinned in the capture stylesheet rather than the
product, because the randomness is real behaviour and making it deterministic
is a product decision to take deliberately:

- `randomAccentColor()` gives each persona one of 12 colours at creation, and
  that colour drives buttons, badges, avatars and highlights app-wide.
- `ServerHub` sets `--server-accent` inline per icon.

Animations and transitions are disabled, and the caret is hidden.

## Speed

Reaching the main UI costs a real in-browser Groth16 membership proof, and
there is no client-side bypass — `ANNEX_ENFORCE_ZK_PROOFS=false` only relaxes
the *server*. So `roles.setup.ts` drives the real startup flow once per role
and saves `storageState({ indexedDB: true })`; Annex keeps identity, keys and
the cached proof in IndexedDB, so restored contexts skip proving entirely.

A cached proof binds to the Merkle root current when it was generated, and each
registration moves that root — so after all roles exist, only the last one
holds a proof matching the live root. A final refresh pass re-saves each role
against the final root, which takes the capture run from ~1.9s to ~150ms per
page load and, more importantly, to **zero** `verify-membership` calls. Without
it the run trips the server's rate limiter and screenshots
"Rate limit exceeded" instead of the UI.

Storage state is regenerated every run and never committed: it is only valid
against the server instance that produced it.

The audit lane also raises rate limits via
`ANNEX_RATE_LIMIT_{REGISTRATION,VERIFICATION,DEFAULT}` (set in
`scripts/e2e-server.sh`). At the shipped defaults — 10/10/60 per minute — a
browser suite exceeds them trivially, since every public route keys its bucket
by IP and all captures share one.

## Baselines

Baselines are committed and diffed with a 0.5% pixel tolerance — tight enough
to catch a colour, spacing or layout change, loose enough to survive font
hinting differences between machines.

When a change is intended:

```bash
bash scripts/ui-audit.sh --update-baselines
git add client/e2e/audit/baselines
```

That is deliberately a separate, reviewable commit. A run that silently
rewrote its own baselines could never detect a regression.

## CI

The `ui-audit` job in `.github/workflows/ci.yml` runs the whole lane on every
PR and uploads the contact sheet plus any diagnostics on failure.
