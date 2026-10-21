mod anthropic;
mod gemini;
mod openai;

pub use anthropic::Anthropic;
pub use gemini::Gemini;
pub use openai::OpenAi;

/// Embedding batch size per request.
pub(crate) const EMBED_BATCH: usize = 96;

/// Vector sizes of well-known embedding models.
pub(crate) fn known_embed_dims(model: &str) -> Option<usize> {
    Some(match model {
        "text-embedding-3-small" | "text-embedding-ada-002" => 1536,
        "text-embedding-3-large" | "gemini-embedding-001" => 3072,
        "text-embedding-004" | "nomic-embed-text" => 768,
        "mxbai-embed-large" | "bge-large" => 1024,
        "all-minilm" => 384,
        _ => return None,
    })
}
