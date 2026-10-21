use std::sync::{Arc, Mutex, OnceLock};

use synapse_core::embed::{EmbedError, Embedder};
use tokio_util::sync::CancellationToken;

use crate::config::{ProviderConfig, ProviderKind};
use crate::error::{Error, Result};
use crate::keys::KeyStore;
use crate::provider::{ChatProvider, EmbeddingProvider};
use crate::providers::{Anthropic, Gemini, OpenAi};
use crate::types::{ChatRequest, ChatResponse, EmbedResponse, TokenSink, Usage};

/// A configured provider: chat, optional embeddings, and cumulative token usage.
#[derive(Clone)]
pub struct AiClient {
    config: ProviderConfig,
    chat: Arc<dyn ChatProvider>,
    embed: Option<Arc<dyn EmbeddingProvider>>,
    usage: Arc<Mutex<Usage>>,
}

impl AiClient {
    /// Builds the client for `config`, fetching the API key from `keys` (key-less local
    /// OpenAI-compatible servers are allowed to have none).
    pub fn new(config: &ProviderConfig, keys: &dyn KeyStore) -> Result<Self> {
        let key = keys.get(config.key_name());
        if key.is_none() && config.kind.key_required() {
            return Err(Error::MissingKey(config.key_name().to_string()));
        }
        let (chat, embed): (Arc<dyn ChatProvider>, Option<Arc<dyn EmbeddingProvider>>) =
            match config.kind {
                ProviderKind::Anthropic => (Arc::new(Anthropic::new(config, key.unwrap())?), None),
                ProviderKind::Gemini => {
                    let p = Arc::new(Gemini::new(config, key.unwrap())?);
                    (p.clone(), config.embed_model.is_some().then_some(p as _))
                }
                ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
                    let p = Arc::new(OpenAi::new(config, key)?);
                    (p.clone(), config.embed_model.is_some().then_some(p as _))
                }
            };
        Ok(Self {
            config: config.clone(),
            chat,
            embed,
            usage: Arc::default(),
        })
    }

    /// Builds a client around custom providers (tests, other back-ends).
    pub fn from_providers(
        config: ProviderConfig,
        chat: Arc<dyn ChatProvider>,
        embed: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self {
            config,
            chat,
            embed,
            usage: Arc::default(),
        }
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    /// Tokens consumed through this client (and its clones) so far.
    pub fn total_usage(&self) -> Usage {
        *self.usage.lock().unwrap()
    }

    pub async fn chat(
        &self,
        req: &ChatRequest,
        sink: Option<TokenSink<'_>>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse> {
        let r = self.chat.chat(req, sink, cancel).await?;
        *self.usage.lock().unwrap() += r.usage;
        Ok(r)
    }

    fn embedder(&self) -> Result<&Arc<dyn EmbeddingProvider>> {
        self.embed.as_ref().ok_or_else(|| {
            if self.config.kind.supports_embeddings() {
                Error::Config("no `embed_model` configured for this provider".into())
            } else {
                Error::Unsupported(
                    "Anthropic has no embeddings API; configure OpenAI, Gemini, Ollama or \
                     Voyage (as OpenAI-compatible) for embeddings"
                        .into(),
                )
            }
        })
    }

    pub async fn embed(
        &self,
        texts: &[String],
        cancel: &CancellationToken,
    ) -> Result<EmbedResponse> {
        let r = self.embedder()?.embed(texts, cancel).await?;
        *self.usage.lock().unwrap() += r.usage;
        Ok(r)
    }

    /// A synchronous [`Embedder`] for `synapse-search`. See [`BlockingEmbedder`].
    pub fn blocking_embedder(&self) -> Result<BlockingEmbedder> {
        BlockingEmbedder::new(self.embedder()?.clone(), self.usage.clone())
    }
}

/// Blocking adapter implementing [`synapse_core::embed::Embedder`].
///
/// It owns a small tokio runtime and waits on a channel, so it may be called from any thread
/// (including threads of another runtime — but it blocks the caller, so use `spawn_blocking`
/// from async code). Dropping it never panics.
pub struct BlockingEmbedder {
    provider: Arc<dyn EmbeddingProvider>,
    runtime: Option<tokio::runtime::Runtime>,
    cancel: CancellationToken,
    dims: OnceLock<usize>,
    usage: Arc<Mutex<Usage>>,
}

impl BlockingEmbedder {
    fn new(provider: Arc<dyn EmbeddingProvider>, usage: Arc<Mutex<Usage>>) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("synapse-ai-embed")
            .enable_all()
            .build()
            .map_err(|e| Error::Config(e.to_string()))?;
        let dims = OnceLock::new();
        if let Some(d) = provider.known_dims() {
            let _ = dims.set(d);
        }
        Ok(Self {
            provider,
            runtime: Some(runtime),
            cancel: CancellationToken::new(),
            dims,
            usage,
        })
    }

    /// Token that aborts in-flight and future calls when cancelled.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    fn run(&self, texts: Vec<String>) -> Result<EmbedResponse> {
        let (tx, rx) = std::sync::mpsc::channel();
        let provider = self.provider.clone();
        let cancel = self.cancel.clone();
        self.runtime
            .as_ref()
            .expect("runtime alive")
            .spawn(async move {
                let _ = tx.send(provider.embed(&texts, &cancel).await);
            });
        rx.recv().map_err(|_| Error::Cancelled)?
    }

    fn try_embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let r = self.run(texts.to_vec())?;
        *self.usage.lock().unwrap() += r.usage;
        if let Some(first) = r.vectors.first() {
            let _ = self.dims.set(first.len());
        }
        Ok(r.vectors)
    }
}

impl Drop for BlockingEmbedder {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

impl Embedder for BlockingEmbedder {
    fn model_id(&self) -> String {
        self.provider.model_id()
    }

    /// Known for common models; otherwise discovered with one tiny embedding request.
    /// Returns 0 if that probe fails.
    fn dims(&self) -> usize {
        if let Some(d) = self.dims.get() {
            return *d;
        }
        let _ = self.try_embed(&["dimension probe".to_string()]);
        self.dims.get().copied().unwrap_or(0)
    }

    fn embed(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
        Ok(self.try_embed(texts)?)
    }
}
