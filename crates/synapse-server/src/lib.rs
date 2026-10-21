//! Synapse sync server. See `docs/sync.md` (protocol) and `docs/server.md` (deployment).

mod admin;
mod api;
mod auth;
mod blobs;
pub mod config;
mod error;
mod graphs;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, head, post};
use axum::{Json, Router, middleware};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tower_http::trace::TraceLayer;

pub use config::Config;

#[derive(Clone)]
pub struct AppState {
    pub(crate) graphs: Arc<graphs::Graphs>,
    pub(crate) auth: Arc<auth::Authenticator>,
    pub(crate) update_token: Option<blake3::Hash>,
    pub(crate) update_script: Arc<std::path::PathBuf>,
    pub(crate) data_dir: Arc<std::path::PathBuf>,
    pub(crate) updater: Arc<admin::Updater>,
    pub(crate) shutdown: Arc<Notify>,
    pub(crate) max_clock_skew: std::time::Duration,
}

impl AppState {
    pub fn new(cfg: &Config) -> std::io::Result<Self> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        Ok(Self {
            graphs: Arc::new(graphs::Graphs::new(cfg.data_dir.clone())),
            auth: Arc::new(auth::Authenticator::from_shared_tokens(&cfg.auth_tokens)),
            update_token: cfg
                .update_token
                .as_ref()
                .map(|t| blake3::hash(t.as_bytes())),
            update_script: Arc::new(cfg.update_script.clone()),
            data_dir: Arc::new(cfg.data_dir.clone()),
            updater: Arc::new(admin::Updater::default()),
            shutdown: Arc::new(Notify::new()),
            max_clock_skew: cfg.max_clock_skew,
        })
    }
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": synapse_sync::protocol::PROTOCOL_VERSION,
    }))
}

pub fn router(state: AppState) -> Router {
    let graph_routes = Router::new()
        .route("/graphs/{id}", get(api::get_graph).put(api::put_graph))
        .route("/graphs/{id}/ops", get(api::pull_ops).post(api::push_ops))
        .route("/graphs/{id}/events", get(api::events))
        .route(
            "/graphs/{id}/blobs/{hash}",
            head(blobs::head_blob)
                .get(blobs::get_blob)
                .put(blobs::put_blob)
                // Blobs can be multi-GB: no body limit, they are streamed to disk.
                .layer(DefaultBodyLimit::disable()),
        )
        .route("/graphs/{id}/pages", get(api::list_pages))
        .route("/graphs/{id}/pages/by-title", get(api::page_by_title))
        .route("/graphs/{id}/pages/{page}", get(api::page_by_id))
        .route("/graphs/{id}/pages/{page}/backlinks", get(api::backlinks))
        .route("/graphs/{id}/journals", get(api::journals))
        .route("/graphs/{id}/journals/{date}", get(api::journal))
        // JSON bodies (op batches) are capped; see MAX_PUSH_OPS.
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    let admin_routes = Router::new()
        .route("/update", post(admin::trigger).get(admin::status))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin::admin_guard,
        ));

    Router::new()
        .route("/healthz", get(healthz))
        .nest("/v1", graph_routes)
        .nest("/admin", admin_routes)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub struct RunningServer {
    pub addr: SocketAddr,
    pub task: tokio::task::JoinHandle<std::io::Result<()>>,
    shutdown: Arc<Notify>,
}

impl RunningServer {
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }
}

/// Binds and serves in the background (used by tests and `serve`).
pub async fn start(cfg: &Config) -> std::io::Result<RunningServer> {
    let state = AppState::new(cfg)?;
    let shutdown = state.shutdown.clone();
    let listener = TcpListener::bind(cfg.bind).await?;
    let addr = listener.local_addr()?;
    let sd = shutdown.clone();
    let app = router(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move { sd.notified().await })
            .await
    });
    Ok(RunningServer {
        addr,
        task,
        shutdown,
    })
}

/// Serves until SIGINT/SIGTERM or a successful self-update.
pub async fn serve(cfg: Config) -> std::io::Result<()> {
    let server = start(&cfg).await?;
    tracing::info!(addr = %server.addr, data_dir = %cfg.data_dir.display(), "synapse-server listening");
    let sd = server.shutdown.clone();
    tokio::spawn(async move {
        wait_for_signal().await;
        sd.notify_one();
    });
    server.task.await.map_err(std::io::Error::other)?
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
