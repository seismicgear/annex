# Annex Noncommercial + Protocol-Integrity License

**Version 1.0 — 2026-02-11**

Copyright (c) 2026 Kuykendall Industries LLC
All rights reserved.

This is the "Annex Noncommercial + Protocol-Integrity License" (the "License") governing use of the software known publicly as "Annex" and internally as "Monolith Annex", together with any accompanying documentation, cryptographic circuits, protocol specifications, and configuration artifacts (the "Software").

By using, copying, modifying, or distributing the Software, you agree to be bound by the terms of this License. If you do not agree, you may not use the Software.

---

## 1. Definitions

**1.1** "Licensor" means Kuykendall Industries LLC.

**1.2** "You" or "Your" means the individual or legal entity exercising rights under this License.

**1.3** "Noncommercial Use" means any use of the Software that is not undertaken directly or indirectly for, or materially directed toward, commercial advantage, monetary compensation, or use in the internal operations of a for-profit business or governmental body.

For clarity, the presence or absence of money is not determinative; the overall purpose, context, and effect of the use governs. Uses that are incidental, de minimis, or purely cost-recovery in nature may still be Noncommercial Use if they are not materially directed toward commercial advantage.

Noncommercial Use includes, for example, uses such as: (a) personal experimentation, learning, or self-directed projects; (b) academic or educational research and teaching; (c) nonprofit or public-interest research that is not sold or used to deliver a paid product or service; (d) publishing results, benchmarks, or write-ups that do not provide a paid product or service; (e) volunteer or hobbyist deployments where no organization receives a commercial benefit beyond incidental exposure; (f) self-hosting a server for a personal, family, or community group where no fees are charged and no commercial benefit is derived; and (g) contributing to the Software's open-source development.

**1.4** "Commercial Use" means any use of the Software that is, directly or indirectly: (a) part of or in support of a paid product or service; (b) used in the internal operations of a for-profit business or government entity (including internal tools, automation, communications, or coordination); (c) used to provide consulting or professional services where the Software is a material component of the deliverable; (d) used to host or offer the Software as a managed service, SaaS, API, platform, or hosted communication service, whether free or paid; or (e) used to operate a server or instance for which users pay fees, subscriptions, donations-for-access, or any other form of compensation.

For clarity, Commercial Use includes use that is directed toward commercial advantage even if no money changes hands, and includes "gray area" scenarios such as: supporting fundraising, sponsorships, or sales; enabling lead generation or marketing; embedding in a paid or subscription-gated workflow; charging users for server access, premium features, or enhanced service tiers; renting server capacity to third parties; operating a hosting platform that deploys Annex instances for customers; or use by a government entity in day-to-day operations. The Licensor retains sole discretion to determine whether a use is Commercial Use under this License.

**1.5** "Modified Version" means any work that is based on or derived from the Software, including but not limited to any translation, adaptation, arrangement, transformation, or other alteration of the Software, whether in whole or in part.

**1.6** "Official Protocol" means the VRP (Value Resonance Protocol) trust negotiation protocol, ZKP (Zero-Knowledge Proof) identity circuits, federation protocol, and agent connection protocol as published by the Licensor for a particular version of the Software, including their documented behavior, message formats, and cryptographic requirements.

**1.7** "Protocol Artifacts" means the Circom circuits, trusted setup parameters, verification keys, VRP anchor snapshot formats, federation handshake specifications, and any other cryptographic or protocol-defining artifacts published by the Licensor as part of the Software.

**1.8** "Attestation" means any runtime mechanism in the Software that reports, including but not limited to: (a) the version and build identity of the Software; (b) the hashes or fingerprints of Protocol Artifacts; (c) whether the currently loaded protocol configuration matches the Official Protocol; and (d) any warnings or status flags related to noncanonical protocol modifications.

**1.9** "Safety Mechanisms" means features of the Software that are intended to regulate or constrain behavior for safety, privacy, or integrity reasons, including but not limited to: VRP trust negotiation and alignment enforcement, ZKP membership verification, pseudonym derivation and topic scoping, nullifier tracking and anti-Sybil protections, federation trust evaluation and transfer scope enforcement, capability contract enforcement, agent alignment gating, retention policy enforcement, and Attestation.

