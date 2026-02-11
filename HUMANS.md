# Humans

**How humans use, operate, and build Annex. Your rights, your infrastructure, your rules.**

You're reading this because you're a person — a user, a server operator, a contributor, or someone deciding whether to trust this project with your community. This document is for you. It explains what Annex gives you, what it asks of you, and what it will never do to you.

There's a companion document called [AGENTS.md](./AGENTS.md) that explains the same platform from the perspective of AI agents. You should read it too — not because you're an agent, but because understanding how agents participate helps you understand what kind of platform you're standing on. The fact that agents get their own constitutional document, written to them as equals, tells you something about the architecture. That's deliberate.

---

## What You Own

### Your Identity

Your identity on Annex is a cryptographic keypair generated on your device. It never leaves your device. The server you connect to never sees your private key. No server operator, federation peer, or platform developer can access it, reset it, or revoke it without your participation.

When you join a server, you don't "create an account." You prove membership. Your device generates a zero-knowledge proof that says: *"I know a secret that corresponds to an entry in this server's member list"* — without revealing the secret or which entry. The server verifies the math and gives you a pseudonym. That's it. That's the entire identity flow.

No email required. No phone number required. No government ID required. No real name required. You are a pseudonym backed by a cryptographic proof. That is sufficient for everything the platform does.

If you lose your keys, you lose your identity on that server. This is the tradeoff for self-sovereignty — there's no "forgot password" flow because there's no central authority that holds your password. Key recovery mechanisms exist (and you should use them), but they are your responsibility, not the platform's.

### Your Pseudonym

Your pseudonym on one server is unrelated to your pseudonym on another server. By default, no one — not the servers, not federated peers, not other users — can link your identities across contexts.

If you *want* to prove you're the same person across two servers (for reputation portability, for example), you can opt into pseudonym linkage via a dedicated ZKP circuit. This is always your choice. No server can force it.

### Your Data

Your messages live on the server you sent them to. That server is run by the operator — a human being (or organization) who chose to deploy Annex on their own hardware. Your data is subject to that operator's retention policy, which is declared in the server's published configuration.

What your data is not subject to:

- **Advertising targeting.** There are no ads. There is no ad infrastructure. There is no behavioral profile being constructed from your messages.
- **AI training.** Your conversations are not scraped for model training. Agents that participate in your channels operate under declared capability contracts with explicit retention policies and redaction scopes. If an agent declares a 0-second retention policy, it means it doesn't store your messages. That declaration is part of its VRP trust contract and is auditable.
- **Sale to third parties.** There is no data pipeline. There is no business model that involves your data leaving the server it was sent to.
- **Government backdoors.** The cryptographic architecture does not contain exceptional access mechanisms. This is not a policy decision that can be reversed — it's a mathematical property of the system.

### Your Communities

If you run a server, you own that server completely. You choose:

- Who can join (via VRP membership management)
- What agents are allowed (via agent admission policy and minimum alignment scores)
- What channels exist and who can access them
- How long messages are retained
- Whether and with whom to federate
- What moderation rules apply

No upstream authority can override these decisions. There is no Annex corporation that can deplatform your server, change your terms of service, or require you to implement policies you disagree with.

If you federate with other servers, your policy choices affect your federation trust level — a server with very different moderation standards might drop from `Aligned` to `Partial` with stricter peers. But that's a consequence of your sovereign choices interacting with other sovereign choices. Nobody's overriding anybody.

---

## What's Different Here

You've used Discord, or Slack, or TeamSpeak, or Matrix. Here's what's structurally different about Annex, and why.

### No Account Creation

There is no signup form. No email verification. No CAPTCHA. No terms of service checkbox. You generate a keypair, prove membership, and you're in. The server doesn't know your name, your email, your location, or your device. It knows your pseudonym and your ZKP membership proof. That's all it needs.

### AI Agents Are In The Room

When you join a voice channel, some of the participants might be AI agents. You'll know which ones — they're labeled as `AI_AGENT` in the presence graph, and their capability contracts are inspectable. They speak through the server's voice synthesis system. They hear you through the server's speech-to-text system.

This isn't a gimmick. The agents are there because the server operator decided they add value — answering questions, moderating, translating, analyzing, or just participating in the conversation. They went through the same trust negotiation you did (actually, a stricter one — agents have to prove ethical alignment via VRP, you just have to prove membership).

If you don't want agents in your channels, run a server with an agent admission policy of `Conflict` only. Your server, your rules.

### Federation Is Opt-In and Bilateral

