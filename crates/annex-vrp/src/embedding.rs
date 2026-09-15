//! A real embedding model for VRP semantic alignment.
//!
//! [`crate::semantic::ConceptEmbedder`] — a curated twelve-concept lexicon plus
//! character-trigram hashing — was the sole input deciding Aligned / Partial /
//! Conflict for every agent registration and every federation handshake. It is
//! a good deterministic heuristic and it is still here as the dev fallback, but
//! it can only see the paraphrases somebody thought to enumerate. "We never
//! trade behavioural data" and "user activity is not a product" share no
//! concept prefix and no trigram of consequence.
//!
//! ## Why a static embedding, not a transformer
//!
//! The model is `minishlab/potion-base-2M` (MIT): a token→vector table
//! distilled from BGE-base-en-v1.5, 29,528 rows × 64 dimensions. Embedding a
//! sentence is a lookup and a mean. Three reasons that shape is right here and
//! a transformer is not:
//!
//! * **Size.** 7.5 MB against ~90 MB for all-MiniLM-L6-v2 in ONNX, on every
//!   desktop installer and every container image.
//! * **Determinism.** This score decides whether a peer is trusted. A mean of
//!   f32 rows accumulated in a fixed token order is reproducible; a
//!   transformer's GEMM kernels are not reproducible across BLAS backends and
//!   CPU feature sets. Two servers scoring the same pair differently is a
//!   disagreement about who is trustworthy, with nothing to surface it.
//! * **Dependencies.** No onnxruntime, no C++ toolchain, no per-platform shared
//!   object inside the bundle.
//!
//! ## Cross-deployment agreement
//!
//! Determinism inside one process is not the property that matters; two peers
//! agreeing is. Three things carry that here:
//!
//! 1. The model files are SHA-256 pinned by `scripts/setup-embedding-model.sh`
//!    and re-verified at load. A different revision does not load.
//! 2. [`ModelFingerprint`] travels in the handshake, so a peer running a
//!    different model is *detected* rather than silently scoring differently.
//! 3. Scores are [quantised](quantize_score) before they meet a threshold, so
//!    a last-bit difference in float accumulation cannot move a pair across
//!    the Aligned/Partial/Conflict boundary.
//!
//! ## Tokenization is not reimplemented
//!
//! WordPiece with BERT normalisation has a lot of corners — NFD accent
//! stripping, CJK spacing, the 100-character word cap, `##` continuations —
//! and getting any of them subtly wrong produces a *slightly* different vector
//! rather than an error. That is the same shape of defect as the relay's
//! canonical signing string: two implementations, each self-consistent, and the
//! pair broken with nothing to notice. So this uses the `tokenizers` crate —
//! the canonical implementation — reading the model's own `tokenizer.json`.
//! `tests/wordpiece_vectors.rs` pins the result against output captured from
//! the same library's Python bindings, so a change in how it is *called* is
//! caught too.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::semantic::SemanticEmbedder;

/// The model this build is pinned to. Must match
/// `scripts/setup-embedding-model.sh`.
pub const MODEL_ID: &str = "minishlab/potion-base-2M";
pub const MODEL_REVISION: &str = "389b9f64be5aa4ae7a6bc6fe95ef20ce485ae5da";
pub const WEIGHTS_SHA256: &str = "f95ffde02ad06f63ae38eb9d400038cd5ccaf8411ec3cb650c6025113f96cbb8";
pub const TOKENIZER_SHA256: &str =
    "e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50";

/// Expected embedding width. Checked at load rather than trusted: a table with
/// the wrong row length would still produce plausible-looking vectors.
pub const EMBED_DIM: usize = 64;

