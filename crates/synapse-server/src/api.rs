//! Op-log sync, realtime events and the read API.

use crate::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::graphs::GraphHandle;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{Extension, Json};
use chrono::NaiveDate;
use futures_util::stream::{self, Stream};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use synapse_core::store::journal_id;
use synapse_sync::protocol::*;

const MAX_PUSH_OPS: usize = 5000;
const MAX_PULL_LIMIT: u32 = 2000;

/// Resolves and authorises the graph of a request.
pub async fn graph_for(
    state: &AppState,
    who: &Principal,
    id: &str,
    create: bool,
) -> ApiResult<Arc<GraphHandle>> {
    if !who.can_access(id) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "no access to this graph",
        ));
    }
    let h = if create {
        Some(state.graphs.get_or_create(id).await?)
    } else {
        state.graphs.get(id).await?
    };
    h.ok_or_else(|| ApiError::not_found("graph not found"))
}

fn info(h: &GraphHandle, last_seq: i64) -> Info {
    Info {
        protocol: PROTOCOL_VERSION,
        server_id: h.server_id.clone(),
        last_seq,
    }
}

/// `PUT /graphs/{id}`: create if missing (idempotent).
pub async fn put_graph(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<Info>> {
    let h = graph_for(&s, &who, &id, true).await?;
    let seq = h.run(|g| g.last_seq()).await?;
    Ok(Json(info(&h, seq)))
}

pub async fn get_graph(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<Info>> {
    let h = graph_for(&s, &who, &id, false).await?;
    let seq = h.run(|g| g.last_seq()).await?;
    Ok(Json(info(&h, seq)))
}

pub async fn push_ops(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<PushRequest>,
) -> ApiResult<Json<PushResponse>> {
    if req.ops.len() > MAX_PUSH_OPS {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("at most {MAX_PUSH_OPS} ops per push"),
        ));
    }
    let h = graph_for(&s, &who, &id, false).await?;
    let max_future_ms = s.max_clock_skew.as_millis() as i64;
    let (applied, last_seq, rejected) = h
        .run(move |g| {
            let (mut applied, mut rejected) = (0u32, Vec::new());
            let now = g.now_ms();
            for (i, w) in req.ops.iter().enumerate() {
                let reject = |reason: String| Rejection {
                    index: i as u32,
                    reason,
                };
                if let Err(reason) = validate_op(&w.hlc, &w.op, now, max_future_ms) {
                    rejected.push(reject(reason));
                    continue;
                }
                match g.apply_remote(&w.hlc, &w.op) {
                    Ok(true) => applied += 1,
                    Ok(false) => {}
                    // Unappliable op: refuse it, keep the rest of the batch.
                    Err(e) if is_poison(&e) => rejected.push(reject(e.to_string())),
                    Err(e) => return Err(e),
                }
            }
            Ok((applied, g.last_seq()?, rejected))
        })
        .await?;
    if applied > 0 {
        h.publish_seq(last_seq);
    }
    if !rejected.is_empty() {
        tracing::warn!(graph = %id, principal = %who.name, rejected = rejected.len(), first = %rejected[0].reason, "push: ops rejected");
    }
    tracing::info!(graph = %id, principal = %who.name, applied, "push");
    Ok(Json(PushResponse {
        applied,
        last_seq,
        rejected,
    }))
}

#[derive(Deserialize)]
pub struct PullQuery {
    #[serde(default)]
    since: i64,
    limit: Option<u32>,
    exclude_actor: Option<String>,
}

pub async fn pull_ops(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
    Query(q): Query<PullQuery>,
) -> ApiResult<Json<PullResponse>> {
    let h = graph_for(&s, &who, &id, false).await?;
    let limit = q.limit.unwrap_or(500).clamp(1, MAX_PULL_LIMIT);
    let server_id = h.server_id.clone();
    let resp = h
        .run(move |g| {
            let recs = g.ops_since(q.since, limit)?;
            let scanned = recs.len() as u32;
            let next_cursor = recs.last().map_or(q.since, |r| r.seq);
            let ops = recs
                .into_iter()
                .filter(|r| {
                    q.exclude_actor
                        .as_deref()
                        .is_none_or(|a| hlc_actor(&r.hlc) != Some(a))
                })
                .collect();
            let last_seq = g.last_seq()?;
            Ok(PullResponse {
                server_id,
                ops,
                next_cursor,
                last_seq,
                has_more: scanned == limit && next_cursor < last_seq,
            })
        })
        .await?;
    Ok(Json(resp))
}

