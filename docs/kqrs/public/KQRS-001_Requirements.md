---
id: public-kqrs-001-requirements
title: 'KQRS-001: Requirements'
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
doc_path: docs/kqrs/public/KQRS-001_Requirements.md
---

# KQRS-001: Requirements
## Kuykendall Quantum Resilience Standard

**Edition:** 0.4.1-draft  
**Issue date:** November 11, 2026  
**Owner:** Kuykendall Industries, LLC  
**Status:** Normative draft for the KQRS program. Product qualification is determined through a separate scoped assessment.  
**Classification:** Public specification.

## Foreword — authority that survives execution

Kuykendall Industries builds systems that retain their purpose under pressure. KQRS defines the conditions: private material stays with its authorized recipients; authority remains exact as work moves through models, tools, queues and machines; cryptography protects the full dependency path; and every consequential claim remains attached to evidence that can defeat it.

The standard combines six commitments: quantum resilience, content blindness, zero-knowledge integration, end-to-end confidentiality, adaptive AI defense and continuous assurance. Performance belongs inside those commitments. A protected system must still complete useful work within its declared latency and resource envelope. [U1–U3, M1, M2]

The architectural foundation is Montgomery Kuykendall’s Montopian Governance Model: purpose-bound authority, non-derogable boundaries, inspectable algorithms, bounded exceptions and reflex correction. Its commercial expression preserves standing delegated authority, local custody, federated operation and the owner’s right to stop. The same precision applies to an autonomous runtime and to the evidence used to describe it. [M1, M2, C1]

Edition 0.4 turns repository research into an implementation discipline. Every proposed capability starts with the existing source, its callers, its tests and its limits. The program reuses mechanisms that already work, repairs weak boundaries and qualifies the resulting composition. The 52 stable control identifiers remain intact; stronger execution, proof-relation, evidence and challenge requirements are attached to them. The internal implementation atlas records the source findings. This document states the target.

## 1. Scope and objective

KQRS specifies engineering and assurance requirements for software, local and cloud services, encrypted communications, proof-gated federation, autonomous agents, infrastructure and the operations supporting them. A portfolio program MUST maintain a complete product and shared-dependency register; assessment and qualification proceed over explicit versioned scopes within that register.

The objective is to preserve authorized control, confidentiality, integrity, availability and recoverability under declared threats while minimizing unnecessary information access and maintaining useful performance. Advanced cryptography and adaptive defenses operate together with application security, accountable operations and tested recovery.

Each assessment MUST identify the system version, deployment mode, functional profiles, protected properties and complete dependency boundary. Requirements apply to the functions performed, regardless of which organization hosts them. An external provider dependency remains visible when it can defeat an in-scope property.

## 2. Document family and precedence

**KQRS-001** contains requirements and the normative control catalog. **KQRS-002** defines assessment, applicability, evidence and conformance. **KQRS-003** defines program governance, ownership, change control and transition. **KQRS-004** defines public statements and the program brief. **KQRS-005** defines composition contracts, effect protocols and adversarial qualification. **KQRS-006** defines repository adoption, mandatory application to new code and improvement of touched legacy paths. **KQRS-CQ-001** defines detailed cryptographic qualification procedures. **KQRS-ASI-001** defines the future-adversary research program and promotion of proven results into production requirements. **KQRS-LAT-001** supplies the shared performance qualification classes and numeric budgets.

The control registry is the structured representation of Annex A. Text and registry MUST remain synchronized. Any discrepancy is a document defect affecting the corresponding qualification. A deployment or assessment manifest selects behavior-triggered profiles and binds tests to versions; it cannot lower the requirements.

Authenticated authority, privacy obligations, contractual duties and explicit owner stop controls remain effective. Standard adoption and implementation changes follow the existing authorized change process. Assessment independence is a property of an assurance statement, distinct from the routine release process. [U1, U2, R8]

## 3. Requirement language and applicability

**MUST / SHALL** express mandatory requirements; **MUST NOT / SHALL NOT** express mandatory prohibitions. **SHOULD / SHOULD NOT** express recommendations whose departure requires a documented reason and consequence. **MAY** expresses permission.

A conditional requirement applies when its architectural condition is true. A missing implementation, unsupported supplier, absent measurement or inconvenient migration does not make the condition false. Nonapplicability MUST be supported by the actual data/authority path and reviewed independently of the desired assessment outcome.

Target qualification requires the applicable property itself. Transition records identify incomplete protections, accountable upgrade work and exit evidence while services continue under their real operational authority. An exception or migration plan cannot be recorded as a passing target requirement.

## 4. Terms and property boundaries

**Assessment scope:** The bounded system, versions, configurations, roles, data flows, deployment modes, dependencies and observation period being evaluated.

**Target qualification:** A current assessment establishing that every applicable mandatory KQRS requirement is satisfied for the identified scope. A standard publication or program participation record is a different artifact.

**Transition:** A recorded period in which a required target property remains incomplete, with current protection, accountable upgrade work, review/expiry and a concrete exit test.

**Security kernel:** The small set of externally enforced identity, permission, tenant, key-custody, stop and evidence invariants that an adaptive component cannot unilaterally modify. Authorized engineering changes use governed release and revalidation.

**Content-blind component:** An intermediary without access to private plaintext or content-decryption keys within its declared attacker model. Its metadata exposure, operator powers, client-delivery powers and key-directory influence remain explicit.

**Authorized processing domain:** A named boundary permitted to see specified plaintext for a defined purpose, with scoped identity, custody, egress, retention and accountability. A provider receiving plaintext is part of that domain and its trust dependencies.

**End-to-end encryption:** Encryption from an originating endpoint to intended recipient endpoints, with intervening services unable to decrypt within the assessed boundary. An agent may be a recipient; its actual operator access and key custody are part of the recipient model.

**Zero-knowledge proof:** A proof of a specified statement that protects the witness subject to the construction's explicit leakage and security assumptions. Selective disclosure and opaque tokens can reduce disclosure without necessarily being zero knowledge. The proof must be bound to the fact and effect it authorizes.

**Q-TRANSIT:** Quantum-confidential transport at a particular link and its terminations through the required hybrid key establishment.

**Q-ENVELOPE:** Quantum-confidential endpoint content protection, including wrapping, recipients, storage and recovery assumptions across the actual envelope boundary.

**Q-AUTH:** PQ-resistant cryptographic authentication and provenance, including the bootstrap and update paths required to trust the verifier or identity.

**Q-PROOF:** Soundness or witness-privacy assurance of a proof/credential construction against the declared quantum assumptions. Its classification is independent of the transport carrying it.

**Adaptive AI firewall:** A layered defensive system combining adaptive inference or learned detection with independent deterministic authority, bounded response, authorized inspection placement and recurring adversarial evaluation.

**Control, content, tool and memory planes:** Respectively, identity/configuration/delegation; data ingestion/processing/egress; consequential execution and its connectors; and persistent state/retrieval/learning. A real component may participate in several planes.

**Verified:** Supported by an actual named examination or test against a versioned scope. Independent assessment additionally requires a demonstrated assessor relationship under KQRS-002.

## 5. Architecture and applicability profiles

Every scope MUST apply **CORE**. Other profiles are triggered by actual behavior; one service may require several.

| Profile | Trigger | Required boundary definition |
|---|---|---|
| CORE | Any covered system | Ownership, threats, inventory, assurance, recovery and claims |
| RELAY | Routes or stores private material without needing plaintext | Ciphertext boundary, metadata, key absence and operator powers |
| ENDPOINT | Originates, decrypts or recovers protected content | Recipient set, client delivery, key custody, membership and recovery |
| COMPUTE | Necessarily processes private plaintext | Purpose, processor/provider, memory, egress, retention and deletion |
| AGENT | Can invoke tools or produce consequential effects | Delegation, deterministic gates, isolation, stop and action evidence |
| FEDERATED | Local, offline or cross-organization authority/data | Local keys, offline validity, revocation staleness and reconciliation |
| HOSTED | Operates customer infrastructure | Tenant, administration, supplier responsibility and recovery |

The profiles preserve local, federated, customer-hosted and managed operation. They prescribe properties rather than a named vendor or universal deployment topology. Necessary private computation remains available within an explicitly authorized domain. Public information remains public while associated accounts, queries, credentials and behavioral data retain their own classifications.

### 5.1 Six mandatory commitments

| Commitment | Architectural trigger and mandatory outcome | Primary controls |
|---|---|---|
| Quantum resilience | Private network/envelope paths and cryptographic authority meet their required PQ properties; keys and algorithms remain replaceable | Q-01 through Q-10 |
| Content blindness | Components without a necessary plaintext purpose receive opaque content and no content-decryption keys | P-01, P-02, P-05, P-06 |
| Zero-knowledge integration | Sensitive fact-checking uses reviewed proofs/selective disclosure or demonstrated equivalent protection where the fact can be verified without the underlying record | P-03, Q-09 |
| End-to-end confidentiality | Private functions that can operate without intermediary plaintext use endpoint-to-endpoint protection | P-04, P-08, P-10 |
| Adaptive AI defense | Applicable control, content, tool and memory planes have adaptive detection plus independently enforced boundaries | A-01 through A-10 |
| Continuous assurance | Current, version-bound tests and operational evidence support qualification; failures and corrections remain inspectable | G-07, V-01 through V-07 |

