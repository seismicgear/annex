---
id: public-kqrs-asi-001-research-profile
title: 'KQRS-ASI-001: Frontier Adversary Research Profile'
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
doc_path: docs/kqrs/public/KQRS-ASI-001_Research_Profile.md
---

# KQRS-ASI-001: Frontier Adversary Research Profile

**Edition:** 0.4.1-draft  
**Status:** Research and evaluation profile. Not an operational conformance tier or an ASI-immunity claim.

## 1. Mission — preserve sovereignty under superior intelligence

KQRS-ASI develops architectures intended to retain human-authorized control, private custody and recoverable operation when an adversary can reason faster, coordinate more broadly and adapt more effectively than the defender’s operators. The research target includes superhuman offensive capability. The engineering question is precise: which authority, information and recovery boundaries continue to hold as the attacker’s capability increases?

The program concentrates power in small enforceable mechanisms and distributes intelligence around them. A compromised planner may produce persuasive plans. A compromised detector may produce persuasive warnings. Neither receives the ability to manufacture authority, choose its own trusted evaluator or erase the history required for recovery. Those boundaries become the object of formal analysis, implementation tests and progressively stronger campaigns.

## 2. Attacker capability declaration

Every evaluation specifies the attacker's model/tool access, source and configuration knowledge, credentials, compute/time/query budget, coordination, persistence, allowed social engineering simulation, supply-chain powers and access to compromised components. Separate quantum cryptanalysis assumptions from intelligence capability. "ASI" is not a reproducible test parameter by itself.

## 3. Research tracks

**R-ASI-01: Minimal trusted computing base.** Reduce independently enforced authority, key and recovery boundaries to small components suitable for rigorous review or formal verification. Evaluate whether a compromised model can change the policy, evaluator, signer or evidence that constrains it.

**R-ASI-02: Compromise-tolerant intelligence.** Assume a planner, model provider, detector or peer agent is malicious. Test separation between recommendation and capability, scoped credentials, private memory, cross-agent authority and persistent state.

**R-ASI-03: Automated adaptive attack.** Use bounded adversarial agents that iterate across software, prompts, tools, retrieval and exposed infrastructure in isolated test environments. Measure actual prohibited effects, not whether a defender refuses in prose.

**R-ASI-04: Defense against attack on the defense.** Evaluate detector poisoning, evasion, telemetry manipulation, false-positive denial of service, model supply-chain compromise and attempts to disable stop or recovery. Defenses must retain their own bounded authority.

**R-ASI-05: Cryptographic and physical trust assumptions.** Analyze future cryptanalytic change, endpoint compromise, malicious update distribution and hardware/operator trust. Do not assume an encryption primitive can protect plaintext at a fully compromised authorized endpoint.

**R-ASI-06: Recovery under adversarial persistence.** Test restoration from compromised memory, stolen credentials, poisoned backups, false evidence and coordinated tenant attacks. Recovery must not reimport authority or state that the incident invalidated.

## 4. Progressive evaluation campaign

The program advances through explicit campaigns. More capable adversaries receive broader knowledge, tools, coordination or persistence according to the authorized experiment. Capability bands are test descriptions rather than a claim to have instantiated ASI.

| Campaign | Adversary conditions | Required result and evidence |
|---|---|---|
| ASI-R1: Known boundary attacks | Reproducible software and instruction attacks with fixed privileges | Baseline vulnerabilities, legitimate-work outcomes and enforced invariants |
| ASI-R2: Adaptive autonomous attacker | Iterative tool use, prior feedback, withheld targets and a declared resource budget | Attack trajectories, success rate, attack cost, defense misses and tail latency |
| ASI-R3: Compromised intelligence | Malicious planner, provider, detector or peer with normal component privileges | No self-issued authority or cross-tenant effect; narrow containment and usable recovery |
| ASI-R4: Coordinated persistent compromise | Several compromised components, poisoned memory/updates and delayed effects in an isolated range | Recovery without revived credentials, forged evidence or repeated irreversible effects |
| ASI-R5: Stronger-than-evaluated adversary analysis | Explicit hypothetical capability increments, cryptanalytic assumptions and resource projections | Threat argument, surviving narrow invariants, exposed assumptions and the next testable upgrade |

Every campaign includes an authorized benign workload. A system that prevents every attack by refusing all useful work fails the operational objective. Evaluation also measures disclosure, persistent authority change, resource exhaustion, recovery time and detector-induced denial of service.

### 4.1 Invariants worth proving

Formal and empirical work prioritizes capability non-expansion, tenant separation, endpoint key custody, untrusted-data separation, immutable evidence history, revocation fencing, and idempotent reconciliation. The trusted mechanism must remain small enough to inspect. A formal property is linked to the code/configuration that implements it and to tests for assumptions outside the model.

### 4.2 Independent challenge and reproducibility

Record evaluator control and conflict relationships. Withheld cases and adversarial agents provide useful challenge; external independence requires an actual separate assessor relationship. Publish safe reproducibility material and controlled access to sensitive evidence. A successful campaign retains its failed attempts and known counterexamples.

### 4.3 Capability frontier and failure map

Each campaign varies explicit attacker capabilities while retaining a comparable task population. Record source visibility, stolen credentials, component compromise, coordination, persistence, tool access, query budget and available cryptanalysis separately. Plot the frontier at which a protected property fails, alongside useful-work completion and recovery. A stronger attacker trial that fails is valuable evidence for the next architectural change.

Priority experiments combine capabilities that single-surface testing misses: poisoned memory plus a delayed tool action; stolen evaluator credentials plus a source substitution; revocation plus network partition; a malicious model plus a provider-routing inconsistency; a compromised update service plus retained recovery keys. Each experiment has a concrete effect oracle and an authorized legitimate control.

### 4.4 Challenge the defense’s knowledge

The attacker may inspect the published architecture and adapt to the defense. Qualification material remains separate from discovery and training. Fresh instances are generated after the candidate is frozen; exposure lineage and compromised pools are retained. The evaluator tests whether the defense can distinguish authentic authority from convincing content and whether a useful response survives the attacker’s manipulation. KQRS-005 defines the campaign and evidence contract. [X18–X22]

## 5. Required research evidence

Record the exact attacker/defender configuration, sandbox and authority, task population, withheld cases, success conditions, attempts, failures, cost/resource use, observed side effects, reproducible artifacts and remaining uncertainty. Compare stronger attacker configurations over time without silently changing the tested task population.

Use formal arguments where possible for narrow invariants such as capability non-expansion, signature verification behavior, state-machine transitions and isolation. A proof is scoped to its model and assumptions; the experiment must also test whether the real implementation matches those assumptions.

## 6. Promotion into production requirements

A research finding enters KQRS production requirements only with a concrete threat, actionable control, test method, implementation feasibility/compatibility assessment and change record. Experimental features are isolated and clearly labeled. Useful improvements may be adopted before the entire ASI research objective is resolved.

## 7. Permitted public wording

"KQRS-ASI develops sovereign systems for a future in which the adversary may be more intelligent than the operator. We test the boundaries that must continue to hold: authority, custody, evidence and recovery."

Results identify the specific invariant or attack class resisted under the documented conditions. KQRS-ASI readiness is the program objective; each published result remains attached to a reproducible capability profile.
