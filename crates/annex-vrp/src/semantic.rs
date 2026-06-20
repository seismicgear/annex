use std::collections::{BTreeSet, HashMap};

/// A trait for text embedding models.
pub trait SemanticEmbedder {
    /// Embeds a text string into a vector of floats.
    fn embed(&self, text: &str) -> Result<Vec<f32>, String>;
}

/// A bag-of-words embedder that creates sparse TF vectors from text.
///
/// No external dependencies — tokenizes on whitespace and punctuation,
/// lowercases, and builds a vector in a shared vocabulary space.
/// Suitable for principle-level text comparison without an ML model.
pub struct BagOfWordsEmbedder {
    /// Vocabulary → dimension index mapping built from all texts seen.
    vocab: HashMap<String, usize>,
}

impl BagOfWordsEmbedder {
    pub fn new() -> Self {
        Self {
            vocab: HashMap::new(),
        }
    }

    /// Pre-builds vocabulary from all principle texts before embedding.
    pub fn build_vocab(&mut self, texts: &[String]) {
        let mut sorted_words = BTreeSet::new();
        for text in texts {
            for word in tokenize(text) {
                sorted_words.insert(word);
            }
        }
        self.vocab.clear();
        for (idx, word) in sorted_words.into_iter().enumerate() {
            self.vocab.insert(word, idx);
        }
    }
}

impl Default for BagOfWordsEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticEmbedder for BagOfWordsEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        if self.vocab.is_empty() {
            return Err("Vocabulary not built — call build_vocab first".to_string());
        }
        let dim = self.vocab.len();
        let mut vec = vec![0.0f32; dim];
        for word in tokenize(text) {
            if let Some(&idx) = self.vocab.get(&word) {
                vec[idx] += 1.0;
            }
        }
        // L2 normalize
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        }
        Ok(vec)
    }
}

/// Tokenizes text into lowercase words, stripping punctuation.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2) // Skip single-char tokens
        .map(|w| w.to_string())
        .collect()
}

/// Concept families for the values-alignment domain. Each entry maps a set of
/// stem prefixes to a stable concept id; any token starting with one of those
/// prefixes contributes to that concept's dimension. This is what lets
/// *paraphrases* align: "privacy", "confidential", and "anonymity" all land on
/// the `privacy` concept dimension even though they share no characters, so two
/// principles that express the same value with different words score as similar.
///
/// The list is curated for Annex's ethical-root vocabulary (FOUNDATIONS.md):
/// privacy, anti-surveillance, sovereignty, decentralization, consent, freedom,
/// cryptographic integrity, identity, anti-censorship, anti-extraction,
/// transparency, and equality. It is deliberately small and auditable rather
/// than a giant learned table — and the [`SemanticEmbedder`] trait keeps a real
/// learned model pluggable when latency/size budget allows (ROADMAP 3.3).
const CONCEPT_FAMILIES: &[(&[&str], usize)] = &[
    (
        &[
            "privac",
            "privat",
            "confiden",
            "anonym",
            "pseudonym",
            "secre",
        ],
        0,
    ),
    (
        &["surveil", "monitor", "track", "spy", "profil", "harvest"],
        1,
    ),
    (
        &[
            "sovereign",
            "autonom",
            "ownership",
            "self-host",
            "selfhost",
            "independ",
        ],
        2,
    ),
    (
        &["decentral", "federat", "distribut", "peer", "mesh", "p2p"],
        3,
    ),
    (&["consent", "voluntar", "opt-in", "optin", "permission"], 4),
    (&["freedom", "liberty", "right", "free"], 5),
    (
        &[
            "cryptograph",
            "encrypt",
            "zero-knowledge",
            "zeroknowledge",
            "zkp",
            "verif",
            "integrity",
            "proof",
        ],
        6,
    ),
    (&["identit", "credential", "keypair", "pseudonymou"], 7),
    (&["censor", "deplatform", "suppress", "silenc", "ban"], 8),
    (
        &[
            "monetiz",
            "advertis",
            "exploit",
            "extract",
            "sell",
            "engagement",
        ],
        9,
    ),
    (
        &[
            "transparen",
            "auditab",
            "accountab",
            "verifiab",
            "open-source",
            "opensource",
        ],
        10,
    ),
    (&["equal", "fair", "equit", "first-class", "equals"], 11),
];

