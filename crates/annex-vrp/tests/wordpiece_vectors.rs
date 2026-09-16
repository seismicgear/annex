//! The tokenizer is half of the embedding, and it has to match everyone else's.
//!
//! [`annex_vrp::embedding::StaticEmbedder`] averages the rows a tokenizer
//! selects. Get the tokenization subtly wrong — one `##` continuation split
//! differently, one accent left on, one CJK character not spaced — and the
//! result is not an error. It is a slightly different vector, a slightly
//! different cosine, and eventually a different Aligned / Partial / Conflict
//! verdict from a peer running the same model. Nothing on either side would
//! notice.
//!
//! `fixtures/wordpiece-vectors.json` was produced by the HuggingFace
//! `tokenizers` Python bindings against the same pinned `tokenizer.json`. This
//! asserts the Rust path reproduces it exactly. The crate under test uses the
//! same library, so this does not prove two independent implementations agree
//! — it pins how that library is *called*, which is where the mistakes
//! actually live: `add_special_tokens` is the one that would silently drag
//! every short sentence toward a constant, and it is a bool argument with no
//! type to protect it.
//!
//! Skipped with a clear message when the model is absent, because a fresh
//! checkout has no `assets/embedding` until
//! `scripts/setup-embedding-model.sh` runs. Skipping is stated rather than
//! silent: a test that quietly passes because its fixture is missing is worse
//! than one that fails.

use annex_vrp::embedding::{StaticEmbedder, EMBED_DIM};
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Vectors {
    model_id: String,
    add_special_tokens: bool,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    text: String,
    ids: Vec<u32>,
    tokens: Vec<String>,
}

fn model_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/embedding")
}

fn vectors() -> Vectors {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wordpiece-vectors.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should exist: {e}", path.display()));
    serde_json::from_str(&raw).expect("vectors should parse")
}

/// `None` (with a printed reason) when the model has not been fetched.
fn embedder() -> Option<StaticEmbedder> {
    let dir = model_dir();
    if !dir.join("model.safetensors").exists() {
        eprintln!(
            "SKIP: no embedding model at {} — run scripts/setup-embedding-model.sh",
            dir.display()
        );
        return None;
    }
    match StaticEmbedder::load(&dir) {
        Ok(e) => Some(e),
        Err(e) => panic!("the model is present but did not load: {e}"),
    }
}

#[test]
fn tokenization_matches_the_reference_for_every_case() {
    let Some(emb) = embedder() else { return };
    let v = vectors();
    assert_eq!(v.model_id, annex_vrp::embedding::MODEL_ID);
    assert!(
        !v.add_special_tokens,
        "the reference must be captured without [CLS]/[SEP]: model2vec averages token \
         rows, so two constant special tokens would pull every short sentence toward \
         the same point"
    );
    assert!(v.cases.len() >= 30, "the fixture looks truncated");

    let mut mismatches = Vec::new();
    for case in &v.cases {
        match emb.token_ids(&case.text) {
            Ok(got) if got == case.ids => {}
            Ok(got) => mismatches.push(format!(
                "  {:?}\n    expected {:?} ({:?})\n    got      {:?}",
                case.text, case.ids, case.tokens, got
            )),
            Err(e) => mismatches.push(format!("  {:?} failed to tokenize: {e}", case.text)),
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} cases tokenize differently from the reference:\n{}",
        mismatches.len(),
        v.cases.len(),
        mismatches.join("\n")
    );
}

/// The corners the fixture exists to cover, named so a regression says which
/// behaviour broke rather than "case 18 differs".
#[test]
fn the_reference_actually_covers_the_hard_cases() {
    let v = vectors();
    let find = |needle: &str| {
        v.cases
            .iter()
            .find(|c| c.text.contains(needle))
            .unwrap_or_else(|| panic!("the fixture should contain a case with {needle:?}"))
    };

    // Over `max_input_chars_per_word` (100) collapses to a single [UNK].
    let long = v
        .cases
        .iter()
        .find(|c| c.text.len() == 101)
        .expect("a 101-character word");
    assert_eq!(long.tokens, vec!["[UNK]"]);

    // Just under the cap still tokenizes normally.
    let short = v
        .cases
        .iter()
        .find(|c| c.text.len() == 99)
        .expect("a 99-character word");
    assert_ne!(short.tokens, vec!["[UNK]"]);

    // Accent stripping: the é must not survive into a token.
    let accented = find("naïve");
    assert!(
        accented.tokens.iter().all(|t| t.is_ascii()),
        "accents should be stripped before WordPiece, got {:?}",
        accented.tokens
    );

    // Subword continuation is exercised at all.
    assert!(
        v.cases
            .iter()
            .any(|c| c.tokens.iter().any(|t| t.starts_with("##"))),
        "no case produces a ## continuation — the fixture is not testing WordPiece"
    );

    // Empty and whitespace-only produce no tokens, which is the branch that
    // returns the zero vector.
    assert!(v.cases.iter().any(|c| c.ids.is_empty()));
}

