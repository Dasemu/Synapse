//! Authenticated self-update (web-deploy): `POST /admin/update` runs the update script
//! asynchronously (one at a time), `GET /admin/update` reports its status.
//!
//! Exit code 0 = updated: the server then exits gracefully so systemd (`Restart=always`)
//! starts the new binary. Exit code 10 = already up to date, no restart. Anything else = failed.

use crate::AppState;
use crate::auth::bearer;
use crate::error::ApiError;
use axum::Json;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const EXIT_UP_TO_DATE: i32 = 10;
const COOLDOWN: Duration = Duration::from_secs(60);
const RATE_WINDOW: Duration = Duration::from_secs(60);
const RATE_MAX: usize = 20;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Running,
    UpToDate,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub state: UpdateState,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub exit_code: Option<i32>,
    /// Last bytes of the script output.
    pub log_tail: String,
}

pub struct Updater {
    status: Mutex<UpdateStatus>,
    last_trigger: Mutex<Option<Instant>>,
    hits: Mutex<VecDeque<Instant>>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            status: Mutex::new(UpdateStatus {
                state: UpdateState::Idle,
                started_at_ms: None,
                finished_at_ms: None,
                exit_code: None,
                log_tail: String::new(),
            }),
            last_trigger: Mutex::new(None),
            hits: Mutex::new(VecDeque::new()),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Updater {
    /// Sliding-window limit on all admin requests (also throttles token guessing).
    fn rate_ok(&self) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        while hits
            .front()
            .is_some_and(|t| now.duration_since(*t) > RATE_WINDOW)
        {
            hits.pop_front();
        }
        if hits.len() >= RATE_MAX {
            return false;
        }
        hits.push_back(now);
        true
    }
}

pub async fn admin_guard(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let Some(expected) = state.update_token.as_ref() else {
        return Err(ApiError::not_found("update endpoint is disabled"));
    };
    if !state.updater.rate_ok() {
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }
    let given = blake3::hash(bearer(&req).unwrap_or("").as_bytes());
    if given != *expected {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid update token",
        ));
    }
    Ok(next.run(req).await)
}

pub async fn status(State(state): State<AppState>) -> Json<UpdateStatus> {
    Json(
        state
            .updater
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
    )
}

pub async fn trigger(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<UpdateStatus>), ApiError> {
    let up = &state.updater;
    {
        let mut st = up.status.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(st.state, UpdateState::Running) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "an update is already running",
            ));
        }
        let mut last = up.last_trigger.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| t.elapsed() < COOLDOWN) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "updated too recently",
            ));
        }
        *last = Some(Instant::now());
        *st = UpdateStatus {
            state: UpdateState::Running,
            started_at_ms: Some(now_ms()),
            finished_at_ms: None,
            exit_code: None,
            log_tail: String::new(),
        };
    }
    let snapshot = up.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let state2 = state.clone();
    tokio::spawn(async move { run_script(state2).await });
    Ok((StatusCode::ACCEPTED, Json(snapshot)))
}

async fn run_script(state: AppState) {
    let log = state.data_dir.join("update.log");
    let result = async {
        let out = std::fs::File::create(&log)?;
        let err = out.try_clone()?;
        tokio::process::Command::new(state.update_script.as_path())
            .stdin(std::process::Stdio::null())
            .stdout(out)
            .stderr(err)
            .status()
            .await
    }
    .await;
    let tail = tokio::fs::read(&log)
        .await
        .map(|b| {
            let start = b.len().saturating_sub(4096);
            String::from_utf8_lossy(&b[start..]).into_owned()
        })
        .unwrap_or_default();
    let (new_state, code) = match result {
        Ok(s) if s.success() => (UpdateState::Succeeded, s.code()),
        Ok(s) if s.code() == Some(EXIT_UP_TO_DATE) => (UpdateState::UpToDate, s.code()),
        Ok(s) => (UpdateState::Failed, s.code()),
        Err(e) => {
            tracing::error!(error = %e, "could not run the update script");
            (UpdateState::Failed, None)
        }
    };
    {
        let mut st = state
            .updater
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        st.state = new_state.clone();
        st.exit_code = code;
        st.finished_at_ms = Some(now_ms());
        st.log_tail = tail;
    }
    tracing::info!(?new_state, ?code, "update finished");
    if matches!(new_state, UpdateState::Succeeded) {
        // Let clients poll the final status, then exit so systemd restarts the new binary.
        tokio::time::sleep(Duration::from_secs(2)).await;
        state.shutdown.notify_waiters();
        state.shutdown.notify_one();
    }
}
