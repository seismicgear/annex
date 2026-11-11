---
id: public-kqrs-lat-001-performance-profile
title: 'KQRS-LAT-001: Low-Latency Cryptography and Path Qualification'
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
doc_path: docs/kqrs/public/KQRS-LAT-001_Performance_Profile.md
---

# KQRS-LAT-001: Low-Latency Cryptography and Path Qualification

**Edition:** 0.4.1-draft  
**Owner:** Kuykendall Industries, LLC  
**Status:** Normative proposed performance profile  
**Related controls:** KQRS-Q-01, Q-02, Q-05, Q-07, Q-09, Q-10; A-02, A-04, A-09; V-01, V-02 and V-07
**Companion:** KQRS-CQ-001 Cryptographic and Protocol Qualification

## 1. Purpose and qualification rule

KQRS requires post-quantum protection to work within an explicit performance envelope. An algorithm name, a mean primitive timing or a configurable customer budget does not establish low latency. Qualification applies to an identified execution path, client and hardware class, workload and load range.

The numerical limits below are KQRS design requirements introduced in this draft. They are not measurements of KI products or requirements issued by NIST or the IETF. A product is qualified only after it passes the published class on the registered minimum hardware and every supported execution path within its claim.

A low-latency claim MUST satisfy all four conditions:

1. The applicable cryptographic class limits pass.
2. The complete customer operation meets its published absolute service latency and reliability commitments with every required control enabled.
3. Security-negative cases retain the required rejection and containment behavior under nominal load, peak admitted load and overload.
4. The build, configuration, algorithms, endpoints, hardware floor and raw measurements are bound to the assessment record.

## 2. Shared performance classes

The endpoints determine the class of the complete authenticated session. A path whose endpoints are native applications, daemons or hosted services uses LAT-N; a path with a supported browser endpoint uses LAT-W. Browser WebAssembly and workers remain in LAT-W. A TypeScript server remains a service endpoint; its language does not create an exception. Endpoint-local timings are also recorded, but the added-latency ceiling applies once to the complete session, not once per hop or endpoint.

| Class | Intended execution | Added fresh-session p95 | Added fresh-session p99 | Low-latency designation |
|---|---|---:|---:|---|
| LAT-N | Session between native applications, daemons or hosted services | 10 ms | 25 ms | Eligible after all gates pass |
| LAT-W | Session with a supported browser endpoint, including workers and WebAssembly | 30 ms | 100 ms | Eligible after all gates pass |
| LAT-C | Declared constrained devices outside the registered LAT-N/LAT-W floors | Published class-specific limit | Published class-specific limit | No inherited designation |
| LAT-D | Delayed, batch or offline operations | Published turnaround limit | Published turnaround limit | No interactive designation |

“Added” means the difference between candidate and matched-baseline elapsed-time quantiles for the same complete authenticated fresh-session operation. It includes key generation required on that path, validation, key agreement, the reviewed combiner/key schedule, authentication, serialization and transmitted-size effects. It is not the duration of one selected primitive. Candidate and baseline absolute quantiles MUST also be reported.

LAT-C and LAT-D preserve constrained and offline architectures. They must meet the same applicable security requirements and their registered service commitments. A service may describe its precise measured performance in those classes; it may not imply that choosing a slower class qualifies it for LAT-N/LAT-W.

### 2.1 Cold initialization and sustained work

Fresh sessions and cold initialization are separate workloads. A fresh session creates the new secrets and transcript required by the protocol without reusing an old session. A cold initialization begins without the process's cryptographic code, keys, tables, verifier preparation or session cache already initialized.

| Requirement | LAT-N | LAT-W |
|---|---:|---:|
| Cold local cryptographic initialization plus first session, p95 | 250 ms | 1,000 ms |
| Cold local cryptographic initialization plus first session, p99 | 500 ms | 2,000 ms |
| Established-session authenticated bulk throughput relative to matched baseline | At least 95% | At least 95% |
| Maximum individual browser main-thread blocking segment attributable to the path | Not applicable | 50 ms |

Cold local timing excludes network transit but includes actual local initialization and required key work. The complete first-use measurement additionally includes network, code delivery, provider calls and queueing and MUST meet the absolute first-use service commitment. Preloading may improve normal operation but does not remove the cold test.

The throughput comparison uses the same authenticated symmetric protection and payload mix; it checks that adding PQ session protection does not damage established-session operation. Fresh-handshake capacity is reported separately and MUST sustain the registered admitted load while meeting its latency and error limits. No fixed throughput percentage is inferred by comparing unlike cryptographic primitives.

Required ZK verification, policy checks, adaptive defense, model calls and storage operations MUST remain enabled in the complete-path test. Their timing is reported separately so that a fast KEM cannot conceal a slow customer operation. Background preparation is counted in resource and cold-start evidence. An action may not be executed early merely to remove a required security decision from its measured critical path.

### 2.2 Required workflow families

The applicable endpoint class remains LAT-N or LAT-W across the entire claimed workflow. Qualification MUST identify each supported family and include its security-critical path:

| Family | Timed boundary and indispensable work |
|---|---|
| Session establishment | First request to authenticated usable session; discovery/introduction, carrier establishment and any inner authenticated session are separate recorded segments |
| Proof-backed decision | Arrival through parsing, root/circuit selection, verification, expected-context comparison, policy and durable decision or effect admission |
| Signed/encrypted state | Accepted operation through signing/encryption, context binding, manifest/epoch update, storage and required acknowledgement |
| Authorized effect | Intent arrival through current grant/lease/generation check, queueing and actual effect launch; every retry is included |
| Revocation and recovery | Stop/revoke arrival, authoritative fence, cessation of new effects, in-flight quiescence and truthful receipt reconciliation, each with its own timestamp |
| Adaptive policy change | Observation/proposal through verification, challenge, adoption and first governed effect; rejected updates and rollback remain in the corpus |

A shared connection or cached proof may reduce warm latency, but its creation, validity, rotation and invalidation costs remain in the cold/renewal workloads. Precomputation MUST preserve the same protocol security and input binding. If a required proof or decision is produced asynchronously, the complete customer operation still includes the wait for the result before its protected effect.

For every proof workload, declare the relation/circuit, parameter set, public-input size, proof size, batch/recursion depth, prepared-key cache and proving/verifying role. For every model-backed defense, declare model/configuration, input length, batching, queue and fallback behavior. Results from a smaller relation, omitted control or different authority contract cannot qualify the claimed path.

### 2.3 Effect fences and adaptive workload integrity

A Stop acknowledgement MUST distinguish acceptance of the request, commitment of the authoritative fence, cessation of new effects and completion of in-flight work. Report arrival-to-fence and fence-to-quiescence p95/p99 separately against the pre-registered service commitments. Any newly launched unauthorized effect after the fence fails the authority control, even when all latency limits pass. An uncertain earlier operation is retained for observation/reconciliation; it is not replayed as a new operation to make the benchmark finish.

The authorization workload MUST exercise revocation while work waits for a concurrency permit and between retry attempts. Queue and retry delays remain on the original operation's clock. Standing permission for sustained autonomous work is compatible with this test; no arbitrary total-turn limit is introduced.

Adaptive defenses MUST be measured with their current deterministic boundaries enabled. Include a frozen-policy run and a registered adaptation run, recording policy/model revisions and changes. An adaptation run may not train on the held-out qualification outcomes and then count those same cases as unseen evidence. An update that improves attack metrics by blocking authorized work must also pass the declared utility and service commitments.

## 3. Hardware floors and supported clients

The standards program MUST maintain a versioned reference-hardware schedule for each class. Before a result can qualify, that schedule MUST identify actual device or instance model, architecture, assigned cores, memory, operating system, runtime/browser version, power policy and virtualization/container limits. CPU frequency or “modern laptop” alone is insufficient.

Each product MUST test its least capable supported hardware in the relevant schedule and representative currently supported platforms. Where several devices have different limiting characteristics, each is a separate cell. A powerful development workstation cannot stand in for the customer floor. Emulation and CPU throttling can supplement the physical measurements; they do not establish a new physical minimum by themselves.

No hardware floor is fabricated in this publication. Until the schedule names actual tested hardware and the product's supported population is reconciled, the performance result remains unqualified. Service owners may narrow the advertised support scope through the normal product process; they may not silently narrow a benchmark after seeing a failure.

## 4. Matched baseline and representative workloads

The matched baseline MUST preserve the same application work, conventional authenticated protection, payloads, validation, caching policy, network, hardware and runtime. The cryptographic construction being evaluated is the controlled difference. The comparison is executed in a test environment; it does not authorize disabling production protection. Both variants MUST receive equivalent optimization and warm-up treatment. Randomize or counterbalance candidate/baseline execution order and retain environmental observations so thermal, scheduling or cache drift cannot favor one variant. A deliberately slowed baseline is invalid evidence. If a matched baseline preserving the declared application and conventional-protection semantics does not exist, record absolute performance and the unmatched comparison; it cannot pass the added-overhead gate by inventing one.

At minimum, the measurement plan MUST include:

- Fresh sessions, resumed/reused sessions where supported, cold process/browser initialization, reconnect and key rotation.
- Payloads of 1 KiB, 16 KiB and 64 KiB, plus the product's real distribution and maximum supported transfer. Large streaming payloads are measured without requiring all bytes to reside in memory.
- Local overhead isolation and controlled network conditions of 1 ms / 100 Mbps / no loss; 50 ms / 25 Mbps / 0.1% loss; and 150 ms / 5 Mbps / 1% loss, stated as round-trip time, effective bottleneck bandwidth and packet loss. A non-networked component marks those cells inapplicable with its boundary rationale. Customer network classes outside these fixtures require additional cells.
- A single operation, normal admitted workload and maximum admitted workload. Admission volume and concurrency MUST be fixed before the run from the product's capacity contract. A burst at 150% of that volume checks bounded queues, truthful rejection, control integrity and recovery; rejected overload is not silently omitted from results.
- Invalid, malformed, replayed, downgraded and revoked requests mixed with benign traffic. Report malicious request cost and impact on authorized operations.

