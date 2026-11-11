---
id: public-kqrs-003-program-charter
title: 'KQRS-003: Program Charter and Operating Model'
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
doc_path: docs/kqrs/public/KQRS-003_Program_Charter.md
---

# KQRS-003: Program Charter and Operating Model

**Edition:** 0.4.1-draft  
**Issue date:** November 11, 2026  
**Sponsor and standard owner:** Montgomery Kuykendall, Kuykendall Industries, LLC  
**Program state:** Written program and implementation workstreams established; product assessments remain scope-specific.

## 1. Mandate — make the architecture earn its claims

KQRS is the operating program for security across Kuykendall Industries. Its mandate is to preserve authorized control, private custody and useful operation as systems become more autonomous, interconnected and exposed to capable adversaries. The six commitments are mandatory where their architectural conditions apply: quantum resilience, content blindness, zero-knowledge integration, end-to-end confidentiality, adaptive AI defense and continuous assurance. Protected performance is part of the requirement. [U1–U3, M1, M2, C1]

The program begins with what KI has built. Every repository in the declared portfolio inventory receives an attributed record. Existing authority engines, cryptography, execution boundaries, evidence graphs and recovery mechanisms become the implementation base. The work then concentrates on the seams: a grant after queueing, a label after transformation, a key after recovery, a proof at its relying action, and a claim after its premise changes.

The standard owner controls the specification and its meaning. Product owners control their systems under existing authority. Assessors control the integrity of their examinations. These responsibilities meet in a versioned assurance case that states the claim, the evidence, the assumptions and the conditions that would defeat it.

The program’s ambition is global technical leadership. Its operating method is reproducible challenge: stronger adversaries, exact observation, preserved failures, useful protected workloads and current evidence. A comparative statement is earned by the specified comparison.

## 2. The operating system of the program

The program maintains nine linked registers:

| Register | Required contents | Operating purpose |
|---|---|---|
| Source and reuse | Complete repository inventory, exact revisions, canonical ownership, callers, tests and transfer conditions | Find and improve existing mechanisms before constructing another |
| Composition and assurance | Boundary contracts, arguments, assumptions, defeaters and dependency invalidation | Establish why individually correct components support the whole claim |
| Systems and applicability | Scope versions, architecture triggers, owners, clients, providers, deployment modes and all control decisions | Establish the denominator for every claim |
| Cryptography and proofs | Algorithms, protocols, keys, roots, circuits, lifetimes, negotiators, custodians and migration dependencies | Show where each quantum property is actually enforced |
| Data and authority | Processing purposes, recipients, permitted effects, standing grants, egress, retention and revocation | Prevent custody or model output from becoming permission |
| Models and defense | Model/rule versions, inspection placement, training/update provenance, protected planes and response bounds | Make adaptive defense inspectable and reproducible |
| Evidence and findings | Exact builds, configurations, test cards, artifacts, failures, observation periods and reassessment triggers | Keep current results connected to the actual system |
| Exceptions and upgrades | Unmet clauses, compensating protection, owner, authority, expiry, dependencies and exit tests | Preserve operation without erasing the upgrade obligation |
| Public statements | Exact wording, scope, evidence, issuer, review date, expiry and correction history | Keep commercial claims supported over time |

The three recurring activities are implementation, challenge, and operational correction. The same record identifiers connect them. Work is complete only when the requested outcome is verified at the claimed stage.

## 3. Responsibilities and actual assignment

| Role | Responsibility | Initial accountable owner |
|---|---|---|
| Standard owner and sponsor | Scope, material requirement changes, resources and public positioning | Montgomery Kuykendall |
| Program stewardship | Registers, applicability decisions, review cadence and corrective actions | Sponsor until a named steward is recorded |
| Cryptography and performance | Protocol/key/proof qualification and KQRS-LAT-001 compliance | Existing responsible engineer per pilot; sponsorship retains unassigned accountability |
| Runtime and product security | Tenant isolation, authority, execution, delivery and recovery | Existing product owner identified in each scope |
| Adaptive defense | Reconcile existing defenses, complete coverage, evaluate detection and bound response | Named engineering owner required before that workstream is executed |
| Privacy and evidence custody | Purpose, processors, access, retention, artifacts and correction history | Scope owner until a separate assignment is recorded |
| Assessor and challenger | Evaluate actual behavior, record findings and assess equivalence | Explicitly identified for each assessment |
| Claims owner | Approve exact supported wording, track expiry and correct claims | Sponsor or an expressly delegated role |

A role is not staffed by listing it. Each executed work item records a real person, authenticated service, or authorized agent identity and its source of authority. One person may hold several roles; the assessment records that relationship. A separate model run does not create external independence.

## 4. Admission and applicability procedure

A product enters the program when its accountable owner records a system scope, architecture-trigger inventory, initial Statement of Applicability, and upgrade plan. Every control receives a decision. The scope includes shared services and suppliers that can defeat the properties being assessed.