/// Where the model lives relative to the working directory, unless
/// `ANNEX_EMBEDDING_MODEL_DIR` says otherwise.
pub const DEFAULT_MODEL_DIR: &str = "assets/embedding";

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("embedding model file not found: {path}")]
    NotFound { path: String },
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error(
        "embedding model digest mismatch for {path}: expected {expected}, got {actual}. \
         This is not the pinned revision — a peer running the pinned model would score \
         the same principles differently. Re-run scripts/setup-embedding-model.sh."
    )]
    DigestMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("malformed safetensors file {path}: {reason}")]
    MalformedWeights { path: String, reason: String },
    #[error("failed to load tokenizer {path}: {reason}")]
    Tokenizer { path: String, reason: String },
}

/// Identifies the model a deployment is scoring with.
///
/// Carried in the VRP handshake. Alignment is a negotiated verdict, and two
/// servers computing it from different tables are not negotiating — they are
/// each asserting something the other cannot reproduce. Comparing this makes
/// that visible at handshake time instead of never.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelFingerprint {
    /// `minishlab/potion-base-2M`, or `lexicon-v1` for the dev fallback.
    pub model_id: String,
    /// SHA-256 of the weights, hex. Empty for the lexicon fallback, which has
    /// no weights — and that emptiness is itself informative: a peer scoring
    /// with the lexicon cannot be expected to agree with one scoring with the
    /// model.
    pub weights_sha256: String,
}

impl ModelFingerprint {
    /// The fingerprint of the lexicon fallback.
    pub fn lexicon() -> Self {
        Self {
            model_id: "lexicon-v1".to_string(),
            weights_sha256: String::new(),
        }
    }
}

/// Quantise a similarity score before it is compared with a threshold.
///
/// Cosine similarity comes out of a dot product over 64 f32 lanes. Two peers
/// running the same model on different hardware can differ in the last bit or
/// two — enough, for a pair sitting exactly on a threshold, to put one server
/// on the Partial side and the other on Conflict. They would then each be
/// certain and neither would be wrong.
///
/// Rounding to four decimal places is far coarser than any accumulated error
/// (which is ~1e-7 for this many lanes) and far finer than any threshold worth
/// setting, so it removes the disagreement without moving any decision that
/// was not already arbitrary. The raw value is kept separately for reporting:
/// operators should see what was measured, not only what was compared.
pub fn quantize_score(raw: f32) -> f32 {
    if !raw.is_finite() {
        return 0.0;
    }
    (raw * 10_000.0).round() / 10_000.0
}

/// A loaded static embedding table plus its tokenizer.
///
/// The manual `Debug` keeps the 7.5 MB table out of any error message that
/// formats it — `expect_err` on a load failure would otherwise try to print
/// 1.9 million floats.
pub struct StaticEmbedder {
    tokenizer: tokenizers::Tokenizer,
    /// Row-major `rows × EMBED_DIM`.
    table: Vec<f32>,
    rows: usize,
    fingerprint: ModelFingerprint,
}

impl std::fmt::Debug for StaticEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticEmbedder")
            .field("model_id", &self.fingerprint.model_id)
            .field("rows", &self.rows)
            .field("dim", &EMBED_DIM)
            .finish_non_exhaustive()
    }
}

impl StaticEmbedder {
    /// Load from `ANNEX_EMBEDDING_MODEL_DIR`, or [`DEFAULT_MODEL_DIR`].
    pub fn load_default() -> Result<Self, EmbeddingError> {
        Self::load(Self::default_dir())
    }

