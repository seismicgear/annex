# Live-session acceptance

Run against a **new dedicated server database**, after building the current
client and server. Do not share that database with the functional, audit or
Puppeteer lanes: the first registrant must become the founder.

From `client/`:

```powershell
$env:ANNEX_LIVE_URL = 'http://127.0.0.1:3000'
npx playwright test --config e2e/live/playwright.config.ts
```

The filename `session.live.ts` intentionally stays outside the ordinary
`*.spec.ts` discovery pattern. This lane owns its founder and independent
member browser contexts, runs serially, and retries nothing.

All mutations use real application controls. No routes or API outcomes are
stubbed, no DB rows or application stores are seeded, and authentication uses
the real startup/proof flow. Server readbacks reuse credentials the application
actually sent; credentials and secret transfer material are not recorded in
attachments. Device linking uses the implemented encrypted transfer/pairing
code flow. Screenshots are captured only after the transfer dialog closes.

The lane covers device-link rejection/success/persistence, independent-peer
message edits/replies/deletions, all five supported channel types across both
scopes, leave/rejoin/delete, broadcast authorization, real image upload/remote
decode, and persona lifecycle. A channel's Federated flag is checked as stored
configuration; actual remote-server federation and media are separate lanes.
Persistent/Ephemeral are not channel types in the shipped enum; retention is a
server policy and is not claimed by this lane.

Results are written to `client/e2e-results/live/` and an HTML report to
`client/e2e-report/live/`. A test definition is not execution evidence: retain
the result together with the exact source revision and build configuration.
