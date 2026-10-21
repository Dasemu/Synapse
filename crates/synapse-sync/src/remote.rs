//! HTTP transport to a Synapse server. Usable on its own as a thin client (read API, no local
//! replica) and as the transport of [`crate::SyncClient`].

use crate::error::{Error, Result};
use crate::protocol::*;
use futures_util::StreamExt;
use reqwest::{Method, RequestBuilder, Response, StatusCode, header};
use serde::de::DeserializeOwned;
use std::path::Path;
use std::time::Duration;
use synapse_core::{Backlink, Page};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// Base URL, e.g. `https://notes.example.com` (no trailing path).
    pub server_url: String,
    pub graph_id: String,
    pub token: String,
    pub connect_timeout: Duration,
    /// Timeout for JSON requests; blob transfers only use an idle (read) timeout.
    pub request_timeout: Duration,
}

impl RemoteConfig {
    pub fn new(
        server_url: impl Into<String>,
        graph_id: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            server_url: server_url.into(),
            graph_id: graph_id.into(),
            token: token.into(),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Remote {
    http: reqwest::Client,
    cfg: RemoteConfig,
}

impl Remote {
    pub fn new(cfg: RemoteConfig) -> Result<Self> {
        if !is_valid_graph_id(&cfg.graph_id) {
            return Err(Error::Protocol(format!(
                "invalid graph id {:?}",
                cfg.graph_id
            )));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(cfg.connect_timeout)
            .read_timeout(Duration::from_secs(60))
            .user_agent(concat!("synapse-sync/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, cfg })
    }

    pub fn config(&self) -> &RemoteConfig {
        &self.cfg
    }

    /// Identifies this remote graph; sync cursors are only valid for the same key.
    pub fn key(&self) -> String {
        format!(
            "{}|{}",
            self.cfg.server_url.trim_end_matches('/'),
            self.cfg.graph_id
        )
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/v1/graphs/{}{}",
            self.cfg.server_url.trim_end_matches('/'),
            self.cfg.graph_id,
            path
        )
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, self.url(path))
            .bearer_auth(&self.cfg.token)
            .header(HEADER_PROTOCOL, PROTOCOL_VERSION)
    }

    fn json_req(&self, method: Method, path: &str) -> RequestBuilder {
        self.req(method, path).timeout(self.cfg.request_timeout)
    }

    async fn check(resp: Response) -> Result<Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(Error::Unauthorized);
        }
        let message = match resp.json::<ErrorBody>().await {
            Ok(b) => b.error,
            Err(_) => status.canonical_reason().unwrap_or("error").to_string(),
        };
        Err(Error::Status {
            status: status.as_u16(),
            message,
        })
    }

    async fn json<T: DeserializeOwned>(rb: RequestBuilder) -> Result<T> {
        Ok(Self::check(rb.send().await?).await?.json().await?)
    }

    // ------------------------------------------------------------------ sync

    /// Creates the graph on the server if needed (idempotent) and returns its identity.
    pub async fn ensure_graph(&self) -> Result<Info> {
        let info: Info = Self::json(self.json_req(Method::PUT, "")).await?;
        if info.protocol != PROTOCOL_VERSION {
            return Err(Error::Protocol(format!(
                "server speaks protocol {}, client {}",
                info.protocol, PROTOCOL_VERSION
            )));
        }
        Ok(info)
    }

    pub async fn push(&self, ops: Vec<WireOp>) -> Result<PushResponse> {
        Self::json(
            self.json_req(Method::POST, "/ops")
                .json(&PushRequest { ops }),
        )
        .await
    }

    pub async fn pull(
        &self,
        since: i64,
        limit: u32,
        exclude_actor: Option<&str>,
    ) -> Result<PullResponse> {
        let mut rb = self
            .json_req(Method::GET, "/ops")
            .query(&[("since", since.to_string()), ("limit", limit.to_string())]);
        if let Some(a) = exclude_actor {
            rb = rb.query(&[("exclude_actor", a)]);
        }
        Self::json(rb).await
    }

    /// Opens the realtime stream (Server-Sent Events). Each `data:` line is a [`SyncEvent`].
    pub async fn open_events(&self) -> Result<Response> {
        let rb = self
            .req(Method::GET, "/events")
            .header(header::ACCEPT, "text/event-stream");
        Self::check(rb.send().await?).await
    }

    // ----------------------------------------------------------------- blobs

    /// `Ok(None)` if the server has the blob; otherwise how many bytes of a partial upload it
    /// already holds (0 if none).
    pub async fn blob_upload_offset(&self, hash: &str) -> Result<Option<u64>> {
        let resp = self
            .json_req(Method::HEAD, &format!("/blobs/{hash}"))
            .send()
            .await?;
        match resp.status() {
            s if s.is_success() => Ok(None),
            StatusCode::NOT_FOUND => Ok(Some(
                resp.headers()
                    .get(HEADER_UPLOAD_OFFSET)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            )),
            _ => Err(Self::check(resp).await.unwrap_err()),
        }
    }

    /// Uploads the file at `path` (whose blake3 is `hash`), skipping it if the server already
    /// has it and resuming a partial upload. The file is streamed, never loaded in memory.
    pub async fn upload_blob(&self, hash: &str, path: &Path) -> Result<()> {
        let size = tokio::fs::metadata(path).await?.len();
        for _ in 0..3 {
            let Some(offset) = self.blob_upload_offset(hash).await? else {
                return Ok(());
            };
            let offset = if offset > size { 0 } else { offset };
            let mut file = tokio::fs::File::open(path).await?;
            file.seek(std::io::SeekFrom::Start(offset)).await?;
            let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
                file,
                256 * 1024,
            ));
            let resp = self
                .req(Method::PUT, &format!("/blobs/{hash}"))
                .header(HEADER_UPLOAD_OFFSET, offset)
                .header(HEADER_UPLOAD_LENGTH, size)
                .header(header::CONTENT_LENGTH, size - offset)
                .body(body)
                .send()
                .await?;
            if resp.status() == StatusCode::CONFLICT {
                continue; // offset out of date: HEAD again
            }
            Self::check(resp).await?;
            return Ok(());
        }
        Err(Error::Protocol(format!(
            "blob {hash}: upload offset keeps changing"
        )))
    }

    /// Downloads blob `hash` into `dest`, streaming through `part` (resumed if it exists), and
    /// verifies the hash. `part` and `dest` must be on the same filesystem.
    pub async fn download_blob(&self, hash: &str, part: &Path, dest: &Path) -> Result<()> {
        if let Some(dir) = part.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        if let Some(dir) = dest.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let have = match tokio::fs::metadata(part).await {
            Ok(m) => m.len(),
            Err(_) => 0,
        };
        let mut rb = self.req(Method::GET, &format!("/blobs/{hash}"));
        if have > 0 {
            rb = rb.header(header::RANGE, format!("bytes={have}-"));
        }
        let resp = rb.send().await?;
        let mut out = match resp.status() {
            StatusCode::PARTIAL_CONTENT => {
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(part)
                    .await?
            }
            StatusCode::RANGE_NOT_SATISFIABLE => {
                // Stale partial file: drop it; the next attempt starts from scratch.
                let _ = tokio::fs::remove_file(part).await;
                return Err(Error::Protocol(format!("blob {hash}: stale partial file")));
            }
            _ => tokio::fs::File::create(part).await?,
        };
        let resp = Self::check(resp).await?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            out.write_all(&chunk?).await?;
        }
        out.flush().await?;
        drop(out);
        let part_owned = part.to_path_buf();
        let actual = tokio::task::spawn_blocking(move || hash_file(&part_owned))
            .await
            .map_err(|e| Error::Task(e.to_string()))??;
        if actual != hash {
            let _ = tokio::fs::remove_file(part).await;
            return Err(Error::Protocol(format!(
                "blob {hash}: downloaded content hashes to {actual}"
            )));
        }
        tokio::fs::rename(part, dest).await?;
        Ok(())
    }

    // -------------------------------------------------------------- read API

    pub async fn list_pages(
        &self,
        include_journals: bool,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Page>> {
        Self::json(self.json_req(Method::GET, "/pages").query(&[
            ("journals", include_journals.to_string()),
            ("limit", limit.to_string()),
            ("offset", offset.to_string()),
        ]))
        .await
    }

    /// Page (with aliases and block tree) by title or alias; `None` if it does not exist.
    pub async fn page_by_title(&self, title: &str) -> Result<Option<PageView>> {
        self.optional(
            self.json_req(Method::GET, "/pages/by-title")
                .query(&[("title", title)]),
        )
        .await
    }

    pub async fn page(&self, id: synapse_core::PageId) -> Result<Option<PageView>> {
        self.optional(self.json_req(Method::GET, &format!("/pages/{id}")))
            .await
    }

    pub async fn backlinks(&self, id: synapse_core::PageId) -> Result<Vec<Backlink>> {
        let v: BacklinksView =
            Self::json(self.json_req(Method::GET, &format!("/pages/{id}/backlinks"))).await?;
        Ok(v.backlinks)
    }

    /// The journal of `date` (`None` if that day has no journal; reads never create it).
    pub async fn journal(&self, date: chrono::NaiveDate) -> Result<Option<PageView>> {
        self.optional(self.json_req(Method::GET, &format!("/journals/{date}")))
            .await
    }

    pub async fn journals_between(
        &self,
        from: chrono::NaiveDate,
        to: chrono::NaiveDate,
    ) -> Result<Vec<Page>> {
        Self::json(
            self.json_req(Method::GET, "/journals")
                .query(&[("from", from.to_string()), ("to", to.to_string())]),
        )
        .await
    }

    async fn optional<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<Option<T>> {
        let resp = rb.send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(Self::check(resp).await?.json().await?))
    }
}

pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = blake3::Hasher::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().to_hex().to_string())
}