A pilot can cover a named flow rather than an entire product. The statement must preserve that boundary. The flow inventory includes origin, recipient, metadata, all plaintext/key holders, processor/provider legs, execution effects, diagnostics, recovery, and offline behavior.

The standard's target applies to new or materially changed protected paths. An existing incomplete path receives a transition finding and exit test. It can continue under its actual operational authority, but it cannot receive full conformance for an unmet property. An exception does not become nonapplicability.

KQRS-006 makes this policy explicit in repository instructions. New code follows applicable controls; refactoring, repair and extension improve relevant legacy boundaries and retain assigned migrations where a justified dependency remains. Adoption records, implementation progress and product qualification remain separate states.

## 5. Compatibility and autonomy

Existing valid standing grants authorize their covered work. KQRS does not add repeated confirmation for the same task or impose arbitrary model-output, context, or task-duration caps. Actual limits on money, infrastructure, tenant resources, and external effects remain enforceable according to the grant.

Stop and revocation fence new effects at the declared boundary. A lost acknowledgement, interrupted tool call, or uncertain external mutation remains uncertain until reconciled. Recovery must not replay an irreversible effect merely to produce a clean status.

Local and federated operation remain first-class. Offline authority uses explicit validity, revocation-staleness, epoch, and reconciliation rules. Blind routing retains ciphertext and local key custody. Necessary plaintext computation occurs inside named purpose-bound domains. No universal central host, mandatory TEE vendor, new paid product, or agent API is imposed.

Ordinary releases follow existing authorization and verification policy. A KQRS conformance statement has its own evidence requirements; that does not turn every release into an independent certification engagement. Target publication alone does not authorize a product shutdown.

## 6. Five workstreams, one evidence lineage

The five established Congress workstreams retain their identities. Each upgrade attaches to the existing workstream, source owner and evidence graph. The internal atlas supplies exact repository revisions and proposed integration points. Documentation completion does not close a product implementation task.

### WS-01: Know the entire system

Reconcile every accessible repository against the declared account boundary. Include application services, desktop runtimes, SDKs, research tools, static sites, redirects, archived workspaces, verification snapshots and generated copies. Record complete tree coverage separately from selected source inspection. Trace applicable first-party authority, data, cryptographic, execution and evidence paths. Identify excluded dependency internals explicitly.

Build the deployment register against that source inventory. A repository head is a source identity, not proof of the version currently serving a request. Bind deployed builds, configuration, owners, providers, clients, keys and shared services to the corresponding path. Select qualification pilots only after this portfolio reconciliation; pilot selection never substitutes for inventory coverage.

**Exit evidence:** Every repository accounted for; canonical implementation owners identified; each applicable flow classified; source-to-deployment discrepancies and unread mechanisms visible; all controls assigned an applicability decision for each assessed scope. Remaining deployment reconnaissance stays open until observed.

### WS-02: Close the cryptographic chain

Construct the obligation graph from transport to recipient envelope, authentication, issuer root, proof relation, update path, custody and recovery. Reuse existing PQ primitives and established envelope/epoch machinery. The integration must preserve destination and transcript binding, supported clients, offline behavior and actual key lifetimes.

Check proof relation correctness before discussing migration: the circuit must constrain the advertised predicate, and the relying action must use the verified public inputs. Then evaluate soundness and witness privacy under the required quantum assumptions. A secure wrapper cannot repair an invalid inner statement.

Qualify compromise behavior and protected-path performance on supported clients. Retained historical recipient keys remain visible in forward-secrecy analysis. Active routes and dormant implementations receive different records. KQRS-CQ-001 and KQRS-LAT-001 define the procedures.

**Exit evidence:** Every required quantum dimension passes within the pilot boundary; downgrade, substitution, compromise, recovery and compatibility tests pass; complete protected workflows meet their predeclared latency/resource class. A design checkpoint can retain an unresolved obligation, but it cannot close that obligation.

### WS-03: Preserve authority through intelligence and effects

Reuse current policy, consent, sandbox, provider-routing, scanner, memory and effect-receipt mechanisms. Connect them across the four defense planes. Repair each identified break in inheritance: consent lost after queueing, sensitivity mismatched at egress, conversation state lacking principal scope, cancellation that never reaches the tool, or uncertainty recorded only after an external effect.

Use a real commit boundary for consequential writes. Preserve standing authority, recheck its current state at the effect, and fence stale attempts. Extend learning and detection without giving the adaptive layer ownership of the trust root. The attacker must be able to challenge the complete composition.

**Exit evidence:** Every active caller maps to an enforced contract; race and recovery tests observe real effects; held-out defender-aware campaigns preserve protected invariants and useful authorized work; detector failure and malicious-planner cases leave independent boundaries effective.

### WS-04: Make evidence operate

