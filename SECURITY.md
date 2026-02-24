# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Annex, **report it privately**. Do not open a public GitHub issue.

**Email:** contact@montgomerykuykendall.com

Include:

- Description of the vulnerability and its impact
- Steps to reproduce or a proof of concept
- Affected component (server, client, ZK circuits, federation, agent protocol)
- Your assessment of severity

We will:

1. Acknowledge receipt within **48 hours**
2. Provide an initial assessment and timeline within **7 business days**
3. Work with you on coordinated disclosure once a fix is available

## Scope

The following components are in scope for security reports:

| Component | Examples |
|-----------|----------|
| **Server** (`annex-server`) | Authentication bypass, SSRF, injection, memory safety, information disclosure |
| **ZK circuits** (`zk/circuits/`) | Soundness breaks, zero-knowledge property violations, proof forgery |
| **VRP trust negotiation** (`annex-vrp`) | Alignment bypass, contract enforcement failures, reputation manipulation |
| **Federation protocol** (`annex-federation`) | Signature forgery, attestation bypass, cross-server data leakage |
| **Agent connection protocol** | Capability escalation, alignment status spoofing, unauthorized channel access |
| **Client** (`client/`) | XSS, credential exposure, CSP bypass |
| **Cryptographic primitives** | Key handling, signing, Poseidon hashing, Merkle tree integrity |

## Out of Scope

- Social engineering attacks against users or operators
- Denial of service against live instances (resource exhaustion without a novel vector)
- Vulnerabilities in upstream dependencies without a working exploit against Annex
- Issues that require physical access to the host machine
- Missing security hardening that is already documented in [Known Limitations](release_v0.1.md#known-limitations)

## Security-Critical Surfaces

Annex includes several security-critical subsystems that carry higher risk than typical web application code:

- **Groth16 ZK proofs** — soundness and zero-knowledge properties are foundational to the identity plane
- **VRP trust negotiation** — alignment classification gates channel access and knowledge transfer
- **Federation signatures** — Ed25519 signed envelopes are the trust boundary between servers
- **Pseudonym derivation** — unlinkability across topics depends on correct nullifier scoping

Changes to these subsystems receive additional review scrutiny. See [CONTRIBUTING.md](CONTRIBUTING.md) for details.

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x | Yes |

This project is in developer preview. All reported vulnerabilities will be evaluated regardless of version.

---

**Kuykendall Industries LLC** — Boise, Idaho
