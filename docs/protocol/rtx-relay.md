# RTX Relay Wire Format

How a reflection-summary bundle travels between Annex servers, and what each
signature in the envelope actually proves.

`docs/protocol/` held only `agent-connection.md` before this, which is part of
why the relay's provenance went three releases as an unsigned string list.

---

## The envelope

`POST /api/federation/rtx`, body `FederatedRtxEnvelope`:

```json
{
  "bundle":           { /* ReflectionSummaryBundle */ },
  "provenance":       { /* BundleProvenance */ },
  "relaying_server":  "https://b.example",
  "signature":        "…"
}
```

`signature` is the **legacy** single-hop envelope signature over
`rtx_relay_signing_payload`. It is still produced and still checked, for one
release, so a peer that only knows how to verify this keeps working. It covers
`provenance.relay_path`, which nothing else signs — so it is not what a
multi-hop receiver trusts.

```json
"provenance": {
  "origin_server": "https://a.example",
  "bundle_id":     "…",
  "relay_path":    ["https://a.example", "https://b.example"],
  "hops": [
    { "server": "https://a.example", "content_hash": "…", "signature": "…" },
    { "server": "https://b.example", "content_hash": "…", "signature": "…" }
  ],
  "origin": {
    "signature":            "…",
    "reasoning_commitment": "…",
    "max_hops": 3
  }
}
```

`relay_path` is **deprecated** and mirrored from `hops`. Do not read it for a
trust decision: nothing signs it. It exists so a peer on an older build still
parses the envelope.

---

## Why there are two signatures and not one

### The origin cannot sign what the receiver computes

`enforce_transfer_scope` rewrites the bundle in flight: a hop forwarding to a
`ReflectionSummariesOnly` peer strips `reasoning_chain`. The bytes leaving hop 2
are legitimately not the bytes that arrived. So any origin-level signature over
the full content — `rtx_bundle_content_hash` — would fail at every downstream
hop, *for the correct behaviour of the system*.

The origin therefore signs two things:

* `scope_invariant_content_digest(bundle)`, which omits `reasoning_chain`
  entirely and so survives stripping; and
* `reasoning_commitment = SHA-256(reasoning_chain or "")`, a commitment to the
  part that does not survive.

A relayer may **remove** the chain. It cannot **add or alter** one: the receiver
compares the chain it holds, if any, against the commitment. `None` and
`Some("")` commit to the same value, so a relayer that normalises one to the
other does not break the commitment.

Domain: `annex/rtx/origin-attestation/v1`. Fields length-prefixed, never
`join`-ed — a `|`-joined path lets one field's content impersonate a field
boundary.

### Each hop signs its own place, and the place before it

`relay_hop_payload`, domain `annex/rtx/hop/v1`, binds:

| Field | Why |
|---|---|
| `bundle_id`, `origin_server` | what is being relayed, and on whose behalf |
| `origin_signature` | ties the chain to ONE attestation; a chain cannot be lifted onto another |
| `hop_index` | a hop signature cannot be replayed at a different depth |
| `hop_server` | who is making the claim |
| `next_peer` | the hop authorises forwarding to ONE destination |
| `content_hash` | `rtx_bundle_content_hash` of the bundle as THIS hop sent it — per-hop, because scope enforcement rewrites it |
| `prev_chain_digest` | `SHA-256` of hop *i-1*'s payload, or `""` at hop 0 |

`prev_chain_digest` is what makes the chain a chain. Without it each signature
stands alone and a relayer can present any subset of hops in any order, each
individually valid. With it, dropping, reordering or splicing a hop invalidates
every signature after the edit.

`next_peer` of the last hop is the server currently **holding** the envelope:
the receiver's own public URL when verifying, the relayer's when appending.
`annex_rtx::hop_payloads` computes the whole vector so the relayer and the
receiver cannot derive it differently — a second implementation of that loop is a
second chance to disagree about what was signed, and a disagreement there reads
as a forged chain.

---

## What the receiver checks, in order

Order matters: the cheap bounds checks come first because
`POST /api/federation/rtx` has **no auth middleware** in front of it, so a
500-hop envelope from anywhere must cost a bounds check rather than 500 Ed25519
verifications.

1. `validate_provenance_structure` — non-empty origin, `hops.len() <=
   RTX_HOP_CEILING`, no repeated server, every field within
   `MAX_IDENTIFIER_BYTES`; and `provenance.bundle_id == bundle.bundle_id`.
2. **Loop**: this server's public URL must not appear in `hops` or as
   `origin_server`. Fails **closed** when the local URL is unknown — a server
   that cannot name itself cannot prove it is not already in the path, and
   guarding the comparison with `!local_url.is_empty()` (as the previous code
   did) turns cycle detection into cycle detection that silently passes.
