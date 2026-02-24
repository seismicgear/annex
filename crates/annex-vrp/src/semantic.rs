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
            .ok_or_else(|| format!("No embedding found for: {}", text))
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
}
