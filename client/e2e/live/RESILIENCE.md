# Actual transport and recovery acceptance

These eight definitions are not execution receipts. Run serially against a fresh disposable database and the currently rebuilt client/server. The first registrant must be the founder. Do not share the server database with another acceptance lane.

From `client`, with the real server already running:

```powershell
$env:ANNEX_LIVE_URL = 'http://127.0.0.1:3000'
node node_modules/playwright/cli.js test --config e2e/live/resilience.playwright.config.ts
```

The test-only TCP relay forwards original bytes to the real server. It never writes an HTTP response, fabricates a WebSocket frame, replaces an API, seeds a database, or writes application stores. Dropping the relay's sockets creates a real connection failure. Pausing the relay models a stalled connection. The tests read public identity/server metadata from IndexedDB and use captured real membership credentials for independent authenticated backend reads. Credentials are not attached to results.

| Definition | Actual trigger and required outcome |
| --- | --- |
| Dropped sockets | Member TCP links close. Composer disables and preserves its draft. Independent founder publishes two messages during the outage. Actual reconnect/resume displays each once; subsequent member send reaches founder and persists once. |
| Edit history Retry | Founder makes a real edit. Member loses transport before opening its uncached history. Error must not become an empty history claim. Restore, click Retry, compare both displayed versions against stored `old_content` and current message. Toggle closed/open. |
| Offline edit/delete | Member opens its real message editor, loses transport, and saves. Visible refusal plus original local and persisted body are required. Confirm actual delete while offline, dismiss refusal, and verify no tombstone. |
| Refused-message controls | An ordinary member actually sends to a Broadcast channel. Server must refuse and persist nothing. Dismiss one failed bubble. Retry another while unauthorized, then founder grants Moderator through the actual member UI; retry must then persist once. Restore the capability afterward. This deliberately tests current authority on an existing socket. |
| Startup addresses | Blank context creates keys, rejects a malformed address, fails against the actually disconnected relay, then registers against the real server. Independent delivery and saved remote preference/reconnect after reload are required. |
| Server hub | Invalid and unreachable Join-a-Server attempts preserve saved entries. After a genuine server summary, stall the network and close the registering tab once its real placeholder is persisted. Reopening must expose the interrupted registration's Remove and Retry actions. Removal and failed Retry are independently read back; original session must still deliver. The relay is another address for the same real node, not federation evidence. |
| Tooltip | Real Create Channel tip receives keyboard focus, blur, hover, and pointer leave. Actual text and measured viewport bounds are checked at 1280 and 390 pixels. The checkbox and backend channels must remain unchanged. This proves those actual placements; no DOM positioning is injected. |
| Narrow device link | Generate real registered-identity transfer at 360 by 740 pixels. Rasterize the actual SVG at displayed dimensions, decode through independent jsQR, compare encrypted payload without logging it, and click the ordinary Done button. Full cryptographic import is in the separate session lane. |

Results are written to `client/e2e-results/resilience/results.json`. Safe JSON attachments contain public message/channel IDs and test text, measured bounds, and action results. Traces, automatic screenshots, and videos are disabled because a device transfer displays encrypted identity material plus its pairing code.

Static typechecking and linting do not establish any row as passed. Unexpected browser errors fail the lane. A failed serial test leaves later definitions unexecuted; preserve that distinction in the final acceptance record.