const N_CONCEPTS: usize = 12;
/// Hashing-trick dimensions for raw tokens and char-trigrams (morphological
/// robustness, e.g. "private" ↔ "privacy" share trigrams).
const HASH_DIM: usize = 256;
const EMBED_DIM: usize = N_CONCEPTS + HASH_DIM;
/// Concept hits dominate the vector so paraphrases align strongly; raw lexical
/// features still differentiate within a concept.
const CONCEPT_WEIGHT: f32 = 3.0;
const TOKEN_WEIGHT: f32 = 1.0;
const TRIGRAM_WEIGHT: f32 = 0.5;

/// Stable FNV-1a hash → bucket in `[0, HASH_DIM)` with a sign bit (signed
/// hashing trick reduces collision bias).
fn hash_bucket(s: &str, salt: u8) -> (usize, f32) {
    let mut h: u64 = 0xcbf29ce484222325 ^ (salt as u64);
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let idx = (h % HASH_DIM as u64) as usize;
    let sign = if (h >> 63) & 1 == 1 { 1.0 } else { -1.0 };
    (idx, sign)
}

fn concept_for(token: &str) -> Option<usize> {
    for (prefixes, concept) in CONCEPT_FAMILIES {
        for p in *prefixes {
            if token.starts_with(p) {
                return Some(*concept);
            }
        }
    }
    None
}

/// A deterministic, fixed-dimension **concept embedding** for value-principle
/// text. Unlike [`BagOfWordsEmbedder`] (which needs a jointly-built vocabulary
/// and scores paraphrases near zero), this embeds every principle into the same
/// `EMBED_DIM`-dimensional space independently — so a local server and a remote
/// peer produce directly-comparable vectors with no shared vocabulary, and
/// principles that express the same value with different words align.
///
/// It is **not** a learned neural model; it is a curated concept lexicon plus a
/// character-trigram hashing trick. That is an honest, deterministic, zero-
/// dependency upgrade that fixes the paraphrase-misclassification failure
/// without bundling a multi-hundred-MB model into a sovereign desktop app. The
/// [`SemanticEmbedder`] trait keeps a learned model pluggable if a deployment
/// accepts the size/latency cost (ROADMAP 3.3).
pub struct ConceptEmbedder;

impl ConceptEmbedder {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ConceptEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticEmbedder for ConceptEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let mut vec = vec![0.0f32; EMBED_DIM];
        let tokens = tokenize(text);
        if tokens.is_empty() {
            // Empty / punctuation-only principle: return the zero vector. The
            // cosine helper treats a zero-norm vector as orthogonal, which is
            // the right "no signal" behaviour.
            return Ok(vec);
        }
        for token in &tokens {
            // Concept dimension (paraphrase bridge).
            if let Some(c) = concept_for(token) {
                vec[c] += CONCEPT_WEIGHT;
            }
            // Whole-token hashed feature (within-concept discrimination).
            let (idx, sign) = hash_bucket(token, 0);
            vec[N_CONCEPTS + idx] += TOKEN_WEIGHT * sign;
            // Char-trigram features (morphological robustness).
            let chars: Vec<char> = token.chars().collect();
            if chars.len() >= 3 {
                for w in chars.windows(3) {
                    let tri: String = w.iter().collect();
                    let (ti, ts) = hash_bucket(&tri, 1);
                    vec[N_CONCEPTS + ti] += TRIGRAM_WEIGHT * ts;
                }
            }
        }
        // L2 normalize.
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        }
        Ok(vec)
    }
}