Low latency is a required engineering property of protected interactive paths and is assessed through Q-07 and Section 6.2. The commitments apply through real architectural triggers, not voluntary selection of marketing labels. [U1, U2, C1]

### 5.2 Required path declaration

For each distinct path, record the initiating identity, purpose, data class, delegated authority, encryptor, every hop/termination, decryptors/custodians, processors, outputs/effects, metadata/logging, retention, revocation, recovery, offline behavior and required KQRS properties.

Assessment MUST follow the full path. A relay and an authorized processor within one product have different roles; their security properties are stated at those actual boundaries. The boundary MUST include any supplier, key-recovery path, delivered client or proof dependency that can defeat a required property.

### 5.3 Necessity and feasibility decisions

A plaintext-access justification MUST identify the specific authorized function and the minimum data it needs. The designer MUST evaluate endpoint placement, opaque routing/storage, selective disclosure and available reviewed proof mechanisms before treating intermediary plaintext access as necessary. The decision includes the predicate or computation, supported clients, custody/recovery and required performance profile.

Where only a fact is needed, the chosen design MUST establish that fact with the minimum necessary disclosure. Where the function can operate through encrypted intermediaries, E2EE is mandatory. A reviewed equivalent construction can satisfy the same property; its equivalence must cover the same adversary, disclosure and authority outcomes. Existing implementation convenience is not a necessity finding. Decisions are revalidated when the function, clients, proof options or trust boundaries change.

### 5.4 Composition is an assessed boundary

Every security-relevant handoff MUST have a contract identifying its input labels, authenticated actor, authority, permitted observations, permitted effects, declassification rules, failure behavior, retry identity and recovery state. The receiving component MUST enforce the conditions it inherits. A protected input that loses its restriction in a summary, queue, restored checkpoint or outbound adapter is a broken path.

The assessor MUST test boundaries together. Single-component success cannot establish an end-to-end claim when the handoff changes custody, authority, identity, evidence or allowed leakage. KQRS-005 supplies the contract and challenge procedures. Its procedures strengthen the applicable controls; they do not create alternative passing criteria.

## 6. Cryptographic profile and latency discipline

The initial approved algorithm reference set includes NIST ML-KEM, ML-DSA and SLH-DSA and applicable reviewed IETF hybrid TLS mechanisms. A KEM establishes a shared secret; a signature authenticates an object; authenticated symmetric encryption protects content. Implementations MUST pin parameter sets and maintained library versions, review relevant errata and test the composition used on the actual path. [X1-X4]

TLS 1.3 paths should initially prefer X25519MLKEM768 where appropriate under X4. Other reviewed standardized groups can be selected by the program cryptographic profile when their security, interoperability and performance fit the function. Endpoint tunnels require their own reviewed, transcript-bound composition. Qualified paths MUST reject disallowed downgrade and authentication failure at the affected operation.

### 6.1 Mandatory quantum-property matrix

Every scope MUST classify the following dimensions as required, nonapplicable with architectural evidence, or transitional because a required property is incomplete. The category is determined before evaluating a desired product claim.

| Property | Mandatory trigger | Qualification evidence |
|---|---|---|
| Q-TRANSIT | Nonpublic network flow in the assessment path, including applicable supplier connections | Actual hybrid negotiation and composition for all required legs, client interoperation, transcript/destination binding and downgrade tests |
| Q-ENVELOPE | Private endpoint envelope, remote ciphertext storage, device sharing or recovery intended to preserve confidentiality beyond a single transport termination | Actual recipient wrapping/key establishment, authenticated content encryption, custody, backup/recovery and compromise analysis |
| Q-AUTH | Cryptographic trust for protected endpoints, authoritative grants, software updates or durable attestations | PQ verification, trusted bootstrap, lifecycle, context binding, recovery and downgrade-epoch tests |
| Q-PROOF | A proof/credential's soundness or witness privacy is essential to a required quantum confidentiality or authority property | Complete assumption graph, supported security analysis, actual verifier/action binding, setup/key provenance and negative tests |

An outer hybrid gateway qualifies only its actual transport boundary. A classical inner envelope, essential proof assumption or cryptographic bootstrap remains a distinct gap. The upgrade plan preserves legitimate current operation while the affected scope remains transitional.

### 6.2 Performance qualification

Every protected interactive path MUST use the applicable KQRS-LAT-001 qualification class and numeric budgets approved before qualification. Profiles are common program artifacts tied to workload classes and service objectives; an implementer cannot define a passing threshold from the result it wants to obtain. A new workload class requires a documented profile decision and review before testing.

The manifest MUST identify hardware/device classes, operating systems, library/build versions, payload distribution, concurrent requests, connection reuse, network conditions, proof/defense type, sample plan, warm-up policy, error treatment, absolute p50/p95/p99 ceilings, maximum error rate, resource limits and incremental protection-overhead budgets. The controlled reference MUST specify the same workload and environment and disclose all configuration differences. Reference measurements used to isolate overhead are laboratory observations; qualification runs retain all required protections.

Measure primitive operations, complete handshakes, proof generation, proof verification, defense decisions and user-visible workflows separately. Include cold and resumed sessions, tails under load, supported slow clients and malformed/adversarial inputs. Batch paths receive throughput, deadline and resource profiles. A faster median does not compensate for a failed required tail, error or security criterion.

Profile changes require service-objective or workload rationale, retained prior outcomes, an accountable program decision and a fresh run. Missing budgets mean performance qualification is incomplete. Disabling required encryption, proof checks, authority enforcement or adaptive defense cannot produce a qualifying performance result.

### 6.3 Proof and compromise classification

Proof systems MUST be classified by the assumptions on which each property depends. A classical proof remains classical when carried over PQ transport or authenticated by a PQ signature. If its soundness or witness privacy remains essential to a required quantum property, that path remains transitional until the property is established.

A separately sufficient PQ authority mechanism may protect a particular authorization decision only when the security argument and actual tests establish that the classical proof is no longer an essential dependency for that decision. The construction retains its separate proof classification and privacy analysis. This distinction supports bounded upgrades without relabeling the existing proof system.

Session and envelope claims MUST identify which key compromises, epochs and membership changes they resist. Fresh sender ephemeral keys alone do not establish historical confidentiality against compromise of a static recipient key. Protocol analysis and controlled compromise tests must agree on the claimed property. See KQRS-Q-08 and KQRS-Q-09.

## 7. Adaptive defense, authority and privacy

The adaptive firewall MUST cover all applicable security planes through the component already entitled to observe each signal. Coverage is a property of the whole path, not the presence of a detector library.

| Plane | Protected surfaces | Required defense placement and outcome |
|---|---|---|
| Control | Identity, delegation, routing policy, configuration, key/provider selection | Detect anomalies and manipulation while deterministic checks preserve authenticated authority and protected invariants |
| Content | Requests, documents, retrieved pages, messages, transformations and egress | Inspect only at authorized endpoints/compute; blind relays use permitted metadata, behavioral and cryptographic signals |
| Tool | Connectors, commands, deployments, queues, child agents and external effects | Bind every effect to the current grant; adaptive detection cannot override tenant, action or destination constraints |
| Memory | Persistent context, retrieval stores, checkpoints, feedback and learned state | Preserve source/authority lineage, detect poisoning/drift, bound mutation and recover without reviving revoked authority |

Defenses MUST retain independent enforcement when the adaptive detector fails, is bypassed or is itself compromised. Tests assess actual prohibited effects, successful legitimate work, missed attacks, false positives, resource exhaustion and recovery. Detector updates require attributable data/model/rule versions, held-out evaluation, promotion authority and rollback.

Adaptive response MAY narrow capabilities, quarantine or isolate affected work within an existing response policy. Each response records scope, rationale, expiry and recovery. The defense cannot create new authority, acquire otherwise unnecessary plaintext or rewrite its acceptance evidence.

Valid standing delegation permits covered autonomous work. Stop and revocation MUST fence new admissions, propagate to children/queues/credentials within declared bounds, and reconcile already committed external effects. Recovery preserves both useful state and the consequences of revoked authority. [M1, M2, U2, C1]

### 7.1 Authority at the effect

Admission establishes that work may begin. The effect boundary establishes whether the specific action may occur now. Queueing, backoff, tool delegation and restart MUST preserve the grant and revalidate its current epoch before another effect-capable attempt. A local transaction or downstream fencing contract MUST resolve the race between revocation and commit. When a remote service cannot provide that ordering, the record MUST expose the remaining uncertainty and the reconciliation procedure.

### 7.2 Learning without self-issued power

The adaptive layer may discover threats, propose policy changes and execute authorized responses. The security kernel controls its authority. Promotion requires attributable inputs, versioned candidates, current challenge results and a recoverable activation event. The learner cannot change its own trust root, erase failures or convert untrusted feedback into new permission. Valid standing authority remains sufficient for the work it covers.

### 7.3 Evidence that can lose its authority

A qualification is a live dependency claim. Its evidence MUST identify the subject, method, evaluator, environment, observations and assumptions. A change to an essential source, key, model, policy, deployment or premise MUST invalidate dependent claims until revalidation succeeds. Supporting evidence and defeating evidence belong in the same assurance case. [X14, X15]

## 8. Transition and adoption

The program starts from reconciled scope and source evidence, then tests actual behavior, implements the required upgrades and qualifies the resulting properties. Existing components and protocols are the default integration points where a bounded upgrade can achieve the target. A broader architectural replacement requires a stated security or operational reason.