    pub fn default_dir() -> PathBuf {
        std::env::var_os("ANNEX_EMBEDDING_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_MODEL_DIR))
    }

    pub fn load(dir: impl AsRef<Path>) -> Result<Self, EmbeddingError> {
        let dir = dir.as_ref();
        let weights_path = dir.join("model.safetensors");
        let tokenizer_path = dir.join("tokenizer.json");

        let weights = read_verified(&weights_path, WEIGHTS_SHA256)?;
        // Verified as well as the weights. The tokenizer decides which rows are
        // averaged, so a different tokenizer with the same table produces a
        // different vector just as surely as a different table would.
        let tokenizer_bytes = read_verified(&tokenizer_path, TOKENIZER_SHA256)?;

        let tokenizer = tokenizers::Tokenizer::from_bytes(&tokenizer_bytes).map_err(|e| {
            EmbeddingError::Tokenizer {
                path: tokenizer_path.display().to_string(),
                reason: e.to_string(),
            }
        })?;

        let (table, rows) = parse_safetensors_f32(&weights, &weights_path)?;

        Ok(Self {
            tokenizer,
            table,
            rows,
            fingerprint: ModelFingerprint {
                model_id: MODEL_ID.to_string(),
                weights_sha256: WEIGHTS_SHA256.to_string(),
            },
        })
    }

    pub fn fingerprint(&self) -> &ModelFingerprint {
        &self.fingerprint
    }

    pub fn vocab_size(&self) -> usize {
        self.rows
    }

    /// Token ids for `text`, without `[CLS]`/`[SEP]`.
    ///
    /// model2vec's own `StaticModel.encode` passes `add_special_tokens=False`,
    /// and it has to: the sentence vector is the MEAN of its token rows, so
    /// including two constant special tokens would drag every short sentence
    /// toward the same point and compress exactly the distinctions this is
    /// being asked to measure.
    pub fn token_ids(&self, text: &str) -> Result<Vec<u32>, String> {
        self.tokenizer
            .encode(text, false)
            .map(|e| e.get_ids().to_vec())
            .map_err(|e| format!("tokenization failed: {e}"))
    }
}

impl SemanticEmbedder for StaticEmbedder {
    /// Measured at 0.5134 — see [`SemanticEmbedder::unrelated_floor`] for why
    /// this is not the same number as the lexicon's, and
    /// `tests/alignment_calibration.rs` for the measurement.
    ///
    /// Static embeddings have a high floor because ordinary English sentences
    /// share a large common direction. This is the fact that made the shipped
    /// `agent_min_alignment_score: 0.8` reject every genuine peer: 0.8 is above
    /// what a true paraphrase pair reaches (0.5740 at worst on the corpus), so
    /// the only anchors that ever passed were the ones that matched by hash and
    /// short-circuited before the comparison.
    fn unrelated_floor(&self) -> f32 {
        0.5134
    }

