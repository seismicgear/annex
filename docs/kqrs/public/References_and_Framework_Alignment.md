---
id: public-references-and-framework-alignment
title: References and Framework Alignment
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
doc_path: docs/kqrs/public/References_and_Framework_Alignment.md
---

# References and Framework Alignment

**Edition:** 0.4.1-draft. **Status:** Informative, not a certification crosswalk.

## Authored foundation

Montgomery Kuykendall's **Montopian Governance Model** supplies purpose-bound authority, the Trust Fabric, selective disclosure, inspectable algorithms, future cryptography, bounded exceptions and reflex correction. Public provenance: https://www.montgomerykuykendall.com/frameworks/montopia . MABOS's stability writing supplies protected invariants, drift monitoring and recoverable state.

Reference labels in the specification identify: **U1**, the owner's original program direction; **U2**, the mandatory-baseline revision; **U3**, the November 11 direction to inspect every repository and rebuild the program on existing code; **C1**, the original Congress direction and program handoff; **M1**, the Montopian Governance Model; **M2**, the MABOS stability design; **R1-R13**, earlier version-pinned architecture/source reviews; **R14**, the complete v0.4 repository inventory and selected source review; and **X1-X30**, the primary external references below. Internal implementation details remain available to authorized assessors through the exact source register. Source presence does not assert deployment or conformance.

KQRS-CQ-001 uses its own **CQ-R1–CQ-R9** reference labels. The Executive Brief defines its own **S1–S4** source notes. These local labels do not replace the shared source register.

## External references

- **X1:** NIST FIPS 203 (ML-KEM): https://csrc.nist.gov/pubs/fips/203/final
- **X2:** NIST FIPS 204 (ML-DSA): https://csrc.nist.gov/pubs/fips/204/final
- **X3:** NIST approval of FIPS 203/204/205 (including SLH-DSA): https://csrc.nist.gov/News/2024/postquantum-cryptography-fips-approved/
- **X4:** IETF RFC 10024 (hybrid TLS 1.3 groups): https://www.rfc-editor.org/rfc/rfc10024.html
- **X5:** SLSA v1.2: https://slsa.dev/spec/v1.2/ and its Build track at https://slsa.dev/spec/v1.2/build-track-basics
- **X6:** NIST CSF 2.0: https://www.nist.gov/news-events/news/2024/02/nist-releases-version-20-landmark-cybersecurity-framework
- **X7:** NIST SP 800-53A Rev. 5: https://csrc.nist.gov/pubs/sp/800/53/a/r5/final and assessment methods at https://csrc.nist.gov/glossary/term/assessment_method
- **X8:** OWASP ASVS: https://owasp.org/projects/asvs and https://asvs.owasp.org/
- **X9:** ISO/IEC 27001:2022: https://www.iso.org/standard/27001
- **X10:** ISO/IEC 42001:2023: https://www.iso.org/standard/42001

These sources were checked on November 11, 2026. Editions are pinned for this release; later changes require review. Cryptographic errata and library advisories remain part of each implementation's obligations.

- **X11:** NIST IR 8547, final transition report, October 27, 2026: https://csrc.nist.gov/pubs/ir/8547/final
- **X12:** Libsodium sealed-box construction: https://libsodium.gitbook.io/doc/public-key_cryptography/sealed_boxes
- **X13:** NIST PQC FAQ: https://csrc.nist.gov/projects/post-quantum-cryptography/faqs

KQRS-LAT-001 performance limits are original program design requirements. They are not benchmarks reported by these sources. Approved cryptographic algorithms do not by themselves establish a qualified end-to-end protocol or a FIPS-validated module. RFC 10024 addresses its specified TLS construction; another protocol requires its own reviewed binding and composition.

## Engineering research added in edition 0.4

