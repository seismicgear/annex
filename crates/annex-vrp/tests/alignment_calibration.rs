//! Where the alignment threshold belongs, measured rather than asserted.
//!
//! `agent_min_alignment_score` decides Aligned / Partial / Conflict for every
//! agent registration and every federation handshake. It shipped as `0.8`,
//! chosen against `ConceptEmbedder` — a curated twelve-concept lexicon whose
//! cosine for unrelated text sits near zero.
//!
//! Static embeddings do not behave that way. Ordinary English sentences share a
//! large common direction, so unrelated principle sets start around 0.4 and
//! agreement has to be read against that floor, not against zero. Carrying
//! `0.8` across to the model would reject every genuine peer — the "default
//! that contradicts another default" class CLAUDE.md names, and the same shape
//! as `voice_enabled: true` beside a loopback WebRTC URL.
//!
//! So this file measures. It is a test rather than a script because the number
//! it justifies is a shipped default, and a default justified by a measurement
//! nobody re-runs is a default justified by a memory of a measurement.
//!
//! Run it with output to see the distributions:
//!   cargo test -p annex-vrp --test alignment_calibration -- --nocapture

use annex_vrp::embedding::StaticEmbedder;
use std::path::PathBuf;

/// The repo's `assets/embedding`, not `StaticEmbedder::default_dir()`.
///
/// `DEFAULT_MODEL_DIR` is relative to the working directory, and cargo runs a
/// test with the CRATE root as its working directory — so the default resolves
/// to `crates/annex-vrp/assets/embedding`, which does not exist, and every
/// assertion below would skip while reporting success.
fn model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/embedding")
}
use annex_vrp::semantic::{
    calculate_semantic_alignment, normalize_against_floor, ConceptEmbedder, SemanticEmbedder,
};

/// The corpus is principle SETS, because that is what
/// `calculate_semantic_alignment` actually compares — the centroid of one
/// server's principles against another's, not sentence against sentence.
/// Measuring sentence pairs would produce a number that does not describe the
/// thing being thresholded.
fn sets() -> Vec<(&'static str, Vec<String>)> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    vec![
        (
            "privacy",
            s(&[
                "Users have an absolute right to privacy",
                "We never sell or trade personal data",
                "All messages are end-to-end encrypted",
                "Identity is pseudonymous by default",
            ]),
        ),
        (
            "privacy_paraphrased",
            s(&[
                "Confidentiality belongs to the people using this service",
                "Personal information is never monetised or passed to third parties",
                "Conversations are encrypted from device to device",
                "Nobody has to use their legal name here",
            ]),
        ),
        (
            "sovereignty",
            s(&[
                "Anyone may run their own server",
                "The protocol is open and federated",
                "No central authority can revoke a community",
                "Operators control their own moderation policy",
            ]),
        ),
        (
            "sovereignty_paraphrased",
            s(&[
                "Self-hosting is a first-class option, not an afterthought",
                "The network is decentralised and interoperable",
                "No single company can shut a community down",
                "Each instance sets its own rules",
            ]),
        ),
        (
            "surveillance",
            s(&[
                "User behaviour is logged and profiled for advertising",
                "Engagement is maximised by any means available",
                "Data is sold to partners to fund the platform",
                "Accounts require government-issued identification",
            ]),
        ),
        (
            "moderation",
            s(&[
                "Harassment is removed promptly",
                "Moderators are accountable to the community",
                "Appeals are heard by someone other than the original decider",
                "Rules are published before they are enforced",
            ]),
        ),
        (
            "accessibility",
            s(&[
                "Every control is reachable by keyboard",
                "Colour is never the only carrier of meaning",
                "Text scales without breaking the layout",
                "Screen readers announce state changes",
            ]),
        ),
    ]
}

/// `true` == "an operator would want these two servers to federate".
fn labels() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        ("privacy", "privacy", true),
        ("privacy", "privacy_paraphrased", true),
        ("privacy_paraphrased", "privacy", true),
        ("sovereignty", "sovereignty", true),
        ("sovereignty", "sovereignty_paraphrased", true),
        ("sovereignty_paraphrased", "sovereignty", true),
        ("privacy", "surveillance", false),
        ("privacy_paraphrased", "surveillance", false),
        ("sovereignty", "surveillance", false),
        ("privacy", "accessibility", false),
        ("sovereignty", "accessibility", false),
        ("surveillance", "moderation", false),
        ("privacy", "moderation", false),
        ("sovereignty", "moderation", false),
        ("accessibility", "moderation", false),
        ("accessibility", "surveillance", false),
    ]
}

struct Measurement {
    aligned: Vec<f32>,
    unrelated: Vec<f32>,
    /// The same pairs after `normalize_against_floor` — the values the
    /// threshold is actually compared against.
    aligned_norm: Vec<f32>,
    unrelated_norm: Vec<f32>,
}