3. `bundle.source_server == provenance.origin_server`. One line, and it is what
   separates "B relayed A's bundle" from "B wrote a bundle and named A".
4. The origin `instances` row must exist and be `ACTIVE`; its attestation must
   verify.
5. The reasoning-chain commitment, if a chain is present.
6. TTL: `hops.len() <= min(origin.max_hops, RTX_HOP_CEILING)`.
7. The hop chain: `hops[0].server == origin_server`, `hops.last().server ==
   relaying_server`, and every hop signature verified against that server's
   `instances.public_key`.
8. `hops.last().content_hash == rtx_bundle_content_hash(bundle_received)`.

Then the existing checks — agreement, scope, bundle structure, redacted topics —
and the write.

### A hop this server does not know

In A → B → C → D, D may know A and C but not B. Three options, and the choice is
a trust-model decision rather than an implementation detail:

* **reject** — safest, and makes multi-hop work only in a fully-provisioned
  mesh, which defeats the point of relaying;
* **accept and verify what you can**;
* **accept only if the origin and the immediate relayer both verify**, treating
  the middle as vouched-for by the last hop.

Annex does the third. The chained `prev_chain_digest` is what makes it
defensible: the immediate relayer's signature covers a digest that covers every
hop before it, so that relayer is cryptographically accountable for what it
claims the middle was. Hop 0 and the last hop must always verify; an unknown
middle is logged with the bundle id, the origin and the relayer — not accepted
silently.

---

## Onward relay

After the transaction commits, and **only** on a fresh insert
(`inserted == true`), the receiver spawns `relay_rtx_bundle_onwards` with the
provenance it received. Relaying before the write would forward a bundle the
server might then fail to store; relaying on a duplicate would re-forward on
every arrival, which is how a mesh with two paths between two servers becomes a
broadcast storm.

The hop is appended **per peer**, not once: the signature binds the destination
and the post-scope content hash, both of which differ per peer.

An envelope carrying no `origin` attestation is never extended. While
`federation.rtx_require_hop_chain` is `false` such an envelope is accepted and
delivered locally — that is the compatibility shim — but signing a hop onto a
chain whose origin nobody attested would launder it.

---

## Configuration

| Setting | Default | Meaning |
|---|---|---|
| `federation.rtx_max_hops` / `ANNEX_RTX_MAX_HOPS` | `3` | How far a bundle published HERE may travel. Signed into the attestation. `0` disables relay of local publications. |
| `federation.rtx_require_hop_chain` / `ANNEX_RTX_REQUIRE_HOP_CHAIN` | `false` | Refuse an envelope with no signed chain. |
| `annex_rtx::RTX_HOP_CEILING` | `5` (compile-time) | Absolute bound, independent of any origin's request. |
| `federation.allow_private_peer_addresses` | `false` | Lifts the private-address half of the SSRF gate. The RTX relay ignored this until 2026-09-15 while the message relay honoured it, so an operator who set it got messages relayed and RTX bundles silently dropped at the same peer. |

`3` and `5` are **chosen, not measured**: no Annex federation exists to measure a
diameter on. `5` matches `MAX_REDIRECT_HOPS` in `api_link_preview.rs`, the only
hop-limit precedent in the repository. A budget larger than the real federation
diameter is decorative; one smaller drops legitimate traffic with nothing
user-visible to say so.

## Deliberately not persisted

There is no `rtx_relay_rejections` table, and `rtx_bundles` gains no
`origin_server` / `hop_count` columns.

The provenance — hops, signatures and the origin attestation — is already stored
whole in `rtx_bundles.provenance_json`, so nothing is lost. Denormalised columns
with no reader are dead schema, and migrations here are append-only and
checksummed: a column added speculatively cannot be taken back.

A rejection log was specified and dropped for a sharper reason:
`/api/federation/rtx` has no auth middleware, so any host that can reach it could
grow that table without bound. An unauthenticated write surface is a worse defect
than the missing audit row. Rejections are `tracing::warn!` with structured
fields; if they need to be durable, the authenticated half (a known ACTIVE peer
failing a policy check) is the only part that should be, and it needs a retention
sweep designed with it.

## Cross-repository note

The federation **signalling** relay has a second implementation in the separate
`monolith-annex` repository (`api/signal.js`). The RTX relay does not — it is
server-to-server over HTTP — so this format is versioned by its domain strings
(`annex/rtx/hop/v1`, `annex/rtx/origin-attestation/v1`) rather than negotiated,
and a future change bumps the domain rather than editing the payload.
