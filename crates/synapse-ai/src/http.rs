//! Shared HTTP plumbing: rustls client, throttling, retries with backoff, SSE decoding.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::config::ProviderConfig;
use crate::error::{Error, Result};

const BASE_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub(crate) struct Http {
    client: reqwest::Client,
    max_retries: u32,
    interval: Duration,
    last_request: Mutex<Option<Instant>>,
    base_backoff: Duration,
}

fn tls_config() -> Result<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Config(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(cfg)
}

impl Http {
    pub fn new(cfg: &ProviderConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .use_preconfigured_tls(tls_config()?)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
            .build()
            .map_err(|e| Error::Config(e.to_string()))?;
        Ok(Self {
            client,
            max_retries: cfg.max_retries,
            interval: Duration::from_millis(cfg.min_request_interval_ms),
            last_request: Mutex::new(None),
            base_backoff: BASE_BACKOFF,
        })
    }

    async fn throttle(&self, cancel: &CancellationToken) -> Result<()> {
        if self.interval.is_zero() {
            return Ok(());
        }
        let mut last = self.last_request.lock().await;
        if let Some(t) = *last {
            let wait = self.interval.saturating_sub(t.elapsed());
            if !wait.is_zero() {
                sleep_or_cancel(wait, cancel).await?;
            }
        }
        *last = Some(Instant::now());
        Ok(())
    }

    /// POSTs JSON and returns the successful response. Retries 408/409/429/5xx and network
    /// errors with exponential backoff (honouring `Retry-After`); aborts on cancellation.
    pub async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &Value,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response> {
        let mut attempt = 0u32;
        loop {
            self.throttle(cancel).await?;
            let mut req = self.client.post(url).json(body);
            for (k, v) in headers {
                req = req.header(*k, v);
            }
            let outcome = tokio::select! {
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                r = req.send() => r,
            };
            let err = match outcome {
                Ok(resp) if resp.status().is_success() => return Ok(resp),
                Ok(resp) => api_error(resp, cancel).await?,
                Err(e) => Error::Network(sanitize(&e)),
            };
            if attempt >= self.max_retries || !err.is_retryable() {
                return Err(err);
            }
            let delay = match &err {
                Error::Api {
                    retry_after: Some(d),
                    ..
                } => (*d).min(MAX_BACKOFF),
                _ => backoff(self.base_backoff, attempt),
            };
            sleep_or_cancel(delay, cancel).await?;
            attempt += 1;
        }
    }
}

/// `reqwest` errors can embed the request URL (which may carry a key in query-string setups);
/// keep only the failure kind.
fn sanitize(e: &reqwest::Error) -> String {
    let kind = if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connection failed"
    } else if e.is_decode() {
        "bad response body"
    } else {
        "request failed"
    };
    kind.to_string()
}

pub(crate) fn backoff(base: Duration, attempt: u32) -> Duration {
    let exp = base.saturating_mul(1u32 << attempt.min(6)).min(MAX_BACKOFF);
    // Cheap jitter (0–25%) from the clock; no RNG dependency needed.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    exp + exp.mul_f64((nanos % 250) as f64 / 1000.0)
}

async fn sleep_or_cancel(d: Duration, cancel: &CancellationToken) -> Result<()> {
    tokio::select! {
        _ = cancel.cancelled() => Err(Error::Cancelled),
        _ = tokio::time::sleep(d) => Ok(()),
    }
}

async fn api_error(resp: reqwest::Response, cancel: &CancellationToken) -> Result<Error> {
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let text = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::Cancelled),
        t = resp.text() => t.unwrap_or_default(),
    };
    Ok(Error::Api {
        status,
        message: error_message(&text),
        retry_after,
    })
}

/// Pulls the human message out of the usual `{"error": {"message": ..}}` shapes.
pub(crate) fn error_message(body: &str) -> String {
    let msg = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        let e = v.get("error").unwrap_or(&v);
        e.get("message")
            .or_else(|| e.as_str().map(|_| e))
            .map(|m| m.as_str().unwrap_or_default().to_string())
    });
    let msg = msg
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| body.trim().to_string());
    msg.chars().take(500).collect()
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental Server-Sent-Events decoder.
#[derive(Default)]
pub(crate) struct SseDecoder {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    out.push(SseEvent {
                        event: self.event.take(),
                        data: self.data.join("\n"),
                    });
                    self.data.clear();
                }
                self.event = None;
            } else if let Some(v) = line.strip_prefix("data:") {
                self.data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
            } else if let Some(v) = line.strip_prefix("event:") {
                self.event = Some(v.trim().to_string());
            }
        }
        out
    }
}

/// Feeds every SSE event to `f` until the stream ends or `f` returns `Ok(false)`.
pub(crate) async fn for_each_sse(
    resp: reqwest::Response,
    cancel: &CancellationToken,
    mut f: impl FnMut(SseEvent) -> Result<bool>,
) -> Result<()> {
    let mut stream = resp.bytes_stream();
    let mut dec = SseDecoder::default();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            n = stream.next() => n,
        };
        let Some(chunk) = next else { return Ok(()) };
        let chunk = chunk.map_err(|e| Error::Network(sanitize(&e)))?;
        for ev in dec.push(&chunk) {
            if !f(ev)? {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_decoder_handles_split_chunks_and_crlf() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"event: a\r\ndata: {\"x\"").is_empty());
        let evs = d.push(b":1}\r\n\r\ndata: [DONE]\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event.as_deref(), Some("a"));
        assert_eq!(evs[0].data, "{\"x\":1}");
        assert_eq!(evs[1].data, "[DONE]");
        assert_eq!(evs[1].event, None);
    }

    #[test]
    fn error_message_shapes() {
        assert_eq!(error_message(r#"{"error":{"message":"bad"}}"#), "bad");
        assert_eq!(error_message(r#"{"error":"nope"}"#), "nope");
        assert_eq!(error_message("plain"), "plain");
    }
}