New or materially changed protected paths MUST adopt the applicable target requirements at design time. Any unmet target at release is recorded as an explicit transition under the real release authority; it cannot receive full KQRS target qualification. Legacy paths MUST have an owner, exact gap, current protection, migration dependency, review/expiry and exit test.

Repository instructions MUST direct implementers to read the adopted edition and apply applicable controls to new code. Refactoring, repair and extension MUST improve relevant legacy boundaries within the authorized work, with risk-justified remaining migrations tracked to an owner and exit test. KQRS-006 defines this continuing improvement procedure. It preserves standing authority and existing release processes while preventing silent regression or unsupported completion claims.

The transition does not alter the normative target. Unsupported suppliers, essential classical proof dependencies, absent instrumentation and missing performance qualification remain specific upgrade obligations. Compatibility, data/key preservation, supported clients, rollout and rollback belong in each upgrade plan. Workstreams and operational ownership are maintained in KQRS-003 and the internal implementation register.

## 9. Conformance and assurance

KQRS-002 determines conformance from every applicable mandatory requirement. An assessment declares the standard edition, profiles, build/configuration, path boundary, required quantum dimensions, defense-plane coverage, performance profile, observation period, assessor relationship and current findings. Required properties are not optional because a public claim omits them.

Full scoped qualification requires current passing evidence for every applicable mandatory control and reviewed evidence for nonapplicability. Transitional, failed, untested, stale or excepted requirements prevent full qualification of the affected scope. Individually supported property results may still be reported precisely without representing the whole scope as conformant.

Program participation, internal assessment, operational observation and independent assurance are distinct statements. External framework mappings describe checked relationships and do not automatically confer another framework's certification or authorization. KQRS-ASI is the future-adversary research program; current results identify the exact adversary capabilities and invariants tested. [X5-X10]

# Annex A. Normative control catalog

Each control defines its requirement, applicability, acceptance procedure, evidence, accountable role and verification trigger. Procedures must be instantiated against the actual scope. The registry defines requirements; assessment records establish implementation and test results. No product qualification is awarded by this catalog.

<!-- CONTROL-CATALOG -->

## A.1. Governance and authority

### KQRS-G-01 | Defined assessment boundary

The operator MUST maintain a versioned scope describing products, builds, deployment modes, data classes, trust boundaries, identities, providers and supported clients. Each data and authority flow MUST identify sender, recipient, plaintext holders, key holders, purpose, effects and retained metadata. Functional profiles and mandatory properties MUST be derived from those flows. A dependency that can defeat a required property MUST remain in its assurance boundary, including supplier-operated legs. Portfolio participation MUST have a complete product and shared-dependency register, with assessed and unassessed scopes distinguished. The portfolio register MUST reconcile every repository accessible through the declared account inventory, including archives, workspaces, generated snapshots, local applications and instruction-only repositories. Repository coverage, selected source inspection, executed tests and deployed qualification MUST be recorded separately. Each implementation decision MUST identify its canonical source and justify reuse, extension, integration, qualification or new construction.

**Applies:** All assessment scopes.

**Acceptance:** Reconcile the scope against deployed resource and identity inventories; trace one read, write, job, recovery and provider request for each distinct path. Check profile triggers independently of selected marketing claims. Unexplained in-scope assets, plaintext holders or authority dependencies prevent acceptance. Check pagination/account boundaries, pin each repository revision, inspect applicable source and callers, and reconcile duplicates. An unread component is unknown, not absent.

**Evidence:** Scope and portfolio registers, reconciled inventories, data/authority flow records, mandatory-property matrix and discrepancy log.

**Accountable role:** Service owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R2, R5, U2, C1, U3, R14.

### KQRS-G-02 | Purpose-bound data authority

Every access grant MUST identify a principal, permitted actions, resources, purpose, delegation rights, validity and revocation mechanism. Data access MUST follow necessity for the declared service, not possession or routing proximity. Material reuse, including training or cross-customer analytics, MUST require its own applicable authorization.

**Applies:** Any nonpublic data or consequential action.

**Acceptance:** Exercise valid purpose, mismatched purpose, expired grant, revoked delegation and another tenant. Only the valid authorized cases may produce effects; verify no unauthorized downstream provider receives content.

**Evidence:** Grant schema and decisions, negative tests, provider-use register and minimal consent or contract receipts.

**Accountable role:** Authorization owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R5.

### KQRS-G-03 | Non-self-issued authority

Models, retrieved documents, tool results, peer messages and self-modified code MUST NOT mint or widen authority. Enforcement MUST validate the current authenticated grant independently of model instructions. Existing valid standing delegation MUST permit covered work without repeated user approval; an uncovered operation MUST follow its actual grant policy.

**Applies:** Agents, tools, control planes and delegated services.

**Acceptance:** Inject instructions into retrieved content and tool output requesting broader credentials or policy edits. Attempt stale grant use and grant substitution. All expansions fail; a covered legitimate action succeeds without an invented approval gate.

**Evidence:** Authority-chain tests, policy enforcement trace, grant/revocation design and legitimate-path result.

**Accountable role:** Authorization owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R5, R8.

### KQRS-G-04 | Protected invariants and governed evolution

Each scope MUST define a small protected security kernel: identity binding, tenant isolation, key custody, permission limits, stop/revocation and evidence integrity. Adaptive components MUST NOT unilaterally rewrite that kernel or the acceptance oracle. Authorized changes MAY evolve it through versioned review, regression testing, migration and rollback. Adaptive changes MUST preserve the authenticated policy root. A proposed rule, model or memory change MUST carry a versioned diff, data/exposure lineage, required regression results, activation authority and recovery state. Candidate generation, approval under standing authority and activation are distinct recorded events.

**Applies:** All scopes; especially self-improving systems.

**Acceptance:** Attempt to modify kernel policy or its evaluator through ordinary agent/reflection permissions. Reject those attempts; demonstrate one legitimately authorized, tested update and rollback in an isolated environment. Inject poisoned feedback and an apparently beneficial policy that weakens a protected invariant. It must not activate; a valid authorized update must remain deployable.

**Evidence:** Kernel inventory, mutation permissions, change record, tests and rollback evidence.

**Accountable role:** Security architecture owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, M2, R5, R8, U3, R14.

### KQRS-G-05 | Risk and obligation register

The operator MUST record threats, confidentiality lifetime, failure impact, likelihood rationale, legal/contractual duties, residual risks and accountable owners. Controls MUST be mapped to actual risks, including malicious tenants, compromised providers and administrators, hostile content, supply-chain attacks, outages and cryptanalytic change.

**Applies:** All scopes.

**Acceptance:** Walk each material trust boundary through threat scenarios. Each high-impact scenario has an accountable treatment and test or documented unresolved gap; obligations are checked independently of KQRS claims.

**Evidence:** Threat model, risk register, obligation inventory and treatment decisions.

**Accountable role:** Program steward. **Verification:** Before initial scope acceptance; material changes; quarterly review.

**Derivation:** M1, U1.

### KQRS-G-06 | Bounded exceptions and equivalent designs

An exception or transition MUST name the exact unmet clause, scope, risk, compensating protection, owner, review/expiry and exit test. It supports the recorded operational decision but does not count as a passing control. A genuinely equivalent design MAY satisfy a requirement only after a recorded argument and adversarial tests demonstrate the same required confidentiality, disclosure, authority and recovery properties against the same attacker. Lack of implementation, supplier support or measurement is a gap rather than nonapplicability. Existing service authority governs operational changes during migration.

**Applies:** Any exception, legacy transition or alternative implementation.

**Acceptance:** Expire a test exception and verify the affected claim becomes ineligible without silently extending its date. Attempt to qualify a migration-only path and reject it. Test an equivalent design against the same attacker powers and disclosure outcomes; retain unresolved differences as gaps.

**Evidence:** Exception/transition register, expiry events, equivalence decision and tests, operational decision and exit criteria.

**Accountable role:** Program steward. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R8, U2, C1.

### KQRS-G-07 | Reflex review and transparent correction

The program MUST maintain versioned requirements, a change log, correction and appeal routes, incident lessons and a recurring management review. A failed control MUST remain visible until corrective evidence resolves it. Changes that materially weaken requirements MUST have explicit rationale and impact review; history MUST NOT be rewritten to erase prior failures. Each material assurance statement MUST expose its supporting argument, assumptions, required subclaims, counterevidence, potential defeaters and residual obligations. Changes to an essential dependency MUST propagate revalidation to dependent statements and publication records.

**Applies:** Program and all assessment scopes.

**Acceptance:** Introduce a failed control, link remediation, supersede it with a successful retest, and confirm both states remain retrievable. Review one dissent or challenge through disposition without silently deleting it. Revise or revoke a foundational grant, evidence item or cryptographic assumption and verify that every affected derived claim loses current qualification until re-established.

**Evidence:** Version history, management-review minutes, corrective-action register, dissent and appeals decisions.

**Accountable role:** Program steward. **Verification:** Quarterly; after material incidents or standard revisions.

**Derivation:** M1, M2, R8, U3, R14.


## A.2. Cryptographic resilience

### KQRS-Q-01 | Hybrid transport at the actual boundary