Shared network fixtures are repeatable test conditions, not promises about public Internet latency. The end-to-end service SLO remains the product's absolute customer-facing requirement. Where public network variability is material, retain the actual path observations in addition to controlled fixtures.

## 5. Measurement and statistical treatment

For each qualifying warm/fresh-session workload cell, collect at least 10,000 completed observations and all failures in each of three separate repetitions. For each cold-initialization cell, collect at least 1,000 independent starts and all failed starts in each of three separate repetitions. Report each repetition separately; a good run cannot average away a failed run.

The harness MUST use a monotonic clock, record queued time from arrival, and maintain an offered-load schedule independent of the system's speed. Closed-loop clients that stop issuing work when the server stalls require explicit coordinated-omission correction or a separate open-loop run. Retries and queue wait belong to the customer operation's timing.

Record p50, p95, p99, sample count, failure count, timeouts, achieved request rate, CPU time, peak and steady memory, bytes sent/received, and cryptographic negotiation results. Retain the distribution or bounded-resolution histogram and its method, not only summary numbers. Timing resolution and histogram precision must be sufficient to evaluate the selected millisecond limits.

Pass decisions use a one-sided 95% upper confidence bound for the p95/p99 added-overhead limits, the cold-local p95/p99 latency limits and the throughput-loss limit, with the estimator declared in advance. Suitable methods include exact order-statistic bounds for independent observations or a documented block bootstrap for temporal correlation. Overhead confidence intervals must include uncertainty from both candidate and baseline. Sparse or correlated samples that cannot support the bound are insufficient evidence. Report the difference of comparable quantiles; do not subtract two unrelated aggregate means or describe their difference as a request-by-request percentile. Where observations are paired, preserve the pairing and state which estimand is being tested. Missing, truncated or censored samples are identified explicitly and cannot silently improve the tail.

All errors remain in the denominator and receive an outcome classification. A timeout is not a successful low-latency response. The full operation's error rate MUST meet its published SLO, and the candidate may not increase non-security failures by more than 0.1 percentage points over the matched baseline at the same admitted load. Any unauthorized disclosure, downgraded acceptance or unauthorized side effect fails the associated security control regardless of latency.

## 6. Class selection, extensions and exceptions

A measurement manifest MUST name the class and reference-schedule revision before execution. Individual product owners cannot replace the table with an easier private threshold. Tighter product SLOs continue to apply.

A new hardware class or a different performance envelope requires a standards-level profile proposal containing the use case, supported population, threat/cryptographic equivalence, measured bottleneck, proposed numeric limits, independent benchmark review and effects on existing claims. Acceptance creates a new versioned class or profile revision. It does not rewrite an old failed result.

An operational exception can authorize continued operation under the program's normal exception process. It cannot award a low-latency pass. LAT-C/LAT-D must not be used to hide a LAT-N/LAT-W failure in a product that still claims to support the original interactive environment.

## 7. Evidence and requalification

The manifest MUST bind scope, path, source/build/config digests, algorithm and library versions, endpoints, reference hardware, selected class, baseline construction, all enabled controls, offered load, sample plan, raw result digests and assessment time. A blank template is not a measurement result. Include workflow family, segment clocks, proof/model workload shape, queue/retry policy, trust-state transitions, effect-fence observations and the exact compiled dependency graph. A connected upstream head does not identify a vendored or submodule dependency unless the build manifest proves that relationship.

Changes to protocol, key/circuit format, library/compiler/runtime, hardware floor, traffic mix, provider, browser, synchronous detector or relevant service limits trigger a documented impact review and targeted remeasurement. Continuous telemetry can detect drift; it does not replace reproducible qualification. Reuse of evidence is permitted only when the relevant dependencies and asserted scope remain unchanged.

## 8. Technical references

NIST FIPS 203 defines ML-KEM and FIPS 204 defines ML-DSA: https://csrc.nist.gov/pubs/fips/203/final and https://csrc.nist.gov/pubs/fips/204/final . Their official pages carry potential-errata notices; the assessment records which publication/errata state was reviewed. RFC 10024 defines reviewed PQ/traditional hybrid key agreement groups for TLS 1.3: https://www.rfc-editor.org/rfc/rfc10024.html . These algorithm/protocol requirements are independent of this document's original performance thresholds.

NIST SP 800-227 supplies KEM-use guidance: https://csrc.nist.gov/pubs/sp/800/227/final . NIST CSWP 39 Update 1 addresses crypto agility while preserving operations: https://csrc.nist.gov/pubs/cswp/39/upd1/considerations-for-achieving-crypto-agility/final . NIST IR 8547, final October 27, 2026, is a transition reference: https://csrc.nist.gov/pubs/ir/8547/final .

The HdrHistogram project's measurement documentation is a primary implementation reference for retaining latency distributions and identifying coordinated omission: https://hdrhistogram.github.io/HdrHistogram/JavaDoc/ . The profile permits equivalent measurement implementations with declared precision and omission treatment.
