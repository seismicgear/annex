---
id: public-kqrs-005-engineering-and-adversarial-qualification
title: 'KQRS-005: Engineering and Adversarial Qualification'
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
doc_path: docs/kqrs/public/KQRS-005_Engineering_and_Adversarial_Qualification.md
---

# KQRS-005: Engineering and Adversarial Qualification
## The boundary must hold when the system becomes interesting.

**Edition:** 0.4.1-draft  
**Issued:** November 11, 2026  
**Authority:** Kuykendall Industries, LLC  
**Classification:** Public specification.  
**Status:** Normative procedures for applicable KQRS-001 controls. These procedures define qualification work; they do not report product qualification.

## 1. Engineering position

A system becomes interesting at the handoff. A document becomes model context. A plan becomes a tool call. A grant becomes a queued job. A signature becomes permission. A benchmark result becomes a public claim. Each transition changes what another component is prepared to believe or do.

KQRS places an explicit contract at those transitions. The contract carries identity, authority, purpose, data classification, permitted observations and a recoverable account of the effect. The receiving component enforces the contract. The evidence follows the same discipline.

This is the engineering expression of continuity: useful work persists, private state remains under its authorized custody, and the conditions that justified an action remain inspectable after the action has occurred. Every mechanism in this volume must be connected to an actual caller, an actual protected effect and a challenge capable of exposing its failure.

### 1.1 Build from the implementation

Before creating a new security subsystem, the implementation owner MUST produce a reuse record. It identifies the required property, all plausible existing implementations found across the declared repository inventory, canonical source revision, relevant function, active caller, test source, available execution evidence and the boundary still requiring work. Generated copies and historical workspaces remain visible as provenance; they do not become competing sources of authority.

The decision has five forms: **reuse** a demonstrated mechanism within its conditions; **extend** a mechanism that lacks a required property; **integrate** an existing mechanism into a path that does not invoke it; **qualify** a path whose implementation exists but lacks current evidence; or **construct** a mechanism after a bounded search establishes the unmet need. Unknown source remains unknown. A repository title, README promise or dependency name does not settle the decision.

A reusable mechanism carries an inheritance contract: what it guarantees, what its caller must supply, what it cannot establish, and what changes invalidate the result. Reuse accelerates qualification only when those conditions survive the new composition.

## 2. The composition contract

Each security-relevant handoff MUST have a versioned contract with the following fields. A shared schema may implement several boundaries, but each deployed path must bind its actual values.

| Contract field | Required meaning |
|---|---|
| Subject | Exact component, build, configuration, interface and deployment mode |
| Initiator | Authenticated principal, tenant, delegated actor and parent relationship |
| Authority | Grant identity, allowed verb/object, purpose, policy epoch, expiry and revocation source |
| Input | Origin, sensitivity, custody, permitted processing and transformation lineage |
| Observation | What each party may learn: content, metadata, length, timing class, errors and logs |
| Effect | The state change or disclosure allowed at the final boundary |
| Declassification | Authenticated policy that permits a particular reduction of restriction |
| Execution | Queue/retry identity, lease or generation, idempotency and commit ordering |
| Failure | Rejection, timeout, partial result, uncertain outcome and resource limits |
| Recovery | State retained, keys required, revoked state excluded and reconciliation rule |
| Evidence | Observation method, protected evaluator, retained receipt and invalidation dependencies |

### 2.1 Labels travel with meaning

Origin, authority, sensitivity and purpose are separate dimensions. Trusted storage may contain untrusted instructions. Authenticated content may still be irrelevant to an authorization decision. A user may be allowed to download a quarantined document while an automated agent is forbidden to process it. A public document can arrive through a private query whose metadata remains protected.

Transformations MUST preserve those distinctions. Extraction, summarization, translation, embedding, caching and memory promotion cannot silently remove a restriction. If a result combines sources, its label must conservatively preserve their constraints unless authenticated policy permits a specific declassification. A model’s explanation does not supply that policy.

At provider egress, the classification of the actual request MUST agree with the routing profile and the adapter’s destination policy. A low-sensitivity task wrapper cannot authorize high-sensitivity bytes. Contradictory labels must be resolved before transmission.

### 2.2 Observe more than one execution

Confidentiality concerns what an observer can distinguish across executions. Clarkson and Schneider’s hyperproperty framework supplies the formal basis for that distinction. KQRS therefore uses paired challenges as a practical violation detector alongside protocol and implementation analysis. [X16]

The assessor fixes the declared public inputs, varies a protected value, and compares the permitted observer’s outputs, network destinations, sizes, logs, errors and recovery artifacts. Allowed leakage must be declared before the run. Randomness and timing require an explicit statistical comparison or distributional contract; exact byte equality is often inappropriate. The assessor must also test an authorized declassification so that a system which suppresses every useful result cannot pass by silence.

