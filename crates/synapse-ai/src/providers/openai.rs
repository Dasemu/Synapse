//! OpenAI Chat Completions + Embeddings; also serves any OpenAI-compatible endpoint
//! (Ollama, LM Studio, OpenRouter, Mistral, Voyage…).

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{EMBED_BATCH, known_embed_dims};
use crate::config::{ProviderConfig, ProviderKind};
use crate::error::{Error, Result};
use crate::http::{Http, for_each_sse};
use crate::keys::Secret;
use crate::provider::{ChatProvider, EmbeddingProvider};
use crate::types::{ChatRequest, ChatResponse, EmbedResponse, Role, TokenSink, Usage};

pub struct OpenAi {
    http: Http,
    base: String,
    key: Option<Secret>,
    kind: ProviderKind,
    model: String,
    embed_model: Option<String>,
    embed_dims: Option<usize>,
    max_tokens: u32,
    temperature: Option<f32>,
}

impl OpenAi {
    pub fn new(cfg: &ProviderConfig, key: Option<Secret>) -> Result<Self> {
        Ok(Self {
            http: Http::new(cfg)?,
            base: cfg.base_url(),
            key,
            kind: cfg.kind,
            model: cfg.model.clone(),
            embed_model: cfg.embed_model.clone(),
            embed_dims: cfg.embed_dims,
            max_tokens: cfg.max_tokens,
            temperature: cfg.temperature,
        })
    }

    fn headers(&self) -> Vec<(&'static str, String)> {
        self.key
            .iter()
            .map(|k| ("authorization", format!("Bearer {}", k.expose())))
            .collect()
    }

    fn body(&self, req: &ChatRequest) -> Value {
        let mut messages = Vec::new();
        if let Some(s) = &req.system {
            messages.push(json!({"role": "system", "content": s}));
        }
        for m in &req.messages {
            let role = if m.role == Role::User {
                "user"
            } else {
                "assistant"
            };
            messages.push(json!({"role": role, "content": m.content}));
        }
        let max = req.max_tokens.unwrap_or(self.max_tokens);
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        // OpenAI's newer models reject `max_tokens`; compatible servers only know that one.
        let key = if self.kind == ProviderKind::OpenAi {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        body[key] = json!(max);
        if let Some(t) = self.temperature {
            body["temperature"] = json!(t);
        }
        if req.json {
            body["response_format"] = json!({"type": "json_object"});
        }
        body
    }
}

#[async_trait]
impl ChatProvider for OpenAi {
    async fn chat(
        &self,
        req: &ChatRequest,
        mut sink: Option<TokenSink<'_>>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base);
        let resp = self
            .http
            .post_json(&url, &self.headers(), &self.body(req), cancel)
            .await?;
        let mut out = ChatResponse::default();
        for_each_sse(resp, cancel, |ev| {
            if ev.data.trim() == "[DONE]" {
                return Ok(false);
            }
            let v: Value =
                serde_json::from_str(&ev.data).map_err(|e| Error::Parse(e.to_string()))?;
            if let Some(msg) = v["error"]["message"].as_str() {
                return Err(Error::Api {
                    status: 500,
                    message: msg.to_string(),
                    retry_after: None,
                });
            }
            if let Some(t) = v["choices"][0]["delta"]["content"]
                .as_str()
                .filter(|t| !t.is_empty())
            {
                out.text.push_str(t);
                if let Some(s) = sink.as_mut() {
                    s(t);
                }
            }
            if let Some(r) = v["choices"][0]["finish_reason"].as_str() {
                out.stop_reason = Some(r.to_string());
            }
            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                out.usage = Usage {
                    input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0),
                    output_tokens: u["completion_tokens"].as_u64().unwrap_or(0),
                };
            }
            Ok(true)
        })
        .await?;
        Ok(out)
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAi {
    fn model_id(&self) -> String {
        let p = match self.kind {
            ProviderKind::OpenAi => "openai",
            _ => "openai-compatible",
        };
        format!("{p}/{}", self.embed_model.as_deref().unwrap_or_default())
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
        let url = format!("{}/embeddings", self.base);
        let mut out = EmbedResponse::default();
        for batch in texts.chunks(EMBED_BATCH) {
            let body = json!({"model": model, "input": batch});
            let resp = self
                .http
                .post_json(&url, &self.headers(), &body, cancel)
                .await?;
            let v: Value = resp.json().await.map_err(|e| Error::Parse(e.to_string()))?;
            let mut rows: Vec<(u64, Vec<f32>)> = v["data"]
                .as_array()
                .ok_or_else(|| Error::Parse("missing `data` in embeddings response".into()))?
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let idx = d["index"].as_u64().unwrap_or(i as u64);
                    let vec = d["embedding"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_f64())
                                .map(|x| x as f32)
                                .collect()
                        })
                        .unwrap_or_default();
                    (idx, vec)
                })
                .collect();
            if rows.len() != batch.len() {
                return Err(Error::Parse(format!(
                    "expected {} embeddings, got {}",
                    batch.len(),
                    rows.len()
                )));
            }
            rows.sort_by_key(|r| r.0);
            out.vectors.extend(rows.into_iter().map(|r| r.1));
            out.usage.input_tokens += v["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        }
        Ok(out)
    }
}