/// `GET /graphs/{id}/events`: SSE, one `SyncEvent` on connect and on every change.
pub async fn events(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let h = graph_for(&s, &who, &id, false).await?;
    let mut rx = h.seq.subscribe();
    let cur = *rx.borrow_and_update();
    let stream = stream::unfold((cur, rx, true), |(cur, mut rx, first)| async move {
        let seq = if first {
            cur
        } else {
            rx.changed().await.ok()?;
            *rx.borrow_and_update()
        };
        let ev = Event::default()
            .event("seq")
            .json_data(SyncEvent { last_seq: seq })
            .expect("serialisable");
        Some((Ok(ev), (seq, rx, false)))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ------------------------------------------------------------------ read API

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    journals: bool,
    limit: Option<u32>,
    #[serde(default)]
    offset: u32,
}

pub async fn list_pages(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<synapse_core::Page>>> {
    let h = graph_for(&s, &who, &id, false).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    Ok(Json(
        h.run(move |g| g.list_pages(q.journals, limit, q.offset))
            .await?,
    ))
}

fn view(g: &synapse_core::Graph, page: synapse_core::Page) -> synapse_core::Result<PageView> {
    Ok(PageView {
        aliases: g.aliases_of(page.id)?,
        tree: g
            .page_tree(page.id)?
            .into_iter()
            .map(|(depth, block)| TreeEntry { depth, block })
            .collect(),
        page,
    })
}

#[derive(Deserialize)]
pub struct TitleQuery {
    title: String,
}

pub async fn page_by_title(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
    Query(q): Query<TitleQuery>,
) -> ApiResult<Json<PageView>> {
    let h = graph_for(&s, &who, &id, false).await?;
    h.run(move |g| g.page_by_title(&q.title)?.map(|p| view(g, p)).transpose())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("page not found"))
}

pub async fn page_by_id(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path((id, page)): Path<(String, uuid_str::Id)>,
) -> ApiResult<Json<PageView>> {
    let h = graph_for(&s, &who, &id, false).await?;
    h.run(move |g| g.get_page(page.0)?.map(|p| view(g, p)).transpose())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("page not found"))
}

pub async fn backlinks(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path((id, page)): Path<(String, uuid_str::Id)>,
) -> ApiResult<Json<BacklinksView>> {
    let h = graph_for(&s, &who, &id, false).await?;
    let backlinks = h.run(move |g| g.backlinks(page.0)).await?;
    Ok(Json(BacklinksView { backlinks }))
}

pub async fn journal(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path((id, date)): Path<(String, String)>,
) -> ApiResult<Json<PageView>> {
    let date: NaiveDate = date
        .parse()
        .map_err(|_| ApiError::bad_request("date must be YYYY-MM-DD"))?;
    let h = graph_for(&s, &who, &id, false).await?;
    // Look up by deterministic id: `Graph::journal` would create (and log) the page.
    h.run(move |g| {
        g.get_page(journal_id(date))?
            .map(|p| view(g, p))
            .transpose()
    })
    .await?
    .map(Json)
    .ok_or_else(|| ApiError::not_found("no journal for that date"))
}

#[derive(Deserialize)]
pub struct RangeQuery {
    from: NaiveDate,
    to: NaiveDate,
}

pub async fn journals(
    State(s): State<AppState>,
    Extension(who): Extension<Principal>,
    Path(id): Path<String>,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<Vec<synapse_core::Page>>> {
    let h = graph_for(&s, &who, &id, false).await?;
    Ok(Json(
        h.run(move |g| g.journals_between(q.from, q.to)).await?,
    ))
}

mod uuid_str {
    use serde::{Deserialize, Deserializer};

    /// Path segment holding a UUID (page id).
    pub struct Id(pub synapse_core::PageId);

    impl<'de> Deserialize<'de> for Id {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = String::deserialize(d)?;
            s.parse().map(Id).map_err(serde::de::Error::custom)
        }
    }
}
