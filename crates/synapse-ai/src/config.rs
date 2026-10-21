use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Gemini,
    /// Any server speaking the OpenAI wire format (Ollama, LM Studio, OpenRouter, Mistral, Voyage…).
    OpenAiCompatible,
}

impl ProviderKind {
    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::OpenAi => "https://api.openai.com/v1",
            ProviderKind::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            ProviderKind::OpenAiCompatible => "http://localhost:11434/v1",
        }
    }

    /// Default key name in the [`crate::KeyStore`].
    pub fn default_key_name(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Gemini => "gemini",
            ProviderKind::OpenAiCompatible => "openai_compatible",
        }
    }

    /// Local OpenAI-compatible servers usually need no key.
    pub fn key_required(self) -> bool {
        self != ProviderKind::OpenAiCompatible
    }

    pub fn supports_embeddings(self) -> bool {
        self != ProviderKind::Anthropic
    }
}

/// Serialisable provider settings. Contains no secrets: the key is looked up in a
/// [`crate::KeyStore`] under [`ProviderConfig::key_name`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    /// Overrides the provider's default endpoint.
    pub base_url: Option<String>,
    /// Chat model id.
    pub model: String,
    /// Embedding model id; `None` disables embeddings for this provider.
    pub embed_model: Option<String>,
    /// Vector size of `embed_model`; discovered with one probe request when unknown.
    pub embed_dims: Option<usize>,
    /// Name under which the API key is stored; defaults to the provider's name.
    pub key_name: Option<String>,
    /// Upper bound on generated tokens per chat call.
    pub max_tokens: u32,
    /// Sampling temperature. Left unset by default (some models reject non-default values).
    pub temperature: Option<f32>,
    pub timeout_secs: u64,
    /// Retries after the first attempt for 429/5xx/network errors.
    pub max_retries: u32,
    /// Client-side throttle: minimum delay between two requests (0 = off).
    pub min_request_interval_ms: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::new(ProviderKind::Anthropic)
    }
}

impl ProviderConfig {
    /// Sensible defaults for a provider; the user normally overrides `model`.
    pub fn new(kind: ProviderKind) -> Self {
        let (model, embed_model) = match kind {
            ProviderKind::Anthropic => ("claude-opus-5-5", None),
            ProviderKind::OpenAi => ("gpt-5-mini", Some("text-embedding-3-small")),
            ProviderKind::Gemini => ("gemini-2.5-flash", Some("gemini-embedding-001")),
            ProviderKind::OpenAiCompatible => ("llama3.1", Some("nomic-embed-text")),
        };
        Self {
            kind,
            base_url: None,
            model: model.into(),
            embed_model: embed_model.map(Into::into),
            embed_dims: None,
            key_name: None,
            max_tokens: 4096,
            temperature: None,
            timeout_secs: 120,
            max_retries: 3,
            min_request_interval_ms: 0,
        }
    }

    pub fn base_url(&self) -> String {
        self.base_url
            .as_deref()
            .unwrap_or_else(|| self.kind.default_base_url())
            .trim_end_matches('/')
            .to_string()
    }

    pub fn key_name(&self) -> &str {
        self.key_name
            .as_deref()
            .unwrap_or_else(|| self.kind.default_key_name())
    }
}
