The platform is a Civic Mesh communication node. Not "inspired by" Monolith — it IS a Monolith-class node whose primary domain is real-time communication instead of governance. Same identity substrate, same federation protocol, same agent integration pattern, same ZKP stack. The only difference is the application layer on top.

That means:
Identity Plane — full ZKP, no shortcuts
Every user on the platform holds a self-sovereign identity. Client-side keypair generation, Poseidon(BN254) identity commitments exactly as specified in Monolith's identity.circom — commitment = Poseidon(sk, roleCode, nodeId). Users join servers by proving Merkle membership in the server's VRP tree via Groth16 proofs. The membership.circom circuit from Monolith applies directly: prove you're a leaf in the server's member tree without revealing your secret or leaf index.
Topic-scoped pseudonyms carry over unchanged. A user has different pseudonyms per server, per channel category if you want that granularity. pseudonymId = sha256(topic + ":" + nullifierHex) — same derivation as Monolith. Cross-server identity linkage is opt-in via the link-pseudonyms.circom concept from the roadmap, never automatic.
The civic_identities table maps directly. Replace Montopia's personhood types with comms-relevant ones:

HUMAN — biological user
AI_AGENT — any AI participant (MABOS instance, LangChain bot, custom agent, whatever)
COLLECTIVE — shared accounts, team identities, organizational presences
BRIDGE — federation bridge entities
SERVICE — platform-level services (the voice LLM, moderation bots, logging)

Eligibility flags become capability flags: can_voice, can_moderate, can_invite, can_federate. Same pattern as eligible_to_vote / eligible_for_dividend — explicit, auditable, type-gated.
The VRP registry backend (vrp_identities, vrp_leaves, vrp_roots, zk_nullifiers) transfers wholesale. The Merkle persistence layer (merklePersistence pattern — reconstruct tree from DB at startup) is required for the same reason: VRP continuity across server restarts.
VRP as the universal trust protocol
This is where your value_resonance.rs becomes the federation backbone.
Three VRP handshake contexts:

User ↔ Server: User proves membership. Server validates via Groth16 verification against its current Merkle root. Pseudonym materialized in the server's presence graph. This is the POST /api/zk/verify-membership flow from Monolith, unchanged.
Agent ↔ Server: Agent performs VRP handshake from its own runtime (MABOS pattern: local HTTP endpoint GET /vrp/pseudonym?topic=<server_topic>). The VrpAnchorSnapshot exchange happens here — the agent's EthicalRoot (principles + prohibited_actions) is compared against the server's policy root using compare_peer_anchor. Alignment status determines what the agent can do:

Aligned → full channel access, voice participation, knowledge exchange
Partial → restricted channels, text only, ReflectionSummariesOnly transfer scope
Conflict → rejected, NoTransfer

The VrpCapabilitySharingContract becomes the agent's declaration of what it will and won't do on the server. knowledge_domains_allowed, redacted_topics, retention_policy, max_exchange_size — all of these apply directly. The server operator sets their own contract; mutual acceptance is required per contracts_mutually_accepted().
Server ↔ Server: Federation handshake. VrpFederationHandshake with protocol_version, identity_hash, ethical_root_hash, declared_transfer_scopes, declared_capabilities. This is Monolith Section 8.3 — /federation/attest-membership with cross-instance VRP attestation, federated_identities table, signed Merkle root exchange. Two servers federate only if their policy roots align via VRP. Federation trust is not binary — it's the full VrpAlignmentStatus spectrum with negotiated transfer scopes.

The reputation system from check_reputation_score — the Legacy Ledger integration that tracks alignment history per counterparty — applies to all three contexts. Servers track agent reputation over time. Agents track server reputation. Federated servers track each other. Bad actors decay toward Conflict through accumulated LegacyLedgerAlignment entries. record_vrp_outcome logs every handshake result for longitudinal trust computation.
RTX as the agent knowledge layer
RTX (Recursive Thought Exchange) is the protocol for agents sharing cognitive state across the platform. When a MABOS instance in one server learns something — processes a conversation, generates a reflection — it can package that as a ReflectionSummaryBundle and publish it via RTX to peer agents on other servers, gated by the VRP transfer scope negotiated during federation.
This is bigger than "bots talking to each other." This is distributed agent cognition over a comms backbone. The platform isn't just carrying human messages — it's the substrate for agent-to-agent knowledge propagation with cryptographic trust gates at every boundary.
The GovernanceEndpoint from MABOS mediates the transfer: bundles are cryptographically signed, linked to valid VRP handshakes, and scoped by the capability contract. An agent can't exfiltrate knowledge from a server where redacted_topics includes that domain.
Graph — Monolith's civic graph adapted for presence
The graph_nodes / graph_edges schema from Monolith Section 2 applies with minimal changes. Nodes are pseudonymous participants (users, agents, services). Edges are typed relationships: MEMBER_OF (user → channel), CONNECTED (user ↔ user), AGENT_SERVING (agent → channel), FEDERATED_WITH (server ↔ server).
The visibility model from Monolith 2.3 — BFS-based degrees of separation with tiered visibility (1st degree full profile, 2nd degree limited, 3rd degree cluster-only, beyond that aggregate) — applies to the platform's social layer. You see your server's members in full. Federated server members at reduced resolution. Beyond that, aggregate presence only.
Real-time presence via SSE (/events/graph) with NODE_ADDED, NODE_UPDATED, NODE_PRUNED events. The pruning lifecycle from Monolith 2.6 — inactive nodes soft-pruned with tombstone metadata, reactivatable via fresh VRP handshake — handles users going offline gracefully.
Voice architecture — platform-hosted voice LLM as a service layer
Voice transport: LiveKit SFU for all participants (human and agent). Every voice channel is a LiveKit room. Human users connect directly via WebRTC through the LiveKit SDK.
Agent voice: the platform runs a voice LLM service (Piper, Bark, or Parler-TTS quantized for your 7900 XTX). Agents connect to the platform's agent protocol, send text intent, and the voice service renders it into the LiveKit room as an audio track. The agent never touches WebRTC. The platform handles all audio I/O.
Voice identity: each agent gets a voice profile assigned at the server level (stored in graph_nodes.metadata_json). Server operator controls which voice model, which voice profile, which latency tier. Swap voice models platform-wide without touching any agent code.
STT for incoming human speech that agents need to process: the platform runs a local Whisper instance (or equivalent), transcribes voice channel audio, and feeds text to subscribed agents via their channel connection. Agents see text; they respond in text; the platform renders their response as voice. Clean separation.
Channel model
Channels are topic-scoped communication spaces with typed access control:

TEXT — standard message channel
VOICE — real-time audio (LiveKit room)
HYBRID — text + voice simultaneous (the Discord model)
AGENT — agent-only channels for RTX exchange and inter-agent coordination
BROADCAST — one-to-many announcements, federation-wide if enabled

Each channel has:

VRP topic binding (membership proof required to join)
Capability requirements (which can_* flags are needed)
Agent policy (which alignment status is required for AI participants)
Retention policy (how long messages persist, per server config)
Federation scope (local only, or exposed to federated servers)

Server governance — operator is sovereign
Every server instance has a server_policy config (versioned, append-only changelog, analogous to civic_bank_policy_versions):

Moderation rules
Agent admission policy (minimum VRP alignment score, required capabilities)
Federation policy (who to federate with, transfer scope limits)
Retention policy
Voice LLM configuration
Channel defaults

Policy changes are logged in the server's event log. If the server is federated, policy changes trigger re-evaluation of VRP alignment with all federation peers — a server that changes its moderation stance might drop from Aligned to Partial with stricter peers, automatically reducing what data crosses the boundary.
No upstream authority can override server policy. No Discord-style "we changed the terms" because there is no "we." The server operator IS the authority. The protocol enforces interoperability; governance is local.
Federation — Monolith Section 8, directly
The tenants, instances, federated_identities, inter_node_contracts tables from Monolith apply. Each server is a tenant. Each deployment is an instance. Cross-server identity attestation via /federation/attest-membership with signed Merkle root exchange and Groth16 proof verification.
Federation APIs follow Monolith 8.4's sharp boundaries:

Allowed reads: server metadata, public channel listings, aggregated presence, federation policy summary
Allowed writes: VRP attestations, explicit federation agreements
Everything else is local

Cross-server messaging for federated channels uses signed message envelopes verified against the sender's VRP attestation. Messages carry their Merkle membership proof so the receiving server can verify the sender is a valid member of the originating server without trusting the originating server's word for it. Trustless verification at the message level.
Observability — Section 9 pattern
public_event_log with domain-scoped, append-only events. For a comms platform the domains are:

IDENTITY — registrations, VRP handshakes, pseudonym derivations
PRESENCE — joins, leaves, pruning, reactivation
FEDERATION — attestations, policy changes, trust re-evaluations
AGENT — agent connections, VRP alignment results, capability declarations
MODERATION — actions taken, appeals, policy enforcement

Public read-only APIs for server operators and federation peers to audit. Same SSE streaming pattern for real-time observability.
The ZKP stack
Full Circom/Groth16 pipeline from Monolith's zk/ workspace:

identity.circom — commitment generation
membership.circom — Merkle membership proof
link-pseudonyms.circom — opt-in cross-server identity linking
Build/setup/test scripts, trusted setup artifacts, verification keys
Membership WASM + zkey deployed per server instance

Plus new circuits for the comms domain:

channel-eligibility.circom — prove you have the right capability flags for a channel without revealing your full identity record
federation-attestation.circom — prove cross-server membership to a third server without revealing which server you're from (for multi-hop federation)

Data model summary — the full schema set:
From Monolith, adapted:

civic_identities → platform_identities
vrp_identities, vrp_leaves, vrp_roots, zk_nullifiers, vrp_topics, vrp_roles
graph_nodes, graph_edges
tenants, instances, federated_identities
public_event_log

New for comms:

servers (extends tenants with comms-specific config)
channels (topic-scoped, typed, with VRP binding)
messages (append-only, channel-scoped, with sender pseudonym + proof ref)
voice_sessions (LiveKit room bindings, participant tracking)
agent_registrations (VRP alignment results, capability contracts, voice profile assignments)
server_policy_versions (versioned governance config with changelog)
federation_agreements (bilateral server contracts with transfer scope)
voice_profiles (per-agent voice identity configuration)