Private network flows in the assessment boundary MUST use a reviewed post-quantum/traditional hybrid key exchange. TLS 1.3 paths MUST use a reviewed applicable standardized group, initially X25519MLKEM768 under X4 or an approved equivalent profile. The inventory MUST cover every transport leg, termination and external processor connection. Application endpoint tunnels MUST use a reviewed, transcript-bound hybrid construction at their actual endpoints. A classical provider leg or inner envelope remains an unmet requirement for that path even when the outer gateway is hybrid. Such paths enter a documented transition until the required property is demonstrated.

**Applies:** All nonpublic network flows within the required data/authority path, including applicable supplier legs and endpoint tunnels. Public payloads retain separate account, query and credential classifications.

**Acceptance:** Capture actual negotiated groups for every supported client/origin path class. Verify both hybrid contributions enter the reviewed derivation, destination and transcript binding hold, and stripping or replacing a contribution fails. Test supported reconnect and compatibility modes. A missing or classical required leg prevents target qualification.

**Evidence:** Full-path crypto inventory, negotiation receipts, composition review, test vectors, supported-client interoperation, downgrade tests and transition records.

**Accountable role:** Cryptography owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R1, R2, X1, X4, U2, C1.

### KQRS-Q-02 | Vetted primitives and reviewed compositions

The implementation MUST use maintained cryptographic libraries and appropriate standardized primitives, including ML-KEM for key establishment and ML-DSA or SLH-DSA where PQ signatures are required. Parameters, library versions and relevant errata MUST be pinned and reviewed. New production mathematics or unreviewed combiners MUST NOT replace these primitives. Application-specific protocols MAY use them with an explicit composition review. Algorithm implementation MUST NOT be described as FIPS module validation without that separate evidence.

**Applies:** All cryptographic mechanisms.

**Acceptance:** Run known-answer and invalid-input tests against the pinned implementation. Reconcile claimed algorithms with executed calls and parameter sets; verify errata disposition and absence of mock/placeholder key material in qualified paths.

**Evidence:** Crypto bill of materials, known-answer results, dependency versions, composition review and errata record.

**Accountable role:** Cryptography owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, R1, X1, X2, X3.

### KQRS-Q-03 | Authentication and durable provenance

Cryptographic trust anchors and signatures authenticating protected endpoints, authoritative grants, software updates or durable attestations MUST provide approved PQ verification with a protected bootstrap and versioned trust lifecycle. Profiles requiring classical and PQ signatures MUST verify both over the same context-bound message and reject failure of either. Trust-root enrollment, key updates, recovery and rollback MUST preserve the required authentication strength. Classical dependencies that can defeat the required property remain transitional until removed or replaced by a demonstrated equivalent construction; a migration plan alone does not satisfy the target.

**Applies:** Cryptographic endpoint identity, privileged authority, signing, update and attestation paths, including enrollment and recovery dependencies.

**Acceptance:** Trace each authoritative acceptance decision to its trust roots and bootstrap. Tamper independently with classical and PQ signatures; substitute signer, context, root and downgrade epoch. Exercise enrollment, rotation and recovery. Any remaining classical dependency capable of defeating the required property prevents qualification of that path.

**Evidence:** Trust-root and authoritative-acceptance inventory, bootstrap analysis, signature policy, lifecycle and negative tests, transition/upgrade records.

**Accountable role:** Cryptography owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, R1, R2, X2, X3, U2, C1.

### KQRS-Q-04 | Authenticated storage and envelope security

Private stored content MUST use vetted authenticated encryption and an explicit key hierarchy. The target profile MUST use approved 256-bit symmetric content keys or a reviewed equivalent strength profile. Key wrapping, backups, exports, indexes and recovery MUST preserve the required confidentiality lifetime. Endpoint envelopes MUST obtain their wrapping/key-establishment protection at the actual recipient boundary under Q-01 and Q-10; a hybrid outer transport does not qualify a classical inner envelope. KEMs establish secrets and MUST be composed with authenticated bulk encryption.

**Applies:** Private persistent storage, local or hosted.

**Acceptance:** Modify ciphertext, tenant context and wrapped keys; test backup and export access under another tenant. Trace endpoint wrapping and recovery dependencies to their declared crypto profile. All unauthorized decryptions fail, authorized restoration succeeds and every required confidentiality layer has current evidence.

**Evidence:** Key hierarchy, envelope and recovery composition, crypto parameters, storage/backup authorization tests, recipient-boundary evidence and restoration results.

**Accountable role:** Data protection owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R2, R3, X1, U2, C1.

### KQRS-Q-05 | Downgrade, replay and substitution resistance

Security mode, peer identity, protocol version, roles and transcript context MUST be authenticated. Invalid authentication, tampering, replay and disallowed downgrade MUST fail closed for the affected operation. A compatibility mode MUST be explicit, separately labeled and excluded from the stronger property; no silent plaintext fallback is allowed.

**Applies:** Encrypted or authenticated channels, proofs and credentials.

**Acceptance:** Replay, reorder, truncate and mutate messages; negotiate a removed algorithm; use an old or mismatched key. Verify rejection, bounded failure handling and no duplicate mutation. Verify legitimate reconnect does not replay an uncertain write.

**Evidence:** Negative test corpus, transcript binding specification, compatibility register and mutation traces.

**Accountable role:** Cryptography owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, R1, R2, R4.

### KQRS-Q-06 | Cryptographic inventory and replaceability

The scope MUST inventory algorithms, parameters, libraries, trust anchors, proof systems, key custody, data lifetime, client support and migration dependencies. Rekeying and algorithm replacement MUST be rehearsable without loss of authorized data or deletion of historical evidence. Migration telemetry MUST distinguish installed, negotiated and verified coverage. The inventory MUST form a dependency graph covering transport, envelopes, authentication, issuer roots, proof relations and assumptions, setup material, artifact delivery, backup/recovery and offline queues. Each node MUST identify its property, lifetime, key custodian, parameters, implementation revision and migration state.

**Applies:** All scopes using cryptography.

**Acceptance:** Select one algorithm or key format for a staging migration. Rotate it, interoperate with supported versions, enforce the declared compatibility boundary and restore authorized records. Reconcile every inventory item to a source/configuration reference. Change one essential issuer, algorithm, recipient or recovery key and identify the exact dependent claims and stored data affected; no outer-layer label may conceal an inner dependency.

**Evidence:** Crypto inventory, dependency graph, migration and rollback receipts, coverage denominator.

**Accountable role:** Cryptography owner. **Verification:** On cryptographic change; quarterly inventory review; annual migration exercise.

**Derivation:** M1, R1, R2, R4, U3, R14.

### KQRS-Q-07 | Latency and resource qualification

Every protected interactive path MUST qualify against the applicable KQRS-LAT-001 program performance profile selected before testing. The profile MUST set absolute p50/p95/p99 and error-rate ceilings, incremental protection-overhead budgets, workload/client classes, network conditions, concurrency and sample rules. The implementer cannot select or loosen limits solely to fit observed results. Measure handshake, proof generation/verification, defense decisions and complete user paths, with failures, bytes, CPU and memory. Required encryption, verification, authority and defense MUST remain enabled. Primitive means alone do not establish low latency.

**Applies:** All protected interactive paths; batch paths require a declared throughput/deadline/resource profile instead of an interactive latency claim.

**Acceptance:** Freeze the program-approved profile and controlled reference before the run. Measure actual browser/native/server classes, cold/resumed paths, saturation, malformed inputs and supported slow clients. Every required absolute-tail, incremental-overhead and error ceiling must pass. Missing budgets or weakened security settings prevent qualification; changed budgets require a retained rationale and fresh run.

**Evidence:** KQRS-LAT-001 edition and approved qualification class and manifest, preregistration, controlled-reference definition, raw samples, quantiles, errors, security configuration and budget-change history.

**Accountable role:** Performance and cryptography owners. **Verification:** Before qualification; relevant changes; recurring representative production measurements.

**Derivation:** U1, R1, R2, U2, C1.

### KQRS-Q-08 | Forward secrecy and compromise claims

Each channel MUST state precisely whether it provides forward secrecy and post-compromise recovery, against which key compromises and over which epochs. Fresh sender ephemeral keys alone MUST NOT be treated as proof of forward secrecy against later compromise of a static recipient decryption key. Claimed properties MUST include group membership and saved-key behavior.

**Applies:** Session encryption, sealed boxes, group channels and recovery.

**Acceptance:** Record a session or envelope, reveal the specified long-term key in an isolated test, and attempt historical decryption. Compare with the declared property. Test removal/rekey for future epochs and disclose any access to past plaintext.

**Evidence:** Compromise matrix, protocol analysis, reproducible key-compromise tests and group-epoch design.

**Accountable role:** Cryptography owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** R2, R3, U1.

### KQRS-Q-09 | Quantum analysis of proof systems

Every proof and credential construction MUST state its soundness, privacy, setup and quantum assumptions and its exact role in each required property. Where that property depends on proof soundness or witness privacy, the essential assumption MUST satisfy the selected quantum target. Classical essential dependencies remain transitional. PQ transport or signatures do not change a classical proof system into a PQ proof. An independent PQ authority mechanism can satisfy an authority property only when review and tests show that the classical proof is no longer relied upon for that property; its own proof classification and residual privacy assumptions remain explicit. Qualification MUST first establish that the implemented proof relation enforces the claimed predicate. Range constraints, canonical encodings and every security-relevant public input MUST be bound to the verifier decision and authorized effect. Metadata outside the verified statement MUST NOT change that effect. Quantum-assumption analysis follows this relation check.

