# Operator and identity acceptance

Build current source, start a separate fresh server database, and run alone
from `client`:

```powershell
$env:ANNEX_LIVE_URL = 'http://127.0.0.1:3000'
npx playwright test --config e2e/live/operator.playwright.config.ts
```

Six serial tests use real registrations, UI mutations, persisted readbacks and
independent browser identities. They neither seed database/application state
nor intercept routes. Network error/retry cases temporarily disconnect only
the tested browser context with Chromium's offline network control.

| Case | Required observations |
| --- | --- |
| Server settings | Empty name cannot save; rename reaches public summary; invalid URL cannot change persisted settings; normalized actual local URL survives reload; Upload Image and Change Image open real file pickers and store browser-decodable images; independent user fetches uploaded bytes; storage panel matches actual health. |
| Policy | Every exposed scalar, checkbox, limit and tag list is submitted and read back, survives reload, and restores through UI. Turning off voice changes real config status; turning off usernames hides the visible-name response. Tags must disappear from persisted policy when removed. |
| Admission | Wrong password is actually rejected; correct password produces a real identity proof/session that sends a persisted message. Invite-only mode actually refuses a new no-invite identity, which can register after public admission is restored. |
| Username privacy | An ordinary member sets, grants, revokes and removes a username. The authorized independent viewer sees it in actual chat; an ungranted identity never does. Revocation/deletion readbacks and reloaded peer rendering must remove it. |
| Recovery | Five actual guardian shards copied via browser clipboard; uncopied close warning; blank-device recovery entry; insufficient/duplicate refusal; commitment-verified reconstruction; restored registration sends as the original pseudonym; reload retains operation. No throwaway registration is allowed before recovery. |
| Search / events | Actual message-search results, no-result semantics, result navigation, close and Escape; genuine offline failure and Retry; all six event filters compared to actual API rows; refresh; actual network outage must show an error rather than an empty successful log, then recover. |

Reports land in `client/e2e-results/operator/`. Explicit screenshots avoid
password and recovery dialogs. Shard payloads stay in memory and the test clears
its own final copied secret from the clipboard. No automatic screenshots,
traces or videos are enabled.

These tests define acceptance; they do not certify it before execution. Policy
readback proves persistence, not every downstream enforcement branch. This lane
does not prove an HTTPS invite's external marketing-router journey, actual
storage exhaustion/507 recovery, federation transport/outbox/sever, message
retention expiry, native desktop data reset, or physical audio/video devices.
Those require their corresponding real environments and separate evidence.
