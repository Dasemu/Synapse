use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    /// Accepted bearer tokens (`SYNAPSE_AUTH_TOKEN`, comma separated).
    pub auth_tokens: Vec<String>,
    /// Token for `/admin/update`; the endpoint is disabled when unset.
    pub update_token: Option<String>,
    pub update_script: PathBuf,
}

pub const MIN_TOKEN_LEN: usize = 16;

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
        let cfg = Self {
            bind,
            data_dir: data_dir.into(),
            auth_tokens,
            update_token: var("SYNAPSE_UPDATE_TOKEN"),
            update_script: var("SYNAPSE_UPDATE_SCRIPT")
                .unwrap_or_else(|| "/opt/synapse/scripts/update.sh".into())
                .into(),
        };
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.auth_tokens.is_empty() {
            return Err("no auth token configured".into());
        }
        for t in self.auth_tokens.iter().chain(self.update_token.iter()) {
            if t.len() < MIN_TOKEN_LEN {
                return Err(format!(
                    "tokens must be at least {MIN_TOKEN_LEN} characters"
                ));
            }
        }
        Ok(())
    }
}