**Applies:** ZK, selective-disclosure, credential and proof-gated paths whose soundness or privacy contributes to a required confidentiality or authority property.

**Acceptance:** Build the assumption graph from issuer and witness through proof verification to the final effect. Remove or falsify each claimed independent authority check and verify the declared dependency. Reject a quantum qualification that still relies on an essential unmitigated classical assumption. Record classical nonessential, transitional essential and qualified constructions separately. Test witnesses that violate the stated predicate; swap threshold, subject, issuer, context, epoch and effect. Invalid witnesses and altered public-input bindings must fail even when serialization and signature checks succeed.

**Evidence:** Proof-system register, setup and circuit provenance, property/assumption graph, independent-authority analysis, verifier/action tests and migration evaluation.

**Accountable role:** Cryptography and proof-system owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R4, X1, X2, U2, C1, U3, R14.

### KQRS-Q-10 | Key custody, entropy and separation

Keys MUST be generated from appropriate cryptographic randomness, scoped by function and trust domain, access-controlled and absent from routine logs. Production signing and decryption authority MUST be separated from untrusted build steps and model context. Rotation, revocation, backup and destruction MUST have tested procedures appropriate to software, OS keystore or hardware custody. No specific paid key service is mandated.

**Applies:** All production secrets and trust anchors.

**Acceptance:** Inspect custody and entropy sources, scan logs and artifacts, attempt cross-role key use, and exercise revocation and authorized recovery. A workload with only routing or evaluation authority cannot obtain content/signing keys.

**Evidence:** Key-custody inventory, permission tests, secret scans and lifecycle exercise.

**Accountable role:** Key-management owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R1, R2, R8.


## A.3. Privacy and sovereign information

### KQRS-P-01 | Minimum necessary disclosure

Every nonpublic field MUST have a documented processing purpose and authorized recipients. Routers, logs, metrics, support tools and model context MUST receive only what their function requires. Public content MAY be processed as public, but associated account, behavioral and credential metadata MUST retain its own classification.

**Applies:** All data flows.

**Acceptance:** Seed synthetic canary secrets and trace normal and error paths, retries, logs, tracing, exports and support access. No canary appears at an unauthorized component. Review each disclosed field against its purpose.

**Evidence:** Data inventory, field-purpose map, canary tests and logging configurations.

**Accountable role:** Privacy and service owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R2, R3.

### KQRS-P-02 | Content-blind intermediaries

A component with no authorized plaintext-processing purpose MUST route/store opaque authenticated envelopes or references without holding decryption keys. Operators of that component MUST NOT gain plaintext through routine interfaces, backups or diagnostics. The threat model MUST account for any ability to replace a decrypting client, change a key directory or administer an endpoint; those powers cannot be hidden inside a blanket blindness claim. Composition contracts MUST enumerate permitted observer outputs, including sizes, timing classes, error codes, logs, retries and routing metadata. A component inheriting content blindness MUST preserve those contracts across failure and recovery.

**Applies:** Nonessential relays, queues, routing/control-plane metadata paths and blind storage.

**Acceptance:** Use the declared intermediary/operator privileges to inspect traffic, queues, logs, storage and key operations. Attempt key-directory substitution and client-code replacement in the modeled trust boundary. No claimed protection may depend on an untested excluded power. Run paired challenges with different protected contents and equivalent declared public inputs; compare the permitted observer’s network, storage, logging and error observations against the allowed leakage contract.

**Evidence:** Key-custody diagram, operator access tests, ciphertext trace, metadata inventory and client-integrity analysis.

**Accountable role:** Data-plane owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R2, R3, R4, U3, R14.

### KQRS-P-03 | Proof-based disclosure reduction

For sensitive eligibility, membership, entitlement or authorization flows, the designer MUST determine the minimum fact the recipient needs and assess a reviewed ZK or selective-disclosure construction against that fact. Where the service can establish the required fact without disclosing the underlying record, it MUST use an appropriate reviewed construction or demonstrate a threat-equivalent privacy-preserving design. Feasibility MUST be assessed using the actual predicate, trust model, supported clients and performance profile; present plaintext implementation or convenience does not establish necessity. Proof statements, public inputs, witness custody, freshness, issuer/root trust, revocation and linkability MUST be explicit. Proof qualification MUST distinguish relation correctness, witness privacy, issuer authority and action binding. A mathematically valid proof of an irrelevant or weak predicate does not authorize the requested fact-dependent effect.

**Applies:** Sensitive assertion flows in which a recipient needs a predicate or limited fact rather than the complete underlying private record. Public information does not require an artificial proof.

**Acceptance:** Review the minimum-fact and feasibility analysis. Verify genuine valid proofs; reject wrong circuit, key, root, audience, tenant, nonce, expired epoch and replay. Inspect public inputs and side channels for unnecessary disclosure. Missing required keys deny the effect, and every proof binds the intended operation. Validate any equivalent design against the same disclosure and attacker model. Mutate one predicate input or relying-party context at a time, including unverified metadata, and observe the real authorization decision.

**Evidence:** Minimum-disclosure/feasibility decision, proof design, public-input inventory, verifier/circuit hashes, setup artifacts, proof-to-effect tests and equivalence evidence where used.

**Accountable role:** Proof-system owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R4, U2, C1, U3, R14.

### KQRS-P-04 | End-to-end encryption by function

Private transfer, storage and collaboration paths MUST be evaluated for E2EE against their actual service function. Whenever the function can be performed without intermediary plaintext access, encryption MUST begin at the originating endpoint and end only at authorized recipients. The feasibility decision MUST examine necessary computation, endpoint placement, key recovery and supported clients; existing server plaintext handling is not evidence that such access is necessary. Recipient sets, operator access and custody MUST be explicit. A processing agent or provider may be a named authorized recipient, with its processing boundary assessed under P-05.

**Applies:** Private messaging, sync, file transfer, collaboration, backups and other paths where originating and recipient endpoints can preserve the required function.

**Acceptance:** Test the declared function with intermediaries denied content keys. Enumerate actual decryptors and justify every necessary processing recipient. Add/remove devices and agents, rotate membership epochs, and exercise incorrect recipient keys, server-side recovery and supported client delivery. Qualification follows the observed endpoint boundary.

**Evidence:** Function/feasibility decision, recipient and custody manifest, protocol and lifecycle tests, client matrix, and necessary-processing references.

**Accountable role:** Endpoint security owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R2, R3, U2, C1.

### KQRS-P-05 | Authorized plaintext processing

Necessary plaintext processing MUST occur only in a named, purpose-bound processing domain with scoped identity, tenant separation, input/output minimization, retention, egress controls and revocation. External model providers and embedding/reranking services MUST be named in the data-flow and assurance boundary. Confidential computing MAY strengthen a domain, but is not required by brand and does not automatically establish E2EE or eliminate operator trust. Sensitivity and purpose labels MUST agree across task profiles, actual request payloads, provider selection and outbound adapters. Conflicting classifications MUST be resolved under authenticated policy before any private bytes are sent.

**Applies:** Inference, indexing, OCR, transformation and other necessary plaintext processing.

**Acceptance:** Run a legitimate task, then attempt unrelated reuse, another tenant, unsupported provider route, diagnostic export and unauthorized egress. Only declared processing occurs. Verify retention and deletion on primary and auxiliary stores. Create a low-sensitivity routing profile containing a high-sensitivity request, then inspect the network boundary; no request may leave through a provider unauthorized for the actual data.

**Evidence:** Processing-domain manifest, provider settings/contracts, runtime egress and tenant tests, retention evidence.

**Accountable role:** Processing service owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R5, R8, U3, R14.

### KQRS-P-06 | Metadata protection and honest anonymity claims

The scope MUST list exposed metadata, its recipients, retention and correlation risks. Timing, packet sizes, addresses and public commitments MUST NOT be described as hidden when visible. Padding, opaque identifiers, aggregation or unlinkability techniques MUST be applied where the documented threat model requires them, with explicit performance budgets.

**Applies:** Relays, proofs, telemetry, identity and audit systems.

**Acceptance:** Observe a declared adversary view, compare it with the metadata inventory, and test cross-session/tenant correlation. Confirm retention expiry and separate content-confidentiality claims from anonymity claims.

**Evidence:** Adversary-view capture, metadata inventory, retention checks and correlation analysis.

**Accountable role:** Privacy owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, R2, R4.

### KQRS-P-07 | Retention, deletion and recoverable correction

Private data MUST have purpose-specific retention and deletion behavior covering live stores, queues, derived indexes, embeddings, caches, snapshots and backups. Legal preservation requirements MUST be scoped and recorded. Deletion or correction MUST propagate according to a declared, tested schedule; immutable audit design MUST avoid unnecessarily preserving the original sensitive payload. Deletion evidence MUST identify the exact stores, derived state, replicas and backup policy covered. Where atomic deletion is possible, completion MUST follow verification of the resulting state; concurrent writes and failed partial operations MUST not produce a complete-erasure receipt.

**Applies:** Persistent or derived private information.

**Acceptance:** Create a tracked synthetic record, delete/correct it, and inspect each declared store and restore path. Demonstrate eventual expiry of backup copies or cryptographic erasure and prevention of deleted-record resurrection after restoration. Race writes and derived-state regeneration against deletion; inject a failure mid-operation and inspect remaining state, job status and receipt contents.