/// A mock embedder for testing purposes.
/// Maps known strings to pre-defined vectors.
pub struct MockEmbedder {
    embeddings: HashMap<String, Vec<f32>>,
}

impl MockEmbedder {
    pub fn new() -> Self {
        Self {
            embeddings: HashMap::new(),
        }
    }

    pub fn insert(&mut self, text: &str, vector: Vec<f32>) {
        self.embeddings.insert(text.to_string(), vector);
    }
}

impl Default for MockEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticEmbedder for MockEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        self.embeddings
            .get(text)
            .cloned()
            .ok_or_else(|| format!("No embedding found for: {text}"))
    }
}

/// Calculates the cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot_product / (norm_a * norm_b)
    }
}

/// Computes the centroid (mean vector) of a list of embeddings.
fn compute_centroid(
    principles: &[String],
    embedder: &impl SemanticEmbedder,
) -> Result<Vec<f32>, String> {
    if principles.is_empty() {
        return Ok(Vec::new());
    }

    let mut sum_vec: Vec<f32> = Vec::new();
    let mut count = 0;

    for principle in principles {
        let embedding = embedder.embed(principle)?;
        if sum_vec.is_empty() {
            sum_vec = embedding;
        } else {
            if sum_vec.len() != embedding.len() {
                return Err("Embedding dimension mismatch".to_string());
            }
            for (i, val) in embedding.iter().enumerate() {
                sum_vec[i] += val;
            }
        }
        count += 1;
    }

    if count == 0 {
        return Ok(Vec::new());
    }

    let centroid: Vec<f32> = sum_vec.into_iter().map(|val| val / count as f32).collect();
    Ok(centroid)
}