    fn fingerprint(&self) -> ModelFingerprint {
        self.fingerprint.clone()
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let ids = self.token_ids(text)?;
        if ids.is_empty() {
            // Empty or punctuation-only text embeds to the zero vector, which
            // `cosine_similarity` treats as orthogonal — "no signal" rather
            // than "maximally dissimilar". Matches `ConceptEmbedder`.
            return Ok(vec![0.0; EMBED_DIM]);
        }

        // Accumulate in f64 and in token order. Both halves matter: f64 keeps
        // the rounding error far below the quantisation step, and a FIXED
        // order is what makes the result reproducible at all, since f32
        // addition is not associative.
        let mut acc = [0f64; EMBED_DIM];
        let mut counted = 0usize;
        for id in ids {
            let start = (id as usize) * EMBED_DIM;
            let Some(row) = self.table.get(start..start + EMBED_DIM) else {
                // A tokenizer that can emit an id outside the table is a
                // mismatched pair, not a bad input. Both files are digest-
                // pinned, so reaching this means the pin is wrong.
                return Err(format!(
                    "token id {id} is outside the {} row embedding table — the tokenizer and \
                     weights do not belong to the same model",
                    self.rows
                ));
            };
            for (a, v) in acc.iter_mut().zip(row) {
                *a += *v as f64;
            }
            counted += 1;
        }

        let inv = 1.0 / counted as f64;
        let mut out = [0f64; EMBED_DIM];
        for (o, a) in out.iter_mut().zip(acc.iter()) {
            *o = *a * inv;
        }

        // L2 normalise, as the model's `modules.json` specifies. Done in f64
        // for the same reason as the sum.
        let norm = out.iter().map(|v| v * v).sum::<f64>().sqrt();
        let scale = if norm > 0.0 { 1.0 / norm } else { 0.0 };
        Ok(out.iter().map(|v| (v * scale) as f32).collect())
    }
}

/// Read a file and check its SHA-256.
fn read_verified(path: &Path, expected: &str) -> Result<Vec<u8>, EmbeddingError> {
    let bytes = std::fs::read(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            EmbeddingError::NotFound {
                path: path.display().to_string(),
            }
        } else {
            EmbeddingError::Io {
                path: path.display().to_string(),
                source,
            }
        }
    })?;
    let actual = hex(&Sha256::digest(&bytes));
    if actual != expected {
        return Err(EmbeddingError::DigestMismatch {
            path: path.display().to_string(),
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse the single `embeddings` F32 tensor out of a safetensors file.
///
/// The format is a u64 little-endian header length, that many bytes of JSON,
/// then the raw tensor data. Hand-parsed rather than pulling a crate for it:
/// it is a documented twenty-line format and this reads exactly one known
/// tensor. Every field is validated — a header that disagrees with the file
/// length would otherwise produce a silently truncated table, which embeds
/// perfectly plausible nonsense.
fn parse_safetensors_f32(bytes: &[u8], path: &Path) -> Result<(Vec<f32>, usize), EmbeddingError> {
    let malformed = |reason: String| EmbeddingError::MalformedWeights {
        path: path.display().to_string(),
        reason,
    };

    if bytes.len() < 8 {
        return Err(malformed(
            "file is shorter than the 8-byte header length".into(),
        ));
    }
    let header_len = u64::from_le_bytes(bytes[..8].try_into().expect("8 bytes")) as usize;
    let header_end = 8usize
        .checked_add(header_len)
        .ok_or_else(|| malformed("header length overflows".into()))?;
    if header_end > bytes.len() {
        return Err(malformed(format!(
            "header claims {header_len} bytes but only {} remain",
            bytes.len() - 8
        )));
    }

    let header: serde_json::Value = serde_json::from_slice(&bytes[8..header_end])
        .map_err(|e| malformed(format!("header is not valid JSON: {e}")))?;
    let tensor = header
        .get("embeddings")
        .ok_or_else(|| malformed("no `embeddings` tensor in the header".into()))?;

    let dtype = tensor.get("dtype").and_then(|v| v.as_str()).unwrap_or("");
    if dtype != "F32" {
        return Err(malformed(format!("expected an F32 tensor, found {dtype}")));
    }

    let shape: Vec<usize> = tensor
        .get("shape")
        .and_then(|v| v.as_array())
        .ok_or_else(|| malformed("tensor has no shape".into()))?
        .iter()
        .map(|v| v.as_u64().unwrap_or(0) as usize)
        .collect();
    if shape.len() != 2 || shape[1] != EMBED_DIM {
        return Err(malformed(format!(
            "expected a 2-D tensor of width {EMBED_DIM}, found shape {shape:?}"
        )));
    }
    let rows = shape[0];

    let offsets = tensor
        .get("data_offsets")
        .and_then(|v| v.as_array())
        .ok_or_else(|| malformed("tensor has no data_offsets".into()))?;
    if offsets.len() != 2 {
        return Err(malformed("data_offsets is not a pair".into()));
    }
    let start = offsets[0].as_u64().unwrap_or(0) as usize;
    let end = offsets[1].as_u64().unwrap_or(0) as usize;
    if end < start {
        return Err(malformed("data_offsets is inverted".into()));
    }

    let expected_bytes = rows
        .checked_mul(EMBED_DIM)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| malformed("tensor size overflows".into()))?;
    if end - start != expected_bytes {
        return Err(malformed(format!(
            "shape {rows}x{EMBED_DIM} needs {expected_bytes} bytes, data_offsets span {}",
            end - start
        )));
    }

    let data = bytes
        .get(header_end + start..header_end + end)
        .ok_or_else(|| malformed("tensor data extends past the end of the file".into()))?;

    let mut table = Vec::with_capacity(rows * EMBED_DIM);
    for chunk in data.chunks_exact(4) {
        table.push(f32::from_le_bytes(chunk.try_into().expect("4 bytes")));
    }
    Ok((table, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantisation_removes_a_last_bit_disagreement() {
        // Two peers, same model, different summation order in the last lane.
        //
        // The neighbours are derived by stepping the bit pattern rather than
        // written as literals: an f32 literal with more precision than f32 can
        // hold is silently rounded, so a hand-written pair like
        // `0.799_999_94` / `0.800_000_06` can collapse to the same value and
        // the test would pass without ever comparing two different numbers.
        let a = 0.8_f32;
        let b = f32::from_bits(a.to_bits() + 1);
        let c = f32::from_bits(a.to_bits() - 1);
        assert_ne!(a, b, "stepping one ULP must produce a different float");
        assert_ne!(a, c);

        assert_eq!(quantize_score(a), quantize_score(b));
        assert_eq!(quantize_score(a), quantize_score(c));
        assert_eq!(quantize_score(a), 0.8);
    }

    #[test]
    fn quantisation_keeps_real_differences() {
        // Coarse enough to erase float noise, fine enough that nothing an
        // operator would set as a threshold is flattened.
        assert_ne!(quantize_score(0.8001), quantize_score(0.8002));
    }

    #[test]
    fn a_non_finite_score_is_zero_rather_than_a_panic() {
        // A zero-norm vector can only yield 0/0 if the guard above it fails.
        // Thresholding NaN silently answers "false" to every comparison, which
        // reads as Conflict — a verdict nobody computed.
        assert_eq!(quantize_score(f32::NAN), 0.0);
        assert_eq!(quantize_score(f32::INFINITY), 0.0);
    }

    #[test]
    fn a_truncated_safetensors_file_is_rejected() {
        let path = Path::new("x.safetensors");
        assert!(parse_safetensors_f32(&[], path).is_err());
        assert!(parse_safetensors_f32(&[0u8; 4], path).is_err());

        // A header that claims more than the file holds.
        let mut bytes = 9_999u64.to_le_bytes().to_vec();
        bytes.extend_from_slice(b"{}");
        assert!(parse_safetensors_f32(&bytes, path).is_err());
    }

    #[test]
    fn a_shape_that_disagrees_with_the_data_is_rejected() {
        // The case that matters: a well-formed header describing more rows
        // than the payload contains. Accepting it would give a table whose
        // tail is whatever memory follows, and every embedding drawn from
        // those rows would be confident nonsense.
        let header = serde_json::json!({
            "embeddings": {
                "dtype": "F32",
                "shape": [4, EMBED_DIM],
                "data_offsets": [0, 4 * EMBED_DIM * 4],
            }
        })
        .to_string();
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&vec![0u8; 2 * EMBED_DIM * 4]); // half the rows
        assert!(parse_safetensors_f32(&bytes, Path::new("x")).is_err());
    }

    #[test]
    fn a_wrong_width_is_rejected() {
        let header = serde_json::json!({
            "embeddings": { "dtype": "F32", "shape": [2, 32], "data_offsets": [0, 256] }
        })
        .to_string();
        let mut bytes = (header.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&vec![0u8; 256]);
        let err = parse_safetensors_f32(&bytes, Path::new("x")).unwrap_err();
        assert!(err.to_string().contains("width"), "{err}");
    }

    #[test]
    fn the_lexicon_fingerprint_is_distinguishable_from_the_model() {
        let lex = ModelFingerprint::lexicon();
        assert_ne!(lex.model_id, MODEL_ID);
        assert!(
            lex.weights_sha256.is_empty(),
            "the fallback must not claim a digest it does not have — a peer \
             comparing fingerprints has to be able to tell the two apart"
        );
    }
}
