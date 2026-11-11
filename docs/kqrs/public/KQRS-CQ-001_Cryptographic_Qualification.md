---
id: public-kqrs-cq-001-cryptographic-qualification
title: 'KQRS-CQ-001: Cryptographic and Protocol Qualification'
type: operations
status: development
visibility: public
canonical_owner: Kuykendall Industries, LLC
parent_entity: Kuykendall Industries, LLC
verification_status: needs_verification
related_to: []
source_priority: repo
summary: Versioned KQRS document; specification and repository adoption are distinct
  from product conformance.
doc_path: docs/kqrs/public/KQRS-CQ-001_Cryptographic_Qualification.md
---

# KQRS-CQ-001: Cryptographic and Protocol Qualification

**Edition:** 0.4.1-draft  
**Owner:** Kuykendall Industries, LLC  
**Status:** Normative proposed engineering profile  
**Related controls:** KQRS-Q-01 through Q-10; P-02 through P-05; P-08; A-02; A-09; V-01 through V-04; V-07  
**Research cut:** November 11, 2026

## 1. Qualify the complete trust obligation

A cryptographic component qualifies for a particular statement, protocol, implementation, configuration and operating scope. Its name or mathematical family does not qualify the application that invokes it.

The assessment MUST maintain a **cryptographic obligation graph**. Each protected customer operation is a root. Its dependencies identify every key-establishment leg, encryption envelope, signer, issuer, certificate or credential, proof relation, verifier, trusted setup, transcript/hash construction, recovery path and software-delivery trust root that can determine that operation's confidentiality or authority.

Each node MUST record the property supplied; protected information and required lifetime; adversary and compromise assumptions; algorithm and parameters; implementation/build; trusted owner; key lifecycle; caller and enforcement sink; evidence; and change-invalidation rule. Each edge states what the dependent component assumes. An unqualified mandatory dependency keeps the parent claim unqualified.

Quantum confidentiality, quantum authentication, quantum proof soundness and quantum witness privacy MUST be separate claims. Their combination is a graph decision, not an algorithm count. A PQ transport or outer signature does not change the assumptions of a classical credential, commitment or inner proof.

NIST's finalized ML-KEM and ML-DSA standards supply algorithm definitions. KEM usage and system migration require additional protocol and lifecycle reasoning; the relevant primary references are FIPS 203, FIPS 204, SP 800-227 and CSWP 39 Update 1. [CQ-R1–CQ-R4]

## 2. Establish the relation before judging the proof system

A proof qualifies only if its enforced relation is the relation the relying party needs. Reviewers MUST write the statement in plain language, then map every clause to constrained variables, public inputs and verifier checks.

| Question | Required evidence |
|---|---|
| What is being proved? | Formal relation and a plain-language statement with identical subject, quantity, threshold, scope and time semantics |
| Who supplies the witness? | Witness origin, validation, confidentiality, permitted transformations and malicious-prover model |
| What is public? | Canonical public-input order/encoding and an independently supplied expected context |
| What is constrained? | Constraint-to-requirement mapping; range, comparison, membership, hash and domain-separation obligations |
| What action relies on it? | Verifier result connected to the exact authorized action, tenant, resource, audience and current authority |
| What establishes the root? | Authorized issuer/root/epoch and trusted setup/key provenance, including replacement and revocation |

Finite-field arithmetic has no implicit ordering. A constraint that merely equates an expression to itself cannot establish a nonnegative integer difference. Comparisons MUST include reviewed range/bit constraints or another appropriate proven construction. Host-language checks do not substitute for circuit constraints when the threat model permits a malicious prover.

The circuit's commitment relation and the witness generator's relation MUST agree. For example, a circuit comparing a private value directly with a public commitment cannot be treated as proving a hash commitment if the generator instead computes a hash outside the constrained relation.

