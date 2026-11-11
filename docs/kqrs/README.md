---
id: readme
title: KQRS repository engineering baseline
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
doc_path: docs/kqrs/README.md
---

# KQRS repository engineering baseline

**Kuykendall Industries | 0.4.1-draft | November 11, 2026**

This repository carries the full public KQRS specification, control registry,
record schemas and working forms. The owner directs agents to apply every
applicable requirement to new code and improve touched legacy code whenever
refactoring, fixing or extending it. Repository adoption and product conformance
are separate states.

## Read before changing code

1. [Repository adoption and legacy improvement](public/KQRS-006_Repository_Adoption.md)
2. [Requirements and applicability](public/KQRS-001_Requirements.md)
3. [Engineering and adversarial qualification](public/KQRS-005_Engineering_and_Adversarial_Qualification.md)
4. [Assessment and conformance](public/KQRS-002_Assessment_and_Conformance.md)
5. [Program charter](public/KQRS-003_Program_Charter.md)
6. [Cryptographic qualification](public/KQRS-CQ-001_Cryptographic_Qualification.md)
7. [Performance profile](public/KQRS-LAT-001_Performance_Profile.md)
8. [Advanced-intelligence research profile](public/KQRS-ASI-001_Research_Profile.md)
9. [Public claims and communication](public/KQRS-004_Public_Claims_and_Brief.md)
10. [Executive doctrine](public/KQRS_Executive_Brief.md)

Read the full requirements and engineering contracts, then apply the profiles
whose conditions match the actual architecture and changed paths. A missing
implementation is an outstanding obligation, not a reason to mark its control
inapplicable. Existing local instructions continue to govern delivery, custody
and verification.

## Work the standard into the change

For new code, identify the applicable controls and make their required behavior
part of implementation and verification. For existing code, inspect the touched
path and its immediate authority, data, effect and recovery boundaries; close
feasible gaps in the same authorized change. Reuse stronger existing KI
mechanisms. Record a larger remaining migration with the control, path, reason,
owner, next action and acceptance evidence.

Preserve useful operation, compatibility, privacy, authenticated standing
delegation, owner Stop, local/federated modes and the current optional independent
Warden verifier. This policy does not authorize unrelated rewrites, shutdowns,
new spending, weaker tests or invented approval gates.

## Versioned files and evidence

[release.json](release.json) binds every distributed file to its original source
and installed bytes. Markdown receives a schema-valid repository metadata wrapper;
the published source filenames and document bodies are preserved. These imported
standard files are a versioned package, not newly invented portfolio entities.
[adoption.json](adoption.json) identifies this repository's source baseline and
submission state. The templates remain deliberately unfilled until actual work
supplies evidence; their presence does not award conformance.

The [canonical KQRS repository package](https://github.com/seismicgear/kuykendall-industries-docs/tree/kqrs/adopt-v0.4.1-20261111/docs/kqrs)
also contains the complete implementation atlas, branded PDFs, record validators,
validator tests and controlled provenance. Executable assessment tooling remains
there; this application-repository mirror is documentation, schemas and forms.
The distribution README in the public folder describes that full publication
package. No runtime source, dependency or deployment configuration is changed by
installing this mirror.

Propose changes to the canonical standard with the affected control IDs and
evidence. Refresh a local mirror as an explicit versioned change and verify its
hashes; do not silently alter its normative meaning.