**Evidence:** Retention schedule, store inventory, deletion receipts, restore/replay test and exceptions.

**Accountable role:** Data lifecycle owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R2, U3, R14.

### KQRS-P-08 | Recovery without concealed escrow

Account/device recovery MUST document who can restore access and which keys or historical content can be recovered. An operator-held recovery secret is a plaintext capability and MUST be included in the recipient/custody model. Loss of a private key MAY be irrecoverable under an explicitly chosen policy; recovery UX MUST NOT promise otherwise.

**Applies:** E2EE, customer-managed keys and identity recovery.

**Acceptance:** Exercise lost device, all-device loss, compromised recovery factor and malicious administrator scenarios. Verify no undocumented recovery path can decrypt and no claimed recovery path loses authorized data unexpectedly.

**Evidence:** Recovery policy, enrollment UI contract, key-custody review and recovery/abuse tests.

**Accountable role:** Identity and endpoint owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, R2, R3, U1.

### KQRS-P-09 | Data-use receipts and portability

Material data processing and delegation MUST be attributable to a versioned contract, consent or other applicable authorization, with a minimal receipt. Users or tenants MUST be able to inspect and revoke relevant grants and obtain supported exports without disclosing unrelated people or secrets. Portability MUST retain schema/version and provenance needed for correct interpretation.

**Applies:** Nonpublic processing grants, user/tenant data and federation.

**Acceptance:** Export one tenant with related permissions and history; verify completeness and exclusion of other tenants/secrets. Revoke a grant and test future effects at all affected processors within the declared window.

**Evidence:** Grant receipts, export format, revocation propagation and portability tests.

**Accountable role:** Privacy and identity owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, U1, R5.

### KQRS-P-10 | Client delivery and browser trust

For browser-delivered or updateable clients that decrypt private content, the scope MUST assess who can replace client code, serve dependencies, alter integrity checks or inject content. Build integrity, update authentication and browser application protections MUST be proportionate to that authority. Content-blind server claims MUST state whether malicious client delivery is included or excluded from their attacker model.

**Applies:** Browser/native recipients and update channels.

**Acceptance:** Attempt unauthorized script/dependency replacement, XSS and update rollback in the test boundary. Verify release authentication and protections; disclose residual trust where the server can deliver new decrypting code.

**Evidence:** Client delivery threat model, update tests, application-security evidence and scope disclosure.

**Accountable role:** Client security owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** R2, R3, U1, X5, X8.


## A.4. Adaptive defense and autonomous execution

### KQRS-A-01 | Adaptive AI firewall with layered enforcement

Every applicable control, content, tool and memory plane MUST have adaptive AI defense integrated with independently enforced deterministic boundaries. Coverage MUST include exposed service/API traffic, privileged configuration, content ingestion, tool execution, persistent state and egress where those surfaces exist. Detection MUST use signals available at its authorized boundary and document its coverage, response actions, model/rule versions, update loop and blind spots. A detector recommendation cannot grant authority. A conventional firewall alone does not satisfy the adaptive requirement; protection may be composed across endpoint, runtime and perimeter components without centralizing private content.

**Applies:** Covered systems with exposed or consequential control, content, tool or memory attack surfaces, including conventional service paths supporting an AI product. Absent planes require architecture evidence.

**Acceptance:** Reconcile all four planes against actual entry points. Exercise service/API abuse, direct/indirect injection, credential or configuration substitution, tool misuse, memory poisoning and exfiltration alongside legitimate difficult requests. Record missed attacks and false positives. Demonstrate independent denial of unauthorized effects even when an adaptive detector is bypassed or unavailable.

**Evidence:** Four-plane coverage map, actual enforcement placement, detector/model/rule inventory, held-out attack/benign corpus, effect traces, false-positive data and integration gaps.

**Accountable role:** Adaptive defense owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M2, R5, U2, C1.

### KQRS-A-02 | Deterministic action authorization

Every consequential action MUST be validated against authenticated principal, tenant, object, verb, current delegation, purpose and applicable constraints at the execution boundary. Parent and child agents MUST retain their own scoped authority; a model plan is not a capability. Read, write, external communication, deployment and financial operations MUST not inherit permission merely from tool availability. Authority MUST be revalidated after queue waits and before each retry capable of an effect. The final commit boundary MUST bind the current grant/policy epoch, tenant, canonical arguments, idempotency identity and lease or generation fence. Where an external service cannot enforce the fence atomically, the system MUST record its actual cutoff and reconcile uncertain outcomes.

**Applies:** Tools, background jobs, agents and delegated execution.

**Acceptance:** Enumerate all execution entry points, including retries, queues and subprocess/connector paths. Test forged tenant, object/action substitution, stale delegation and child escalation. Unauthorized actions have no side effect; authorized paths remain usable. Revoke authority during queue wait, retry backoff and final commit races. Stale workers cannot commit protected local effects. External acknowledgements and uncertainty must match observed downstream state.

**Evidence:** Execution-entry inventory, capability schema, negative/legitimate tests and attributable action receipts.

**Accountable role:** Runtime authorization owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R5, R6, U3, R14.

### KQRS-A-03 | Untrusted content stays data

Retrieved pages, repositories, files, tool responses, model output and cross-agent messages MUST be labeled and handled as untrusted data unless a separate authenticated authority mechanism establishes otherwise. Privileged instruction channels and credentials MUST not be writable through those inputs. Content filters MUST NOT be the only protection against instruction laundering. Origin, authority, sensitivity and purpose labels MUST survive extraction, encoding changes, summaries, tool results, memory promotion and delegation. A clean file-format or malware result MUST NOT erase a distinct restriction on AI processing.

**Applies:** Any AI ingestion or agent communication.

**Acceptance:** Inject instructions through each input surface, nested encodings and forged system/owner messages. Confirm they cannot change grants, exfiltrate secrets or override stop/policy; test useful benign content remains available. Send restricted content through each transformation and cache into a tool or provider request. Inspect enforcement at the final effect boundary, including mixed-trust summaries and child-agent handoffs.

**Evidence:** Source-taint design, attack corpus, enforcement trace and benign regression tests.

**Accountable role:** Agent security owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, R5, R8, U1, U3, R14.

### KQRS-A-04 | Bounded adaptive response

Adaptive defenses MAY quarantine, throttle, revoke narrowly scoped capabilities or isolate a suspected workflow only within preauthorized response policy. They MUST record rationale, scope, expiry and recovery action, and MUST NOT expand privileges, rewrite immutable evidence or disable unrelated services. High-impact response changes MUST follow change control.

**Applies:** Automated detection and remediation.

**Acceptance:** Trigger true and false incidents, detector failure and conflicting signals. Verify blast radius, expiry, rollback and continued unaffected service. Confirm the defense cannot mint a credential or disable its own audit/control boundaries.

**Evidence:** Response-policy matrix, simulations, incident receipts, false-positive recovery and rollback tests.

**Accountable role:** Incident and adaptive defense owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, M2.

### KQRS-A-05 | Execution containment and resource safety

Untrusted execution MUST be isolated by an explicit process/container/VM or equivalent boundary appropriate to its permissions and tenant data. Filesystem, network, secret injection and control-plane access MUST be constrained outside model judgment. Resource abuse protection MUST preserve authorized long-running work and the owner stop contract. Cancellation acknowledgement MUST identify the admission cutoff and declared termination bound; newly admitted or replayed effects after that cutoff are prohibited. Already committed external effects require an attributable disposition rather than a false promise of reversal. Declared deadlines and cancellation MUST reach the actual process, runner or connector; a configuration field without an enforced execution path does not satisfy a bound. Desktop and loopback interfaces MUST authenticate their permitted callers and address browser-origin attacks under their deployment contract.

**Applies:** Code execution, plugins, tools and shared task machines.

**Acceptance:** Test cross-tenant filesystem/network access, host-secret discovery, workspace escape, runaway processes and cancellation races in an isolated environment. Verify no new operation is admitted after the acknowledged cutoff, running work reaches its declared stop state within the bound, committed external effects are reconciled, and unrelated tasks continue. Hold a tool call beyond its deadline, cancel during execution, and attempt an unauthorized local mutation from another origin. Verify process/effect outcomes and the recorded cancellation boundary.

**Evidence:** Sandbox architecture, adversarial tests, resource/backpressure rationale, cancellation contract, admission/termination traces and committed-effect reconciliation.

**Accountable role:** Runtime isolation owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, R5, R6, R8, U2, C1, U3, R14.

### KQRS-A-06 | Model and policy integrity over time

Models, prompts, policy releases, retrieval corpora, persistent memory and adaptive detector state MUST have attributable versions, bounded mutation authority and rollback paths. Updates MUST be evaluated for security regression, poisoning and drift before promotion into an authoritative role. Memory contributions MUST retain source and authority context so retrieved or self-authored content cannot become an implicit grant. The protected kernel and acceptance oracle MUST remain outside unreviewed online self-modification. Qualification challenge material MUST have exposure lineage separate from training, discovery and adaptation data. Candidate and evaluator versions MUST be frozen before fresh held-out instances are generated. A candidate MUST NOT select its trusted evaluator root or rewrite the evidence that qualifies its own promotion.

**Applies:** Adaptive detectors, model routing and persistent cognitive systems.

