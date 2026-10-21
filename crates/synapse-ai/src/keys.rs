//! API key access. Keys are never part of [`crate::ProviderConfig`], the graph or any log.

use std::collections::HashMap;
use std::fmt;
use std::sync::RwLock;

/// A secret string whose `Debug` output is redacted.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// Source of API keys, addressed by a name (see `ProviderConfig::key_name`).
/// The app backs this with the OS keystore.
pub trait KeyStore: Send + Sync {
    fn get(&self, name: &str) -> Option<Secret>;
}

/// In-memory store (tests, or a cache in front of the OS keystore).
#[derive(Default)]
pub struct MemoryKeyStore(RwLock<HashMap<String, Secret>>);

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&self, name: &str, key: &str) {
        self.0
            .write()
            .unwrap()
            .insert(name.to_string(), Secret::new(key));
    }
    pub fn remove(&self, name: &str) {
        self.0.write().unwrap().remove(name);
    }
}

impl KeyStore for MemoryKeyStore {
    fn get(&self, name: &str) -> Option<Secret> {
        self.0.read().unwrap().get(name).cloned()
    }
}

/// Reads `<NAME>_API_KEY` from the environment, with `NAME` upper-cased and non-alphanumerics
/// replaced by `_` (`anthropic` → `ANTHROPIC_API_KEY`).
#[derive(Default)]
pub struct EnvKeyStore;

impl EnvKeyStore {
    pub fn var_name(name: &str) -> String {
        let up: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_uppercase()
                } else {
                    '_'
                }
            })
            .collect();
        format!("{up}_API_KEY")
    }
}

impl KeyStore for EnvKeyStore {
    fn get(&self, name: &str) -> Option<Secret> {
        std::env::var(Self::var_name(name))
            .ok()
            .filter(|v| !v.is_empty())
            .map(Secret::new)
    }
}