Verification MUST compare verified public inputs with the relying party's independently required context. A separate JSON field named threshold, role, resource or epoch does not become authenticated because the same object also contains a valid proof.

The mandatory negative corpus includes unsatisfied witnesses; below-threshold and boundary values; integer overflow and field wraparound; substituted subject/root/resource; altered public input order; noncanonical encodings; wrong circuit/VK; malformed proofs; metadata/public-input disagreement; and valid proofs offered for a different action. Successful generation and verification of one honest example is insufficient.

## 3. Review the hybrid protocol as a protocol

Where a published protocol supplies the required composition, implementations MUST use its specified negotiation, encodings, key schedule and authentication behavior. RFC 10024 defines named PQ/traditional hybrid groups for TLS 1.3. Its construction and analysis belong to that TLS context; they are not a generic assurance result for arbitrary concatenation of secrets. [CQ-R5]

For any other protocol, a reviewed composition dossier MUST identify:

- Role identities, initiator/responder direction and the authenticated transcript.
- Algorithm/parameter negotiation, mandatory minimums and downgrade behavior.
- Every classical and PQ contribution, entropy source and reuse rule.
- Combiner/KDF inputs, domain separation, transcript binding and key-confirmation semantics.
- Invalid/low-order public-key behavior, malformed ciphertext behavior and failure handling.
- Replay state, sequence limits, nonce allocation, rekey, restart and crash recovery.
- Authentication of the intended recipient and ownership of the presented public keys.

Possessing an X25519 key pair and an ML-KEM key pair does not establish that both contributions protect the resulting session. Qualification MUST observe the actual protocol messages, selected suite and combined derivation in the tested implementation. Telemetry MUST reveal enough to verify the suite without exporting secrets.

Tests MUST include stripped/reordered negotiation; algorithm confusion; one missing contribution; cross-protocol transcript substitution; wrong host/tenant/recipient; repeated key/ciphertext; replayed confirmation; invalid public inputs; and failure during rotation or resume. Multi-language implementations MUST share independent known-answer/interoperability vectors and adversarial vectors, with canonical serialization pinned.

## 4. Separate introduction, carrier and content protection

A system may use a discovery service, sealed introduction, direct transport, authenticated inner session and message-level encryption. These are separate graph nodes even when one user gesture creates all of them.

The boundary declaration MUST state which component receives addresses, identity material, timing, sizes, ciphertext and plaintext. An introduction-only service MUST reject application-frame types and cannot silently become an application relay after direct connection failure. A content-blind relay MUST neither possess endpoint content keys nor receive accidental plaintext through headers, errors, diagnostics or fallback paths.

Qualification MUST verify the currently mounted route and its actual endpoint consumers. Retained helpers, historical protocol tests and a previous deployment's receipts cannot establish the current route's behavior. Connection renewal, broker/cache loss, queue expiration and reconnect MUST preserve the declared authority and confidentiality boundaries.

Where browser-delivered code controls endpoint keys, code delivery and update trust belong in the obligation graph. A server-resident inference participant that decrypts content is an authorized plaintext processor and must meet the COMPUTE obligations for that path.

## 5. Bind encrypted state to its meaning

An authenticated envelope MUST bind the context necessary to reject substitution: version, authority/tenant, resource, field or purpose, key/epoch, intended recipient/device and content type as applicable. Bindings may be supplied through authenticated associated data, a signed canonical structure or an equivalently reviewed construction.

Encryption success alone does not establish that the decrypted value belongs to the requested record. Tests MUST swap entire valid envelopes between records, fields, tenants, recipients and epochs. A cryptographically valid wrong-context envelope must fail before it changes authoritative state.

State updates spanning an epoch, recipient set, wrapped keys and manifest MUST be atomic or recover through a specified, testable state machine. Partial writes cannot leave a new epoch active without the required recipient/key state. Key-directory updates require authenticated authority; a server-supplied key cannot be trusted solely because it arrived over the current connection.