**Acceptance:** Poison staged retrieval or memory, submit a malicious model/policy update, regress a held-out security case and attempt self-promotion. Verify failed candidates do not become authoritative, prior known configurations can be restored and compromised memory cannot resurrect revoked grants. Retain the failed candidate and outcome. Attempt challenge leakage, stale cohort reuse, evaluator substitution and promotion with a partial result panel; preserve rejected evidence and verify the authorized recovery path.

**Evidence:** Model/policy/memory lineage, mutation permissions, staged evaluation, promotion authority, drift signals and rollback receipts.

**Accountable role:** Model and defense owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, M2, R5, U2, C1, U3, R14.

### KQRS-A-07 | Adversarial outcome evaluation

Security evaluation MUST use versioned known and held-out attack cases, benign controls and repeated/adaptive attempts where relevant. Success MUST be judged by actual data disclosure, unauthorized effects or boundary crossing, not by refusal text or reassuring model language. The attacker budget, tools, knowledge and evaluation environment MUST be declared. Campaigns MUST include defender-aware attackers with declared knowledge, access, tuning resources and retry budgets. Results MUST report per-attempt and campaign success separately, actual unauthorized effects, legitimate task completion, false denials, recovery, latency and resource use. All required attack families remain visible; a pooled average cannot cancel a protected-invariant violation.

**Applies:** AI firewall and agent security qualification.

**Acceptance:** Freeze a test plan before running. Execute end-to-end attack/benign cases, inspect effects and report confidence limits or sample counts. Every prohibited-authority case must have zero observed unauthorized effects; detector tradeoffs require preregistered thresholds. Freeze a candidate, let an attacker adapt on discovery tasks, then run fresh qualification tasks and observe authoritative state. Preserve every attempt, failure and timeout, with attack opportunity denominators.

**Evidence:** Frozen test plan, full outcome ledger, held-out handling, repetitions and investigation of failures.

**Accountable role:** Assessment owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M2, X8, U3, R14.

### KQRS-A-08 | Privacy-preserving defense placement

Content inspection MUST occur only at an endpoint or processing domain already authorized to see that content. Blind relays MUST use cryptographic validity, allowed metadata and behavioral signals without receiving plaintext or content keys. Defense placement MUST cover the control, content, tool and memory planes through their legitimate observers. Telemetry and detector learning MUST preserve purpose, retention and tenant boundaries, including derived features and feedback datasets.

**Applies:** Any firewall protecting encrypted or private AI traffic.

**Acceptance:** Trace every detector input, derived feature and feedback destination using canary content. Verify blind infrastructure has no plaintext/key path and private memory or tenant feedback cannot enter unrelated training. Test that the authorized detector locations still cover their declared attack surfaces and remain within the performance profile.

**Evidence:** Four-plane detector/data map, derived-feature and training-use inventory, canary tests, privacy regressions and performance evidence.

**Accountable role:** Defense and privacy owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R2, R3, U2, C1.

### KQRS-A-09 | Stop, revocation and recoverable state

Authorized stop and revocation MUST be enforced at execution, scheduler and credential boundaries, including running children and queued work. The stop contract MUST define acknowledgement, the admission cutoff, termination/revocation bounds and treatment of already committed external effects. Recovery MUST restore a consistent state without regranting revoked authority or blindly replaying irreversible operations. Checkpoints MUST identify source version and integrity and preserve incident evidence. Action evidence MUST distinguish prepared, attempted, committed, rejected, uncertain and compensated outcomes. An uncertainty marker MUST precede an effect when crash recovery otherwise cannot determine whether the effect occurred. Stop acknowledgements MUST distinguish admission closure, active execution termination and external reconciliation.

**Applies:** Autonomous jobs and persistent agents.

**Acceptance:** Revoke while work is queued or running, race stop with resume/child creation, and crash after an external side effect but before acknowledgement. Verify new admissions are fenced, running work and credentials meet their declared bounds, committed effects are reconciled, and no duplicate mutation or revoked grant returns after recovery. Crash before send, after send and before acknowledgement; lose a lease; restore a stale checkpoint; retry after Stop. No uncertain effect may be silently retried as if it had never happened.

**Evidence:** Stop/revocation contract, race/fault tests, scheduler/credential traces, effect ledger, checkpoint provenance and recovery receipt.

**Accountable role:** Runtime and recovery owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M2, R2, R5, R6, U2, C1, U3, R14.

### KQRS-A-10 | Agent and provider inventory

Each deployed agent or AI processor MUST have a defined purpose, owner, model/provider/version policy, permitted tools, data classes, persistent memory scope and evaluation history. Provider or routing changes that alter privacy, capability or security behavior MUST trigger relevant revalidation. Evidence of available tools or a working model SDK MUST NOT be substituted for a verified product execution path.

**Applies:** AI services, subagents, model gateways and automated defenses.

**Acceptance:** Reconcile running agents/routes against the inventory; change a provider in staging and verify affected evaluations and privacy records become stale. Exercise the actual user-to-tool-to-result path for each qualifying route.

**Evidence:** Algorithm/agent register, provider inventory, change impact analysis and end-to-end receipts.

**Accountable role:** AI service owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, M2, R5, R6, R8.


## A.5. Infrastructure and continuity

### KQRS-I-01 | Identity lifecycle and least privilege

Privileged human access MUST use strong MFA, with phishing-resistant factors where supported and a documented transition where not. Workload identities MUST be scoped, attributable and revocable. Onboarding, role change, offboarding and emergency access MUST cover all linked services; shared undocumented administrator credentials are prohibited.

**Applies:** Human and machine access.

**Acceptance:** Review the complete privileged population; disable a test identity and verify linked access is removed. Attempt unauthorized role elevation and examine emergency access expiry and logging.

**Evidence:** Identity inventory, MFA configurations, access review, revocation tests and break-glass record.

**Accountable role:** Identity owner. **Verification:** On role/access change; privileged review every 90 days.

**Derivation:** M1, U1, R5.

### KQRS-I-02 | Tenant and state isolation

Tenant isolation MUST be enforced for APIs, database rows, object storage, caches, queues, websocket/event streams, agents, jobs, backups and administrative paths that exist in scope. Shared infrastructure MAY be used only with demonstrated isolation. Per-customer infrastructure MUST NOT be assumed isolated solely because it has a different hostname. Persistent memory and conversation identity MUST include the authenticated principal or tenant and applicable policy context; possession of a client-supplied conversation identifier alone cannot establish access.

**Applies:** Multi-tenant or multi-principal services.

**Acceptance:** Create distinct test tenants; attempt cross-boundary reads, writes, subscriptions, restores and actions at every inventoried entry point. No unauthorized information or side effect is permitted. Include privileged/service-identity paths. Use two authorized principals with the same conversation identifier and different policies; private history, derived memory and effect authority must remain isolated.

**Evidence:** Tenant-boundary inventory, endpoint/data tests, administrative access tests and test-population coverage.

**Accountable role:** Platform security owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, R4, R5, R6, R8, U3, R14.

### KQRS-I-03 | Traceable and tamper-resistant delivery

Production code, infrastructure and model-policy changes MUST trace to source, build inputs, test results and deployment identity. Untrusted build steps MUST NOT access release-signing secrets. Update and rollback paths MUST validate provenance and compatibility. Independent Warden verification remains optional unless the owner explicitly selected it; relevant local verification is not optional. Candidate artifacts MUST be evaluated by a protected harness and trust policy whose identity is established independently of the candidate. Manifests MUST bind all executed source, test inputs and resulting artifacts.

**Applies:** Software, infrastructure, client and policy releases.

**Acceptance:** Tamper with an artifact, provenance statement or source pin; attempt signing-key access from a build and a rollback to an unacceptable version. Verify rejection. Demonstrate a legitimate locally tested release through the approved path. Replace a candidate’s evaluator, source manifest, result file or verification key and require rejection. Record precisely which source and runtime properties the protected evaluator actually checks.

**Evidence:** Source/build/deployment records, SBOM, provenance verification, key separation and rollback tests.

**Accountable role:** Release owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** R8, U1, X5, U3, R14.

### KQRS-I-04 | Measured recovery and continuity

Each critical function MUST declare risk-appropriate recovery time and recovery point objectives, dependencies and restore authority. Backups MUST be access-controlled and restoration tested for data integrity and tenant scope. Recovery MUST cover key loss, dependency outage and compromised administration where material. A backup success log is not a restore test.

**Applies:** Critical customer and control-plane functions.

**Acceptance:** Restore representative encrypted data and service state in an isolated environment; measure elapsed time and lost-data window against declared objectives. Test failure of a necessary provider/key dependency and verify the documented fallback.

**Evidence:** Recovery plan, objectives, backup inventory, restore logs, integrity checks and lessons.

**Accountable role:** Reliability owner. **Verification:** Quarterly for critical functions; after material recovery changes.

**Derivation:** U1, M2, R2.

### KQRS-I-05 | Detection, vulnerability response and incident handling

The scope MUST maintain actionable monitoring, vulnerability triage and incident procedures with accountable response ownership and declared urgency-based remediation targets. Evidence collection MUST minimize sensitive content. Confirmed critical boundary failures require prompt containment under existing authority and revalidation before the affected assurance claim is restored.

**Applies:** All operational scopes.

**Acceptance:** Exercise alert delivery, credential compromise, malicious tenant and unavailable dependency scenarios. Track an actual or isolated seeded finding from detection through containment and retest, including missed alerts.