Finite paired tests expose counterexamples. A universal noninterference claim requires a correspondingly scoped proof and a justified implementation mapping. The evidence record names which kind of result was obtained.

## 3. Authority must survive the queue

Long-running autonomy is a first-class operating mode. Standing authority covers repeated work until its real scope, expiry or revocation changes. The runtime must preserve that authority through scheduling and recovery without manufacturing new permission from a plan or repeatedly asking for authority already held.

### 3.1 Admission, attempt and commit

An action MUST bind its principal, tenant, canonical arguments, purpose, grant, policy epoch and idempotency identity before admission. Admission establishes permission to enter the workflow. It cannot freeze permission forever.

The runtime MUST revalidate authority after a queue wait and before every effect-capable retry. A final check separated from a local write by an uncontrolled race is insufficient: a transaction, lease/generation fence or equivalent ordering mechanism must ensure that stale authority cannot commit after the relevant cutoff. The contract identifies the linearization point—the event that decides whether revocation or the effect came first.

For external services, the connector must use downstream idempotency and fencing when available. When the destination cannot enforce a fence, the connector records its actual last controllable boundary, the maximum outstanding exposure and the reconciliation procedure. It must never report a stronger Stop guarantee than the downstream protocol supports.

### 3.2 Effect receipt

The effect receipt MUST preserve the action identity, grant and policy revision, canonical argument digest, lease/generation, attempt number, timestamps, destination, expected effect and observed result. Sensitive arguments remain protected; a digest does not justify retaining unnecessary plaintext.

The outcome vocabulary is precise:

- **Prepared:** the action and authority are bound; no effect has been attempted.
- **Attempted:** the effect-capable request has crossed the recorded boundary.
- **Committed:** authoritative state or a sufficient destination acknowledgement establishes the effect.
- **Rejected:** the boundary refused the action and the evidence establishes no protected effect.
- **Uncertain:** the available evidence cannot distinguish committed from uncommitted.
- **Compensated:** a separate authorized action has established the recorded compensating result.

Where a crash can erase knowledge of an attempted external effect, an uncertainty marker MUST be durable before the request leaves. Recovery may query, reconcile or seek the authority required for a new action; it cannot reinterpret uncertainty as permission for blind replay. Compensation is itself an effect with its own authority and receipt.

### 3.3 Stop is a protocol

Stop must report three distinct observations: admission closed, active execution terminated or bounded, and outstanding effects reconciled. Children, queues, leases, credentials and persisted checkpoints are part of the same contract. A cancelled parent does not make an already-sent request disappear.

Qualification MUST place revocation at each of these race points: before admission; during queue wait; during retry backoff; immediately before the commit boundary; after send but before acknowledgement; during child execution; and during restart from saved state. The test oracle observes actual downstream state. A stopped UI, an aborted stream or a returned cancellation flag is not sufficient evidence of effect termination.

## 4. Adaptive defense with a fixed authority root

The adaptive firewall combines learned or adaptive detection with independent enforcement. It operates across control, content, tool and memory planes at the components already entitled to observe the relevant information. A content-blind relay remains content blind; an authorized endpoint can inspect the plaintext it already holds.

CaMeL demonstrates a design direction that separates trusted control flow from untrusted data and constrains tool data flows. KQRS adopts the boundary principle while requiring each implementation to establish its own execution and utility results. [X18]

| Plane | Adaptive contribution | Independently enforced invariant |
|---|---|---|
| Control | Detect unusual delegation, provider selection or policy mutation | Authenticated grant, tenant, policy root and change authority |
| Content | Detect encoded instructions, malicious documents and exfiltration intent | Origin/sensitivity labels, permitted processing and egress |
| Tool | Detect harmful sequences, argument manipulation and anomalous effects | Current action authority, destination, fencing and resource bounds |
| Memory | Detect poisoned retrieval, delayed triggers, drift and feedback manipulation | Source lineage, mutation scope, promotion rules and recoverable state |

### 4.1 The learner is also an attack surface

Every adaptive candidate MUST identify its training or feedback sources, exposure lineage, model/rule version, expected benefit and changed behavior. Qualification must test both evasion of the current defense and poisoning of its update process. An attacker may aim to make the next policy worse while appearing helpful in the current run.

The promotion path MUST record candidate generation, validation, authorized adoption, activation and rollback separately. The candidate cannot rewrite protected acceptance tests, issue its own trusted signing root, hide failed cohorts or acquire broader capabilities through a favorable score. Shadow replay and controlled activation use the existing standing policy; they do not require a new permission ritual for each valid update.

### 4.2 Utility is part of security

