# 0015. VRP Alignment Embedding Model

- **Status**: Accepted (2026-09-15)

- **Context**:
  - `calculate_semantic_alignment` is not a search ranking. Its output decides
    `Aligned` / `Partial` / `Conflict` for **every** agent and federation
    handshake, and that verdict gates message sending, editing, deletion, voice
    media, channel creation and knowledge transfer. A wrong score is an
    authorization error.
  - What shipped was `semantic::ConceptEmbedder`: a curated 12-concept lexicon
    plus character-trigram hashing. It is deterministic and dependency-free,
    which is why it was chosen, and it is also a hand-written instrument whose
    agreement with any other deployment's copy is an accident of the same
    source file being compiled.
  - ROADMAP 3.3 left "integrate a *learned* embedding model behind the
    `SemanticEmbedder` trait" and this ADR unchecked, and Phase 3 stayed
    `PARTIAL` on exactly that gap.
  - Two constraints ruled out most of the field. The build must stay hermetic —
    no C++ toolchain, no onnxruntime, no download at first run — and the score
    must be reproducible across machines, because two servers comparing the
    same pair of anchors have to reach the same verdict or federation means
    nothing.

- **Decision**:
  - **`minishlab/potion-base-2M`** (MIT), revision
    `389b9f64be5aa4ae7a6bc6fe95ef20ce485ae5da`, as `embedding::StaticEmbedder`.
    It is a *static* distilled embedding: a 29,528 × 64 f32 table plus the
    canonical WordPiece tokenizer, so inference is a table lookup and a mean.
    No runtime, no graph executor, no GPU path, 7.5 MB on disk.
  - **Pinned by digest, not by name.** `WEIGHTS_SHA256` and `TOKENIZER_SHA256`
    are constants in `crates/annex-vrp/src/embedding.rs`, checked on load, and
    the same two digests appear in `scripts/setup-embedding-model.sh` and the
    `Dockerfile`. A file that does not hash is a load error, not a warning.
  - **Mandatory under a production profile.** A missing or mismatched model is
    a hard startup failure (`UnusableAlignmentModel`), the same treatment the
    dummy verification key already gets. The lexicon survives as a *dev-only*
    fallback.
  - **Scores are normalised against the loaded scorer's measured noise floor**
    before they meet a threshold, and the raw value is reported separately.
    `SemanticEmbedder::unrelated_floor()` is measured per scorer by
    `annex-vrp/tests/alignment_calibration.rs`: 0.5134 for this model, 0.3060
    for the lexicon.
  - **Quantise before comparing.** `quantize_score` rounds to four decimals —
    coarser than accumulated float error (~1e-7 over 64 lanes) and finer than
    any threshold worth setting — so summation order on a different CPU cannot
    move a pair across a category boundary.
  - **The scorer identifies itself in the handshake.** `ModelFingerprint`
    (`model_id` + `weights_sha256`) travels with the verdict, so a peer running
    a different instrument is *detected* rather than silently disagreeing.
  - **Stored verdicts are re-scored when the instrument changes.**
    `servers.alignment_scorer_id` records `{model_id}@{digest[..16]}`;
    `rescore_alignments_if_scorer_changed` runs before the workers start and,
    on a change, recomputes every stored agent and federation verdict.

- **Consequences**:
  - **The default threshold had to move, and this is the part to read twice.**
    `agent_min_alignment_score` shipped at **0.8** against a raw cosine. On the
    16-pair labelled corpus, genuine paraphrases score 0.5740 at worst and
    unrelated English tops out at 0.5134 — so 0.8 was above *both* bands. It
    did not make the server strict; it made the semantic path unreachable, and
    the only agents ever admitted were those whose anchors matched by hash and
    short-circuited before the comparison ran. The default is now **0.06** on
    the floor-normalised scale, which is 1.000 precision and 1.000 recall on
    that corpus. Migration `046` carries stored thresholds across (0.8 → 0.06,
    other values through `(v - 0.3060) / 0.6940` clamped) and `047` adds the
    scorer column. `server_policy_versions` is deliberately untouched: it is
    the audit trail of what an operator actually set.
  - **The corpus is 16 pairs.** That is enough to establish the bands are
    separated and not enough to call the threshold tuned. It is why Phase 3
    stays `PARTIAL`.
  - **Static embeddings have no word order.** "The server may retain user data"
    and "user data may retain the server" embed identically. For comparing
    value statements this is an acceptable loss and for parsing policy it would
    not be; nothing here should be read as the latter.
  - +7.5 MB on every installer and image, and one more digest-pinned download
    in the Docker build and `scripts/setup-embedding-model.sh`.
  - **Not an API.** A hosted embedding endpoint would have been more accurate
    and would put a third party inside every trust decision, make verdicts
    unreproducible once the vendor updates the model, and break the local-first
    posture ADR-0003 and ADR-0004 already committed to for TTS and STT.

- **Alternatives rejected**:
  - `all-MiniLM-L6-v2` in ONNX (90.4 MB) via `tract-onnx` + `tokenizers`. More
    accurate, pure Rust, and the plan's original choice — dropped for being 12×
    the size for a comparison of short value statements, and for adding a graph
    executor to the startup path of a server that must boot in a container.
  - Keeping the lexicon as the default. Its verdicts are not reproducible by
    any other implementation, which is the property federation needs most.
  - Leaving the threshold at 0.8 and normalising nothing. That is the state
    this ADR replaces, and it read as a strict default while being an
    unreachable one.
