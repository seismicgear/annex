# Architecture and capability status

This is a map of **implemented v0.1.0 behavior**, with explicit qualification
boundaries. [FOUNDATIONS](../FOUNDATIONS.md) describes the full vision;
[ROADMAP](../ROADMAP.md) retains its milestones and acceptance criteria.
The foundational text predates the current custom license; its references to
“open-source” express the architectural intent, while [LICENSE.md](../LICENSE.md)
and [the license guide](licensing.md) govern actual source-available permissions.

## What runs where

The Rust/Axum server owns admission, SQLite persistence, permissions, federation,
agent policy, uploads, observation, and the in-process `webrtc-rs` SFU. The React
client runs in a browser or inside Tauri 2. The desktop app can start an embedded
server or connect to another server. The standalone Linux archive is a backend
package; deploy a web build separately if you need browser access.

| Area | Implementation | Qualification boundary |
| --- | --- | --- |
| Identity | Local secrets, Poseidon commitments, Groth16 membership, challenge-bound sessions, device linking/recovery | Single-contributor setup plus beacon; not a multi-party ceremony or independent audit |
| Messaging | Channels, history, edits/replies, permissions, attachments, WebSocket events | Broad internet/load/soak acceptance remains incomplete |
| Privacy | ChaCha20-Poly1305 at-rest message bodies; opt-in client E2EE with certified devices, manifests, epochs, private attachments | See [encryption](ENCRYPTION.md); no blanket metadata, endpoint, or media secrecy claim |
| Live media | Native WebRTC SFU, voice rooms, camera, screen share | Network/ICE/TURN and OS permissions matter; platform/provider matrix remains partial |
| Speech | Piper/Bark/system provider adapters, Whisper transcription; pinned desktop resources | An adapter/configured profile is not proof that every provider works on each platform |
| Agent participation | Identity and VRP negotiation, scoped actions, presence, speech, RTX policy | External runtime required; alignment threshold corpus is small |
| Federation | Authenticated HTTP, negotiated trust, durable outbox, membership/authority reconciliation, encrypted upload transfer | Operators configure peers and reachable origins; broad topology/partition acceptance remains ongoing |
| Experimental transport | Opt-in WebRTC relay/rendezvous transport is wired into server startup | Disabled by default; not equivalent to a public app tunnel or fully qualified universal transport |
| Desktop delivery | Four OS/architecture targets, six installer formats, updater metadata/signatures | Linux receipts cover v0.1.0; both Mac v0.1.1 invite/message journeys passed. Windows/Linux v0.1.1 journeys, browser-download Gatekeeper acceptance, and in-place updates remain unqualified |

## Identity and trust

A client proves membership without disclosing its identity secret. Pseudonyms
scope participation; cryptographic identity does not make network metadata or
behavior anonymous. VRP negotiates trust and transfer limits. The alignment
scorer uses a pinned static embedding model and carries a fingerprint, but its
threshold calibration corpus is currently too small for broad confidence.

Local authorization, server policy, agent limits, and negotiated transfer scope
must all permit an action. A peer or agent being “aligned” does not grant every
capability. Restricted topics and no-transfer settings still apply to RTX.
The protocol documents and implementation determine the actual message shapes;
architectural prose is not a substitute for testing an integration.

## Federation, routing, and hosting are separate

The ordinary federation path uses authenticated HTTP and a persistent outbox.
`ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED` activates the experimental
rendezvous/relay transport in server startup; it is no longer a dormant library.
A production configuration requires trusted signaling peers. The marketing
site's signed signaling endpoint exchanges SDP/ICE envelopes; it does not proxy
application HTTP, WebSockets, or media.

Desktop host mode starts a local listener and can attempt endpoint registration.
The default router-service contract is not a demonstrated working automatic
internet tunnel. `ANNEX_PUBLIC_URL` advertises an origin; it cannot make that
origin reachable. Operators must establish network reachability and HTTPS.

## The longer-term direction

Annex aims for independently operated communities, portable identity, meaningful
agent participation, negotiated federation, and accountable knowledge transfer.
Remaining work includes independent cryptographic review and setup contributors,
broader alignment calibration, speech/platform qualification, the remaining native
installation and OS-trust journeys, updater acceptance, public-host routing, and measured performance
and soak evidence. Preserve those goals without presenting them as shipped
acceptance. See [release evidence](RELEASE_READINESS.md) for exact completed runs.