/// Calculates the semantic alignment score between two sets of principles.
///
/// Returns a score between 0.0 (completely orthogonal) and 1.0 (perfectly aligned).
/// This implementation computes the cosine similarity between the centroids of
/// the embedded principles.
pub fn calculate_semantic_alignment(
    local_principles: &[String],
    remote_principles: &[String],
    embedder: &impl SemanticEmbedder,
) -> Result<f32, String> {
    if local_principles.is_empty() && remote_principles.is_empty() {
        return Ok(1.0); // Both empty = aligned
    }
    if local_principles.is_empty() || remote_principles.is_empty() {
        return Ok(0.0); // One empty, one not = conflict? Or maybe neutral. Let's say 0.0 for now.
    }

    let local_centroid = compute_centroid(local_principles, embedder)?;
    let remote_centroid = compute_centroid(remote_principles, embedder)?;

    if local_centroid.is_empty() || remote_centroid.is_empty() {
        return Ok(0.0);
    }

    Ok(cosine_similarity(&local_centroid, &remote_centroid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let v1 = vec![1.0, 0.0];
        let v2 = vec![1.0, 0.0];
        assert!((cosine_similarity(&v1, &v2) - 1.0).abs() < 1e-5);

        let v3 = vec![0.0, 1.0];
        assert!((cosine_similarity(&v1, &v3)).abs() < 1e-5);

        let v4 = vec![-1.0, 0.0];
        assert!((cosine_similarity(&v1, &v4) - -1.0).abs() < 1e-5);
    }

    #[test]
    fn test_calculate_semantic_alignment_identical() {
        let mut embedder = MockEmbedder::new();
        embedder.insert("A", vec![1.0, 0.0]);
        embedder.insert("B", vec![0.0, 1.0]);

        let principles = vec!["A".to_string(), "B".to_string()];
        let score = calculate_semantic_alignment(&principles, &principles, &embedder).unwrap();
        assert!((score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_calculate_semantic_alignment_orthogonal() {
        let mut embedder = MockEmbedder::new();
        embedder.insert("A", vec![1.0, 0.0]);
        embedder.insert("B", vec![0.0, 1.0]);

        let p1 = vec!["A".to_string()];
        let p2 = vec!["B".to_string()];

        let score = calculate_semantic_alignment(&p1, &p2, &embedder).unwrap();
        assert!((score - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_calculate_semantic_alignment_partial() {
        let mut embedder = MockEmbedder::new();
        // A is [1, 0]
        // C is [0.707, 0.707] (45 degrees from A)
        embedder.insert(
            "C",
            vec![
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
        );
        embedder.insert("A", vec![1.0, 0.0]);

        let p1 = vec!["A".to_string()];
        let p2 = vec!["C".to_string()];

        let score = calculate_semantic_alignment(&p1, &p2, &embedder).unwrap();
        // Cosine similarity should be close to 0.707
        assert!((score - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
    }

    // ───────────────────────── ConceptEmbedder ─────────────────────────

    fn bow_score(a: &[String], b: &[String]) -> f32 {
        let mut e = BagOfWordsEmbedder::new();
        let all: Vec<String> = a.iter().chain(b.iter()).cloned().collect();
        e.build_vocab(&all);
        calculate_semantic_alignment(a, b, &e).unwrap()
    }

    fn concept_score(a: &[String], b: &[String]) -> f32 {
        calculate_semantic_alignment(a, b, &ConceptEmbedder::new()).unwrap()
    }

    #[test]
    fn concept_embedder_is_fixed_dimension_without_vocab() {
        // The whole point: no build_vocab, and any two texts embed into the
        // same space directly.
        let e = ConceptEmbedder::new();
        let a = e.embed("users deserve privacy").unwrap();
        let b = e.embed("totally different sentence here").unwrap();
        assert_eq!(a.len(), b.len());
        assert_eq!(a.len(), super::EMBED_DIM);
    }

    #[test]
    fn concept_embedder_aligns_paraphrases_that_bow_misses() {
        // Same value, different words, ZERO shared content words.
        let p1 = vec!["users deserve privacy and anonymity".to_string()];
        let p2 = vec!["people are entitled to confidentiality and pseudonymity".to_string()];

        let bow = bow_score(&p1, &p2);
        let concept = concept_score(&p1, &p2);

        // Bag-of-words sees almost no overlap (maybe "and") → low.
        // The concept embedder bridges privacy↔confidentiality,
        // anonymity↔pseudonymity → materially higher.
        assert!(
            concept > bow + 0.2,
            "concept paraphrase score ({concept}) should beat bag-of-words ({bow}) by a clear margin"
        );
        assert!(
            concept > 0.5,
            "paraphrased-but-aligned principles should score as at least Partial-able: {concept}"
        );
    }

    #[test]
    fn concept_embedder_surveillance_synonyms_align() {
        let p1 = vec!["we reject mass surveillance".to_string()];
        let p2 = vec!["we oppose pervasive tracking and monitoring".to_string()];
        let score = concept_score(&p1, &p2);
        assert!(
            score > 0.5,
            "surveillance/tracking/monitoring should align: {score}"
        );
    }

    #[test]
    fn concept_embedder_keeps_opposing_values_low() {
        // Genuinely different value domains should NOT spuriously align high.
        let privacy = vec!["privacy and anonymity are fundamental".to_string()];
        let throughput = vec!["maximize advertising revenue and engagement".to_string()];
        let score = concept_score(&privacy, &throughput);
        assert!(
            score < 0.4,
            "unrelated/opposing value statements must stay low: {score}"
        );
    }

    #[test]
    fn concept_embedder_identical_text_is_one() {
        let p = vec!["self-sovereign identity via zero-knowledge proofs".to_string()];
        let score = concept_score(&p, &p);
        assert!(
            (score - 1.0).abs() < 1e-4,
            "identical principles → 1.0: {score}"
        );
    }

    #[test]
    fn concept_embedder_morphological_variants_align() {
        // private/privacy/privately share trigrams AND the privacy concept.
        let a = vec!["the system keeps data private".to_string()];
        let b = vec!["the system preserves privacy".to_string()];
        let score = concept_score(&a, &b);
        assert!(score > 0.5, "morphological variants should align: {score}");
    }
}
