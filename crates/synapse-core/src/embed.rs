//! Embedding abstraction shared by `synapse-search` (consumer) and `synapse-ai` (providers).

pub type EmbedError = Box<dyn std::error::Error + Send + Sync>;

/// Turns texts into dense vectors. Implementations may block (network or local model).
pub trait Embedder: Send + Sync {
    /// Stable identifier, e.g. `openai/text-embedding-3-small`. Indexes are keyed by it.
    fn model_id(&self) -> String;
    fn dims(&self) -> usize;
    fn embed(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, EmbedError>;
}
