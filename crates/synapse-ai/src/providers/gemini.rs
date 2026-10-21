//! Google Gemini (Generative Language API): streaming chat and batch embeddings.

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{EMBED_BATCH, known_embed_dims};
use crate::config::ProviderConfig;
use crate::error::{Error, Result};
use crate::http::{Http, for_each_sse};
use crate::keys::Secret;
use crate::provider::{ChatProvider, EmbeddingProvider};
use crate::types::{ChatRequest, ChatResponse, EmbedResponse, Role, TokenSink, Usage};

pub struct Gemini {
    http: Http,
    base: String,
    key: Secret,
    model: String,
    embed_model: Option<String>,
    embed_dims: Option<usize>,
    max_tokens: u32,
    temperature: Option<f32>,
}

impl Gemini {
    pub fn new(cfg: &ProviderConfig, key: Secret) -> Result<Self> {
        Ok(Self {
            http: Http::new(cfg)?,
            base: cfg.base_url(),
            key,
            model: cfg.model.clone(),
            embed_model: cfg.embed_model.clone(),
            embed_dims: cfg.embed_dims,
            max_tokens: cfg.max_tokens,
            temperature: cfg.temperature,
        })
    }

    // The key travels in a header, never in the URL, so it cannot leak through logs or errors.
    fn headers(&self) -> [(&'static str, String); 1] {
        [("x-goog-api-key", self.key.expose().to_string())]
    }

    fn body(&self, req: &ChatRequest) -> Value {
        let contents: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                let role = if m.role == Role::User {
                    "user"
                } else {
                    "model"
                };
                json!({"role": role, "parts": [{"text": m.content}]})
            })
            .collect();
        let mut gen_cfg = json!({"maxOutputTokens": req.max_tokens.unwrap_or(self.max_tokens)});
        if let Some(t) = self.temperature {
            gen_cfg["temperature"] = json!(t);
        }
        if req.json {
            gen_cfg["responseMimeType"] = json!("application/json");
        }
        let mut body = json!({"contents": contents, "generationConfig": gen_cfg});
        if let Some(s) = &req.system {
            body["systemInstruction"] = json!({"parts": [{"text": s}]});
        }
        body
    }
}

#[async_trait]
impl ChatProvider for Gemini {
    async fn chat(
        &self,
        req: &ChatRequest,
        mut sink: Option<TokenSink<'_>>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse> {
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse",
            self.base, self.model
        );
        let resp = self
            .http
            .post_json(&url, &self.headers(), &self.body(req), cancel)
            .await?;
        let mut out = ChatResponse::default();
        for_each_sse(resp, cancel, |ev| {
            let v: Value =
                serde_json::from_str(&ev.data).map_err(|e| Error::Parse(e.to_string()))?;
            if let Some(reason) = v["promptFeedback"]["blockReason"].as_str() {
                return Err(Error::Refused(format!("prompt blocked: {reason}")));
            }
            if let Some(parts) = v["candidates"][0]["content"]["parts"].as_array() {
                for t in parts.iter().filter_map(|p| p["text"].as_str()) {
                    out.text.push_str(t);
                    if let Some(s) = sink.as_mut() {
                        s(t);
                    }
                }
            }
            if let Some(r) = v["candidates"][0]["finishReason"].as_str() {
                out.stop_reason = Some(r.to_string());
            }
            if let Some(u) = v.get("usageMetadata").filter(|u| u.is_object()) {
                out.usage = Usage {
                    input_tokens: u["promptTokenCount"].as_u64().unwrap_or(0),
                    output_tokens: u["candidatesTokenCount"].as_u64().unwrap_or(0)
                        + u["thoughtsTokenCount"].as_u64().unwrap_or(0),
                };
            }
            Ok(true)
        })
        .await?;
        Ok(out)
    }
}

#[async_trait]
impl EmbeddingProvider for Gemini {
    fn model_id(&self) -> String {
        format!("gemini/{}", self.embed_model.as_deref().unwrap_or_default())
    }

    fn known_dims(&self) -> Option<usize> {
        self.embed_dims
            .or_else(|| self.embed_model.as_deref().and_then(known_embed_dims))
    }

    async fn embed(&self, texts: &[String], cancel: &CancellationToken) -> Result<EmbedResponse> {
        let model = self
            .embed_model
            .as_deref()
            .ok_or_else(|| Error::Config("no embed_model configured".into()))?;
        let url = format!("{}/models/{model}:batchEmbedContents", self.base);
        let mut out = EmbedResponse::default();
        for batch in texts.chunks(EMBED_BATCH) {
            let requests: Vec<Value> = batch
                .iter()
                .map(|t| json!({"model": format!("models/{model}"), "content": {"parts": [{"text": t}]}}))
                .collect();
            let body = json!({"requests": requests});
            let resp = self
                .http
                .post_json(&url, &self.headers(), &body, cancel)
                .await?;
            let v: Value = resp.json().await.map_err(|e| Error::Parse(e.to_string()))?;
            let rows = v["embeddings"]
                .as_array()
                .ok_or_else(|| Error::Parse("missing `embeddings` in response".into()))?;
            if rows.len() != batch.len() {
                return Err(Error::Parse(format!(
                    "expected {} embeddings, got {}",
                    batch.len(),
                    rows.len()
                )));
            }
            for r in rows {
                out.vectors.push(
                    r["values"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_f64())
                                .map(|x| x as f32)
                                .collect()
                        })
                        .unwrap_or_default(),
                );
            }
        }
        Ok(out)
    }
}
