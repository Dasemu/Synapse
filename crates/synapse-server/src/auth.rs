//! Bearer-token authentication. Tokens map to a [`Principal`] that may be restricted to some
//! graphs, so per-user tokens (loaded from a file or DB) can replace the env list later
//! without touching the handlers.

use crate::AppState;
use crate::error::ApiError;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct Principal {
    pub name: String,
    /// `None` = every graph.
    pub graphs: Option<HashSet<String>>,
}

impl Principal {
    pub fn can_access(&self, graph: &str) -> bool {
        self.graphs.as_ref().is_none_or(|g| g.contains(graph))
    }
}

/// Stores only hashes of the tokens; compares in constant time.
#[derive(Debug, Default)]
pub struct Authenticator {
    entries: Vec<(blake3::Hash, Principal)>,
}

impl Authenticator {
    pub fn from_shared_tokens(tokens: &[String]) -> Self {
        let entries = tokens
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    blake3::hash(t.as_bytes()),
                    Principal {
                        name: format!("token-{}", i + 1),
                        graphs: None,
                    },
                )
            })
            .collect();
        Self { entries }
    }

    pub fn authenticate(&self, token: &str) -> Option<Principal> {
        let h = blake3::hash(token.as_bytes());
        let mut found = None;
        // No early exit: the time taken does not depend on which token matched.
        for (stored, p) in &self.entries {
            if *stored == h {
                found = Some(p.clone());
            }
        }
        found
    }
}

pub fn bearer(req: &Request) -> Option<&str> {
    req.headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let principal = bearer(&req)
        .and_then(|t| state.auth.authenticate(t))
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "missing or invalid token"))?;
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}