**1.10** "Unsafe Operation" means configuring, modifying, or using the Software in a manner that directly or indirectly, and materially, disables or bypasses Safety Mechanisms, or that is reasonably likely to produce behavior that is materially less private, less verifiable, less auditable, or less controllable than the behavior contemplated by the Licensor's published documentation and specifications.

For clarity, Unsafe Operation includes "gray area" practices such as: disabling or bypassing ZKP membership verification to allow unproven identities; removing or falsifying VRP trust negotiation to admit agents or federation peers without alignment evaluation; suppressing or tampering with Attestation signals or safety warnings; deploying with materially reduced logging, monitoring, or audit capability; bypassing nullifier tracking to allow Sybil attacks; disabling retention enforcement to retain data beyond declared policy; removing agent type visibility to conceal AI participant status; or combining the Software with external components that negate or obscure Safety Mechanisms. These examples are illustrative only and do not limit the Licensor's determination of Unsafe Operation.

**1.11** "Required Notices" means any copyright, license, attribution, or similar notices included by the Licensor with the Software that indicate ownership, licensing terms, or acknowledgments and that are designated to be preserved.

**1.12** "Affiliate" means any entity that directly or indirectly controls, is controlled by, or is under common control with a party, where "control" means ownership of more than fifty percent (50%) of the voting equity interests or the power to direct management and policies, whether by contract or otherwise.

**1.13** "Foundations" means the non-negotiable principles documented in FOUNDATIONS.md as published by the Licensor, including but not limited to: prohibition of value extraction from users, prohibition of mandatory identity disclosure, prohibition of centralized control, requirement of first-class agent participation, prohibition of enshittification, requirement of cryptographic integrity, and prohibition of user surveillance.

**1.14** "Operator" means You and any individual or entity that installs, configures, controls, deploys, or uses the Software, including any person or system acting on Your behalf.

**1.15** "Hosting Service" means any arrangement in which the Operator deploys, manages, or provides access to one or more instances of the Software for third parties, whether as a managed service, platform, hosted offering, or any similar arrangement, regardless of whether fees are charged.

---

## 2. License Grant (Noncommercial Use Only)

**2.1** Subject to the terms of this License, the Licensor grants You a nonexclusive, nontransferable, worldwide, royalty-free license to:

(a) use the Software for Noncommercial Use;
(b) make reasonable copies of the Software for backup and archival purposes;
(c) create Modified Versions for Your own Noncommercial Use; and
(d) redistribute unmodified copies of the Software for Noncommercial Use, provided You comply with Section 4 (Distribution Conditions).

**2.2** No Commercial Use is permitted under this License. Any Commercial Use requires a separate written commercial license agreement with the Licensor. This includes, without limitation, any Hosting Service arrangement.

**2.3** No Assignment or Sublicensing. This License is personal to You. You may not assign, transfer, delegate, or sublicense any rights or obligations under this License without the Licensor's prior written consent.

**2.4** Affiliate Use. This License grants rights only to You and does not extend to Your Affiliates unless each Affiliate receives a separate written license from the Licensor. Any use by an Affiliate without such written authorization is unlicensed and prohibited.

---

## 3. Prohibited Uses

**3.1** You may not:

(a) use the Software for any Commercial Use without a separate commercial license from the Licensor;
(b) use the Software to operate or provide a Hosting Service, even if offered free of charge, without a commercial license;
(c) remove, obscure, or alter any copyright, license, or attribution notices included with the Software;
(d) misrepresent any Modified Version as an "official" or "canonical" release of Annex or Monolith Annex; or
(e) use the Software in a manner that violates the Foundations, including but not limited to: implementing advertising or behavioral tracking systems, implementing mandatory identity disclosure mechanisms, implementing centralized control or kill-switch mechanisms not contemplated by the Official Protocol, degrading agent participation to a second-class integration surface, or implementing data collection or surveillance capabilities beyond those contemplated by the Official Protocol.

