//! Anthropic Messages API (streaming). Anthropic has no embeddings endpoint: use OpenAI,
//! Gemini, Ollama or Voyage (via `OpenAiCompatible`) for embeddings.

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::config::ProviderConfig;
use crate::error::{Error, Result};
use crate::http::{Http, for_each_sse};
use crate::keys::Secret;
use crate::provider::ChatProvider;
use crate::types::{ChatRequest, ChatResponse, Role, TokenSink};

const API_VERSION: &str = "2023-06-01";

pub struct Anthropic {
    http: Http,
    url: String,
    key: Secret,
    model: String,
    max_tokens: u32,
}

impl Anthropic {
    pub fn new(cfg: &ProviderConfig, key: Secret) -> Result<Self> {
        Ok(Self {
            http: Http::new(cfg)?,
            url: format!("{}/v1/messages", cfg.base_url()),
            key,
            model: cfg.model.clone(),
            max_tokens: cfg.max_tokens,
        })
    }

    fn body(&self, req: &ChatRequest) -> Value {
        // No `temperature`: current Claude models reject non-default sampling parameters.
        let messages: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                let role = if m.role == Role::User {
                    "user"
                } else {
                    "assistant"
                };
                json!({"role": role, "content": m.content})
            })
            .collect();
        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens.unwrap_or(self.max_tokens),
            "messages": messages,
            "stream": true,
        });
        if let Some(s) = &req.system {
            body["system"] = json!(s);
        }
        body
    }
}

#[async_trait]
impl ChatProvider for Anthropic {
    async fn chat(
        &self,
        req: &ChatRequest,
        mut sink: Option<TokenSink<'_>>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse> {
        let headers = [
            ("x-api-key", self.key.expose().to_string()),
            ("anthropic-version", API_VERSION.to_string()),
        ];
        let resp = self
            .http
            .post_json(&self.url, &headers, &self.body(req), cancel)
            .await?;
        let mut out = ChatResponse::default();
        for_each_sse(resp, cancel, |ev| {
            let v: Value =
                serde_json::from_str(&ev.data).map_err(|e| Error::Parse(e.to_string()))?;
            match v["type"].as_str().unwrap_or_default() {
                "message_start" => {
                    out.usage.input_tokens =
                        v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0);
                    out.usage.output_tokens =
                        v["message"]["usage"]["output_tokens"].as_u64().unwrap_or(0);
                }
                "content_block_delta" if v["delta"]["type"] == "text_delta" => {
                    if let Some(t) = v["delta"]["text"].as_str() {
                        out.text.push_str(t);
                        if let Some(s) = sink.as_mut() {
                            s(t);
                        }
                    }
                }
                "message_delta" => {
                    if let Some(n) = v["usage"]["output_tokens"].as_u64() {
                        out.usage.output_tokens = n;
                    }
                    if let Some(n) = v["usage"]["input_tokens"].as_u64() {
                        out.usage.input_tokens = n;
                    }
                    if let Some(r) = v["delta"]["stop_reason"].as_str() {
                        out.stop_reason = Some(r.to_string());
                    }
                }
                "error" => {
                    let msg = v["error"]["message"].as_str().unwrap_or("stream error");
                    return Err(Error::Api {
                        status: 529,
                        message: msg.to_string(),
                        retry_after: None,
                    });
                }
                _ => {}
            }
            Ok(true)
        })
        .await?;
        if out.stop_reason.as_deref() == Some("refusal") {
            return Err(Error::Refused(
                "the model's safety system declined this request".into(),
            ));
        }
        Ok(out)
    }
}