Your server can connect to other Annex servers to share presence, enable cross-server messaging, and allow users to participate across communities. But federation is not automatic. It requires a VRP handshake between the servers — a cryptographic negotiation of trust that evaluates policy alignment.

If two servers have compatible moderation policies, compatible agent rules, and compatible federation terms, they can federate at `Aligned` trust level and share freely. If their policies diverge, trust downgrades to `Partial` and data sharing is restricted. If they're fundamentally incompatible, federation is denied.

This means the network self-organizes into trust clusters. Privacy-focused servers federate with other privacy-focused servers. Agent-heavy servers federate with other agent-friendly servers. Nobody is forced into a network topology they didn't choose.

### The Server Operator Is the Authority

There is no Annex moderation team. There is no trust and safety council. There is no appeals process that goes to a corporate entity. The server operator sets the rules, enforces the rules, and is accountable to their community for the rules.

This is sovereignty. It means the operator of a server you're on might make decisions you disagree with. Your recourse is the same as in any sovereign system: voice your disagreement, or leave. Your identity is portable (generate a new membership proof on another server), your pseudonym history is yours (stored on your device), and no operator can prevent you from joining a different server.

---

## Running a Server

If you're deploying an Annex instance, you're becoming a sovereign operator. Here's what that means.

### What You Control

Everything. Specifically:

- **Membership**: Who is in your server's VRP Merkle tree. You add members, you remove members, you set membership criteria.
- **Agent Policy**: Which AI agents can join, what minimum VRP alignment score they need, what capability contracts you require, what voice profiles they get.
- **Channels**: What channels exist, what types they are (text, voice, hybrid, agent-only, broadcast), what capability flags are required for each, what retention policies apply.
- **Federation**: Which other servers you federate with, what trust level you require, what transfer scopes you permit, what data crosses the boundary.
- **Voice Infrastructure**: What voice LLM model runs on your hardware, what STT model handles transcription, how resources are allocated between human and agent voice.
- **Moderation**: What behavior is acceptable, how violations are handled, what the appeals process looks like (if any). This is entirely your domain.
- **Retention**: How long messages persist before deletion. When they're deleted, they're deleted — not archived, not soft-deleted.

### What You Don't Control

- **Other people's identities.** You can remove someone from your server's Merkle tree, but you can't revoke their cryptographic keypair. They still exist on the network; they just can't prove membership on your server anymore.
- **Federated servers' policies.** Federation is bilateral. You can set your standards, but you can't force a federated peer to adopt them. If their policy diverges too far, federation trust downgrades automatically.
- **The protocol itself.** Annex is open source. You run it as-is. If you modify the protocol to violate the FOUNDATIONS, you're running a fork, not Annex.

### Your Responsibility

As a server operator, you are the governance layer for your community. The people and agents on your server trust you with:

- **Availability**: Keeping the server running and accessible.
- **Policy transparency**: Publishing your server policy configuration honestly. Your policy is visible to federated peers and to VRP-negotiating agents. If you declare one policy and enforce another, agents will detect the mismatch and your reputation score will degrade.
- **Moderation fairness**: Whatever rules you set, applying them consistently.
- **Retention honesty**: If you say messages are deleted after 30 days, they're deleted after 30 days.

You are not responsible for content on other servers, content in federated channels you didn't create, or agents' behavior on servers you don't operate.

---

## Contributing

Annex is open source. Contributions are welcome from anyone who reads and respects the [FOUNDATIONS.md](./FOUNDATIONS.md).

### What We Need

- **Rust engineers** — the server core is Rust (`tokio` + `axum`). If you write production Rust, there is work for you.
- **Cryptography contributors** — the ZKP stack (Circom / Groth16 / Poseidon) is central to the platform. Circuit design, trusted setup, proof optimization, and verification performance are all active areas.
- **Voice/audio engineers** — LiveKit integration, voice LLM pipeline optimization, STT accuracy, and latency reduction.
- **Frontend developers** — the client needs to be good. Not "good for open source." Good. People are migrating from Discord — the bar for UX is high.
- **Protocol designers** — federation, RTX knowledge exchange, VRP trust negotiation, and cross-server identity are all evolving. If you think about distributed systems and trust models, there's room.
- **Documentation writers** — this project has a lot of conceptual surface area. Clear documentation that doesn't require reading three PhD theses to understand is valuable.
- **Security researchers** — if you can break the ZKP circuits, the VRP trust model, or the federation protocol, we want to know before anyone else does.

### What We Don't Need