/// Text that yields no tokens embeds to zeros; text that yields tokens does
/// not — and punctuation is the second kind, not the first.
///
/// This test first asserted that `"!!!"` embedded to zeros, on the assumption
/// that punctuation-only text tokenizes to nothing. It does not: BERT's
/// pre-tokenizer splits punctuation into its own tokens and `!` is vocabulary
/// entry 5, so `"!!!"` is three real tokens and a real unit vector. The
/// assumption was wrong, not the code.
///
/// Worth pinning precisely, because the distinction is load-bearing. Only the
/// empty case reaches the zero vector, which `cosine_similarity` reads as
/// orthogonal — "no signal", the honest verdict for a principle with no words
/// in it. A punctuation-only principle instead gets a low-information vector
/// that will happily score against anything, so an operator who writes `"???"`
/// as a principle gets a comparison rather than an abstention.
#[test]
fn only_tokenless_text_embeds_to_the_zero_vector() {
    use annex_vrp::semantic::SemanticEmbedder;
    let Some(emb) = embedder() else { return };

    for text in ["", "   ", "\t\n", "\u{a0}"] {
        let v = emb.embed(text).expect("embedding should not fail");
        assert_eq!(v.len(), EMBED_DIM);
        assert!(
            v.iter().all(|x| *x == 0.0),
            "{text:?} produces no tokens and must embed to zeros"
        );
    }

    for text in ["!!!", "..."] {
        let v = emb.embed(text).expect("embedding should not fail");
        assert_eq!(v.len(), EMBED_DIM);
        assert!(
            v.iter().any(|x| *x != 0.0),
            "{text:?} tokenizes to real vocabulary entries, so it must not be \
             mistaken for the no-signal case"
        );
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "{text:?} norm {norm}");
    }
}

#[test]
fn embeddings_are_unit_length_and_reproducible() {
    use annex_vrp::semantic::SemanticEmbedder;
    let Some(emb) = embedder() else { return };

    let text = "Consent is required before any data leaves the device";
    let a = emb.embed(text).unwrap();
    let b = emb.embed(text).unwrap();
    assert_eq!(a, b, "the same text must embed identically, bit for bit");

    let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-5,
        "the model declares normalize: true in modules.json, got norm {norm}"
    );
}

/// The reason for replacing the lexicon: paraphrases that share no vocabulary
/// must score closer than unrelated text.
///
/// Asserted as an ORDERING, not against an absolute threshold. Static
/// embeddings have a high similarity floor for ordinary English — unrelated
/// sentences sit near 0.4, not near 0 — so a test written against an absolute
/// number would be pinning a property of the English language rather than of
/// this model, and it would be the wrong number to calibrate thresholds from.
#[test]
fn paraphrases_score_above_unrelated_text() {
    use annex_vrp::semantic::SemanticEmbedder;
    let Some(emb) = embedder() else { return };

    let cos = |a: &str, b: &str| -> f32 {
        let (x, y) = (emb.embed(a).unwrap(), emb.embed(b).unwrap());
        x.iter().zip(y.iter()).map(|(p, q)| p * q).sum()
    };

    let pairs = [
        (
            "users deserve privacy",
            "people are entitled to confidentiality",
            "we monetize attention and sell user data to advertisers",
        ),
        (
            "we never trade behavioural data",
            "user activity is not a product",
            "all content is ranked to maximise time on site",
        ),
        (
            "anyone may run their own server",
            "operators can self-host the software",
            "accounts are issued only by the central authority",
        ),
    ];

    for (a, paraphrase, unrelated) in pairs {
        let near = cos(a, paraphrase);
        let far = cos(a, unrelated);
        assert!(
            near > far,
            "paraphrase should score above unrelated text:\n  {a:?}\n  vs {paraphrase:?} = {near:.4}\n  vs {unrelated:?} = {far:.4}"
        );
    }
}

/// A digest mismatch must refuse to load rather than scoring with whatever is
/// on disk. Two peers with different weights produce verdicts neither can
/// reproduce, and nothing downstream would surface it.
#[test]
fn a_tampered_model_refuses_to_load() {
    let dir = model_dir();
    if !dir.join("model.safetensors").exists() {
        eprintln!("SKIP: no embedding model present");
        return;
    }

    let tmp = tempfile::tempdir().expect("temp dir");
    let mut weights = std::fs::read(dir.join("model.safetensors")).unwrap();
    std::fs::copy(
        dir.join("tokenizer.json"),
        tmp.path().join("tokenizer.json"),
    )
    .unwrap();

    // Flip one byte deep in the tensor data — the file stays the right length
    // and parses fine, which is exactly the case a size check cannot catch.
    let last = weights.len() - 1;
    weights[last] ^= 0x01;
    std::fs::write(tmp.path().join("model.safetensors"), &weights).unwrap();

    let err = StaticEmbedder::load(tmp.path()).expect_err("a tampered model must not load");
    assert!(
        err.to_string().contains("digest mismatch"),
        "expected a digest error, got: {err}"
    );
}

#[test]
fn a_missing_model_reports_the_path_rather_than_a_generic_failure() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let err = StaticEmbedder::load(tmp.path()).expect_err("an empty directory must not load");
    let msg = err.to_string();
    assert!(
        msg.contains("model.safetensors"),
        "the error should name the file an operator has to provide, got: {msg}"
    );
}