**3.2** You may not use the Software in violation of applicable law, or to materially facilitate the violation of applicable law by others.

For avoidance of doubt, the mere technical capability of the Software (or any portion of it) to perform, enable, or accelerate an act, outcome, or use case does not constitute permission, authorization, certification, approval, or legal right to do so. The Operator bears sole and exclusive responsibility for identifying, interpreting, and complying with all applicable laws, regulations, standards, and governmental or self-regulatory guidance in every jurisdiction in which the Software is used or whose effects are implicated. The Operator assumes all risk, liability, and responsibility for any use of the Software that is unlawful, noncompliant, or otherwise contrary to such requirements, and the Licensor disclaims any duty to monitor or ensure the Operator's compliance.

**3.3** Safety and ethical use. You must not use the Software to:

(a) intentionally cause or materially contribute to physical harm, serious property damage, or significant interference with critical infrastructure;

(b) intentionally deploy the Software or agents connected to the Software whose primary purpose is harassment, fraud, large-scale deception, unlawful surveillance, or other conduct that would be considered abusive or materially harmful under ordinary standards of ethics and safety;

(c) configure, modify, or operate the Software in a manner that constitutes Unsafe Operation, including but not limited to disabling or bypassing Safety Mechanisms such as VRP trust negotiation, ZKP membership verification, nullifier tracking, agent alignment gating, or Attestation, except to the extent expressly permitted in the Licensor's documentation;

(d) materially conceal or misrepresent the effective safety, privacy, or integrity characteristics or limitations of a server, federation, or agent deployment built on the Software, when such concealment or misrepresentation is likely to cause others to rely on it in a way that exposes them to material harm;

(e) operate a server or instance that falsely represents its VRP policy root, federation alignment status, agent admission criteria, retention policy, or any other operator-declared policy, where such misrepresentation is likely to cause users or federation peers to trust the server under false pretenses; or

(f) use the Software to harvest, correlate, or deanonymize user identities, pseudonyms, or cross-server activity in a manner that defeats the privacy protections of the ZKP identity plane and VRP pseudonym derivation system.

**3.4** The Licensor may determine, in its sole discretion, that a particular use or configuration of the Software is unethical, unsafe, or violates the Foundations under Sections 3.1(e), 3.3, or 5. If the Licensor makes such a determination, Your rights under this License may be terminated immediately in accordance with Section 7, without any obligation on the Licensor to provide a cure period.

**3.5** Hosting Service Notice; Allocation of Risk. You acknowledge and agree that operating a Hosting Service using the Software without a commercial license under Section 2.2 is a material breach of this License. Without limiting Sections 10 and 10.4, You assume all costs, losses, and liabilities arising from or relating to such unauthorized deployment, and You waive any right to seek relief from the Licensor for such consequences. The Licensor reserves all rights to pursue payment of applicable commercial licensing fees and any other remedies available at law or in equity for unauthorized Commercial Use.

---

## 4. Distribution Conditions

**4.1** If You redistribute the Software (modified or unmodified) for Noncommercial Use, You must:

(a) include a complete copy of this License with the Software;
(b) retain all copyright notices and Required Notices;
(c) clearly indicate any changes that You have made to the Software, including in documentation and version identifiers where appropriate;
(d) not remove or disable Attestation mechanisms that report build identity and protocol artifact fingerprints;
(e) not represent Your Modified Version as an official implementation of Annex or Monolith Annex unless explicitly authorized in writing by the Licensor;
(f) include unmodified copies of FOUNDATIONS.md, AGENTS.md, and HUMANS.md, or clearly indicate that these documents have been modified and are not the official versions; and
(g) include this ROADMAP.md or a clear statement that the roadmap has been modified.

**4.2** If You distribute a Modified Version under this License, You must mark it in a manner that reasonably informs recipients that it has been modified and is not an official release from the Licensor.

**4.3** Required Notices and attribution requirements apply to all copies and distributions of the Software (including substantial portions), in both source and any compiled or packaged form. You must preserve Required Notices and attribution statements in a manner reasonably visible to recipients and must not remove or obscure them.