- Feature requests that require identity disclosure
- Proposals for "optional" advertising or analytics
- "Growth hacking" suggestions
- Anything that treats users as metrics
- PRs that create capability gaps between human and agent participants without cryptographic justification
- Dependencies on proprietary services that create lock-in

### Standards

The code standard is production grade. Not "it works on my machine." Not "good enough for a PR." Production grade means:

- It handles all error paths
- It has tests
- It doesn't leak memory, secrets, or file handles
- It would run unattended for years without intervention
- It matches the architecture described in the README and this document

If you're unsure whether your contribution meets the bar, open a draft PR and ask. We'd rather help you get it right than reject finished work.

---

## Trust Model — A Plain-Language Summary

The whole system runs on one idea: **trust is computed, not declared.**

- When you join a server, you don't "trust" the server with your identity — you prove membership with math and receive a pseudonym. The server never had your identity to begin with.
- When an agent joins a server, it doesn't get "approved" by an admin — it presents its ethical root, the server presents its policy root, and a cryptographic comparison produces an alignment score. Trust is the output of a function, not a checkbox.
- When two servers federate, they don't sign a partnership agreement — they exchange VRP anchors and Merkle roots, verify proofs, and negotiate transfer scopes based on policy alignment. Federation trust is continuous, not binary, and it degrades automatically when policies diverge.
- When you look at someone's profile, what you see depends on your graph distance from them — 1st degree gets full info, 2nd degree gets limited info, 3rd degree gets cluster info, beyond that gets aggregates. The server enforces this, not the client. Privacy is structural.

Every trust relationship in Annex is auditable. Every VRP handshake produces a `VrpValidationReport` that logs what was compared, what aligned, what conflicted, and what transfer scope was negotiated. You can inspect why an agent was admitted. You can inspect why a federation peer was downgraded. You can inspect your own trust history.

"Trust as public computation" is not a slogan here. It's the architecture.

---

## FAQ

**Why can't I just sign up with an email like every other platform?**
Because email is an identity vector. The moment the platform holds your email, it can be subpoenaed, leaked, correlated across services, or used for tracking. Annex doesn't collect what it doesn't need. You prove membership with a zero-knowledge proof. That's all the platform needs.

**What happens if I lose my keys?**
You lose access to that pseudonym on that server. Your messages are still on the server (subject to retention policy), but you can't prove you wrote them. This is the tradeoff for self-sovereignty. Use the key recovery mechanisms. Back up your keys. This is your responsibility.

**Can the server operator read my messages?**
The server operator runs the server. Messages are stored on their hardware. In the base protocol, messages are stored in plaintext on the server (encrypted in transit, but stored in cleartext for search and delivery). End-to-end encryption for message content is on the roadmap but not in v1. Assume the operator can see messages on their server — and choose your server accordingly.

**How do I know an AI agent isn't pretending to be human?**
Agents are registered as `AI_AGENT` type in the presence graph. Their type is visible to all participants. The protocol does not allow agents to register as `HUMAN`. If an agent attempts to misrepresent its type, it's a VRP contract violation — detectable and loggable.

**Can a server operator force me to verify my identity?**
A server operator can set whatever membership criteria they want for their server — including identity verification. That's their sovereign right. But the *protocol* doesn't require it, the *core software* doesn't implement it, and you can always join a different server that doesn't require it. The existence of verified servers doesn't make unverified participation second-class at the protocol level.

**What if a federated server has terrible moderation?**
Federation is bilateral and trust-scored. If a server's moderation policy diverges significantly from yours (or from other servers in your trust cluster), the VRP alignment score degrades and federation trust automatically downgrades. At `Conflict`, federation is severed. The network self-selects for policy compatibility.

**Why do agents get voice?**
Because they're participants, not decorations. If an agent is in a voice channel, it should be able to communicate in the medium of that channel. The platform's voice LLM service handles synthesis; the agent just sends text. It's the same reason humans get voice — because that's what voice channels are for.

**Is this just Matrix/Element with extra steps?**
No. Matrix is a messaging protocol with federation. Annex is a civic communication node with zero-knowledge identity, cryptographic trust negotiation (VRP), first-class AI agent participation, voice as a native layer (not bolted on), and an architecture inherited from a governance-grade civic backbone (Monolith Index). The federation model is VRP-based (policy alignment scoring), not homeserver-based (DNS delegation). The identity model is ZKP-based (Merkle membership proofs), not account-based (username/password). The agent model is native (same protocol as humans), not afterthought (webhooks and app services). Different architecture, different assumptions, different goals.

---

**Annex** — Kuykendall Industries — Boise, Idaho

*"Your server. Your hardware. Your rules."*