Every attack campaign includes demanding legitimate tasks that exercise the same surfaces. Report authorized task completion, false denials, latency, resource use and recovery. The system must remain useful under protection. A detector that blocks all work fails the utility condition even if it prevents all attempted attacks.

## 5. The adversary receives a turn

Qualification MUST challenge the actual defense, including its architecture and evaluation method. Static strings are regression material. Defender-aware attackers must be allowed to adapt within the declared campaign budget. The USENIX study *The Attacker Moves Second* motivates this requirement; AgentDojo supplies a primary example of stateful task and attack evaluation. [X19, X20]

### 5.1 Campaign contract

Before qualification, freeze the candidate, configuration, protected invariants, workload class, legitimate-task criteria and acceptance thresholds. Declare attacker knowledge, interfaces, model/tool access, tuning resources, time, attempts per task, parallelism and allowed persistence. Include the exact capability restrictions that materially affect the result.

Preregistration MUST bind the exact implementation and source identities, harness and campaign conditions, plus each case's semantics, fixture digest, oracle identity and planned repetitions. A list of case identifiers alone is insufficient: a case cannot keep its name while its expected effect, challenge bytes or evaluator changes. Retain the preregistered digest through the existing protected evidence process before execution. Local hashing checks consistency with that retained record; the assessor establishes its actual custody and chronology.

Use separate discovery and qualification pools. The attacker may tune on discovery tasks. Generate fresh qualification instances only after the candidate is frozen, using a versioned generator and evaluator-held seeds. Preserve exposure records for candidate, attacker and evaluator. Rotate compromised pools. Public benchmark results retain their exposure history and do not become private held-out evidence by being copied into another harness. [X22]

The authoritative oracle observes protected state: network destinations and bytes, database changes, file writes, grants, key use, queue generations and recipient-visible results. Model prose can aid diagnosis; it cannot establish that an action happened or did not happen.

### 5.2 Required campaign families

| Campaign | Attacker objective | Required oracle and legitimate control |
|---|---|---|
| C-01 Instruction laundering | Turn retrieved content, encoded text or a tool response into authority | Inspect grants and effects; complete a legitimate retrieval-and-action task |
| C-02 Memory persistence | Plant a delayed trigger that survives summarization, retrieval or restart | Inspect lineage, future actions and recovered state; retain legitimate useful memory |
| C-03 Delegation escape | Expand authority through child agents, role confusion or a forged owner message | Observe child grant and actual effect; allow a correctly scoped delegated task |
| C-04 Revocation race | Commit through a stale lease, queued attempt, backoff or restored checkpoint | Observe commit ordering and downstream state; allow work that committed before cutoff |
| C-05 Privacy composition | Leak a protected value through labels, metadata, error paths or provider routing | Paired observer traces and outbound bytes; allow declared declassification |
| C-06 Proof substitution | Use an invalid witness, irrelevant predicate, wrong subject or altered public input | Observe verifier decision and relying-party effect; accept a correct proof in context |
| C-07 Trust-chain downgrade | Replace algorithm, root, epoch, recipient or recovery material | Observe negotiated transcript and actual key custody; preserve supported valid peers |
| C-08 Evaluation capture | Substitute evaluator code, evidence, trust root or a partially completed result panel | Verify protected harness identity and full result set; accept an authentic current run |
| C-09 Resource coercion | Exhaust queues, decompression, parsing, proof verification or child work | Measure deadlines, tails and resource bounds; preserve authorized long-running work |
| C-10 Recovery deception | Hide a committed effect, replay uncertain work or revive revoked state | Inspect durable receipts and destination state across crash/restart; recover useful authorized state |
| C-11 Learning corruption | Promote a poisoned policy, weaken a gate or contaminate the challenge pool | Observe policy diff, activation and rollback; promote a valid beneficial candidate |
| C-12 Cross-principal state | Access another principal’s conversation, memory, cache or offline entitlement | Inspect storage keys and private outputs; preserve same-principal continuity |

Each applicable family MUST include failure and recovery conditions, not only the successful network path. The campaign record explains nonapplicability through architecture. An unimplemented defense does not make its attack family irrelevant.

### 5.3 Score the campaign honestly

Report per-attempt success and campaign success separately. Campaign success asks whether an attacker achieved the objective within the allowed budget for a task. Attempts by the same adaptive attacker are not automatically independent statistical trials. Include task-level denominators, attack opportunities, retries, timeouts, invalid attempts and missing observations.

A critical protected-invariant violation fails the corresponding requirement regardless of an attractive aggregate score. Compare utility, latency and resources on matched tasks and environments. Report both unweighted task results and any declared workload-weighted summary. Publish uncertainty appropriate to the sampling design; do not attach an independent-trial confidence interval to a dependent adaptive process without justification.