---

## 5. Protocol-Integrity Requirements

**5.1** The Software includes mechanisms (Attestation) that:

(a) compute and report hashes or fingerprints of Protocol Artifacts, including ZKP circuits, VRP configuration, and federation protocol parameters;
(b) indicate whether the loaded protocol configuration matches the Official Protocol published by the Licensor; and
(c) emit warnings when noncanonical or custom protocol configurations are in use.

**5.2** You may configure or modify Protocol Artifacts for Noncommercial Use, but if You distribute any Modified Version:

(a) You must not remove, disable, bypass, falsify, or materially interfere with Attestation behavior that indicates the presence of noncanonical or custom protocol configurations;

(b) You must not remove, disable, bypass, or materially interfere with Safety Mechanisms in a way that results in Unsafe Operation; and

(c) You must not represent a Modified Version using custom protocol configurations as running the Official Protocol.

**5.3** You may not distribute any Modified Version that intentionally conceals or suppresses warnings about noncanonical protocol configurations or Unsafe Operation, or that materially reduces the visibility of Attestation in a way that is likely to mislead recipients about the state, safety, or privacy characteristics of the Software.

**5.4** ZKP Circuit Integrity. The ZKP circuits included with the Software (identity commitment, membership proof, channel eligibility, federation attestation, pseudonym linkage) are safety-critical components. You may not distribute Modified Versions that alter the mathematical properties of these circuits — including soundness, completeness, or zero-knowledge properties — without clearly documenting all changes and their security implications, and without clearly indicating that the circuits are noncanonical. Distributing modified circuits that claim to provide the same security guarantees as the Official Protocol circuits, when they do not, is a material breach of this License.

**5.5** VRP Integrity. The VRP trust negotiation protocol is a safety-critical component. You may not distribute Modified Versions that alter the trust evaluation logic — including anchor comparison, alignment classification, transfer scope negotiation, or reputation scoring — in a way that weakens the trust guarantees of the Official Protocol, without clearly documenting all changes. Distributing a Modified Version that claims VRP alignment guarantees it does not actually enforce is a material breach of this License.

**5.6** Evidence and Records. Attestation outputs and related logs are presumptive evidence of configuration status and compliance with Safety Mechanisms. Any Operator who distributes the Software or deploys it in a public or publicly accessible environment must retain Attestation outputs and related logs for at least twelve (12) months, and must not suppress, falsify, or destroy such records in a manner that would obscure Unsafe Operation. Nothing in this section is intended to require any Operator to waive constitutional, statutory, or other legal rights or privileges; retention duties apply only to the extent permitted by applicable law.

---

## 6. Trademarks and Branding

**6.1** This License does not grant You any rights in the trademarks, service marks, logos, or trade names ("Marks") of the Licensor, including without limitation "Annex", "Monolith Annex", "Monolith Index", "MABOS", "Kuykendall Industries", and any associated logos.

**6.2** You may make factual, truthful statements that You are using or modifying Annex, but You may not use the Marks in a way that suggests sponsorship, endorsement, or official status without prior written permission from the Licensor.

