---
id: public-kqrs-006-repository-adoption
title: 'KQRS-006: Repository Adoption and Continuous Improvement'
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
doc_path: docs/kqrs/public/KQRS-006_Repository_Adoption.md
---

# KQRS-006: Repository Adoption and Continuous Improvement

**Edition:** 0.4.1-draft  
**Issue date:** November 11, 2026  
**Owner:** Kuykendall Industries, LLC  
**Status:** Normative adoption procedure for repositories covered by the owner's direction.  
**Related documents:** KQRS-001 Sections 3, 5 and 8; KQRS-002; KQRS-003 Sections 5 and 7; KQRS-005.

## 1. Apply the standard where work happens

Repository adoption makes KQRS part of ordinary engineering. Applicable controls are mandatory for new code. Existing code improves when it is refactored, extended, repaired or otherwise offers a justified opportunity within the authorized work. Adoption records the standard that governs the work; product conformance still requires its own current evidence.

Every covered repository MUST provide an accessible, version-pinned public KQRS package or an authenticated reference to that package. Its repository-level `AGENTS.md` and `CLAUDE.md` MUST direct contributors to read and apply that package and name the same adopted edition and local entry point. Add a concise instruction file where one is missing, while respecting canonical files and existing links. Existing repository instructions, build commands, test practices, owner authority and useful implementation guidance MUST be preserved. The adoption entry supplements those instructions.

A repository's language, size, archive status or hosting model does not decide applicability. Its actual functions do. Application code, services, agents, scripts, infrastructure, build/release tooling and security-relevant configuration receive the controls triggered by their behavior. A content-only or instruction-only repository does not acquire a nonexistent runtime merely because it receives the standard.

## 2. Read the applicable requirements before implementation

Before changing code, the implementer MUST read the repository's governing instructions, KQRS-001 and this adoption procedure, then the technical and assessment sections applicable to the affected behavior. Relevant cached knowledge is useful, but it must match the repository's adopted edition. The implementer MUST inspect the actual code and callers before deciding that a required mechanism is missing.

The work record identifies the affected behavior, applicable control IDs, existing mechanisms selected for reuse or extension, and meaningful verification. An existing issue, change description, engineering note or authorized program record may carry this information; a second tracking system is not required. The detail is proportional to the change and its security consequences.

Conditional requirements remain conditional. A genuine absence of the triggering function may justify nonapplicability. A difficult implementation, unavailable supplier feature, failed test or missing measurement is an unresolved obligation, not an absent function. Full assessments retain all 52 control decisions. A change-specific list identifies the controls affected by that change and cannot substitute for a full assessment. [KQRS-001 Sections 3 and 5; KQRS-002 Sections 2 and 5]

## 3. New code and new behavior

New or materially changed behavior MUST implement the applicable KQRS requirements within its boundary and preserve the requirements it inherits at every security-relevant handoff. This includes code, schemas, policies, prompts, agent/tool definitions, deployment configuration and infrastructure changes that affect authority, data, custody, execution or evidence.

The implementer MUST:

- Start from existing suitable authority, cryptographic, privacy, containment, recovery and evidence mechanisms, with their caller obligations understood.
- Enforce identity, tenant, purpose and current authority at the actual data or effect boundary; preserve those bindings through queues, retries, child work and recovery.
- Preserve data origin, sensitivity and processing restrictions through transformations, storage and provider egress.
- Keep cryptographic, proof and performance claims attached to their exact implementation and tested scope.
- Verify the affected positive and negative behavior with tests or examinations that can expose a real failure, and retain truthful outcomes.

A new path MUST NOT copy a known insecure pattern merely because legacy code uses it, introduce an avoidable regression, or bypass a required check to finish the task. Introducing a parallel security subsystem requires the bounded search and reuse decision in KQRS-005.

Where an inherited dependency or a justified staged migration prevents immediate completion of a target property, the implementer MUST record the exact unmet control, present protection, risk, dependency, owner, review point and exit test. The release follows the existing transition and release authority in KQRS-001 and KQRS-003. The new work still implements the applicable protections it can establish at its own boundary; a transition does not permit silent omission or an unsupported conformance claim. No new routine permission step is created by recording that work.

## 4. Improve touched legacy code

The improvement rule follows the affected behavior and its security boundary. When authorized work refactors or materially touches a legacy path, the implementer MUST examine the applicable existing protections, preserve what works and improve the relevant gaps that can be corrected within that work. Changes to a helper require inspection of the callers that inherit its guarantee.

“Touched” includes a functional refactor, bug repair, feature extension, dependency/configuration change or discovered security defect affecting the path. A formatting-only edit or an unrelated documentation change does not require a rewrite of every function in the same file. A discovered material defect still receives a truthful finding and disposition.