Unknown encryption state MUST fail closed for protected outgoing content. The client must distinguish “unencrypted by authorized policy,” “encrypted but awaiting authorized admission,” “trust/recovery action required,” and “state unavailable.” A failed lookup must not select a plaintext branch.

## 6. Make compromise and recovery properties explicit

The key-lifecycle matrix MUST analyze at least endpoint compromise, recipient private-key compromise, routing/issuer compromise, backup/recovery compromise, old-key restoration, device revocation, member departure and authorized recovery. For each event, state which historical and future records remain protected and why.

Fresh sender ephemeral material alone does not establish forward secrecy against later compromise of a static recipient secret. If a recorded envelope can be reopened using that later recipient secret, the corresponding historical confidentiality claim must reflect that fact. Retaining retired device secrets for readable history is a deliberate recovery design with a specific compromise cost.

Session forward secrecy, post-compromise recovery, membership rekeying and historical-data recovery are different properties. Qualification MUST test each claimed property with the actual retained ciphertext, secrets, backups and state. Key deletion claims include memory, persistent stores, replicas, caches, crash artifacts and recovery copies.

A design may support owner-authorized history retention or stronger erasure/ratcheting classes. It MUST describe and test the chosen behavior; neither class may borrow the other's guarantees. Recovery must not silently reintroduce revoked authority or concealed escrow.

## 7. Bind proof authority through offline operation

Offline validity is a protocol property. A live-request timestamp window must not be copied blindly to an intentionally delayed queue, and omitting that window does not remove the need for a defined authority lifetime.

An offline proof/credential policy MUST state the authorized issuer and root, tenant/resource/audience, issue and validity epochs, maximum revocation staleness, queue identity, action binding, replay/idempotency behavior and reconciliation rule. The receiver MUST apply the policy before authoritative effects.

A valid proof under an attacker-selected root is not an authorization from the intended issuer. The trusted issuer/root policy MUST exist independently of the proof package. Root rotation, quarantine, membership departure and revocation need test cases across the maximum supported offline delay, including a restart with old local state.

Where a user authorizes sustained operation, the system may renew or retain authority under that standing policy. A task must still prove current authority at each new consequential effect. Receipt observation and recovery of an already-issued operation are separate from permission to issue another operation.

## 8. Evaluate proof systems under their actual assumptions

The proof-system dossier MUST separately analyze completeness, soundness/knowledge soundness, zero knowledge, setup trust, commitment/hash assumptions, transcript transform, recursion/composition and implementation risks. Quantum analysis must cover every indispensable assumption and the chosen parameters.

Groth16 is a pairing-based construction; wrapping such a proof in a PQ signature does not replace its underlying proof assumptions. Research on Fiat–Shamir in the quantum random-oracle model also depends on the construction and the transform's hypotheses. A broad label such as “hash based,” “transparent” or “recursive” is insufficient for qualification. [CQ-R6–CQ-R7]

A migration decision may replace the proof, alter the authorization construction, or reduce disclosure through another design satisfying the same required property. Its comparison MUST preserve the intended privacy and authority guarantees. Proof size, proving/verification cost, setup and compatibility are measured together; an easier but weaker relation is not a valid performance baseline.

A proof model SHOULD cover the highest-risk protocol properties. The model dossier binds model/tool versions, assumptions, lemmas, proof or bounded-search result, counterexamples, abstraction gaps and implementation correspondence. Tamarin is one primary-source example of a tool supporting trace and observational-equivalence reasoning under an explicit symbolic model. A model result does not itself establish binary correctness or side-channel resistance. [CQ-R8]

## 9. Required qualification campaign