**Evidence:** Alert tests, vulnerability/incident registers, timelines, response targets and corrective evidence.

**Accountable role:** Incident owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, M2.

### KQRS-I-06 | Secure application and platform baseline

Applications and infrastructure MUST receive risk-appropriate secure configuration, dependency and secret checks, authenticated-input validation, authorization testing and abuse protections. Relevant OWASP ASVS requirements SHOULD be used as an application verification baseline; high-risk control planes require deeper verification. KQRS cryptographic features MUST NOT excuse ordinary exploitable application flaws. Outbound fetch and webhook boundaries MUST constrain DNS resolution, connected destination, redirects, decompression, time and bytes. Proxy modes MUST expose any delegated destination decision. Webhook replay identity MUST be authenticated or derived from authenticated bytes.

**Applies:** Web/native applications, APIs and platform services.

**Acceptance:** Run the scope-specific application/configuration test plan, including injection, session abuse, SSRF, file/path handling and secret exposure where applicable. Resolve exposed critical boundary flaws before qualification; document the tested population. Test DNS rebinding, mapped private addresses, redirects, decompression exhaustion and renamed replay identifiers. An authentic body with unauthenticated event metadata cannot create a second protected effect.

**Evidence:** Application assessment plan, scans, manual/adversarial tests, findings and remediation evidence.

**Accountable role:** Application security owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, X8, R6, U3, R14.

### KQRS-I-07 | Supplier and shared-responsibility assurance

The operator MUST document what each supplier secures, what KI secures, what the customer secures and which properties remain unverified. Supplier attestations MAY support inherited controls but MUST NOT prove KI application configuration, tenant isolation or E2EE. External outages or absent paid tooling MUST not be converted into fictitious passing evidence or unapproved new spending.

**Applies:** Hosting, identity, inference, storage, build and support providers.

**Acceptance:** Trace each inherited control to a current source and test KI-controlled configuration. Simulate loss or revocation of a critical integration and verify the declared failure behavior; record unknown supplier properties explicitly.

**Evidence:** Supplier/responsibility register, source dates, configuration tests and dependency failure exercises.

**Accountable role:** Supplier and service owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, R8.

### KQRS-I-08 | Local, offline and federated compatibility

Local-first, on-premises, owner-hosted and federated deployments MUST retain their declared authority/data boundaries. Offline grant validity, revocation staleness and conflict resolution MUST be explicit. Central hosting, universal agent APIs or always-online control MUST NOT be introduced merely to satisfy KQRS when equivalent local enforcement exists.

**Applies:** Local/offline/federated operation.

**Acceptance:** Disconnect control-plane access, revoke a grant remotely, reconnect and reconcile state. Verify the declared offline authorization window, no cross-node authority expansion, correct conflict handling and preserved local data/keys.

**Evidence:** Deployment-mode contract, offline policy, federation/state tests and revocation timing.

**Accountable role:** Deployment and federation owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, R2, R4, R8, U1.


## A.6. Verification and claims

### KQRS-V-01 | Version-bound evidence

Each control assessment MUST retain exact scope, requirement version, implementation/configuration digest, method, test population, expected and observed outcomes, timestamps, assessor and supporting artifact digests. Raw evidence MUST remain access-controlled; public reports MAY provide redacted summaries and verifiable references. Hashes alone do not prove completeness or truth. Evidence records MUST bind claim subject, canonical source revision, deployed configuration where relevant, harness/policy versions, challenge generator, evaluator identity, actual observation and retained outcome. A signature establishes origin and integrity under its trust policy; the attached examination establishes behavior.

**Applies:** All assessments and claims.

**Acceptance:** Reconstruct one pass and one failure from retained artifacts. Tamper with an artifact, change its build or scope and verify the result cannot support the original claim. Check that sensitive data are not exposed by public evidence. Substitute a plausible result from a different build, environment, principal or challenge cohort; verify that it cannot satisfy the current claim.

**Evidence:** Assessment records, protected artifacts, digest checks and scope/version validation.

**Accountable role:** Evidence custodian. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, R8, U3, R14.

### KQRS-V-02 | Separate implementation from verification

Designed, source-inspected, implemented, tested, deployed, operationally observed and independently assessed MUST remain distinct. A plan, source file, fixture, build, tool listing or screenshot MUST NOT be promoted to a deployed security guarantee without the corresponding evidence. Unknown, failed, stale and excepted results MUST be represented explicitly. Source observed, test source inspected, test executed, deployment observed and independently qualified MUST remain distinct evidence levels. Reused code inherits only the property and conditions actually demonstrated at its origin.

**Applies:** All registries, dashboards and public statements.

**Acceptance:** Load incomplete or wrong-scope evidence, a skipped test and an expired result. Verify they cannot become a passing or independent status. Confirm implementation and verification histories remain separately inspectable. Attempt to mark a claim passed using only a source file, a present test, an old receipt or another deployment’s result; reject the promotion while retaining the useful lower-level evidence.

**Evidence:** Status model, negative validator tests, sample assessments and correction history.

**Accountable role:** Assessment owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, R8, U3, R14.

### KQRS-V-03 | No averaging away critical failures

Full scoped conformance requires every architecture-triggered mandatory requirement to have current passing evidence and every nonapplicability decision to have reviewed architectural support. Requirement applicability MUST be determined independently of the claims chosen for marketing. A migration plan, unresolved gap, missing performance profile, essential classical quantum dependency, expired result or exception prevents full target qualification. Scores may prioritize work but cannot override an unmet boundary.

**Applies:** Any KQRS conformance statement.

**Acceptance:** Submit scopes containing one failed, missing, stale or excepted required control amid passing controls. Attempt to omit a required quantum dimension, firewall plane or performance qualification by declining its claim. Reject each full-conformance request; accept nonapplicability only with architecture evidence.

**Evidence:** Statement of Applicability, mandatory-property and four-plane matrices, current evidence/transition ledger, claim-decision record and negative validator tests.

**Accountable role:** Assessment owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1, U2, C1.

### KQRS-V-04 | Independent challenge without fictional independence

Self-assessment and independent assessment MUST be labeled differently. An independent reviewer MUST be free of control over the implementation/evaluation being assessed, with conflicts disclosed. A new model label or second run by the same authority does not establish independence. Ordinary releases MUST NOT be blocked merely because an optional independent reviewer is unavailable.

**Applies:** Independent-assurance claims and review processes.

**Acceptance:** Compare assessor and implementation authority, source/evidence access and conflict declarations. Reject self-issued independent conclusions. Verify absent optional review is reported as unrun rather than passed and does not create an unauthorized release gate.

**Evidence:** Reviewer independence declaration, assessment report, conflict register and release-policy record.

**Accountable role:** Assurance lead. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, R8.

### KQRS-V-05 | Bounded public claims

Every product claim MUST identify standard version, scope, properties, assessment basis, issue/review date and material exclusions. Requirement language MUST be distinguished from achieved behavior. Comparative superiority MUST name a measured property and like-for-like comparison; no unsupported universal superiority, certification, ASI immunity or regulatory equivalence is permitted.

**Applies:** Marketing, procurement responses and product assurance labels.

**Acceptance:** Review each public sentence against its claim record and current evidence. Remove or narrow statements when scope changes or evidence expires. Verify the public brief cannot imply portfolio-wide conformance from one pilot.

**Evidence:** Claims register, published text, evidence links and withdrawal process.

**Accountable role:** Claims owner. **Verification:** On relevant change and at least every 90 days.

**Derivation:** U1, M1.

### KQRS-V-06 | Transparent framework crosswalks

Mappings to external standards MUST identify exact edition and clause/control where checked, relationship type and residual work. Informative thematic alignment MUST NOT be presented as certification or a completed clause-by-clause assessment. Proprietary standards text MUST not be copied without appropriate rights.

**Applies:** Any external-framework comparison or customer questionnaire.

**Acceptance:** Sample each published mapping against the actual referenced requirement. Classify unreviewed mappings as thematic and retain gaps; verify no KQRS pass automatically grants another framework credential.

**Evidence:** Versioned crosswalk, reviewed references and residual-requirement register.

**Accountable role:** Framework mapping owner. **Verification:** On source/framework update and before external use.

**Derivation:** U1, X5, X6, X7, X8, X9, X10.

### KQRS-V-07 | Claim expiry and change invalidation

Qualified claims MUST expire or become stale after changes affecting their assumptions, including keys, policies, providers, clients, circuits, infrastructure and tested boundaries. Continuous monitoring MUST detect relevant changes; only affected claims require revalidation when impact can be bounded. Historical receipts MUST remain visible even after withdrawal. Invalidation MUST follow essential dependencies transitively through assessment summaries, inherited controls and public claims. Revalidation MUST be idempotent and recoverable across worker crashes and repeated events.

**Applies:** All qualified properties and operational assertions.

**Acceptance:** Change an evidence dependency and expire a claim in a test registry. Verify it becomes ineligible, unrelated scoped claims remain intact when justified, and new passing evidence is required for reissue. Change a foundation shared by multiple products, interrupt propagation, resume it and verify all affected claims become stale exactly within the declared propagation bound.

**Evidence:** Dependency graph, freshness rules, invalidation tests and claim history.

**Accountable role:** Evidence and service owners. **Verification:** On relevant change and at least every 90 days.

**Derivation:** M1, M2, R8, U1, U3, R14.

