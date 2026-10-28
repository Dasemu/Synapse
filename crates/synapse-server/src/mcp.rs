//! MCP over Streamable HTTP at `/v1/graphs/{id}/mcp` (`docs/mcp.md`).
//!
//! Its own bearer tokens (`SYNAPSE_MCP_TOKEN`), checked here; the graph must already exist
//! (MCP never creates one). Tools run on the server's shared graph handle, so writes are
//! normal ops: they are logged, pushed to SSE subscribers and pulled by every client.

use crate::AppState;
use crate::auth::{Authenticator, bearer};
use crate::config::McpConfig;
use crate::error::ApiError;
use crate::graphs::GraphHandle;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use std::sync::Arc;
use synapse_mcp::rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use synapse_mcp::rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use synapse_mcp::tools::ToolError;
use synapse_mcp::{GraphHost, HostFuture, HostRef, Job, SynapseMcp};

pub struct McpState {
    auth: Authenticator,
}

impl McpState {
    pub fn new(cfg: &McpConfig) -> Self {
        Self {
            auth: Authenticator::from_shared_tokens(&cfg.tokens),
        }
    }
}

/// Runs tool jobs on a server graph and wakes SSE subscribers after writes.
struct ServerHost(Arc<GraphHandle>);

impl GraphHost for ServerHost {
    fn run(&self, job: Job) -> HostFuture {
        let h = self.0.clone();
        Box::pin(async move {
            let (out, seq) = h
                .run(move |g| Ok((job(g), g.last_seq())))
                .await
                .map_err(|e| ToolError::new(e.message))?;
            if let Ok(seq) = seq {
                h.publish_seq(seq);
            }
            out
        })
    }
}

/// Authenticates, resolves the graph and hands it to the MCP service via an extension.
async fn guard(
    State(s): State<AppState>,
    Path(id): Path<String>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let mcp = s
        .mcp
        .as_ref()
        .ok_or_else(|| ApiError::not_found("not found"))?;
    let who = bearer(&req)
        .and_then(|t| mcp.auth.authenticate(t))
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "missing or invalid token"))?;
    if !who.can_access(&id) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "no access to this graph",
        ));
    }
    let handle = s
        .graphs
        .get(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("graph not found"))?;
    req.extensions_mut()
        .insert(HostRef(Arc::new(ServerHost(handle))));
    Ok(next.run(req).await)
}

pub fn routes(state: AppState, cfg: &McpConfig) -> Router<AppState> {
    let proto = SynapseMcp::new(None, cfg.allow_write);
    let mut http = StreamableHttpServerConfig::default()
        // Stateless request/response: no sessions to keep, JSON replies.
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None);
    http = if cfg.allowed_hosts.is_empty() {
        http.disable_allowed_hosts()
    } else {
        http.with_allowed_hosts(cfg.allowed_hosts.clone())
    };
    let service = StreamableHttpService::new(
        move || Ok(proto.clone()),
        Arc::new(NeverSessionManager::default()),
        http,
    );
    Router::new()
        .route_service("/v1/graphs/{id}/mcp", service)
        .route_layer(middleware::from_fn_with_state(state, guard))
}
