//! Deterministic fake embedder for tests, examples and benchmarks.

use synapse_core::embed::{EmbedError, Embedder};

/// Bag-of-words hashing embedder: texts sharing words get a high cosine similarity.
pub struct HashEmbedder {
    dims: usize,
    model: String,
}

impl HashEmbedder {
    pub fn new(dims: usize) -> Self {
        Self::with_model(dims, "test/hash")
    }

    pub fn with_model(dims: usize, model: &str) -> Self {
        Self {
            dims,
            model: model.to_string(),
        }
    }

    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dims];
        for word in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            // FNV-1a over the lowercased word.
            let mut h: u64 = 0xcbf29ce484222325;
            for b in word.to_lowercase().bytes() {
                h = (h ^ b as u64).wrapping_mul(0x100000001b3);
            }
            v[(h % self.dims as u64) as usize] += 1.0;
        }
        v
    }
}

impl Embedder for HashEmbedder {
    fn model_id(&self) -> String {
        self.model.clone()
    }
    fn dims(&self) -> usize {
        self.dims
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
}
