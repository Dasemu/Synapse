use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::types::{ChatRequest, ChatResponse, EmbedResponse, TokenSink};

#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Runs one chat completion. Text deltas are pushed to `sink` as they arrive; the full text
    /// is also returned. Cancelling `cancel` aborts the request with `Error::Cancelled`.
    async fn chat(
        &self,
        req: &ChatRequest,
        sink: Option<TokenSink<'_>>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse>;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// `provider/model`, e.g. `openai/text-embedding-3-small`.
    fn model_id(&self) -> String;
    /// Known vector size, if it does not need a probe request.
    fn known_dims(&self) -> Option<usize>;
    async fn embed(&self, texts: &[String], cancel: &CancellationToken) -> Result<EmbedResponse>;
}