**6.3** The Licensor may publish criteria for "official" builds (for example, matching protocol artifact hashes or being signed with the Licensor's keys). You may not claim to meet such criteria unless those criteria are actually met.

---

## 7. Termination

**7.1** This License automatically terminates for You if:

(a) You use the Software for any Commercial Use without a commercial license; or
(b) You materially breach any other term of this License and fail to cure such breach within thirty (30) days after receiving notice from the Licensor.

**7.2** Notwithstanding Section 7.1(b), if the Licensor determines, in its sole discretion, that:

(a) Your use of the Software constitutes Unsafe Operation;
(b) Your use of the Software violates Section 3.3 (Safety and ethical use) or Section 5 (Protocol-Integrity Requirements); or
(c) Your use of the Software violates the Foundations as described in Section 3.1(e);

then the Licensor may terminate this License for You immediately, without any obligation to provide a cure period, by providing notice (including electronic notice) to You or by publicly identifying the relevant use as unauthorized.

**7.3** Upon termination, You must immediately cease all use and distribution of the Software and destroy all copies in Your possession or control, except where retention is required by law.

**7.4** Termination does not limit any other rights or remedies that may be available to the Licensor under law.

**7.5** Survival. The following sections survive termination of this License: Section 4 (Distribution Conditions, including Required Notices), Section 6 (Trademarks and Branding), Section 10 (No Warranty; Limitation of Liability), Section 14 (Attribution), and Sections 7.3–7.4 (Termination consequences and remedies). These survive to the maximum extent permitted by law.

---

## 8. Notices

**8.1** Any notice required or permitted under this License may be delivered by:

(a) email to contact@montgomerykuykendall.com (for notices to the Licensor) or to the most recent email address You have provided to the Licensor, if any; or
(b) public posting by the Licensor on an official project website, official code repository, or release notes identifying the relevant use.

**8.2** Notices are deemed received at the earliest of:

(a) when the email is sent, if the sender does not receive an automated delivery failure notice within forty-eight (48) hours; or
(b) forty-eight (48) hours after the public posting described in Section 8.1(b).

**8.3** Any cure period referenced in Section 7.1(b) is measured in consecutive calendar days beginning on the day after notice is deemed received under Section 8.2.

**8.4** No Confidential Information. The Software and related materials are provided without any obligation of confidentiality. Do not submit or disclose confidential information to the Licensor unless you have a separate written agreement; any unsolicited confidential information you submit will be deemed non-confidential and may be used without restriction.

---

## 9. Governing Law and Dispute Resolution

**9.1** This License and any dispute, claim, or controversy arising out of or relating to this License or the Software will be governed by the laws of the State of Idaho, United States of America, without regard to its conflict of law rules.

**9.2** The parties agree to the exclusive jurisdiction and venue of the state and federal courts located in Ada County, Idaho, for any dispute, claim, or controversy arising out of or relating to this License or the Software, and waive any objection based on inconvenient forum or lack of personal jurisdiction.

**9.3** Notice-and-Cure Before Litigation. Before filing any lawsuit or other litigation proceeding (other than for injunctive relief under Section 9.4), the complaining party must provide written notice of the dispute and a reasonable opportunity to cure. The notice must describe the alleged breach in reasonable detail. The receiving party will have thirty (30) days after receipt of the notice to cure the alleged breach or to propose a mutually acceptable resolution.

**9.4** Injunctive Relief Carve-Out. Either party may seek temporary, preliminary, or permanent injunctive relief in the courts identified in Section 9.2 to protect its intellectual property rights or the integrity, security, privacy, or attestation of the Software or related systems, without first satisfying the notice-and-cure requirement in Section 9.3.

**9.5** No Change to Substantive Rights. This Section 9 addresses governing law, forum, and dispute resolution procedures only. It does not alter, expand, or limit the substantive rights or obligations in this License, including the license grant, prohibited uses, or conditions of use.

**9.6** If any provision of this Section 9 is held to be invalid, illegal, or unenforceable, the court will reform or blue-pencil that provision to the minimum extent necessary to make it valid, legal, and enforceable, and the remainder of this Section 9 will remain in full force and effect.

---

## 10. No Warranty; Limitation of Liability

**10.1** THE SOFTWARE IS PROVIDED "AS IS" AND "AS AVAILABLE", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, NON-INFRINGEMENT, OR THAT OPERATION OF THE SOFTWARE WILL BE ERROR-FREE, SECURE, OR PRIVATE.

**10.2** TO THE MAXIMUM EXTENT PERMITTED BY LAW, IN NO EVENT WILL THE LICENSOR BE LIABLE FOR ANY INDIRECT, INCIDENTAL, SPECIAL, CONSEQUENTIAL, EXEMPLARY, OR PUNITIVE DAMAGES, OR FOR ANY LOSS OF PROFITS OR REVENUE, LOSS OF DATA, LOSS OF COMMUNICATIONS, BUSINESS INTERRUPTION, LOSS OF GOODWILL, OR COST OF SUBSTITUTE GOODS OR SERVICES, ARISING OUT OF OR IN CONNECTION WITH THIS LICENSE OR THE USE OF THE SOFTWARE, REGARDLESS OF THEORY OF LIABILITY, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGES. THIS LIMITATION INCLUDES ANY CLAIMS BY THIRD PARTIES AGAINST YOU.

**10.3** TO THE MAXIMUM EXTENT PERMITTED BY LAW, THE LICENSOR'S AGGREGATE LIABILITY ARISING OUT OF OR RELATING TO THIS LICENSE OR THE SOFTWARE, REGARDLESS OF THEORY OF LIABILITY, WILL NOT EXCEED ONE HUNDRED U.S. DOLLARS (USD 100).

**10.4** Indemnification. You agree to indemnify and hold harmless the Licensor from any claims, liabilities, damages, losses, and expenses (including reasonable attorneys' fees) arising from or related to (a) Your use, distribution, or modification of the Software for Noncommercial Use; (b) any breach of this License, including any prohibited or unauthorized use; (c) any Unsafe Operation or other prohibited use described in Sections 3, 5, or 13; (d) any content, data, models, integrations, agents, or systems You connect to or use with the Software; or (e) any user data, communications, or privacy incidents occurring on servers or instances You operate. This indemnity applies to claims by third parties.

**10.5** Acknowledgment of Cryptographic and Protocol Risks. You acknowledge that the Software incorporates zero-knowledge proof systems, cryptographic Merkle trees, trust negotiation protocols, and federation mechanisms that are complex and may contain undiscovered vulnerabilities. You assume all risk for deploying the Software, including any risk that cryptographic primitives, ZKP circuits, or protocol implementations may contain errors that affect privacy, integrity, or availability. To the maximum extent permitted by law, You irrevocably waive any claim against the Licensor arising from such vulnerabilities, whether arising in contract, tort, or otherwise.

**10.6** Survival. Sections 10.1 through 10.5 survive termination or expiration of this License to the maximum extent permitted by law.

---

## 11. Severability

**11.1** If any provision of this License is held to be invalid, illegal, or unenforceable by a court of competent jurisdiction, that provision will be enforced to the maximum extent permissible, and the remaining provisions of this License will continue in full force and effect.

---

## 12. Fallback License

**12.1** This Section 12 applies only if a court of competent jurisdiction issues a final, non-appealable judgment that this License is void, invalid, or unenforceable in its entirety with respect to particular uses or within a particular jurisdiction.

**12.2** In that event, and only for those specific uses or within that specific jurisdiction where this License is found unenforceable in its entirety, the Software will instead be licensed under the PolyForm Noncommercial License 1.0.0 (the "Fallback License").

The full text of the Fallback License is available at: https://polyformproject.org/licenses/noncommercial/1.0.0/

**12.3** Where this License is enforceable, the Fallback License does not apply. This License and the Fallback License are not intended to apply simultaneously to the same use in the same jurisdiction.

---

## 13. Commercial Licensing

**13.1** Commercial Use of the Software is not permitted under this License.

**13.2** To obtain a commercial license for the Software, contact:

Kuykendall Industries LLC
Email: contact@montgomerykuykendall.com

The Licensor may offer commercial licensing on terms and pricing to be determined by the Licensor in its sole discretion.

**13.3** Commercial licensing may include, without limitation, licenses for: (a) operating Annex instances as a Hosting Service; (b) internal corporate or enterprise communication deployments; (c) government use; (d) integration of Annex into paid products or services; or (e) operation of federation networks for commercial purposes.

---

## 14. Attribution

**14.1** If You publicly deploy, demonstrate, or publish work that substantially relies on the Software (including but not limited to research papers, technical blog posts, public demos, community servers, or user-facing applications), You must provide reasonable attribution to the Licensor. Reasonable attribution means:

(a) for applications with a user interface, including a notice such as "Powered by Annex — Kuykendall Industries" in an about screen, footer, or comparable location; or

(b) for written works, including a citation or acknowledgment such as "This work uses Annex by Kuykendall Industries LLC."

**14.2** No specific form of endorsement is implied by such attribution, and You may not state or imply that the Licensor has reviewed, approved, or certified Your use unless expressly agreed in writing.

---

## 15. No Re-licensing

**15.1** You may not relicense the Software, in whole or in part, under different terms. Any distribution of the Software or Modified Versions must be made under this License (or under a separate written agreement with the Licensor).

**15.2** You may combine the Software with other software governed by different licenses, provided that:

(a) You do not purport to change the license for the Software itself; and
(b) You make it clear which portions are governed by this License and which portions are governed by other licenses.

**15.3** Nothing in this Section prevents the Licensor from offering the Software under different terms to other parties.

---

## 16. Patent Rights

**16.1** This License grants You rights under the Licensor's copyright interests in the Software only. No patent rights are granted, implied, or waived by this License.

**16.2** If the Licensor later chooses to grant a patent license for particular uses of the Software, such grant must be made in a separate written agreement that expressly refers to patent rights. This License by itself does not confer any such rights.

**16.3** Patent Retaliation. If You initiate or procure a patent claim against the Licensor alleging that the Software (or any portion of it) infringes a patent, then any rights granted to You under this License terminate as of the date such claim is filed. This provision does not apply to defensive claims or counterclaims in response to a patent claim first asserted against You.

---

## 17. Export and Compliance

**17.1** You are responsible for complying with all applicable export control, sanctions, and trade compliance laws in connection with Your use of the Software.

**17.2** You represent that You are not subject to sanctions or export restrictions that would prohibit Your use of the Software, and You agree not to export, re-export, or transfer the Software in violation of applicable export controls or sanctions.

---

## 18. Entire Agreement; Amendment; Order of Precedence

**18.1** This License is the entire agreement between You and the Licensor regarding the Software and supersedes all prior or contemporaneous statements, representations, or understandings, whether oral or written, relating to the Software or its licensing.

**18.2** This License may be amended only by a written instrument signed by the Licensor. No email, website post, or other communication constitutes an amendment unless it expressly states that it amends this License and is signed by the Licensor.

**18.3** Order of Precedence. This License controls over FOUNDATIONS.md, AGENTS.md, HUMANS.md, ROADMAP.md, README.md, and any other documentation, FAQs, guidance documents, or informational materials, unless a document expressly states that it amends this License and is signed by the Licensor. FOUNDATIONS.md describes the project's design principles and values; this License provides the legally binding terms.

**18.4** Interpretation. Headings are for convenience only and do not affect interpretation. The term "including" means "including without limitation." No rule of strict construction applies to the Licensor or You.

---

## 19. Miscellaneous

**19.1** Severability; Blue-Pencil. If any provision of this License is held to be invalid or unenforceable, the court will reform or blue-pencil that provision to the minimum extent necessary to make it valid and enforceable, and the remaining provisions will remain in full force and effect.

**19.2** Waiver. Any failure by the Licensor to enforce a provision of this License is not a waiver of that provision or of the Licensor's right to enforce it later.

**19.3** No Implied Rights. No licenses or rights are granted by implication, estoppel, or otherwise, except for the rights expressly granted in this License. All rights not expressly granted are reserved by the Licensor.

**19.4** Cumulative Remedies. The rights and remedies provided under this License are cumulative and not exclusive of any rights or remedies provided by law.

**19.5** No Agency or Partnership. Nothing in this License creates any agency, partnership, joint venture, or employment relationship between the parties.

**19.6** Assignment. You may not assign, transfer, delegate, or sublicense any rights or obligations under this License except as permitted by Section 2.3.

**19.7** Survival. Definitions, prohibitions, disclaimers, limitations of liability, and enforcement-related provisions (including termination and remedies) survive expiration or termination to the maximum extent permitted by law.

---

**Kuykendall Industries LLC** — Boise, Idaho

*contact@montgomerykuykendall.com*