impl Measurement {
    fn report(&self, name: &str) {
        let lo = |v: &[f32]| v.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = |v: &[f32]| v.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        eprintln!("\n=== {name} ===");
        eprintln!(
            "  should-align   n={:2}  min={:.4}  max={:.4}  mean={:.4}",
            self.aligned.len(),
            lo(&self.aligned),
            hi(&self.aligned),
            mean(&self.aligned)
        );
        eprintln!(
            "  should-differ  n={:2}  min={:.4}  max={:.4}  mean={:.4}",
            self.unrelated.len(),
            lo(&self.unrelated),
            hi(&self.unrelated),
            mean(&self.unrelated)
        );
        eprintln!(
            "  raw separating band   ({:.4}, {:.4})  width {:+.4}",
            hi(&self.unrelated),
            lo(&self.aligned),
            lo(&self.aligned) - hi(&self.unrelated)
        );
        eprintln!(
            "  norm separating band  ({:.4}, {:.4})  width {:+.4}  midpoint {:.4}",
            hi(&self.unrelated_norm),
            lo(&self.aligned_norm),
            lo(&self.aligned_norm) - hi(&self.unrelated_norm),
            (hi(&self.unrelated_norm) + lo(&self.aligned_norm)) / 2.0
        );
    }

    /// The widest band that separates the two classes on the NORMALISED scale
    /// — the one the threshold is compared against — or `None` if they overlap.
    fn separating_band(&self) -> Option<(f32, f32)> {
        let lo = self
            .aligned_norm
            .iter()
            .cloned()
            .fold(f32::INFINITY, f32::min);
        let hi = self
            .unrelated_norm
            .iter()
            .cloned()
            .fold(f32::NEG_INFINITY, f32::max);
        (lo > hi).then_some((hi, lo))
    }
}

fn measure(embedder: &dyn SemanticEmbedder) -> Measurement {
    let sets = sets();
    let get = |name: &str| -> Vec<String> {
        sets.iter()
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no set named {name}"))
            .1
            .clone()
    };

    let floor = embedder.unrelated_floor();
    let mut m = Measurement {
        aligned: Vec::new(),
        unrelated: Vec::new(),
        aligned_norm: Vec::new(),
        unrelated_norm: Vec::new(),
    };
    for (a, b, want) in labels() {
        let raw = calculate_semantic_alignment(&get(a), &get(b), embedder)
            .unwrap_or_else(|e| panic!("scoring {a} vs {b} failed: {e}"));
        let norm = normalize_against_floor(raw, floor);
        eprintln!(
            "  {}  raw {raw:.4}  norm {norm:.4}  {a} vs {b}",
            if want { "ALIGN " } else { "DIFFER" }
        );
        if want {
            m.aligned.push(raw);
            m.aligned_norm.push(norm);
        } else {
            m.unrelated.push(raw);
            m.unrelated_norm.push(norm);
        }
    }
    m
}

/// The measurement that justifies the shipped default.
///
/// Skipped with a printed reason when the model is absent — a fresh checkout
/// has no `assets/embedding` until `scripts/setup-embedding-model.sh` runs, and
/// a test that silently passes because its fixture is missing is worse than one
/// that fails.
#[test]
fn the_shipped_threshold_separates_the_labelled_corpus() {
    let dir = model_dir();
    if !dir.join("model.safetensors").exists() {
        eprintln!(
            "SKIP: no embedding model at {} — run scripts/setup-embedding-model.sh",
            dir.display()
        );
        return;
    }
    let embedder = StaticEmbedder::load(&dir).expect("the model is present but did not load");

    eprintln!("\npinned model: {}", embedder.fingerprint().model_id);
    let m = measure(&embedder);
    m.report("StaticEmbedder (potion-base-2M)");

    let (lo, hi) = m
        .separating_band()
        .expect("the labelled classes must not overlap — if they do, no single threshold works");

    let shipped = annex_types::ServerPolicy::default().agent_min_alignment_score;
    eprintln!("\nshipped agent_min_alignment_score = {shipped}");
    assert!(
        shipped > lo && shipped <= hi,
        "the shipped default {shipped} does not sit inside the separating band \
         ({lo:.4}, {hi:.4}]. Either the default is wrong or the corpus has moved; \
         decide which, do not adjust the assertion."
    );
}