For each relevant gap, take one of these actions under the current authority:

1. Correct and verify it in the present change when the correction is justified, compatible and within scope.
2. Complete the necessary bounded integration or migration when the existing work provides a suitable opportunity.
3. Record the remaining work with its control, affected path, risk, current protection, owner, reason it cannot be completed now, review trigger or date, dependencies and testable exit condition.

Deferral MUST have a concrete reason such as a protocol transition, external dependency, compatibility constraint or separately scoped architectural change. Convenience alone does not justify carrying a known weak pattern into new behavior. Findings and previous failures remain visible until corrective evidence closes them. Repeatedly touching the same path without addressing its recorded risk requires a fresh rationale, not automatic renewal.

This is a continuing improvement obligation. It does not direct an indiscriminate portfolio rewrite. Incidents and demonstrated urgent failures follow the existing incident authority, with containment scoped to the actual affected boundary. [KQRS-G-06, G-07; KQRS-003 Sections 7 and 8]

## 5. Preserve authority, continuity and operating modes

The owner's current task, authenticated delegation and valid standing grants remain the source of execution authority. Covered work proceeds without repeated confirmation. An uncovered action follows its actual authority policy; neither a model plan nor this document grants new access, spending, disclosure or deployment rights.

KQRS adoption MUST preserve owner Stop, revocation, local custody, offline and federated operation, supported deployment modes and useful authorized long-running work. Resource limits follow the actual grant and system contract. This procedure introduces no arbitrary context, model-output, recursion or total-task-duration cap.

Routine verification follows the repository's current process. Independent Warden verification remains optional unless the owner explicitly selected it. Local verification relevant to the change remains required; a new certification engagement, paid service, universal central host or repeated approval ceremony is not a prerequisite to ordinary development.

Committing the standard or detecting a transition gap does not authorize shutdown, destructive migration, secret rotation, product deployment or a change in customer custody. Those actions retain their existing authority requirements. Documentation adoption must not be reported as completion of any such action. [KQRS-003 Section 5; KQRS-A-02, A-09; KQRS-I-03, I-08]

## 6. Verify the changed boundary and record the result

Verification MUST address the actual risk of the change. Security-relevant behavior requires appropriate legitimate, prohibited, failure and recovery cases. Queue/lease changes need effect-level race checks; cryptographic/proof changes need the corresponding CQ procedures; latency claims need the applicable LAT profile. A documentation-only edit does not need invented product tests.

Every result distinguishes implemented code, inspected test definitions, executed tests, deployed observations and qualification. Report failures, skipped checks and unavailable environments as they are. Record remaining gaps in the existing work system. A passing build or local record validator cannot establish a product security claim by itself.

Assessments MUST bind the exact implementation and configuration. New campaign records MUST bind the candidate, source identities, harness, conditions and each case's immutable semantics, fixture, oracle and planned repetitions before execution. Historical records remain labeled with the edition and evidence they actually support; they are not relabeled to gain a current pass. [KQRS-V-01, V-02, V-07; KQRS-002; KQRS-005 Section 5]

## 7. Distribute and maintain the adopted edition

The repository adoption record MUST name the edition, source/release identity, local document location and retained digest manifest. A digest detects changed bytes after the expected digest is obtained through trusted custody; it does not authenticate an arbitrary downloaded package by itself.

Public repositories receive public specifications and suitable public tooling. Restricted implementation findings, private source locators, customer material, credentials and evidence stay inside their authorized audience. A public reference may point to a controlled internal work item without exposing its contents.

Generated files, copied workspaces and vendored code retain their provenance. Prefer changes in the canonical source and its normal generation or dependency-update process. Do not patch every generated or third-party copy independently merely to satisfy an adoption check.

An edition update records the changed obligations, affected implementations and necessary targeted verification. Existing valid evidence may be reused only where its subject, assumptions and criteria remain sufficient. Incomplete migrations stay assigned to their existing workstreams. Installing a newer document never closes an implementation task automatically.

## 8. Minimum repository instruction

The adoption section in each repository's maintained instruction files MUST convey these requirements in the repository's own format:

> Read the adopted KQRS edition and apply its applicable requirements to all new code and materially changed behavior. Reuse existing suitable mechanisms. When refactoring, repairing or extending legacy code, improve the relevant security boundaries and track any justified remaining migration with an owner and exit test. Preserve current authenticated authority, standing grants, owner Stop and supported local/offline/federated modes. Perform relevant verification and report the actual result. Documentation adoption does not establish product conformance or add routine approval gates; independent Warden review remains optional unless explicitly selected by the owner.

The instruction MUST include the repository's actual local entry path and edition. It MUST NOT replace useful existing instructions or claim that every control already passes.
