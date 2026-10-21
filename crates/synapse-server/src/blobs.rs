//! Blob endpoints: HEAD (existence / partial-upload offset), GET (Range, streamed) and PUT
//! (streamed to `<graph>/uploads/<hash>.part`, resumable, verified with blake3).

use crate::AppState;
use crate::api::graph_for;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::graphs::GraphHandle;
use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use std::sync::Arc;
use synapse_sync::protocol::{HEADER_UPLOAD_LENGTH, HEADER_UPLOAD_OFFSET, is_valid_hash};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

fn check_hash(h: &str) -> ApiResult<()> {
    if is_valid_hash(h) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "blob hash must be 64 lowercase hex chars",
        ))
    }
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

fn part_path(h: &GraphHandle, hash: &str) -> std::path::PathBuf {
    h.dir.join("uploads").join(format!("{hash}.part"))
}

pub async fn head_blob(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path((id, hash)): Path<(String, String)>,
) -> ApiResult<Response> {
    check_hash(&hash)?;
    let h = graph_for(&s, &who, &id, false).await?;
    if let Ok(meta) = tokio::fs::metadata(h.blobs.path_of(&hash)).await {
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_LENGTH, meta.len().to_string())],
        )
            .into_response());
    }
    let offset = tokio::fs::metadata(part_path(&h, &hash))
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    Ok((
        StatusCode::NOT_FOUND,
        [(HEADER_UPLOAD_OFFSET, offset.to_string())],
    )
        .into_response())
}

/// Parses `bytes=START-[END]`.
fn parse_range(v: &str, size: u64) -> Option<(u64, u64)> {
    let spec = v.strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let start: u64 = a.parse().ok()?;
    let end = if b.is_empty() {
        size.checked_sub(1)?
    } else {
        b.parse::<u64>().ok()?.min(size.checked_sub(1)?)
    };
    (start <= end).then_some((start, end))
}

pub async fn get_blob(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path((id, hash)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    check_hash(&hash)?;
    let h = graph_for(&s, &who, &id, false).await?;
    let mut file = tokio::fs::File::open(h.blobs.path_of(&hash))
        .await
        .map_err(|_| ApiError::not_found("blob not found"))?;
    let size = file.metadata().await?.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|v| parse_range(v, size));
    let (status, start, len) = match range {
        None => (StatusCode::OK, 0, size),
        Some(Some((a, b))) => (StatusCode::PARTIAL_CONTENT, a, b - a + 1),
        Some(None) => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
            )
                .into_response());
        }
    };
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
        file.take(len),
        256 * 1024,
    ));
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    let hd = resp.headers_mut();
    hd.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    hd.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    hd.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        hd.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{}/{size}", start + len - 1)).unwrap(),
        );
    }
    Ok(resp)
}

/// Removes the in-flight marker when the request ends (or is cancelled).
struct UploadGuard {
    graph: Arc<GraphHandle>,
    hash: String,
}

impl Drop for UploadGuard {
    fn drop(&mut self) {
        self.graph
            .uploads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.hash);
    }
}

pub async fn put_blob(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path((id, hash)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    check_hash(&hash)?;
    let h = graph_for(&s, &who, &id, false).await?;
    if h.blobs.contains(&hash) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let offset = header_u64(&headers, HEADER_UPLOAD_OFFSET).unwrap_or(0);
    let total = header_u64(&headers, HEADER_UPLOAD_LENGTH)
        .ok_or_else(|| ApiError::bad_request("Upload-Length header is required"))?;

    if !h
        .uploads
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(hash.clone())
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "upload already in progress",
        ));
    }
    let _guard = UploadGuard {
        graph: h.clone(),
        hash: hash.clone(),
    };

    let part = part_path(&h, &hash);
    tokio::fs::create_dir_all(part.parent().unwrap()).await?;
    let have = tokio::fs::metadata(&part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if offset > 0 && offset != have {
        return Ok((
            StatusCode::CONFLICT,
            [(HEADER_UPLOAD_OFFSET, have.to_string())],
        )
            .into_response());
    }
    let mut out = if offset == 0 {
        tokio::fs::File::create(&part).await?
    } else {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await?
    };
    let mut written = offset;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::bad_request(format!("upload interrupted: {e}")))?;
        written += chunk.len() as u64;
        if written > total {
            drop(out);
            let _ = tokio::fs::remove_file(&part).await;
            return Err(ApiError::bad_request("body exceeds Upload-Length"));
        }
        out.write_all(&chunk).await?;
    }
    out.flush().await?;
    drop(out);

    if written < total {
        // Partial upload stored; the client resumes from `Upload-Offset`.
        return Ok((
            StatusCode::ACCEPTED,
            [(HEADER_UPLOAD_OFFSET, written.to_string())],
        )
            .into_response());
    }

    // Complete: verify the content hash before publishing it into the blob store.
    let (part2, hash2) = (part.clone(), hash.clone());
    let dest = h.blobs.path_of(&hash);
    let ok = tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
        let mut f = std::fs::File::open(&part2)?;
        let mut hasher = blake3::Hasher::new();
        std::io::copy(&mut f, &mut hasher)?;
        if hasher.finalize().to_hex().as_str() != hash2 {
            std::fs::remove_file(&part2)?;
            return Ok(false);
        }
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::rename(&part2, &dest)?;
        Ok(true)
    })
    .await
    .map_err(ApiError::internal)??;
    if !ok {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "content does not match the blob hash",
        ));
    }
    tracing::info!(graph = %id, %hash, size = total, "blob stored");
    Ok(StatusCode::CREATED.into_response())
}