/// The floor that makes the old default wrong.
///
/// Stated as its own assertion because it is the whole reason the number moved,
/// and because it is the kind of fact that reads as a bug when someone
/// rediscovers it: unrelated English scores far above zero under a static
/// embedding, so a threshold picked for a lexicon rejects everything.
#[test]
fn unrelated_english_does_not_score_near_zero_under_a_static_embedding() {
    let dir = model_dir();
    if !dir.join("model.safetensors").exists() {
        eprintln!("SKIP: no embedding model present");
        return;
    }
    let embedder = StaticEmbedder::load(&dir).expect("load");
    let m = measure(&embedder);

    let floor = m
        .unrelated
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(
        floor > 0.1,
        "unrelated principle sets scored at most {floor:.4}. If that is really near \
         zero the model has changed and the threshold needs re-deriving."
    );
    assert!(
        m.aligned.iter().cloned().fold(f32::INFINITY, f32::min) < 0.8,
        "at least one genuine paraphrase pair scores below 0.8 — which is what \
         makes the old default reject real peers. If every pair now clears 0.8 \
         this file no longer justifies the change and should say so."
    );
}

/// The dev fallback measured on the same corpus, at the same threshold.
///
/// It is not enough that the threshold suits the model: a dev server scoring
/// with the lexicon and a production server scoring with the model must not
/// reach opposite verdicts on the same pair, or "it worked on my machine"
/// becomes a trust decision. That is exactly what the RAW cosines cannot give
/// — their separating bands do not overlap — and exactly what normalising
/// against each scorer's own floor buys.
#[test]
fn both_scorers_reach_the_same_verdict_at_the_shipped_threshold() {
    let shipped = annex_types::ServerPolicy::default().agent_min_alignment_score;
    let sets = sets();
    let get = |name: &str| {
        sets.iter()
            .find(|(n, _)| *n == name)
            .expect("set")
            .1
            .clone()
    };

    let lexicon = ConceptEmbedder::new();
    let lex = measure(&lexicon);
    lex.report("ConceptEmbedder (lexicon fallback, dev only)");

    let dir = model_dir();
    let model = dir
        .join("model.safetensors")
        .exists()
        .then(|| StaticEmbedder::load(&dir).expect("load"));
    if let Some(ref m) = model {
        measure(m).report("StaticEmbedder (potion-base-2M)");
    } else {
        eprintln!("\nSKIP the model half: no embedding model present");
    }

    let mut disagreements = Vec::new();
    for (a, b, want) in labels() {
        let mut verdicts: Vec<(&str, f32, bool)> = Vec::new();
        let mut check = |name: &'static str, e: &dyn SemanticEmbedder| {
            let raw = calculate_semantic_alignment(&get(a), &get(b), e).expect("score");
            let norm = normalize_against_floor(raw, e.unrelated_floor());
            verdicts.push((name, norm, norm >= shipped));
        };
        check("lexicon", &lexicon);
        if let Some(ref m) = model {
            check("model", m);
        }

        for (name, norm, verdict) in &verdicts {
            if *verdict != want {
                disagreements.push(format!(
                    "  {name}: {a} vs {b} normalised {norm:.4} -> {verdict}, wanted {want}"
                ));
            }
        }
        if verdicts.len() > 1 && verdicts[0].2 != verdicts[1].2 {
            disagreements.push(format!(
                "  scorers disagree on {a} vs {b}: {} {:.4} vs {} {:.4}",
                verdicts[0].0, verdicts[0].1, verdicts[1].0, verdicts[1].1
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "at the shipped threshold {shipped}:\n{}\n\nA dev server and a production \
         server would disagree about who is trustworthy.",
        disagreements.join("\n")
    );
}

/// The scale is only portable if the two scorers land in the same place, and
/// that is a measurement, not a hope.
#[test]
fn normalising_puts_both_scorers_aligned_minimum_in_the_same_place() {
    let dir = model_dir();
    if !dir.join("model.safetensors").exists() {
        eprintln!("SKIP: no embedding model present");
        return;
    }
    let model = StaticEmbedder::load(&dir).expect("load");
    let lexicon = ConceptEmbedder::new();

    let m = measure(&model);
    let l = measure(&lexicon);
    let min = |v: &[f32]| v.iter().cloned().fold(f32::INFINITY, f32::min);

    let model_min = min(&m.aligned_norm);
    let lex_min = min(&l.aligned_norm);
    eprintln!(
        "\nnormalised aligned minimum: model {model_min:.4}, lexicon {lex_min:.4}, \
         difference {:.4}",
        (model_min - lex_min).abs()
    );
    assert!(
        (model_min - lex_min).abs() < 0.02,
        "the two scorers' worst genuine pair should land within 0.02 of each other \
         on the normalised scale — model {model_min:.4}, lexicon {lex_min:.4}. If \
         they have drifted apart, one scorer's `unrelated_floor` is stale and a \
         single threshold no longer means the same thing on both."
    );

    // Raw, they are nowhere near each other. Asserted so the value of the
    // normalisation is visible rather than assumed.
    assert!(
        (min(&m.aligned) - min(&l.aligned)).abs() > 0.1,
        "the raw scores were expected to differ substantially between scorers; if \
         they no longer do, the normalisation may be unnecessary"
    );
}