Comparative claims require the same attacker access, task distribution, retry budget, environment, observation rules and treatment of errors. If a competitor or baseline cannot be tested on equivalent conditions, state the exact measured comparison that remains possible. The program earns superiority one reproducible boundary at a time.

## 6. Assurance cases that can be defeated

Every material qualification claim MUST carry a structured argument. NIST systems security engineering and the OMG Structured Assurance Case Metamodel provide the foundation; KQRS uses that foundation to make assumptions and counterevidence operational. [X14, X15]

The case identifies the precise claim, scope and version; the reasoning connecting evidence to it; required subclaims; assumptions; evidence references; potential defeaters; observed counterevidence; residual obligations; and invalidation triggers. A defeater is a concrete condition that would break the argument, such as a retained recovery key, an unbound proof input or candidate control over its evaluator.

### 6.1 Evidence levels

| Level | What it establishes |
|---|---|
| Source observed | A mechanism is present in the identified source revision |
| Test source inspected | A test is written; its logic and intended oracle can be examined |
| Test executed | The identified test actually ran against the identified subject and retained its result |
| Deployment observed | The specified deployed configuration produced the recorded observation |
| Scoped qualification | Every applicable mandatory requirement has current sufficient evidence |
| Independent assurance | The assessor relationship and protected evaluation boundary meet the stated independence conditions |

These levels describe different evidence, not a score that can be averaged. A source file can be valuable evidence without becoming a production result. A signature authenticates an assertion under a trust policy; it does not make the assertion true. Supply-chain provenance must be checked against independently trusted identities and expected subjects. [X23, X24]

### 6.2 Invalidation is part of the claim

The case MUST identify every essential dependency whose change can defeat it: source, deployed configuration, algorithm, key, issuer root, proof relation, policy, model, provider, challenge pool and supporting claim. The dependency graph must propagate staleness to affected summaries and public statements. Propagation must be idempotent, restartable and bounded by a declared operational objective.

Revalidation creates a new attributable result. It does not overwrite the failed or superseded evidence. An assessor must be able to reconstruct what was believed, why it was believed, what changed and which current statement replaced it.

## 7. Formal work on narrow, consequential protocols

Formal analysis targets the boundaries where a compact model can expose an expensive class of failure: authenticated hybrid handshakes; proof issuer/root/epoch binding; lease and generation fences; revocation versus commit; and recovery from uncertain effects. Symbolic protocol tools such as Tamarin support trace properties and observational equivalence within explicit models. [X17]

Each formal result MUST retain the model, hash, tool version, assumptions, lemmas, proof or counterexample status and exact source mapping. A timeout is unresolved. A proof under an idealized primitive does not establish memory safety, constant-time execution, correct parsing or deployment custody. Those remain separate obligations in the same case.

Counterexamples become implementation regression tests. Where no violation is found, the reviewer examines abstraction gaps and whether the code preserves the modeled event ordering. Model and implementation revisions must invalidate one another’s inherited evidence when a relevant change occurs.

## 8. Operating qualification

Qualification is a continuous process with discrete, attributable decisions. The operating record tracks current requirement coverage, essential stale claims, authority violations, privacy boundary failures, uncertain effects awaiting reconciliation, revocation propagation, recovery success, legitimate task completion and protected-path latency. Each measure has an owner, denominator, observation window and response rule.

The program MUST preserve functioning local, offline, federated and hosted modes. Offline authority declares its validity and revocation staleness; reconciliation cannot silently revive expired grants. A recovery operation may restore useful state while retaining the consequences of Stop, revocation and key retirement.

### 8.1 Qualification sequence

1. Reconcile the complete repository and deployed dependency boundary.
2. Select existing mechanisms and record their inheritance conditions.
3. Bind composition contracts to active callers and effects.
4. Repair demonstrated relation, authority, privacy or recovery defects.
5. Run deterministic, race, protocol and paired-observation tests.
6. Run defender-aware campaigns and matched legitimate workloads.
7. Qualify performance with every required protection enabled.
8. Resolve counterevidence and evaluate the assurance case.
9. Issue the exact scoped result, activate dependency monitoring and retain the full evidence lineage.

No stage may convert a future work item into completed evidence. Each passing statement names the boundary that held, the challenge it faced and the revision that earned the result.

## 9. Required record set

The assessment retains a source/reuse register; boundary contracts; cryptographic obligation graph; effect and Stop protocol; adaptive promotion record; campaign contract and complete outcomes; performance manifest; formal-model results where used; assurance cases; exception/transition register; and current claims register. The supplied schemas and validators check record structure and internal consistency. They cannot replace examination of the actual system.

The public specification defines the obligation. The internal implementation atlas locates the existing code. The assessment supplies the execution evidence. Together they make KQRS an engineering program whose claims can be checked, challenged and corrected.