Extend existing evidence, audit, source-binding and claim-invalidation machinery. Map CI results, protected verifier outputs, runtime observations and incident findings into one attributable assurance case. Preserve exact subject and evaluator identities. Record the difference between source presence, executed tests and deployed outcomes.

Dependency changes must make downstream claims stale. Exercise propagation through worker restarts, duplicate events, shared foundations and public statement adapters. Keep private evidence within its authorized readers. Start an operating observation period when the collector actually produces retained evidence.

**Exit evidence:** A reproducible current assessment; working collector and claim adapters; protected evidence custody; successful substitution and invalidation challenges; complete operational observation for the stated period. Local schema tests establish the record tooling only.

### WS-05: Challenge, qualify, publish

Run the assessment against the complete pilot contracts. Give defender-aware attackers the declared knowledge and adaptation budget. Keep qualification instances separate from discovery. Measure actual unauthorized effects, legitimate completion, tails, resource use and recovery. Investigate every counterexample and retain it after repair.

Issue precise property results and the whole-scope decision. Publish the specification, methodology and supported findings through the existing publication process. Independent assurance requires the actual assessor relationship stated in the report. Version 1.0 of the specification can coexist with visible product transition work; a product’s qualification follows its own evidence.

**Exit evidence:** Current scoped reports, resolved mandatory failures, supported claims with expiry/invalidation conditions, recorded comparative methodology and publication receipts. Unfinished product work remains attached to its owner and testable exit condition.

## 7. Change and exception procedure

Every normative change records the affected clause/control, rationale, owner intent or threat basis, security effect, affected architectures, compatibility impact, test method, migration path, and rollback. Editorial changes must not alter obligation strength. Stable control identifiers retain their meaning; replacements use explicit supersession.

A proposed weakening requires a standards-level decision with its security consequences visible. A performance miss cannot be cured by privately raising a service's budget. A proof assumption cannot be repaired by renaming the proof. Both create real engineering or profile work.

An exception identifies the unmet requirement, exact scope, reason, compensating protection, accountable owner, actual approving authority, expiry, review trigger, and exit test. Expiry invalidates the associated claim; incident actions and service operation continue under their existing authority. Renewal requires a fresh recorded decision.

## 8. Evidence custody, findings, and incidents

Congress holds program decisions, ownership, work state, source references, and evidence links. Restricted evidence remains in an access-controlled store. The evidence custodian records hashes, source/build/configuration versions, producer, collection time, retention, authorized readers, and the properties supported.

A material incident creates a linked finding, affected scope list, containment decision, recovery plan, and reassessment trigger. The defender may act within a preauthorized response policy. It must preserve tenant isolation, evidence, and essential recovery access, and must not expand its privileges to resolve the incident.

A closed finding retains its original failure and links the corrective implementation and successful retest. Failed, skipped, expired, missing, and excepted evidence cannot be averaged into a pass.

## 9. Operating cadence and metrics

Review material failures and scope changes when they occur. Review privileged access, open exceptions, evidence freshness, supplier changes, and public claims at least quarterly. Exercise critical restoration at least quarterly and incident response at least twice yearly. These are program requirements; no scheduled automation is created by writing this charter.

Measure: covered flows / all applicable flows; current mandatory-control evidence / required evidence; quantum coverage by property and path; plaintext holders by role; unauthorized effects; legitimate-work completion; detector misses and false positives; p95/p99 security overhead; Stop/revocation bounds; restore objectives; unresolved finding age; and independent-assessment coverage.

Every metric identifies its denominator, period, population, configuration, and missing coverage. The program never treats an attractive aggregate as permission to ignore a failed security boundary.

## 10. Publication and research

The Executive Brief explains the program in customer language. KQRS-004 controls product statements and comparisons. Public requirements may be published before every product finishes implementation; they describe the target with the product's actual status separately stated.

KQRS-ASI owns the research horizon for stronger autonomous adversaries. Results move into production requirements through the same source, mechanism, test, compatibility, and change process. The actual trial record names attacker capability and observed outcome.

## 11. Edition record

Version 0.4 expands the source baseline to the complete connected repository inventory, converts generic gaps into source-grounded reuse and repair decisions, strengthens 21 of the 52 stable controls, and adds a dedicated engineering and adversarial qualification volume. It requires composition contracts, effect fencing, proof-relation correctness, adaptive challenge, evaluator separation, exposure lineage and transitive assurance invalidation.

The package includes the standard, operating and assessment program, executive doctrine, technical qualification procedures, internal implementation atlas, editable sources, machine records and validation tools. The edition’s completion receipt states exactly which artifacts and local checks were completed. Product rollout and deployment qualification remain separate workstream outcomes.

The 0.4.1 corrective release adds repository adoption and repairs three record-integrity gaps: configuration binding in completed assessments, fixed catalog membership independent of editable counts, and preregistration of immutable campaign semantics and source/oracle/fixture identities. The 52 controls, cryptographic target and numeric performance classes remain unchanged from 0.4.0.
