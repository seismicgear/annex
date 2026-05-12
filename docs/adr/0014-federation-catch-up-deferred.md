# ADR 0014 — Federation catch-up endpoint (deferred)

Status: Deferred — scaffolding intentionally not landed in this pass (2026-05-12)
Context tag: `hardening-pass`

## Context

Even with the outbox (ADR-0008) and receipt ledger (ADR-0007), there is a real scenario where a peer is offline long enough for its outbox queue to mark some rows `failed`. That peer would never receive those messages. A catch-up endpoint lets the receiving peer request "everything I missed after seq/marker X."

The reviewer correctly identified this as the next reliability fix after the outbox.

## Decision

The catch-up endpoint is **deferred** in this pass, for two reasons:

1. **Sequence number prerequisite.** Catch-up is naturally keyed on a strictly-monotonic per-(originating_server, channel) sequence. The current `FederatedMessageEnvelope` does not carry one — message_id is a UUIDv4, not a sequence. Adding the sequence number is a coordinated wire-format change (envelope_version `v3`) that ADR-0007 deliberately deferred.
2. **History pruning.** A correct catch-up endpoint must return `history_pruned` (per the brief) when retention has already removed the requested range. The retention sweep currently has no notion of "what the peer was expecting" — so an "after_seq=N" request against a server that has pruned the original rows can only respond with the timestamp of its oldest surviving event, not with the seq.

## Sketch (when both prerequisites are in place)

```text
POST /api/federation/catch-up
{
    "originating_server": "<base url>",
    "channel_id": "<channel>",
    "after_seq": 1234,
    "max_envelopes": 200
}
->
{
    "status": "ok" | "history_pruned",
    "envelopes": [ ... v3 envelopes ... ],
    "next_after_seq": 1434,
    "available_from_seq": 5000  // only on "history_pruned"
}
```

Receiver-side behaviour:

- Verify signature on every returned envelope using the existing receive path's machinery.
- Insert into the receipt ledger with `delivery_mode = 'catchup'` so the freshness gate accepts older `created_at` (the receipt ledger schema already supports this — see migration 036).
- Persist messages normally.
- `history_pruned` is an explicit "ask earlier than I can serve" signal; clients render it as "some messages were lost to retention" rather than silently treating it as "no new messages."

## Consequences of deferring

- Federation under sustained partition still loses messages whose outbox rows hit `attempts >= max_attempts`. The operator can manually mark those rows `pending` and let the worker retry, but there is no peer-driven catch-up.
- The receipt ledger's `delivery_mode = 'catchup'` column exists today so the future endpoint can write through it without another migration.

## What would unblock this

1. Land envelope_version `v3` with a per-originating-server, per-channel sequence number signed into the envelope.
2. Extend the receipt-ledger schema with `channel_id` + `origin_seq` so catch-up "after_seq" lookups are O(log n).
3. Build the endpoint + service function + tests.

Each of (1)/(2)/(3) is its own task, properly scoped. Doing them together in this hardening pass would have made the pass too large to verify.
