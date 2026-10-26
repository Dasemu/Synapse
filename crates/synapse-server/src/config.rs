use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    /// Accepted bearer tokens (`SYNAPSE_AUTH_TOKEN`, comma separated).
    pub auth_tokens: Vec<String>,
    /// Token for `/admin/update`; the endpoint is disabled when unset.
    pub update_token: Option<String>,
    pub update_script: PathBuf,
    /// Ops whose HLC is further ahead of the server clock than this are rejected.
    pub max_clock_skew: Duration,
    /// MCP endpoint (`/v1/graphs/{id}/mcp`); disabled when `None`.
    pub mcp: Option<McpConfig>,
}

/// MCP over Streamable HTTP (`docs/mcp.md`).
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Accepted bearer tokens (`SYNAPSE_MCP_TOKEN`, comma separated), separate from sync.
    pub tokens: Vec<String>,
    /// Expose the write tools (`SYNAPSE_MCP_ALLOW_WRITE=1`).
    pub allow_write: bool,
    /// Accepted `Host` headers (`SYNAPSE_MCP_ALLOWED_HOSTS`); empty = any host.
    pub allowed_hosts: Vec<String>,
}

pub const MIN_TOKEN_LEN: usize = 16;

/// Loopback only, like the MCP SDK default (set `SYNAPSE_MCP_ALLOWED_HOSTS` behind a proxy).
pub const DEFAULT_MCP_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let bind = var("SYNAPSE_BIND").unwrap_or_else(|| "127.0.0.1:7878".into());
        let bind = bind
            .parse()
            .map_err(|e| format!("SYNAPSE_BIND={bind:?}: {e}"))?;
        let data_dir = var("SYNAPSE_DATA_DIR").ok_or("SYNAPSE_DATA_DIR is required")?;
        let tokens = var("SYNAPSE_AUTH_TOKEN").ok_or("SYNAPSE_AUTH_TOKEN is required")?;
        let auth_tokens: Vec<String> = tokens
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        let list = |v: String| -> Vec<String> {
            v.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        };
        let mcp = var("SYNAPSE_MCP_TOKEN").map(|t| McpConfig {
            tokens: list(t),
            allow_write: var("SYNAPSE_MCP_ALLOW_WRITE")
                .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes")),
            allowed_hosts: match var("SYNAPSE_MCP_ALLOWED_HOSTS") {
                Some(h) if h.trim() == "*" => Vec::new(),
                Some(h) => list(h),
                None => DEFAULT_MCP_HOSTS.iter().map(|h| h.to_string()).collect(),
            },
        });
        let cfg = Self {
            bind,
            data_dir: data_dir.into(),
            auth_tokens,
            update_token: var("SYNAPSE_UPDATE_TOKEN"),
            update_script: var("SYNAPSE_UPDATE_SCRIPT")
                .unwrap_or_else(|| "/opt/synapse/scripts/update.sh".into())
                .into(),
            max_clock_skew: Duration::from_secs(
                60 * var("SYNAPSE_MAX_CLOCK_SKEW_MINUTES")
                    .map(|v| v.parse::<u64>())
                    .transpose()
                    .map_err(|e| format!("SYNAPSE_MAX_CLOCK_SKEW_MINUTES: {e}"))?
                    .unwrap_or(10),
            ),
            mcp,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.auth_tokens.is_empty() {
            return Err("no auth token configured".into());
        }
        if let Some(m) = &self.mcp
            && m.tokens.is_empty()
        {
            return Err("SYNAPSE_MCP_TOKEN has no token".into());
        }
        let mcp_tokens = self.mcp.iter().flat_map(|m| m.tokens.iter());
        for t in self
            .auth_tokens
            .iter()
            .chain(self.update_token.iter())
            .chain(mcp_tokens)
        {
            if t.len() < MIN_TOKEN_LEN {
                return Err(format!(
                    "tokens must be at least {MIN_TOKEN_LEN} characters"
                ));
            }
        }
        Ok(())
    }
}