- **X14:** NIST SP 800-160 Vol. 1 Rev. 1, *Engineering Trustworthy Secure Systems*, final November 2022: https://csrc.nist.gov/pubs/sp/800/160/v1/r1/final . Foundation for structured security arguments and assurance throughout the engineering lifecycle.
- **X15:** OMG, *Structured Assurance Case Metamodel*, formal version 2.3, October 2023: https://www.omg.org/spec/SACM/2.3 . Reference for organizing claims, argument and evidence; KQRS does not claim SACM serialization conformance.
- **X16:** Clarkson and Schneider, *Hyperproperties*, Journal of Computer Security 18 (2010): https://www.cs.cornell.edu/fbs/publications/Hyperproperties.pdf . Basis for reasoning about information flow across sets of executions.
- **X17:** Tamarin Prover, official manual, Introduction: https://tamarin-prover.com/manual/master/book/001_introduction.html . Symbolic protocol analysis and explicit adversary/property models.
- **X18:** Debenedetti et al., *Defeating Prompt Injections by Design*, arXiv:2503.18813v2, June 24, 2025: https://arxiv.org/abs/2503.18813v2 . Trusted control/data separation and capability-based restrictions around tool use.
- **X19:** Nasr et al., *The Attacker Moves Second: Stronger Adaptive Attacks Bypass Defenses Against LLM Jailbreaks and Prompt Injections*, USENIX Security 2026: https://www.usenix.org/conference/usenixsecurity26/presentation/nasr . Motivation for defender-aware adaptive evaluation.
- **X20:** Debenedetti et al., *AgentDojo*, NeurIPS 2024 Datasets and Benchmarks: https://proceedings.neurips.cc/paper_files/paper/2024/hash/97091a5177d8dc64b1da8bf3e1f6fb54-Abstract-Datasets_and_Benchmarks_Track.html . Stateful tasks, attacks and utility evaluation.
- **X21:** NIST AI 100-2 E2025, *Adversarial Machine Learning: A Taxonomy and Terminology of Attacks and Mitigations*, March 2025 with published update notes: https://csrc.nist.gov/pubs/ai/100/2/e2025/final . Vocabulary for attacker knowledge, goals, capabilities and lifecycle stages.
- **X22:** Oren et al., *Proving Test Set Contamination in Black-Box Language Models*, ICLR 2024: https://proceedings.iclr.cc/paper_files/paper/2024/hash/46e624c244cff669223d488defd4e835-Abstract-Conference.html . Evidence that benchmark exposure needs explicit analysis; the paper's method depends on its stated exchangeability assumptions.
- **X23:** Sigstore, official security model: https://docs.sigstore.dev/about/security/ . Identity, trust roots and the limits of signed artifact assertions.
- **X24:** SLSA v1.2, *Build: Verifying artifacts*: https://slsa.dev/spec/v1.2/verifying-artifacts . Subject matching, preconfigured trust roots, expected builder and build parameters.
- **X25:** NIST SP 800-227, *Recommendations for Key-Encapsulation Mechanisms*, final September 2025: https://csrc.nist.gov/pubs/sp/800/227/final . KEM use and associated cryptographic obligations.
- **X26:** NIST CSWP 39, update 1, *Considerations for Achieving Crypto Agility*, June 2026: https://csrc.nist.gov/pubs/cswp/39/upd1/considerations-for-achieving-crypto-agility/final . Algorithm, protocol and implementation transition planning.
- **X27:** Groth, *On the Size of Pairing-based Non-interactive Arguments*, EUROCRYPT 2016: https://doi.org/10.1007/978-3-662-49896-5_11 . Exact pairing-based proof construction and assumptions.
- **X28:** Don, Fehr, Majenz and Schaffner, *Security of the Fiat-Shamir Transformation in the Quantum Random-Oracle Model*, CRYPTO 2019: https://ir.cwi.nl/pub/28973/ . Construction-specific quantum-model reasoning; a hash-based label alone is insufficient.
- **X29:** IETF RFC 9420, *The Messaging Layer Security Protocol*: https://www.rfc-editor.org/rfc/rfc9420.html . Group state, epochs and compromise-related properties under the protocol's deletion and update assumptions; no automatic PQ property is inferred.
- **X30:** OWASP, *Top 10 for Agentic Applications for 2026*: https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026/ . Supplemental attack-surface taxonomy; KQRS qualification requires its own executed evidence.

The qualification procedures are KI design requirements informed by these references. Citing a paper does not transfer its benchmark results, proofs or implementation guarantees to a KI product. Exact test parameters and implementation mappings remain in the assessment.

## Thematic framework alignment

| External framework | KQRS relationship | Remaining independent work |
|---|---|---|
| SOC 2 Trust Services Criteria | Governance, access, operations, change, recovery and evidence support similar assurance subjects | Full scoped criteria mapping and any CPA examination remain separate; no SOC 2 report issued |
| ISO/IEC 27001:2022 | G and V families establish risk, ownership, review, correction and evidence discipline | Licensed clause-level mapping, Statement of Applicability and an actual certification engagement if desired |
| ISO/IEC 42001:2023 | A family and algorithm register address AI lifecycle, authority, monitoring and evaluation | Full AI management-system requirements and assessment beyond security alone |
| NIST SP 800-53 Rev. 5 | Q/P/A/I controls overlap access, audit, system protection, configuration, incident and contingency subjects | Exact baseline selection, tailoring and control/enhancement mapping; no High-baseline completion claimed |
| NIST CSF 2.0 | Govern/Identify/Protect/Detect/Respond/Recover provide an organizational view across KQRS | Organization-specific profile and risk outcomes |
| OWASP ASVS 5.0 | I-06 and agent-facing application tests use application-verification practices | Actual version/level-specific application assessment and every applicable test |
| SLSA v1.2 | I-03 and Q-03 address artifact integrity, provenance and signing boundaries | Level-specific source/build platform implementation and verification; no automatic SLSA level |

This table is deliberately thematic. It does not claim a reviewed one-to-one mapping of every external clause. Future detailed mappings must distinguish supports, partially supports, exceeds a named prescription, and not covered. There is no automatic conversion from KQRS conformance into any external credential.