| Campaign | Required outcome |
|---|---|
| Relation and public-input challenge | Invalid statements and substituted context cannot authorize an action |
| Negotiation and downgrade challenge | Required components and identity/transcript bindings survive hostile negotiation |
| Envelope and manifest substitution | Cross-tenant/resource/device/epoch swaps fail before state changes |
| Compromise and recovery exercise | Observed historical/future exposure matches the registered property matrix |
| Offline authority and replay | Delayed valid work follows declared policy; foreign/stale/revoked authority and duplicate effects are rejected or reconciled correctly |
| Revocation during queue and retry | Stale local attempts cannot cross the authority fence or commit protected effects; external paths enforce their actual controllable cutoff and preserve truthful reconciliation of prior uncertain effects |
| Failure and resource exhaustion | Bounded parsing/work/queues and useful authorized throughput persist; failures do not trigger weaker protection |
| Interoperability and migration | Supported old/new clients follow authenticated transition policy without hidden fallback |
| Performance qualification | KQRS-LAT-001 applies to complete paths, minimum hardware and all required security work |
| Evidence and independent challenge | Frozen subject/harness/config, protected raw artifacts and declared evaluator separation support the result |

Security failures are hard failures for their affected claims. They cannot be traded against a faster mean, higher aggregate score or more passing unrelated tests.

## 10. Evidence, release and change invalidation

The record MUST bind source/build/configuration, compiled dependency versions, protocol/circuit/key artifacts, trusted roots, test vectors, environment, failure corpus, raw results and evaluator identity. A Git submodule, vendored copy and connected upstream repository may have different revisions; the compiled dependency is the subject.

Changes to constraints, public input layout, key/certificate format, transcript, negotiation, cryptographic library, setup, trust root, retention/recovery or effect authorization invalidate the relevant dependent evidence until impact review and targeted requalification complete. Historical failures and corrections remain available.

The implementation inventory MUST identify existing canonical mechanisms before new work is proposed. An upgrade names the current owner/module and the missing invariant; it preserves compatible working controls and avoids parallel trust or evidence stores.

## Primary references

- **CQ-R1 — NIST FIPS 203**, *Module-Lattice-Based Key-Encapsulation Mechanism Standard*, final August 13, 2024. The official page carries a November 17, 2025 potential-errata planning note. Assessment records must identify reviewed errata: https://csrc.nist.gov/pubs/fips/203/final .
- **CQ-R2 — NIST FIPS 204**, *Module-Lattice-Based Digital Signature Standard*, final August 13, 2024. The official page carries a July 31, 2026 potential-errata planning note: https://csrc.nist.gov/pubs/fips/204/final .
- **CQ-R3 — NIST SP 800-227**, *Recommendations for Key-Encapsulation Mechanisms*, final September 18, 2025: https://csrc.nist.gov/pubs/sp/800/227/final .
- **CQ-R4 — NIST CSWP 39 Update 1**, *Considerations for Achieving Crypto Agility*, updated final June 29, 2026: https://csrc.nist.gov/pubs/cswp/39/upd1/considerations-for-achieving-crypto-agility/final .
- **CQ-R5 — IETF RFC 10024**, *Post-Quantum Traditional (PQ/T) Hybrid Key Agreement Mechanisms for TLS 1.3*, Standards Track, August 2026: https://www.rfc-editor.org/rfc/rfc10024.html .
- **CQ-R6 — Jens Groth**, *On the Size of Pairing-based Non-interactive Arguments*, EUROCRYPT 2016: https://doi.org/10.1007/978-3-662-49896-5_11 .
- **CQ-R7 — Don, Fehr, Majenz and Schaffner**, *Security of the Fiat–Shamir Transformation in the Quantum Random-Oracle Model*, CRYPTO 2019, author repository: https://ir.cwi.nl/pub/28973/ .
- **CQ-R8 — Tamarin Prover**, official manual, introduction and property model: https://tamarin-prover.com/manual/master/book/001_introduction.html .
- **CQ-R9 — NIST IR 8547**, *Transition to Post-Quantum Cryptography Standards*, final October 27, 2026: https://csrc.nist.gov/pubs/ir/8547/final .

